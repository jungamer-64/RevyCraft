use crate::RuntimeError;
use crate::runtime::{AcceptedGenerationSession, RuntimeServer};
use mc_plugin_host::host::plugin_reload_poll_interval_ms;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

type PersistenceTask = JoinHandle<Result<(), RuntimeError>>;

pub(super) fn spawn_runtime_loop(
    run_server: Arc<RuntimeServer>,
    mut shutdown_rx: oneshot::Receiver<()>,
    mut accepted_rx: mpsc::Receiver<AcceptedGenerationSession>,
    runtime_completion_tx: watch::Sender<bool>,
) -> JoinHandle<Result<(), RuntimeError>> {
    tokio::spawn(async move {
        let result = async {
            let mut persistence_task: Option<PersistenceTask> = None;
            let mut tick_interval = tokio::time::interval(Duration::from_millis(50));
            let mut save_interval = tokio::time::interval(Duration::from_secs(2));
            let mut config_reload_interval =
                tokio::time::interval(Duration::from_millis(plugin_reload_poll_interval_ms()));
            // Core systems advance from an absolute monotonic timestamp. Replaying missed
            // interval tokens would not recover simulation time; it would only monopolize the
            // admission loop after an overloaded frame and amplify the original stall.
            tick_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            save_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            config_reload_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        await_persistence(&mut persistence_task).await?;
                        // Admission stops with this loop. Session writers still need the shared
                        // UDP router to publish disconnects before its peers are destroyed.
                        accepted_rx.close();
                        run_server
                            .terminate_all_sessions("Server shutting down")
                            .await;
                        run_server.join_all_session_tasks().await;
                        run_server.shutdown_listener_workers().await;
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
                            return finish_after_persistence(
                                &run_server,
                                &mut persistence_task,
                                error,
                                true,
                            ).await;
                        }
                        if let Err(error) = run_server.enforce_generation_drains().await {
                            return finish_after_persistence(
                                &run_server,
                                &mut persistence_task,
                                error,
                                true,
                            ).await;
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
                                        return finish_after_persistence(
                                            &run_server,
                                            &mut persistence_task,
                                            error,
                                            true,
                                        ).await;
                                    }
                                    eprintln!("runtime full reload failed: {error}");
                                }
                            }
                        }
                    }
                    _ = save_interval.tick(), if persistence_task.is_none() => {
                        let persistence_server = Arc::clone(&run_server);
                        persistence_task = Some(tokio::spawn(async move {
                            persistence_server.maybe_save().await
                        }));
                    }
                    persistence_result = async {
                        persistence_task
                            .as_mut()
                            .expect("persistence task is present while selected")
                            .await
                    }, if persistence_task.is_some() => {
                        let _completed = persistence_task.take();
                        match persistence_result {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => {
                                return run_server.finish_with_runtime_error(error, false).await;
                            }
                            Err(error) => {
                                return run_server
                                    .finish_with_runtime_error(RuntimeError::from(error), false)
                                    .await;
                            }
                        }
                    }
                }
                run_server.reap_completed_session_tasks().await;
                if let Some(error) = run_server.take_pending_plugin_fatal_error() {
                    return finish_after_persistence(
                        &run_server,
                        &mut persistence_task,
                        error,
                        true,
                    ).await;
                }
            }
        }
        .await;
        let _ = runtime_completion_tx.send(true);
        result
    })
}

async fn await_persistence(task: &mut Option<PersistenceTask>) -> Result<(), RuntimeError> {
    match task.take() {
        Some(task) => task.await?,
        None => Ok(()),
    }
}

async fn finish_after_persistence(
    server: &RuntimeServer,
    task: &mut Option<PersistenceTask>,
    error: RuntimeError,
    attempt_best_effort_save: bool,
) -> Result<(), RuntimeError> {
    if let Err(persistence_error) = await_persistence(task).await {
        eprintln!("concurrent persistence failed during runtime shutdown: {persistence_error}");
    }
    server
        .finish_with_runtime_error(error, attempt_best_effort_save)
        .await
}
