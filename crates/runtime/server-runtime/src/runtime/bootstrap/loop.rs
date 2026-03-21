use crate::RuntimeError;
use crate::runtime::{AcceptedTopologySession, RuntimeServer};
use mc_plugin_host::host::plugin_reload_poll_interval_ms;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

pub(super) fn spawn_runtime_loop(
    run_server: Arc<RuntimeServer>,
    mut shutdown_rx: oneshot::Receiver<()>,
    mut accepted_rx: mpsc::UnboundedReceiver<AcceptedTopologySession>,
) -> JoinHandle<Result<(), RuntimeError>> {
    tokio::spawn(async move {
        let mut tick_interval = tokio::time::interval(Duration::from_millis(50));
        let mut save_interval = tokio::time::interval(Duration::from_secs(2));
        let mut plugin_reload_interval =
            tokio::time::interval(Duration::from_millis(plugin_reload_poll_interval_ms()));
        let mut topology_reload_interval =
            tokio::time::interval(Duration::from_millis(plugin_reload_poll_interval_ms()));
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    run_server.shutdown_listener_workers().await;
                    run_server.maybe_save().await?;
                    return Ok(());
                }
                maybe_accepted = accepted_rx.recv() => {
                    let Some(accepted) = maybe_accepted else {
                        continue;
                    };
                    run_server
                        .spawn_transport_session(accepted.topology_generation_id, accepted.session)
                        .await;
                }
                _ = tick_interval.tick() => {
                    if let Err(error) = run_server.tick().await {
                        return run_server.finish_with_runtime_error(error).await;
                    }
                    if let Err(error) = run_server.enforce_topology_drains().await {
                        return run_server.finish_with_runtime_error(error).await;
                    }
                }
                _ = topology_reload_interval.tick(), if run_server.reload_host.is_some() => {
                    if let Some(reload_host) = run_server.reload_host.as_ref() {
                        let previous_generation = run_server.active_topology_generation_id();
                        match run_server.maybe_reload_topology_watch(reload_host.as_ref()).await {
                            Ok(result) => {
                                if result.changed(previous_generation) {
                                    run_server
                                        .log_status_summary(&format!(
                                            "topology reload applied: activated_generation={} reconfigured={}",
                                            result.activated_generation_id.0,
                                            if result.reconfigured_adapter_ids.is_empty() {
                                                "-".to_string()
                                            } else {
                                                result.reconfigured_adapter_ids.join(",")
                                            }
                                        ))
                                        .await;
                                }
                            }
                            Err(error) => {
                                if matches!(error, RuntimeError::PluginFatal(_)) {
                                    return run_server.finish_with_runtime_error(error).await;
                                }
                                eprintln!("topology reload failed: {error}");
                            }
                        }
                    }
                }
                _ = plugin_reload_interval.tick(), if run_server.config.plugin_reload_watch && run_server.reload_host.is_some() => {
                    if let Some(reload_host) = run_server.reload_host.as_ref() {
                        match run_server.reload_plugins(reload_host.as_ref()).await {
                            Ok(reloaded) => {
                                if !reloaded.is_empty() {
                                    run_server
                                        .log_status_summary(&format!(
                                            "plugin reload applied: {}",
                                            reloaded.join(",")
                                        ))
                                        .await;
                                }
                            }
                            Err(error) => {
                                if matches!(error, RuntimeError::PluginFatal(_)) {
                                    return run_server.finish_with_runtime_error(error).await;
                                }
                                eprintln!("plugin reload failed: {error}");
                            }
                        }
                    }
                }
                _ = save_interval.tick() => {
                    if let Err(error) = run_server.maybe_save().await {
                        return run_server.finish_with_runtime_error(error).await;
                    }
                }
            }
            if let Some(error) = run_server.take_pending_plugin_fatal_error() {
                return run_server.finish_with_runtime_error(error).await;
            }
        }
    })
}
