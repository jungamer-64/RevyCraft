#![allow(clippy::multiple_crate_versions)]

mod grpc;
mod process_surfaces;
mod upgrade;

use crate::grpc::{
    spawn_admin_grpc_server, spawn_admin_grpc_server_from_std_listener, wait_for_shutdown_signal,
};
use crate::process_surfaces::{ConsoleControl, PausedProcessSurfaces, ProcessSurfaceCommand};
use crate::upgrade::UpgradeCoordinator;
use server_runtime::RuntimeError;
use server_runtime::config::ServerConfigSource;
use server_runtime::runtime::{
    AdminControlPlaneHandle, AdminRequest, AdminResponse, AdminSubject, ServerSupervisor,
    format_runtime_status_summary,
};
use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};

const DEFAULT_SERVER_CONFIG_PATH: &str = "runtime/server.toml";
const SERVER_CONFIG_ENV: &str = "REVY_SERVER_CONFIG";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConsoleLoopExit {
    ShutdownRequested,
    Detached,
    NoAdminSurface,
    ExternalShutdown,
    PausedForUpgrade,
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
    mut control_rx: mpsc::Receiver<ConsoleControl>,
) -> Result<ConsoleLoopExit, RuntimeError> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            Some(control) = control_rx.recv() => {
                match control {
                    ConsoleControl::PauseForUpgrade { ack_tx } => {
                        let _ = ack_tx.send(());
                        return Ok(ConsoleLoopExit::PausedForUpgrade);
                    }
                }
            }
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
                let upgrade_requested = matches!(request, AdminRequest::UpgradeRuntime { .. });
                let response = control_plane.execute_local_console(request).await;
                let shutdown_requested = matches!(response, AdminResponse::ShutdownScheduled);
                let upgrade_committed = upgrade_requested && matches!(response, AdminResponse::UpgradeRuntime(_));
                match control_plane.render_local_response(&response).await {
                    Ok(text) => println!("{text}"),
                    Err(error) => eprintln!("{error}"),
                }
                if shutdown_requested {
                    let _ = shutdown_tx.send(true);
                    return Ok(ConsoleLoopExit::ShutdownRequested);
                }
                if upgrade_committed {
                    return Ok(ConsoleLoopExit::PausedForUpgrade);
                }
            }
        }
    }
}

