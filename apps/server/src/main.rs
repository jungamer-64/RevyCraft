#![allow(clippy::multiple_crate_versions)]

mod grpc;

use crate::grpc::{spawn_admin_grpc_server, wait_for_shutdown_signal};
use server_runtime::RuntimeError;
use server_runtime::config::ServerConfigSource;
use server_runtime::runtime::{
    AdminControlPlaneHandle, AdminResponse, ServerSupervisor, format_runtime_status_summary,
};
use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;

const DEFAULT_SERVER_CONFIG_PATH: &str = "runtime/server.toml";
const SERVER_CONFIG_ENV: &str = "REVY_SERVER_CONFIG";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConsoleLoopExit {
    ShutdownRequested,
    Detached,
    NoAdminSurface,
    ExternalShutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConsoleInputMode {
    Terminal,
    NonTerminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConsoleEofAction {
    Shutdown,
    Detach,
    WarnAndExit,
}

fn console_input_mode() -> ConsoleInputMode {
    if std::io::stdin().is_terminal() {
        ConsoleInputMode::Terminal
    } else {
        ConsoleInputMode::NonTerminal
    }
}

fn decide_console_eof_action(
    input_mode: ConsoleInputMode,
    has_other_admin_surface: bool,
) -> ConsoleEofAction {
    match input_mode {
        ConsoleInputMode::Terminal => ConsoleEofAction::Shutdown,
        ConsoleInputMode::NonTerminal if has_other_admin_surface => ConsoleEofAction::Detach,
        ConsoleInputMode::NonTerminal => ConsoleEofAction::WarnAndExit,
    }
}

async fn wait_for_ctrl_c() -> Result<(), RuntimeError> {
    tokio::signal::ctrl_c()
        .await
        .map_err(|error| RuntimeError::Config(format!("failed to wait for ctrl-c: {error}")))
}

async fn run_console_loop(
    control_plane: &AdminControlPlaneHandle,
    input_mode: ConsoleInputMode,
    has_other_admin_surface: bool,
    shutdown_tx: &watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<ConsoleLoopExit, RuntimeError> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            signal = wait_for_ctrl_c() => {
                signal?;
                let _ = shutdown_tx.send(true);
                return Ok(ConsoleLoopExit::ShutdownRequested);
            }
            _ = wait_for_shutdown_signal(shutdown_rx.clone()) => {
                return Ok(ConsoleLoopExit::ExternalShutdown);
            }
            line = lines.next_line() => {
                let Some(line) = line.map_err(|error| RuntimeError::Config(format!("failed to read stdin: {error}")))? else {
                    return Ok(match decide_console_eof_action(input_mode, has_other_admin_surface) {
                        ConsoleEofAction::Shutdown => {
                            let _ = shutdown_tx.send(true);
                            ConsoleLoopExit::ShutdownRequested
                        }
                        ConsoleEofAction::Detach => ConsoleLoopExit::Detached,
                        ConsoleEofAction::WarnAndExit => ConsoleLoopExit::NoAdminSurface,
                    });
                };
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let request = match control_plane.parse_local_command(line).await {
                    Ok(request) => request,
                    Err(error) => {
                        let response = AdminResponse::Error {
                            message: error,
                        };
                        match control_plane.render_local_response(&response).await {
                            Ok(text) => println!("{text}"),
                            Err(render_error) => eprintln!("{render_error}"),
                        }
                        continue;
                    }
                };
                let response = control_plane.execute_local_console(request).await;
                let shutdown_requested = matches!(response, AdminResponse::ShutdownScheduled);
                match control_plane.render_local_response(&response).await {
                    Ok(text) => println!("{text}"),
                    Err(error) => eprintln!("{error}"),
                }
                if shutdown_requested {
                    let _ = shutdown_tx.send(true);
                    return Ok(ConsoleLoopExit::ShutdownRequested);
                }
            }
        }
    }
}

async fn wait_for_runtime_completion(server: &ServerSupervisor) -> Result<(), RuntimeError> {
    server.wait_for_runtime_completion().await
}

async fn wait_for_exit_signal(shutdown_rx: watch::Receiver<bool>) -> Result<(), RuntimeError> {
    tokio::select! {
        signal = wait_for_ctrl_c() => signal,
        _ = wait_for_shutdown_signal(shutdown_rx.clone()) => Ok(()),
    }
}

async fn wait_for_grpc_server(
    grpc_monitor: &mut tokio::task::JoinHandle<Result<(), RuntimeError>>,
) -> Result<(), RuntimeError> {
    let result = grpc_monitor.await.map_err(RuntimeError::from)?;
    if let Err(error) = &result {
        eprintln!("admin gRPC server exited with an error: {error}");
    }
    result
}

async fn wait_for_console_loop(
    console_monitor: &mut tokio::task::JoinHandle<Result<ConsoleLoopExit, RuntimeError>>,
) -> Result<ConsoleLoopExit, RuntimeError> {
    console_monitor.await.map_err(RuntimeError::from)?
}

fn selected_server_config_path(env_override: Option<OsString>) -> PathBuf {
    env_override
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SERVER_CONFIG_PATH))
}

fn missing_server_config_warning(path: &Path) -> Option<String> {
    (!path.exists()).then(|| {
        format!(
            "server config path `{}` was not found; booting with default config",
            path.display()
        )
    })
}

