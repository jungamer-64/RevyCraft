#![allow(clippy::multiple_crate_versions)]

mod grpc;

use crate::grpc::{spawn_admin_grpc_server, wait_for_shutdown_signal};
use mc_plugin_host::host::plugin_host_from_config;
use server_runtime::RuntimeError;
use server_runtime::config::{ServerConfig, ServerConfigSource};
use server_runtime::runtime::{
    AdminControlPlaneHandle, AdminResponse, ServerBuilder, format_runtime_status_summary,
};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;

async fn wait_for_ctrl_c() -> Result<(), RuntimeError> {
    tokio::signal::ctrl_c()
        .await
        .map_err(|error| RuntimeError::Config(format!("failed to wait for ctrl-c: {error}")))
}

async fn run_console_loop(
    server: &server_runtime::runtime::RunningServer,
    control_plane: &AdminControlPlaneHandle,
    shutdown_tx: &watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<(), RuntimeError> {
    let Some(_) = server.admin_ui().await else {
        return Ok(());
    };
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            signal = wait_for_ctrl_c() => {
                signal?;
                let _ = shutdown_tx.send(true);
                break;
            }
            _ = wait_for_shutdown_signal(shutdown_rx.clone()) => break,
            line = lines.next_line() => {
                let Some(line) = line.map_err(|error| RuntimeError::Config(format!("failed to read stdin: {error}")))? else {
                    break;
                };
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let Some(ui) = server.admin_ui().await else {
                    eprintln!("admin-ui became unavailable; console commands are disabled");
                    continue;
                };
                let request = match ui.parse_line(line) {
                    Ok(request) => request,
                    Err(error) => {
                        let response = AdminResponse::Error {
                            message: error.to_string(),
                        };
                        match ui.render_response(&response) {
                            Ok(text) => println!("{text}"),
                            Err(render_error) => eprintln!("{render_error}"),
                        }
                        continue;
                    }
                };
                let response = control_plane.execute_local_console(request).await;
                let shutdown_requested = matches!(response, AdminResponse::ShutdownScheduled);
                match ui.render_response(&response) {
                    Ok(text) => println!("{text}"),
                    Err(error) => eprintln!("{error}"),
                }
                if shutdown_requested {
                    let _ = shutdown_tx.send(true);
                    break;
                }
            }
        }
    }
    Ok(())
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

#[tokio::main]
async fn main() -> Result<(), RuntimeError> {
    let config = ServerConfig::from_toml(Path::new("runtime/server.toml"))?;
    let plugin_host =
        plugin_host_from_config(&config.plugin_host_bootstrap_config())?.ok_or_else(|| {
            RuntimeError::Config(format!(
                "no packaged plugins discovered under `{}`",
                config.bootstrap.plugins_dir.display()
            ))
        })?;
    let loaded_plugins =
        plugin_host.load_plugin_set(&config.plugin_host_runtime_selection_config())?;

    let server = ServerBuilder::new(
        ServerConfigSource::Toml(Path::new("runtime/server.toml").to_path_buf()),
        loaded_plugins,
    )
    .with_reload_host(plugin_host)
    .build()
    .await?;
    for binding in server.listener_bindings() {
        println!(
            "server listening on {} via {:?} for {:?}",
            binding.local_addr, binding.transport, binding.adapter_ids
        );
    }
    println!("{}", format_runtime_status_summary(&server.status().await));

    let control_plane = server.admin_control_plane();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let grpc = if config.admin_grpc_enabled() {
        Some(
            spawn_admin_grpc_server(
                config.admin_grpc_bind_addr(),
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
    let has_admin_ui = server.admin_ui().await.is_some();
    let mut grpc_monitor = grpc.map(|grpc| tokio::spawn(async move { grpc.join().await }));

    match (has_admin_ui, grpc_monitor.as_mut()) {
        (true, Some(grpc_monitor)) => {
            tokio::select! {
                result = wait_for_grpc_server(grpc_monitor) => {
                    let _ = shutdown_tx.send(true);
                    result?;
                }
                result = run_console_loop(&server, &control_plane, &shutdown_tx, shutdown_rx.clone()) => {
                    result?;
                    let _ = shutdown_tx.send(true);
                    wait_for_grpc_server(grpc_monitor).await?;
                }
            }
        }
        (false, Some(grpc_monitor)) => {
            tokio::select! {
                result = wait_for_grpc_server(grpc_monitor) => {
                    let _ = shutdown_tx.send(true);
                    result?;
                }
                result = wait_for_exit_signal(shutdown_rx.clone()) => {
                    result?;
                    let _ = shutdown_tx.send(true);
                    wait_for_grpc_server(grpc_monitor).await?;
                }
            }
        }
        (true, None) => {
            run_console_loop(&server, &control_plane, &shutdown_tx, shutdown_rx.clone()).await?;
        }
        (false, None) => {
            eprintln!("admin-ui unavailable at boot; stdio control loop is disabled");
            wait_for_exit_signal(shutdown_rx.clone()).await?;
            let _ = shutdown_tx.send(true);
        }
    }

    server.shutdown().await
}
