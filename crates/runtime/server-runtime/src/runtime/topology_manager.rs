use super::{
    ActiveGeneration, DrainingGeneration, GenerationAdmission, GenerationId,
    RuntimeGenerationState, SessionControl, TopologyListenerWorker, TopologyReloadResult, now_ms,
};
use crate::ListenerBinding;
use crate::RuntimeError;
use crate::runtime::bootstrap::{activate_protocols, spawn_listener_worker};
use crate::runtime::kernel::RuntimeKernel;
use crate::runtime::session_registry::SessionRegistry;
use crate::transport::{bind_transport_listener, build_listener_plans};
use mc_plugin_host::registry::ProtocolRegistry;
use mc_plugin_host::runtime::RuntimeProtocolTopologyCandidate;
use mc_proto_common::{BedrockListenerDescriptor, Edition, TransportKind, WireFormatKind};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::oneshot;

pub(crate) struct TopologyManager {
    state: std::sync::RwLock<RuntimeGenerationState>,
    #[cfg(test)]
    fail_next_precommit: AtomicBool,
}

pub(crate) enum PreparedTopologyReload {
    Noop(TopologyReloadResult),
    ProtocolOnly {
        candidate_generation: Arc<ActiveGeneration>,
        result: TopologyReloadResult,
    },
    Generation {
        candidate_generation: Arc<ActiveGeneration>,
        new_bound_listeners: Vec<crate::transport::BoundTransportListener>,
        reused_transports: HashSet<TransportKind>,
        applied_config_change: bool,
        reconfigured_adapter_ids: Vec<String>,
        drain_grace_secs: u64,
    },
}

pub(crate) enum PrecommittedTopologyReload {
    Noop(TopologyReloadResult),
    ProtocolOnly {
        candidate_generation: Arc<ActiveGeneration>,
        result: TopologyReloadResult,
    },
    Generation {
        candidate_generation: Arc<ActiveGeneration>,
        new_listener_workers: HashMap<TransportKind, TopologyListenerWorker>,
        reused_transports: HashSet<TransportKind>,
        applied_config_change: bool,
        reconfigured_adapter_ids: Vec<String>,
        drain_grace_secs: u64,
    },
}

impl PreparedTopologyReload {
    pub(crate) fn candidate_generation(
        &self,
        active_generation: &Arc<ActiveGeneration>,
    ) -> Arc<ActiveGeneration> {
        match self {
            Self::Noop(_) => Arc::clone(active_generation),
            Self::ProtocolOnly {
                candidate_generation,
                ..
            } => Arc::clone(candidate_generation),
            Self::Generation {
                candidate_generation,
                ..
            } => Arc::clone(candidate_generation),
        }
    }
}

impl TopologyManager {
    pub(crate) fn new(
        active: Arc<ActiveGeneration>,
        listener_workers: HashMap<TransportKind, TopologyListenerWorker>,
        next_generation_id: u64,
    ) -> Self {
        Self {
            state: std::sync::RwLock::new(RuntimeGenerationState {
                active,
                draining: Vec::new(),
                listener_workers,
                next_generation_id,
            }),
            #[cfg(test)]
            fail_next_precommit: AtomicBool::new(false),
        }
    }

    pub(crate) fn listener_bindings(&self) -> Vec<ListenerBinding> {
        self.active_generation().listener_bindings.clone()
    }

    pub(crate) fn active_generation(&self) -> Arc<ActiveGeneration> {
        Arc::clone(
            &self
                .state
                .read()
                .expect("runtime topology lock should not be poisoned")
                .active,
        )
    }

    pub(crate) fn active_generation_id(&self) -> GenerationId {
        self.active_generation().generation_id
    }

    #[cfg(test)]
    pub(crate) fn generation(&self, generation_id: GenerationId) -> Option<Arc<ActiveGeneration>> {
        let generation_state = self
            .state
            .read()
            .expect("runtime topology lock should not be poisoned");
        if generation_state.active.generation_id == generation_id {
            return Some(Arc::clone(&generation_state.active));
        }
        generation_state
            .draining
            .iter()
            .find(|entry| entry.generation.generation_id == generation_id)
            .map(|entry| Arc::clone(&entry.generation))
    }

