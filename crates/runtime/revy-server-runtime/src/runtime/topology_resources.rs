use super::{
    ActiveGeneration, DrainingGeneration, GenerationAdmission, GenerationId,
    ListenerIngressCommand, SessionControl, TopologyListenerWorker, TopologyReloadResult,
    TopologyResourceState, now_ms,
};
use crate::runtime::bootstrap::{
    activate_protocols, bind_ephemeral_transport_pair, spawn_paused_listener_worker,
};
use crate::runtime::session_registry::SessionRegistry;
use crate::transport::{bind_transport_listener, build_listener_plans};
use crate::{ListenerBinding, RuntimeError};
use mc_plugin_host::registry::ProtocolRegistry;
use mc_plugin_host::runtime::RuntimeProtocolTopologyCandidate;
use mc_proto_common::{BedrockListenerDescriptor, Edition, TransportKind, WireFormatKind};
use revy_raknet::PeerSnapshot;
use revy_runtime_transfer::SocketTransferTarget;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;

pub(crate) struct TopologyResources {
    state: std::sync::RwLock<TopologyResourceState>,
}

pub(crate) struct FrozenTopologyIngress {
    controls: Vec<mpsc::Sender<ListenerIngressCommand>>,
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

pub(crate) struct CommittedTopologyReload {
    pub(crate) result: TopologyReloadResult,
    workers_to_shutdown: Vec<TopologyListenerWorker>,
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

impl TopologyResources {
    pub(crate) fn new(
        listener_workers: HashMap<TransportKind, TopologyListenerWorker>,
        next_generation_id: u64,
    ) -> Self {
        Self {
            state: std::sync::RwLock::new(TopologyResourceState {
                draining: Vec::new(),
                listener_workers,
                next_generation_id,
            }),
        }
    }