fn spawn_console_monitor(
    control_plane: AdminControlPlaneHandle,
    input_mode: ConsoleInputMode,
    has_other_admin_surface: bool,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
) -> (
    mpsc::Sender<ConsoleControl>,
    tokio::task::JoinHandle<Result<ConsoleLoopExit, RuntimeError>>,
) {
    let (control_tx, control_rx) = mpsc::channel(4);
    let monitor = tokio::spawn(async move {
        run_console_loop(
            &control_plane,
            input_mode,
            has_other_admin_surface,
            &shutdown_tx,
            shutdown_rx,
            control_rx,
        )
        .await
    });
    (control_tx, monitor)
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

async fn wait_for_console_loop(
    console_monitor: &mut tokio::task::JoinHandle<Result<ConsoleLoopExit, RuntimeError>>,
) -> Result<ConsoleLoopExit, RuntimeError> {
    console_monitor.await.map_err(RuntimeError::from)?
}

enum ProcessStartupMode {
    Normal,
    UpgradeChild(upgrade::PendingUpgradeChild),
}

async fn pause_console_for_upgrade(
    console_control_tx: &mut Option<mpsc::Sender<ConsoleControl>>,
    console_monitor: &mut Option<tokio::task::JoinHandle<Result<ConsoleLoopExit, RuntimeError>>>,
) -> Result<bool, RuntimeError> {
    let Some(control_tx) = console_control_tx.take() else {
        return Ok(false);
    };
    let Some(mut monitor) = console_monitor.take() else {
        return Ok(false);
    };
    let (ack_tx, ack_rx) = oneshot::channel();
    control_tx
        .send(ConsoleControl::PauseForUpgrade { ack_tx })
        .await
        .map_err(|_| RuntimeError::Config("failed to pause console for upgrade".to_string()))?;
    let _ = ack_rx.await;
    match wait_for_console_loop(&mut monitor).await? {
        ConsoleLoopExit::PausedForUpgrade | ConsoleLoopExit::ExternalShutdown => Ok(true),
        ConsoleLoopExit::Detached => Ok(false),
        ConsoleLoopExit::ShutdownRequested => Err(RuntimeError::Config(
            "console requested shutdown while pausing for upgrade".to_string(),
        )),
        ConsoleLoopExit::NoAdminSurface => Err(RuntimeError::Config(
            "console lost stdin while pausing for upgrade".to_string(),
        )),
    }
}

fn spawn_console_surface(
    control_plane: &AdminControlPlaneHandle,
    console_input_mode: ConsoleInputMode,
    has_other_admin_surface: bool,
    shutdown_tx: &watch::Sender<bool>,
    shutdown_rx: &watch::Receiver<bool>,
    console_control_tx: &mut Option<mpsc::Sender<ConsoleControl>>,
    console_monitor: &mut Option<tokio::task::JoinHandle<Result<ConsoleLoopExit, RuntimeError>>>,
) {
    let (control_tx, monitor) = spawn_console_monitor(
        control_plane.clone(),
        console_input_mode,
        has_other_admin_surface,
        shutdown_tx.clone(),
        shutdown_rx.clone(),
    );
    *console_control_tx = Some(control_tx);
    *console_monitor = Some(monitor);
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

fn upgrade_control_plane(
    server: &Arc<ServerSupervisor>,
    coordinator: &Arc<UpgradeCoordinator>,
) -> AdminControlPlaneHandle {
    let coordinator = Arc::clone(coordinator);
    server.admin_control_plane().with_runtime_upgrader(Arc::new(
        move |subject: AdminSubject, executable_path| {
            let coordinator = Arc::clone(&coordinator);
            Box::pin(async move { coordinator.upgrade(subject, executable_path).await })
        },
    ))
}

async fn run_server_process(
    server: Arc<ServerSupervisor>,
    control_plane: AdminControlPlaneHandle,
    grpc_listener_override: Option<std::net::TcpListener>,
    enable_console: bool,
    mut startup_mode: ProcessStartupMode,
    upgrade_coordinator: Arc<UpgradeCoordinator>,
) -> Result<(), RuntimeError> {
    for binding in server.listener_bindings() {
        println!(
            "server listening on {} via {:?} for {:?}",
            binding.local_addr, binding.transport, binding.adapter_ids
        );
    }
    println!("{}", format_runtime_status_summary(&server.status().await));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    upgrade_coordinator
        .set_process_shutdown_sender(shutdown_tx.clone())
        .await;
    let (surface_control_tx, mut surface_control_rx) = mpsc::channel(4);
    upgrade_coordinator
        .set_surface_control_sender(surface_control_tx)
        .await;
    let grpc = if let Some(listener) = grpc_listener_override {
        match spawn_admin_grpc_server_from_std_listener(
            listener,
            control_plane.clone(),
            shutdown_tx.clone(),
            shutdown_rx.clone(),
        )
        .await
        {
            Ok(grpc) => Some(grpc),
            Err(error) => {
                if let ProcessStartupMode::UpgradeChild(pending_child) = &mut startup_mode {
                    let _ = pending_child.report_error(error.to_string()).await;
                }
                return Err(error);
            }
        }
    } else if let Some(bind_addr) = server.admin_grpc_bind_addr() {
        match spawn_admin_grpc_server(
            bind_addr,
            control_plane.clone(),
            shutdown_tx.clone(),
            shutdown_rx.clone(),
        )
        .await
        {
            Ok(grpc) => Some(grpc),
            Err(error) => {
                if let ProcessStartupMode::UpgradeChild(pending_child) = &mut startup_mode {
                    let _ = pending_child.report_error(error.to_string()).await;
                }
                return Err(error);
            }
        }
    } else {
        None
    };
    if let Some(grpc) = grpc.as_ref() {
        println!("admin gRPC listening on {}", grpc.local_addr());
    }
    let mut grpc = grpc;
    let console_input_mode = console_input_mode();
    let mut has_other_admin_surface = grpc.is_some();
    let mut console_control_tx = None;
    let mut console_monitor = None;

    if matches!(startup_mode, ProcessStartupMode::Normal) && enable_console {
        spawn_console_surface(
            &control_plane,
            console_input_mode,
            has_other_admin_surface,
            &shutdown_tx,
            &shutdown_rx,
            &mut console_control_tx,
            &mut console_monitor,
        );
    }

    if let ProcessStartupMode::UpgradeChild(pending_child) = &mut startup_mode {
        if let Some(error) = upgrade::child_upgrade_fault_before_ready() {
            let _ = pending_child.report_error(error.to_string()).await;
            return Err(error);
        }
        upgrade::child_upgrade_ready_delay_if_needed().await;
        pending_child.report_ready_and_wait_for_commit().await?;
        server.finish_child_runtime_upgrade_commit().await?;
        eprintln!("runtime upgrade phase: child committed cutover");
        if enable_console {
            spawn_console_surface(
                &control_plane,
                console_input_mode,
                has_other_admin_surface,
                &shutdown_tx,
                &shutdown_rx,
                &mut console_control_tx,
                &mut console_monitor,
            );
        }
    }

    loop {
        tokio::select! {
            Some(surface_command) = surface_control_rx.recv() => {
                match surface_command {
                    ProcessSurfaceCommand::PauseForUpgrade { skip_console, ack_tx } => {
                        let admin_listener_for_child = if let Some(grpc) = grpc.as_mut() {
                            Some(grpc.pause_for_upgrade().await?)
                        } else {
                            None
                        };
                        let console_was_paused = if skip_console {
                            false
                        } else {
                            pause_console_for_upgrade(&mut console_control_tx, &mut console_monitor).await?
                        };
                        let _ = ack_tx.send(Ok(PausedProcessSurfaces {
                            admin_listener_for_child,
                            console_was_paused,
                            grpc_accept_was_paused: grpc.is_some(),
                        }));
                    }
                    ProcessSurfaceCommand::ResumeAfterUpgradeRollback { paused, ack_tx } => {
                        if paused.grpc_accept_was_paused
                            && let Some(grpc) = grpc.as_mut()
                        {
                            grpc.resume_after_upgrade_rollback()?;
                        }
                        has_other_admin_surface = grpc.is_some();
                        if paused.console_was_paused {
                            spawn_console_surface(
                                &control_plane,
                                console_input_mode,
                                has_other_admin_surface,
                                &shutdown_tx,
                                &shutdown_rx,
                                &mut console_control_tx,
                                &mut console_monitor,
                            );
                        }
                        let _ = ack_tx.send(Ok(()));
                    }
                }
            }
            result = async {
                let Some(grpc) = grpc.as_mut() else {
                    std::future::pending().await
                };
                grpc.wait_for_server_exit().await
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
                        break;
                    }
                    ConsoleLoopExit::Detached => {
                        console_control_tx = None;
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
                        console_control_tx = None;
                        console_monitor = None;
                    }
                    ConsoleLoopExit::PausedForUpgrade => {
                        console_control_tx = None;
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
                break;
            }
        }
    }

    let committed_upgrade = upgrade_coordinator.take_committed_upgrade().await;
    if let Some(_committed_upgrade) = committed_upgrade {
        if let Some(grpc) = grpc.take() {
            grpc.join().await?;
        }
        drop(control_plane);
        drop(upgrade_coordinator);
        return Ok(());
    }
    if let Some(grpc) = grpc.take() {
        grpc.join().await?;
    }
    drop(control_plane);
    drop(upgrade_coordinator);
    let _ = server.request_shutdown();
    server.join_runtime().await
}

#[tokio::main]
async fn main() -> Result<(), RuntimeError> {
    let args = std::env::args().collect::<Vec<_>>();
    if let Some(mut pending_child) = upgrade::try_boot_upgrade_child(&args).await? {
        let server = pending_child.server();
        let grpc_listener_override = pending_child.take_grpc_listener_override();
        let coordinator = Arc::new(UpgradeCoordinator::new(Arc::clone(&server)));
        let control_plane = upgrade_control_plane(&server, &coordinator);
        return run_server_process(
            server,
            control_plane,
            grpc_listener_override,
            true,
            ProcessStartupMode::UpgradeChild(pending_child),
            coordinator,
        )
        .await;
    }

    let (config_source, missing_config_warning) = resolve_server_config_source();
    if let Some(warning) = missing_config_warning {
        eprintln!("{warning}");
    }
    let server = Arc::new(ServerSupervisor::boot(config_source).await?);
    let coordinator = Arc::new(UpgradeCoordinator::new(Arc::clone(&server)));
    let control_plane = upgrade_control_plane(&server, &coordinator);
    run_server_process(
        server,
        control_plane,
        None,
        true,
        ProcessStartupMode::Normal,
        coordinator,
    )
    .await
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