    pub(crate) fn generation_admission(&self, generation_id: GenerationId) -> GenerationAdmission {
        let generation_state = self
            .state
            .read()
            .expect("runtime topology lock should not be poisoned");
        if generation_state.active.generation_id == generation_id {
            return GenerationAdmission::Active(Arc::clone(&generation_state.active));
        }
        let Some(draining) = generation_state
            .draining
            .iter()
            .find(|entry| entry.generation.generation_id == generation_id)
        else {
            return GenerationAdmission::Missing;
        };
        if draining.drain_deadline_ms <= now_ms() {
            return GenerationAdmission::ExpiredDraining;
        }
        GenerationAdmission::Draining(Arc::clone(&draining.generation))
    }

    pub(crate) fn snapshot_generations(&self) -> (Arc<ActiveGeneration>, Vec<DrainingGeneration>) {
        let generation_state = self
            .state
            .read()
            .expect("runtime topology lock should not be poisoned");
        (
            Arc::clone(&generation_state.active),
            generation_state.draining.clone(),
        )
    }

    pub(crate) fn noop_generation_reload_result(&self) -> TopologyReloadResult {
        TopologyReloadResult {
            activated_generation_id: self.active_generation_id(),
            retired_generation_ids: Vec::new(),
            applied_config_change: false,
            reconfigured_adapter_ids: Vec::new(),
        }
    }

    fn next_generation_id(&self) -> GenerationId {
        let mut generation_state = self
            .state
            .write()
            .expect("runtime topology lock should not be poisoned");
        let generation_id = GenerationId(generation_state.next_generation_id);
        generation_state.next_generation_id = generation_state.next_generation_id.saturating_add(1);
        generation_id
    }

    #[cfg(test)]
    pub(crate) fn fail_next_precommit_for_test(&self) {
        self.fail_next_precommit.store(true, Ordering::SeqCst);
    }

    pub(crate) async fn shutdown_listener_workers(&self) {
        let workers = {
            let mut generation_state = self
                .state
                .write()
                .expect("runtime topology lock should not be poisoned");
            generation_state
                .listener_workers
                .drain()
                .map(|(_, worker)| worker)
                .collect::<Vec<_>>()
        };
        Self::shutdown_workers(workers).await;
    }

    pub(crate) async fn export_tcp_listener_for_upgrade(
        &self,
    ) -> Result<std::net::TcpListener, RuntimeError> {
        let worker = {
            let mut generation_state = self
                .state
                .write()
                .expect("runtime topology lock should not be poisoned");
            generation_state.listener_workers.remove(&TransportKind::Tcp)
        }
        .ok_or_else(|| RuntimeError::Config("tcp listener worker is not active".to_string()))?;
        let mut worker = worker;
        let (ack_tx, ack_rx) = oneshot::channel();
        worker
            .control_tx
            .send(super::ListenerWorkerControl::Export { ack_tx })
            .await
            .map_err(|_| {
                RuntimeError::Config(
                    "failed to request tcp listener export from topology worker".to_string(),
                )
            })?;
        let listener = ack_rx.await.map_err(|_| {
            RuntimeError::Config("tcp listener export worker closed unexpectedly".to_string())
        })??;
        if let Some(join_handle) = worker.join_handle.take() {
            let _ = join_handle.await;
        }
        Ok(listener)
    }

    pub(crate) async fn import_tcp_listener_after_upgrade_rollback(
        &self,
        listener: std::net::TcpListener,
        sessions: &SessionRegistry,
    ) -> Result<(), RuntimeError> {
        let active = self.active_generation();
        let adapter_ids = active
            .listener_bindings
            .iter()
            .find(|binding| binding.transport == TransportKind::Tcp)
            .map(|binding| binding.adapter_ids.clone())
            .unwrap_or_else(|| {
                active
                    .protocol_registry
                    .adapter_ids_for_transport(TransportKind::Tcp)
            });
        let worker = spawn_listener_worker(
            crate::transport::BoundTransportListener::import_tcp_listener(listener, adapter_ids)?,
            active.generation_id,
            sessions.accepted_sender(),
            sessions.queued_accepts(),
        )?;
        let replaced = {
            let mut generation_state = self
                .state
                .write()
                .expect("runtime topology lock should not be poisoned");
            generation_state.listener_workers.insert(TransportKind::Tcp, worker)
        };
        if let Some(replaced) = replaced {
            Self::shutdown_workers(vec![replaced]).await;
        }
        Ok(())
    }

