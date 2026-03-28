#![allow(clippy::multiple_crate_versions)]

mod admin_surface;
mod process_surfaces;
mod upgrade;

use crate::admin_surface::AdminSurfaceSupervisor;
use crate::process_surfaces::{
    PausedAdminSurfaceInstance, PausedProcessSurfaces, ProcessSurfaceCommand,
};
use crate::upgrade::UpgradeCoordinator;
use revy_server_runtime::RuntimeError;
use revy_server_runtime::config::ServerConfigSource;
use revy_server_runtime::runtime::{
    AdminControlPlaneHandle, AdminSubject, ServerSupervisor, format_runtime_status_summary,
};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

const DEFAULT_SERVER_CONFIG_PATH: &str = "runtime/server.toml";
const SERVER_CONFIG_ENV: &str = "REVY_SERVER_CONFIG";

async fn wait_for_ctrl_c() -> Result<(), RuntimeError> {
    tokio::signal::ctrl_c()
        .await
        .map_err(|error| RuntimeError::Config(format!("failed to wait for ctrl-c: {error}")))
}

async fn wait_for_shutdown_signal(mut shutdown_rx: watch::Receiver<bool>) {
    if *shutdown_rx.borrow() {
        return;
    }
    while shutdown_rx.changed().await.is_ok() {
        if *shutdown_rx.borrow() {
            break;
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

enum ProcessStartupMode {
    Normal,
    UpgradeChild(upgrade::PendingUpgradeChild),
}

fn selected_server_config_path(env_override: Option<OsString>) -> PathBuf {
    env_override
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SERVER_CONFIG_PATH))
}

fn resolve_server_config_source() -> ServerConfigSource {
    let config_path = selected_server_config_path(std::env::var_os(SERVER_CONFIG_ENV));
    ServerConfigSource::Toml(config_path)
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
    admin_surface_resume: Option<Vec<PausedAdminSurfaceInstance>>,
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
        .set_surface_control_sender(surface_control_tx.clone())
        .await;

    let mut admin_surfaces = AdminSurfaceSupervisor::new(
        Arc::clone(&server),
        control_plane.clone(),
        surface_control_tx,
    );
    let startup_result = if let Some(paused_instances) = admin_surface_resume {
        admin_surfaces.resume_from_upgrade(paused_instances).await
    } else {
        admin_surfaces.reconcile().await
    };
    if let Err(error) = startup_result {
        if let ProcessStartupMode::UpgradeChild(pending_child) = &mut startup_mode {
            let _ = pending_child.report_error(error.to_string()).await;
        }
        return Err(error);
    }

    if let ProcessStartupMode::UpgradeChild(pending_child) = &mut startup_mode {
        if let Some(error) = upgrade::child_upgrade_fault_before_ready() {
            let _ = pending_child.report_error(error.to_string()).await;
            return Err(error);
        }
        upgrade::child_upgrade_ready_delay_if_needed().await;
        pending_child.report_ready_and_wait_for_commit().await?;
        server.finish_child_runtime_upgrade_commit().await?;
        admin_surfaces.activate_after_upgrade_commit()?;
        eprintln!("runtime upgrade phase: child committed cutover");
    }

    loop {
        tokio::select! {
            Some(surface_command) = surface_control_rx.recv() => {
                match surface_command {
                    ProcessSurfaceCommand::PauseForUpgrade { ack_tx } => {
                        let paused = admin_surfaces.pause_for_upgrade().await?;
                        let _ = ack_tx.send(Ok(PausedProcessSurfaces {
                            admin_surfaces: paused,
                        }));
                    }
                    ProcessSurfaceCommand::ResumeAfterUpgradeRollback { paused, ack_tx } => {
                        if !paused.admin_surfaces.is_empty() {
                            admin_surfaces
                                .resume_after_upgrade_rollback(paused.admin_surfaces)?;
                        }
                        let _ = ack_tx.send(Ok(()));
                    }
                    ProcessSurfaceCommand::ReconcileAdminSurfaces => {
                        admin_surfaces.reconcile().await?;
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
    if let Some(committed_upgrade) = committed_upgrade {
        drop(committed_upgrade);
        drop(control_plane);
        drop(upgrade_coordinator);
        return Ok(());
    }

    admin_surfaces.shutdown_current()?;
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
        let admin_surface_resume = pending_child.take_admin_surface_resume();
        let coordinator = Arc::new(UpgradeCoordinator::new(Arc::clone(&server)));
        let control_plane = upgrade_control_plane(&server, &coordinator);
        return run_server_process(
            server,
            control_plane,
            Some(admin_surface_resume),
            ProcessStartupMode::UpgradeChild(pending_child),
            coordinator,
        )
        .await;
    }

    let config_source = resolve_server_config_source();
    let server = Arc::new(ServerSupervisor::boot(config_source).await?);
    let coordinator = Arc::new(UpgradeCoordinator::new(Arc::clone(&server)));
    let control_plane = upgrade_control_plane(&server, &coordinator);
    run_server_process(
        server,
        control_plane,
        None,
        ProcessStartupMode::Normal,
        coordinator,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_SERVER_CONFIG_PATH, selected_server_config_path};
    use std::ffi::OsString;
    use std::path::PathBuf;

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
}