    pub(crate) fn generation_admission(
        &self,
        active: &Arc<ActiveGeneration>,
        generation_id: GenerationId,
    ) -> GenerationAdmission {
        let generation_state = self
            .state
            .read()
            .expect("runtime topology lock should not be poisoned");
        if active.generation_id == generation_id {
            return GenerationAdmission::Active(Arc::clone(active));
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

    pub(crate) async fn freeze_ingress(&self) -> Result<FrozenTopologyIngress, RuntimeError> {
        let controls = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .listener_workers
            .values()
            .map(|worker| worker.ingress_tx.clone())
            .collect::<Vec<_>>();
        if let Err(error) = set_listener_ingress(&controls, true).await {
            let _ = set_listener_ingress(&controls, false).await;
            return Err(error);
        }
        Ok(FrozenTopologyIngress { controls })
    }

    pub(crate) async fn activate_imported_listeners(&self) -> Result<(), RuntimeError> {
        let (serving, controls) = {
            let state = self
                .state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                state
                    .listener_workers
                    .values()
                    .map(|worker| worker.serving_tx.clone())
                    .collect::<Vec<_>>(),
                state
                    .listener_workers
                    .values()
                    .map(|worker| worker.ingress_tx.clone())
                    .collect::<Vec<_>>(),
            )
        };
        for serving_tx in serving {
            serving_tx.send(true).map_err(|_| {
                RuntimeError::Config(
                    "imported listener closed before authority publication".to_string(),
                )
            })?;
        }
        set_listener_ingress(&controls, false).await
    }

    pub(crate) async fn duplicate_for_process_transfer(
        &self,
        target: SocketTransferTarget,
    ) -> Result<Vec<super::ExecutableListenerResource>, RuntimeError> {
        let controls = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .listener_workers
            .values()
            .map(|worker| worker.ingress_tx.clone())
            .collect::<Vec<_>>();
        let mut pending = JoinSet::new();
        for control in controls {
            pending.spawn(async move {
                let (ack_tx, ack_rx) = oneshot::channel();
                control
                    .send(ListenerIngressCommand::DuplicateForProcessTransfer { target, ack_tx })
                    .await
                    .map_err(|_| {
                        RuntimeError::Config(
                            "listener closed before process-transfer duplication".to_string(),
                        )
                    })?;
                ack_rx.await.map_err(|_| {
                    RuntimeError::Config(
                        "listener dropped process-transfer duplication acknowledgement".to_string(),
                    )
                })?
            });
        }
        let mut resources = Vec::with_capacity(pending.len());
        while let Some(result) = pending.join_next().await {
            resources.push(result.map_err(RuntimeError::from)??);
        }
        resources.sort_by_key(|resource| match resource.binding().transport {
            TransportKind::Tcp => 0,
            TransportKind::Udp => 1,
        });
        Ok(resources)
    }

    pub(crate) fn draining_generations(&self) -> Vec<DrainingGeneration> {
        let generation_state = self
            .state
            .read()
            .expect("runtime topology lock should not be poisoned");
        generation_state.draining.clone()
    }

    pub(crate) fn noop_generation_reload_result(
        &self,
        active: &Arc<ActiveGeneration>,
    ) -> TopologyReloadResult {
        TopologyReloadResult {
            activated_generation_id: active.generation_id,
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
        active: &Arc<ActiveGeneration>,
        candidate_config: crate::config::ServerConfig,
        force_generation: bool,
        protocol_topology: &RuntimeProtocolTopologyCandidate,
    ) -> Result<PreparedTopologyReload, RuntimeError> {
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
                self.noop_generation_reload_result(active),
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
        let reuse_tcp =
            can_reuse_listener(&candidate_config, tcp_plan.bind_addr, &current_tcp_binding);
        let candidate_bindings = match (tcp_plan.bind_addr.port(), udp_plan) {
            // Port zero requests an OS allocation, not retention of the previously chosen
            // number. Adding UDP (or changing the bind address) needs a new joint allocation:
            // an existing TCP-only port may be reserved or occupied for UDP. Established TCP
            // streams remain session-owned while both inactive candidate listeners are bound.
            (0, Some(udp_plan)) if !reuse_tcp || current_udp_binding.is_none() => {
                new_bound_listeners =
                    bind_ephemeral_transport_pair(tcp_plan, udp_plan, &candidate_config).await?;
                new_bound_listeners
                    .iter()
                    .map(crate::transport::BoundTransportListener::listener_binding)
                    .collect::<Result<Vec<_>, _>>()?
            }
            (_, udp_plan) => {
                let tcp_binding = if reuse_tcp {
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
                        udp_plan.bind_addr =
                            SocketAddr::new(tcp_local_addr.ip(), tcp_local_addr.port());
                    }
                    let udp_binding = if let Some(current_udp_binding) = current_udp_binding {
                        if can_reuse_listener(
                            &candidate_config,
                            udp_plan.bind_addr,
                            &current_udp_binding,
                        ) {
                            let _ = reused_transports.insert(TransportKind::Udp);
                            current_udp_binding
                        } else {
                            let listener =
                                bind_transport_listener(udp_plan, &candidate_config).await?;
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
                candidate_bindings
            }
        };

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

    pub(crate) async fn precommit_generation_reload(
        &self,
        prepared: PreparedTopologyReload,
        sessions: &SessionRegistry,
    ) -> Result<PrecommittedTopologyReload, RuntimeError> {
        match prepared {
            PreparedTopologyReload::Noop(result) => Ok(PrecommittedTopologyReload::Noop(result)),
            PreparedTopologyReload::ProtocolOnly {
                candidate_generation: _,
                result,
            } => Ok(PrecommittedTopologyReload::ProtocolOnly { result }),
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
                    let worker = match spawn_paused_listener_worker(
                        listener,
                        candidate_generation.generation_id,
                        sessions.accepted_sender(),
                        sessions.queued_accepts(),
                    ) {
                        Ok(worker) => worker,
                        Err(error) => {
                            Self::shutdown_workers(new_listener_workers.into_values().collect())
                                .await;
                            return Err(error);
                        }
                    };
                    if new_listener_workers
                        .insert(worker.transport, worker)
                        .is_some()
                    {
                        Self::shutdown_workers(new_listener_workers.into_values().collect()).await;
                        return Err(RuntimeError::Config(
                            "multiple candidate listeners for one transport".to_string(),
                        ));
                    }
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

    pub(crate) fn commit_generation_resources(
        &self,
        previous_active: Arc<ActiveGeneration>,
        prepared: PrecommittedTopologyReload,
    ) -> CommittedTopologyReload {
        match prepared {
            PrecommittedTopologyReload::Noop(result) => CommittedTopologyReload {
                result,
                workers_to_shutdown: Vec::new(),
            },
            PrecommittedTopologyReload::ProtocolOnly { result } => CommittedTopologyReload {
                result,
                workers_to_shutdown: Vec::new(),
            },
            PrecommittedTopologyReload::Generation {
                candidate_generation,
                new_listener_workers,
                reused_transports,
                applied_config_change,
                reconfigured_adapter_ids,
                drain_grace_secs,
            } => {
                let new_generation_id = candidate_generation.generation_id;
                let mut state = self
                    .state
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.draining.push(DrainingGeneration {
                    generation: previous_active,
                    drain_deadline_ms: now_ms()
                        .saturating_add(drain_grace_secs.saturating_mul(1_000)),
                });
                let mut workers_to_shutdown = Vec::new();
                for transport in [TransportKind::Tcp, TransportKind::Udp] {
                    if reused_transports.contains(&transport) {
                        if let Some(worker) = state.listener_workers.get(&transport) {
                            let _ = worker.generation_tx.send(new_generation_id);
                        }
                    } else if let Some(worker) = state.listener_workers.remove(&transport) {
                        workers_to_shutdown.push(worker);
                    }
                }
                for worker in new_listener_workers.into_values() {
                    let _ = worker.serving_tx.send(true);
                    state.listener_workers.insert(worker.transport, worker);
                }
                CommittedTopologyReload {
                    result: TopologyReloadResult {
                        activated_generation_id: new_generation_id,
                        retired_generation_ids: Vec::new(),
                        applied_config_change,
                        reconfigured_adapter_ids,
                    },
                    workers_to_shutdown,
                }
            }
        }
    }

    pub(crate) async fn abort_precommitted_reload(&self, prepared: PrecommittedTopologyReload) {
        if let PrecommittedTopologyReload::Generation {
            new_listener_workers,
            ..
        } = prepared
        {
            Self::shutdown_workers(new_listener_workers.into_values().collect()).await;
        }
    }

    pub(crate) async fn finish_committed_reload(
        &self,
        mut committed: CommittedTopologyReload,
        sessions: &SessionRegistry,
    ) -> TopologyReloadResult {
        Self::shutdown_workers(std::mem::take(&mut committed.workers_to_shutdown)).await;
        committed.result.retired_generation_ids = self.retire_drained_generations(sessions).await;
        committed.result
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
                .send(SessionControl::EnforceGenerationDrain)
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

impl FrozenTopologyIngress {
    pub(crate) async fn seal_process_transfer(&self) -> Result<Vec<PeerSnapshot>, RuntimeError> {
        let mut transitions = JoinSet::new();
        for control in self.controls.iter().cloned() {
            transitions.spawn(async move {
                let (ack_tx, ack_rx) = oneshot::channel();
                control
                    .send(ListenerIngressCommand::SealProcessTransfer { ack_tx })
                    .await
                    .map_err(|_| {
                        RuntimeError::Config(
                            "listener closed before process-transfer seal".to_string(),
                        )
                    })?;
                ack_rx.await.map_err(|_| {
                    RuntimeError::Config(
                        "listener dropped process-transfer seal acknowledgement".to_string(),
                    )
                })?
            });
        }
        let mut peers = Vec::new();
        let mut first_error = None;
        while let Some(result) = transitions.join_next().await {
            match result {
                Ok(Ok(mut sealed)) => peers.append(&mut sealed),
                Ok(Err(error)) if first_error.is_none() => first_error = Some(error),
                Err(error) if first_error.is_none() => {
                    first_error = Some(RuntimeError::from(error));
                }
                Ok(Err(_)) | Err(_) => {}
            }
        }
        first_error.map_or_else(|| Ok(peers), Err)
    }

    pub(crate) async fn resume(self) -> Result<(), RuntimeError> {
        set_listener_ingress(&self.controls, false).await
    }
}

async fn set_listener_ingress(
    controls: &[mpsc::Sender<ListenerIngressCommand>],
    pause: bool,
) -> Result<(), RuntimeError> {
    let mut transitions = JoinSet::new();
    for control in controls.iter().cloned() {
        transitions.spawn(async move {
            let (ack_tx, ack_rx) = oneshot::channel();
            let command = if pause {
                ListenerIngressCommand::Pause { ack_tx }
            } else {
                ListenerIngressCommand::Resume { ack_tx }
            };
            control.send(command).await.map_err(|_| {
                RuntimeError::Config("listener ingress authority is closed".to_string())
            })?;
            ack_rx.await.map_err(|_| {
                RuntimeError::Config(
                    "listener ingress transition acknowledgement was dropped".to_string(),
                )
            })?
        });
    }
    let mut first_error = None;
    while let Some(result) = transitions.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) if first_error.is_none() => first_error = Some(error),
            Err(error) if first_error.is_none() => first_error = Some(RuntimeError::from(error)),
            Ok(Err(_)) | Err(_) => {}
        }
    }
    first_error.map_or(Ok(()), Err)
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