    pub(crate) async fn reload_generation_with_config(
        &self,
        candidate_config: crate::config::ServerConfig,
        force_generation: bool,
        protocol_topology: &RuntimeProtocolTopologyCandidate,
        kernel: &RuntimeKernel,
        sessions: &SessionRegistry,
    ) -> Result<TopologyReloadResult, RuntimeError> {
        let prepared = self
            .prepare_generation_reload(candidate_config, force_generation, protocol_topology)
            .await?;
        let prepared = self.precommit_generation_reload(prepared, sessions).await?;
        self.commit_generation_reload(prepared, kernel, sessions)
            .await
    }

    async fn shutdown_workers(workers: Vec<TopologyListenerWorker>) {
        for mut worker in workers {
            if let Some(shutdown_tx) = worker.shutdown_tx.take() {
                let shutdown_tx: tokio::sync::oneshot::Sender<()> = shutdown_tx;
                let _ = shutdown_tx.send(());
            }
            if let Some(join_handle) = worker.join_handle.take() {
                let _ = join_handle.await;
            }
        }
    }

    pub(crate) async fn prepare_generation_reload(
        &self,
        candidate_config: crate::config::ServerConfig,
        force_generation: bool,
        protocol_topology: &RuntimeProtocolTopologyCandidate,
    ) -> Result<PreparedTopologyReload, RuntimeError> {
        let active = self.active_generation();
        let applied_config_change = active.config.network != candidate_config.network
            || active.config.topology != candidate_config.topology;

        let current_signature = protocol_topology_signature(&active.protocol_registry);
        let candidate_active_protocols =
            activate_protocols(&candidate_config, protocol_topology.registry())?;
        let candidate_signature =
            protocol_topology_signature(&candidate_active_protocols.protocols);
        let protocol_buffer_limits_changed = protocol_buffer_limit_signature(&active.config)
            != protocol_buffer_limit_signature(&candidate_config);
        let mut current_managed_ids = active
            .protocol_registry
            .adapter_ids_for_transport(TransportKind::Tcp);
        current_managed_ids.extend(
            active
                .protocol_registry
                .adapter_ids_for_transport(TransportKind::Udp),
        );
        let mut current_managed_ids = current_managed_ids
            .into_iter()
            .map(|adapter_id| adapter_id.to_string())
            .collect::<Vec<_>>();
        current_managed_ids.sort();
        current_managed_ids.dedup();
        let reconfigured_adapter_ids = reconfigured_adapter_ids(
            &current_signature,
            &candidate_signature,
            &current_managed_ids,
            protocol_topology.managed_protocol_ids(),
        );
        if !force_generation
            && !applied_config_change
            && !protocol_buffer_limits_changed
            && current_signature == candidate_signature
        {
            if current_managed_ids != protocol_topology.managed_protocol_ids()
                || protocol_topology.requires_protocol_swap()
            {
                return Ok(PreparedTopologyReload::ProtocolOnly {
                    candidate_generation: Arc::new(ActiveGeneration {
                        generation_id: active.generation_id,
                        config: candidate_config.clone(),
                        protocol_registry: candidate_active_protocols.protocols.clone(),
                        default_adapter: candidate_active_protocols.default_adapter,
                        default_bedrock_adapter: candidate_active_protocols.default_bedrock_adapter,
                        listener_bindings: active.listener_bindings.clone(),
                    }),
                    result: TopologyReloadResult {
                        activated_generation_id: active.generation_id,
                        retired_generation_ids: Vec::new(),
                        applied_config_change: false,
                        reconfigured_adapter_ids,
                    },
                });
            }
            return Ok(PreparedTopologyReload::Noop(
                self.noop_generation_reload_result(),
            ));
        }

        let new_generation_id = self.next_generation_id();
        let listener_plans =
            build_listener_plans(&candidate_config, &candidate_active_protocols.protocols)?;
        let current_bindings = active.listener_bindings.clone();
        let current_tcp_binding = listener_binding_for_transport(
            &current_bindings,
            TransportKind::Tcp,
        )
        .ok_or_else(|| {
            RuntimeError::Config("active topology is missing a tcp listener binding".to_string())
        })?;
        let current_udp_binding =
            listener_binding_for_transport(&current_bindings, TransportKind::Udp);

        let mut tcp_plan = None;
        let mut udp_plan = None;
        for plan in listener_plans {
            match plan.transport {
                TransportKind::Tcp => tcp_plan = Some(plan),
                TransportKind::Udp => udp_plan = Some(plan),
            }
        }
        let tcp_plan = tcp_plan.ok_or_else(|| {
            RuntimeError::Config("no tcp listener plan was generated".to_string())
        })?;

        let mut new_bound_listeners = Vec::new();
        let mut reused_transports = HashSet::new();
        let tcp_binding =
            if can_reuse_listener(&candidate_config, tcp_plan.bind_addr, &current_tcp_binding) {
                let _ = reused_transports.insert(TransportKind::Tcp);
                current_tcp_binding
            } else {
                let listener = bind_transport_listener(tcp_plan, &candidate_config).await?;
                let binding = listener.listener_binding()?;
                new_bound_listeners.push(listener);
                binding
            };
        let tcp_local_addr = tcp_binding.local_addr;
        let mut candidate_bindings = vec![tcp_binding];

        if let Some(mut udp_plan) = udp_plan {
            if udp_plan.bind_addr.port() == 0 {
                udp_plan.bind_addr = SocketAddr::new(tcp_local_addr.ip(), tcp_local_addr.port());
            }
            let udp_binding = if let Some(current_udp_binding) = current_udp_binding {
                if can_reuse_listener(&candidate_config, udp_plan.bind_addr, &current_udp_binding) {
                    let _ = reused_transports.insert(TransportKind::Udp);
                    current_udp_binding
                } else {
                    let listener = bind_transport_listener(udp_plan, &candidate_config).await?;
                    let binding = listener.listener_binding()?;
                    new_bound_listeners.push(listener);
                    binding
                }
            } else {
                let listener = bind_transport_listener(udp_plan, &candidate_config).await?;
                let binding = listener.listener_binding()?;
                new_bound_listeners.push(listener);
                binding
            };
            candidate_bindings.push(udp_binding);
        }

        Ok(PreparedTopologyReload::Generation {
            candidate_generation: Arc::new(ActiveGeneration {
                generation_id: new_generation_id,
                config: candidate_config.clone(),
                protocol_registry: candidate_active_protocols.protocols.clone(),
                default_adapter: candidate_active_protocols.default_adapter,
                default_bedrock_adapter: candidate_active_protocols.default_bedrock_adapter,
                listener_bindings: candidate_bindings,
            }),
            new_bound_listeners,
            reused_transports,
            applied_config_change,
            reconfigured_adapter_ids,
            drain_grace_secs: candidate_config.topology.drain_grace_secs,
        })
    }

