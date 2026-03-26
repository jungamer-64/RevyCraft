use crate::RuntimeError;
use crate::runtime::{AcceptedGenerationSession, RuntimeServer};
use mc_plugin_host::host::plugin_reload_poll_interval_ms;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

pub(super) fn spawn_runtime_loop(
    run_server: Arc<RuntimeServer>,
    mut shutdown_rx: oneshot::Receiver<()>,
    mut accepted_rx: mpsc::Receiver<AcceptedGenerationSession>,
    runtime_completion_tx: watch::Sender<bool>,
) -> JoinHandle<Result<(), RuntimeError>> {
    tokio::spawn(async move {
        let result = async {
            let mut tick_interval = tokio::time::interval(Duration::from_millis(50));
            let mut save_interval = tokio::time::interval(Duration::from_secs(2));
            let mut config_reload_interval =
                tokio::time::interval(Duration::from_millis(plugin_reload_poll_interval_ms()));
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        run_server.shutdown_listener_workers().await;
                        run_server
                            .terminate_all_sessions("Server shutting down")
                            .await;
                        run_server.join_all_session_tasks().await;
                        run_server.maybe_save().await?;
                        return Ok(());
                    }
                    maybe_accepted = accepted_rx.recv() => {
                        let Some(accepted) = maybe_accepted else {
                            continue;
                        };
                        run_server.spawn_accepted_transport_session(accepted).await;
                    }
                    _ = tick_interval.tick() => {
                        if let Err(error) = run_server.tick().await {
                            return run_server.finish_with_runtime_error(error, true).await;
                        }
                        if let Err(error) = run_server.enforce_generation_drains().await {
                            return run_server.finish_with_runtime_error(error, true).await;
                        }
                    }
                    _ = config_reload_interval.tick(), if run_server.reload.reload_host().is_some() => {
                        if let Some(reload_host) = run_server.reload.reload_host() {
                            let previous_generation = run_server.active_generation_id();
                            match run_server.maybe_reload_runtime_watch(reload_host.as_ref()).await {
                                Ok(Some(result)) => {
                                    if !result.reloaded_plugin_ids.is_empty() || result.topology.changed(previous_generation) {
                                        run_server
                                            .log_status_summary(&format!(
                                                "runtime full reload applied: plugins={} activated_generation={} reconfigured={}",
                                                if result.reloaded_plugin_ids.is_empty() {
                                                    "-".to_string()
                                                } else {
                                                    result.reloaded_plugin_ids.join(",")
                                                },
                                                result.topology.activated_generation_id.0,
                                                if result.topology.reconfigured_adapter_ids.is_empty() {
                                                    "-".to_string()
                                                } else {
                                                    result.topology.reconfigured_adapter_ids.join(",")
                                                },
                                            ))
                                            .await;
                                    }
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    if matches!(error, RuntimeError::PluginFatal(_)) {
                                        return run_server.finish_with_runtime_error(error, true).await;
                                    }
                                    eprintln!("runtime full reload failed: {error}");
                                }
                            }
                        }
                    }
                    _ = save_interval.tick() => {
                        if let Err(error) = run_server.maybe_save().await {
                            return run_server.finish_with_runtime_error(error, false).await;
                        }
                    }
                }
                run_server.reap_completed_session_tasks().await;
                if let Some(error) = run_server.take_pending_plugin_fatal_error() {
                    return run_server.finish_with_runtime_error(error, true).await;
                }
            }
        }
        .await;
        let _ = runtime_completion_tx.send(true);
        result
    })
}