fn resolve_server_config_source() -> (ServerConfigSource, Option<String>) {
    let config_path = selected_server_config_path(std::env::var_os(SERVER_CONFIG_ENV));
    let warning = missing_server_config_warning(&config_path);
    (ServerConfigSource::Toml(config_path), warning)
}

#[tokio::main]
async fn main() -> Result<(), RuntimeError> {
    let (config_source, missing_config_warning) = resolve_server_config_source();
    if let Some(warning) = missing_config_warning {
        eprintln!("{warning}");
    }
    let server = ServerSupervisor::boot(config_source).await?;
    for binding in server.listener_bindings() {
        println!(
            "server listening on {} via {:?} for {:?}",
            binding.local_addr, binding.transport, binding.adapter_ids
        );
    }
    println!("{}", format_runtime_status_summary(&server.status().await));

    let control_plane = server.admin_control_plane();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let grpc = if let Some(bind_addr) = server.admin_grpc_bind_addr() {
        Some(
            spawn_admin_grpc_server(
                bind_addr,
                control_plane.clone(),
                shutdown_tx.clone(),
                shutdown_rx.clone(),
            )
            .await?,
        )
    } else {
        None
    };
    if let Some(grpc) = grpc.as_ref() {
        println!("admin gRPC listening on {}", grpc.local_addr());
    }
    let mut grpc_monitor = grpc.map(|grpc| tokio::spawn(async move { grpc.join().await }));
    let console_input_mode = console_input_mode();
    let has_other_admin_surface = grpc_monitor.is_some();
    let mut console_monitor = Some({
        let control_plane = control_plane.clone();
        let shutdown_tx = shutdown_tx.clone();
        let shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            run_console_loop(
                &control_plane,
                console_input_mode,
                has_other_admin_surface,
                &shutdown_tx,
                shutdown_rx,
            )
            .await
        })
    });

    loop {
        tokio::select! {
            result = async {
                let Some(grpc_monitor) = grpc_monitor.as_mut() else {
                    std::future::pending().await
                };
                wait_for_grpc_server(grpc_monitor).await
            } => {
                let _ = shutdown_tx.send(true);
                result?;
                break;
            }
            result = async {
                let Some(console_monitor) = console_monitor.as_mut() else {
                    std::future::pending().await
                };
                wait_for_console_loop(console_monitor).await
            } => {
                match result? {
                    ConsoleLoopExit::ShutdownRequested => {
                        let _ = shutdown_tx.send(true);
                        if let Some(grpc_monitor) = grpc_monitor.as_mut() {
                            wait_for_grpc_server(grpc_monitor).await?;
                        }
                        break;
                    }
                    ConsoleLoopExit::Detached => {
                        console_monitor = None;
                    }
                    ConsoleLoopExit::NoAdminSurface => {
                        eprintln!(
                            "stdin reached EOF and no other admin surface is available; shutting down to avoid running headless"
                        );
                        let _ = shutdown_tx.send(true);
                        break;
                    }
                    ConsoleLoopExit::ExternalShutdown => {
                        console_monitor = None;
                    }
                }
            }
            result = wait_for_runtime_completion(&server) => {
                result?;
                let _ = shutdown_tx.send(true);
                break;
            }
            result = wait_for_exit_signal(shutdown_rx.clone()) => {
                result?;
                let _ = shutdown_tx.send(true);
                if let Some(grpc_monitor) = grpc_monitor.as_mut() {
                    wait_for_grpc_server(grpc_monitor).await?;
                }
                break;
            }
        }
    }

    server.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::{
        ConsoleEofAction, ConsoleInputMode, DEFAULT_SERVER_CONFIG_PATH, decide_console_eof_action,
        missing_server_config_warning, selected_server_config_path,
    };
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    #[test]
    fn terminal_eof_requests_shutdown() {
        assert_eq!(
            decide_console_eof_action(ConsoleInputMode::Terminal, false),
            ConsoleEofAction::Shutdown
        );
        assert_eq!(
            decide_console_eof_action(ConsoleInputMode::Terminal, true),
            ConsoleEofAction::Shutdown
        );
    }

    #[test]
    fn non_terminal_eof_detaches_when_another_admin_surface_exists() {
        assert_eq!(
            decide_console_eof_action(ConsoleInputMode::NonTerminal, true),
            ConsoleEofAction::Detach
        );
    }

    #[test]
    fn non_terminal_eof_warns_and_exits_without_other_admin_surface() {
        assert_eq!(
            decide_console_eof_action(ConsoleInputMode::NonTerminal, false),
            ConsoleEofAction::WarnAndExit
        );
    }

    #[test]
    fn config_path_defaults_to_runtime_server_toml() {
        assert_eq!(
            selected_server_config_path(None),
            PathBuf::from(DEFAULT_SERVER_CONFIG_PATH)
        );
    }

    #[test]
    fn config_path_prefers_env_override() {
        assert_eq!(
            selected_server_config_path(Some(OsString::from("custom/server.toml"))),
            PathBuf::from("custom/server.toml")
        );
    }

    #[test]
    fn missing_config_path_warns_with_selected_path() {
        let path = Path::new("missing/server.toml");
        let warning = missing_server_config_warning(path).expect("missing config path should warn");
        assert!(warning.contains("booting with default config"));
        assert!(warning.contains("missing/server.toml"));
    }

    #[test]
    fn existing_config_path_does_not_warn() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempdir()?;
        let path = temp_dir.path().join("server.toml");
        fs::write(&path, "")?;
        assert_eq!(missing_server_config_warning(&path), None);
        Ok(())
    }
}