    pub(crate) async fn commit_generation_reload(
        &self,
        prepared_reload: PrecommittedTopologyReload,
        kernel: &RuntimeKernel,
        sessions: &SessionRegistry,
    ) -> Result<TopologyReloadResult, RuntimeError> {
        match prepared_reload {
            PrecommittedTopologyReload::Noop(result) => Ok(result),
            PrecommittedTopologyReload::ProtocolOnly {
                candidate_generation,
                result,
            } => {
                {
                    let mut generation_state = self
                        .state
                        .write()
                        .expect("runtime topology lock should not be poisoned");
                    generation_state.active = candidate_generation;
                }
                Ok(result)
            }
            PrecommittedTopologyReload::Generation {
                candidate_generation,
                new_listener_workers,
                reused_transports,
                applied_config_change,
                reconfigured_adapter_ids,
                drain_grace_secs,
            } => {
                let new_generation_id = candidate_generation.generation_id;
                let workers_to_shutdown = {
                    let mut generation_state = self
                        .state
                        .write()
                        .expect("runtime topology lock should not be poisoned");
                    let previous_active = Arc::clone(&generation_state.active);
                    let mut workers_to_shutdown = Vec::new();

                    generation_state.active = Arc::clone(&candidate_generation);
                    generation_state.draining.push(DrainingGeneration {
                        generation: previous_active,
                        drain_deadline_ms: now_ms()
                            .saturating_add(drain_grace_secs.saturating_mul(1_000)),
                    });

                    for transport in [TransportKind::Tcp, TransportKind::Udp] {
                        if reused_transports.contains(&transport) {
                            if let Some(worker) = generation_state.listener_workers.get(&transport)
                            {
                                let _ = worker.generation_tx.send(new_generation_id);
                            }
                            continue;
                        }
                        if let Some(worker) = generation_state.listener_workers.remove(&transport) {
                            workers_to_shutdown.push(worker);
                        }
                    }
                    for worker in new_listener_workers.into_values() {
                        generation_state
                            .listener_workers
                            .insert(worker.transport, worker);
                    }
                    workers_to_shutdown
                };

                kernel
                    .set_max_players(candidate_generation.config.network.max_players)
                    .await;
                Self::shutdown_workers(workers_to_shutdown).await;
                let retired_generation_ids = self.retire_drained_generations(sessions).await;
                Ok(TopologyReloadResult {
                    activated_generation_id: new_generation_id,
                    retired_generation_ids,
                    applied_config_change,
                    reconfigured_adapter_ids,
                })
            }
        }
    }

    pub(crate) async fn precommit_generation_reload(
        &self,
        prepared_reload: PreparedTopologyReload,
        sessions: &SessionRegistry,
    ) -> Result<PrecommittedTopologyReload, RuntimeError> {
        match prepared_reload {
            PreparedTopologyReload::Noop(result) => Ok(PrecommittedTopologyReload::Noop(result)),
            PreparedTopologyReload::ProtocolOnly {
                candidate_generation,
                result,
            } => Ok(PrecommittedTopologyReload::ProtocolOnly {
                candidate_generation,
                result,
            }),
            PreparedTopologyReload::Generation {
                candidate_generation,
                new_bound_listeners,
                reused_transports,
                applied_config_change,
                reconfigured_adapter_ids,
                drain_grace_secs,
            } => {
                let mut new_listener_workers = HashMap::new();
                for listener in new_bound_listeners {
                    let worker = match spawn_listener_worker(
                        listener,
                        candidate_generation.generation_id,
                        sessions.accepted_sender(),
                        sessions.queued_accepts(),
                    ) {
                        Ok(worker) => worker,
                        Err(error) => {
                            Self::shutdown_workers(
                                new_listener_workers.into_values().collect::<Vec<_>>(),
                            )
                            .await;
                            return Err(error);
                        }
                    };
                    if new_listener_workers
                        .insert(worker.transport, worker)
                        .is_some()
                    {
                        Self::shutdown_workers(
                            new_listener_workers.into_values().collect::<Vec<_>>(),
                        )
                        .await;
                        return Err(RuntimeError::Config(
                            "multiple listener workers for the same transport are not supported"
                                .to_string(),
                        ));
                    }
                }
                #[cfg(test)]
                if self.fail_next_precommit.swap(false, Ordering::SeqCst) {
                    Self::shutdown_workers(new_listener_workers.into_values().collect::<Vec<_>>())
                        .await;
                    return Err(RuntimeError::Config(
                        "injected topology precommit failure".to_string(),
                    ));
                }
                Ok(PrecommittedTopologyReload::Generation {
                    candidate_generation,
                    new_listener_workers,
                    reused_transports,
                    applied_config_change,
                    reconfigured_adapter_ids,
                    drain_grace_secs,
                })
            }
        }
    }

    pub(crate) fn rollback_generation_reload(&self, prepared_reload: PreparedTopologyReload) {
        drop(prepared_reload);
    }

    pub(crate) async fn enforce_generation_drains(
        &self,
        sessions: &SessionRegistry,
    ) -> Result<(), RuntimeError> {
        let expired_generation_ids = {
            let generation_state = self
                .state
                .read()
                .expect("runtime topology lock should not be poisoned");
            let now = now_ms();
            generation_state
                .draining
                .iter()
                .filter(|entry| entry.drain_deadline_ms <= now)
                .map(|entry| entry.generation.generation_id)
                .collect::<Vec<_>>()
        };
        if expired_generation_ids.is_empty() {
            let _ = self.retire_drained_generations(sessions).await;
            return Ok(());
        }

        let session_handles = sessions
            .handles_for_generations(&expired_generation_ids)
            .await;
        for handle in session_handles {
            let _ = handle
                .control_tx
                .send(SessionControl::Terminate {
                    reason: "Server generation reloaded".to_string(),
                })
                .await;
        }
        let _ = self.retire_drained_generations(sessions).await;
        Ok(())
    }

    pub(crate) async fn retire_drained_generations(
        &self,
        sessions: &SessionRegistry,
    ) -> Vec<GenerationId> {
        let live_generations = sessions.live_generation_ids().await;
        let mut generation_state = self
            .state
            .write()
            .expect("runtime topology lock should not be poisoned");
        let mut retired = Vec::new();
        generation_state.draining.retain(|entry| {
            let keep = live_generations.contains(&entry.generation.generation_id);
            if !keep {
                retired.push(entry.generation.generation_id);
            }
            keep
        });
        retired
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProtocolTopologyEntry {
    adapter_id: String,
    transport: TransportKind,
    edition: Edition,
    protocol_number: i32,
    wire_format: WireFormatKind,
    bedrock_listener_descriptor: Option<BedrockListenerDescriptor>,
}

fn protocol_topology_signature(protocols: &ProtocolRegistry) -> Vec<ProtocolTopologyEntry> {
    let mut adapter_ids = protocols.adapter_ids_for_transport(TransportKind::Tcp);
    adapter_ids.extend(protocols.adapter_ids_for_transport(TransportKind::Udp));
    adapter_ids.sort();
    adapter_ids.dedup();
    adapter_ids
        .into_iter()
        .filter_map(|adapter_id| {
            let adapter = protocols.resolve_adapter(adapter_id.as_str())?;
            let descriptor = adapter.descriptor();
            Some(ProtocolTopologyEntry {
                adapter_id: adapter_id.to_string(),
                transport: descriptor.transport,
                edition: descriptor.edition,
                protocol_number: descriptor.protocol_number,
                wire_format: descriptor.wire_format,
                bedrock_listener_descriptor: adapter.bedrock_listener_descriptor(),
            })
        })
        .collect()
}

fn reconfigured_adapter_ids(
    current: &[ProtocolTopologyEntry],
    candidate: &[ProtocolTopologyEntry],
    current_managed_ids: &[String],
    candidate_managed_ids: &[String],
) -> Vec<String> {
    let current_map = current
        .iter()
        .map(|entry| (entry.adapter_id.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    let candidate_map = candidate
        .iter()
        .map(|entry| (entry.adapter_id.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut ids = current_map
        .keys()
        .chain(candidate_map.keys())
        .cloned()
        .collect::<Vec<_>>();
    ids.extend(current_managed_ids.iter().cloned());
    ids.extend(candidate_managed_ids.iter().cloned());
    ids.sort();
    ids.dedup();
    ids.into_iter()
        .filter(|adapter_id| {
            current_map.get(adapter_id) != candidate_map.get(adapter_id)
                || current_managed_ids.binary_search(adapter_id).is_err()
                || candidate_managed_ids.binary_search(adapter_id).is_err()
        })
        .collect()
}

fn protocol_buffer_limit_signature(config: &crate::config::ServerConfig) -> (usize, usize) {
    (
        config.plugins.buffer_limits.protocol_response_bytes,
        config.plugins.buffer_limits.metadata_bytes,
    )
}

fn listener_binding_for_transport(
    bindings: &[ListenerBinding],
    transport: TransportKind,
) -> Option<ListenerBinding> {
    bindings
        .iter()
        .find(|binding| binding.transport == transport)
        .cloned()
}

fn can_reuse_listener(
    config: &crate::config::ServerConfig,
    desired_addr: SocketAddr,
    current_binding: &ListenerBinding,
) -> bool {
    let same_bind_ip = |left: IpAddr, right: IpAddr| {
        left == right || (left.is_unspecified() && right.is_unspecified())
    };
    if config.network.server_port == 0 {
        return same_bind_ip(current_binding.local_addr.ip(), desired_addr.ip());
    }
    current_binding.local_addr.port() == desired_addr.port()
        && same_bind_ip(current_binding.local_addr.ip(), desired_addr.ip())
}
