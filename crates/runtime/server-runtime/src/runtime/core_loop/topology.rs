use crate::RuntimeError;
use crate::runtime::bootstrap::{activate_protocols, spawn_listener_worker};
use crate::runtime::{
    RuntimeServer, RuntimeTopologyGeneration, TopologyGenerationId, TopologyReloadResult, now_ms,
};
use crate::transport::{bind_transport_listener, build_listener_plans};
use mc_plugin_host::registry::{ListenerBinding, ProtocolRegistry};
use mc_plugin_host::runtime::RuntimePluginHost;
use mc_proto_common::{BedrockListenerDescriptor, Edition, TransportKind, WireFormatKind};
use std::collections::{BTreeMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

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
            let adapter = protocols.resolve_adapter(&adapter_id)?;
            let descriptor = adapter.descriptor();
            Some(ProtocolTopologyEntry {
                adapter_id,
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
    config.server_port != 0 && current_binding.local_addr == desired_addr
}

impl RuntimeServer {
    pub(in crate::runtime) fn listener_bindings(&self) -> Vec<ListenerBinding> {
        self.active_topology().listener_bindings.clone()
    }

    pub(in crate::runtime) fn active_topology(&self) -> Arc<RuntimeTopologyGeneration> {
        Arc::clone(
            &self
                .topology
                .read()
                .expect("runtime topology lock should not be poisoned")
                .active,
        )
    }

    pub(in crate::runtime) fn active_topology_generation_id(&self) -> TopologyGenerationId {
        self.active_topology().generation_id
    }

    pub(in crate::runtime) fn topology_generation(
        &self,
        generation_id: TopologyGenerationId,
    ) -> Option<Arc<RuntimeTopologyGeneration>> {
        let topology = self
            .topology
            .read()
            .expect("runtime topology lock should not be poisoned");
        if topology.active.generation_id == generation_id {
            return Some(Arc::clone(&topology.active));
        }
        topology
            .draining
            .iter()
            .find(|entry| entry.generation.generation_id == generation_id)
            .map(|entry| Arc::clone(&entry.generation))
    }

    pub(in crate::runtime) fn noop_topology_reload_result(&self) -> TopologyReloadResult {
        TopologyReloadResult {
            activated_generation_id: self.active_topology_generation_id(),
            retired_generation_ids: Vec::new(),
            applied_config_change: false,
            reconfigured_adapter_ids: Vec::new(),
        }
    }

    fn next_topology_generation_id(&self) -> TopologyGenerationId {
        let mut topology = self
            .topology
            .write()
            .expect("runtime topology lock should not be poisoned");
        let generation_id = TopologyGenerationId(topology.next_generation_id);
        topology.next_generation_id = topology.next_generation_id.saturating_add(1);
        generation_id
    }

    pub(in crate::runtime) async fn shutdown_listener_workers(&self) {
        let workers = {
            let mut topology = self
                .topology
                .write()
                .expect("runtime topology lock should not be poisoned");
            topology
                .listener_workers
                .drain()
                .map(|(_, worker)| worker)
                .collect::<Vec<_>>()
        };
        for mut worker in workers {
            if let Some(shutdown_tx) = worker.shutdown_tx.take() {
                let _ = shutdown_tx.send(());
            }
            if let Some(join_handle) = worker.join_handle.take() {
                let _ = join_handle.await;
            }
        }
    }

    pub(in crate::runtime) async fn terminate_all_sessions(&self, reason: &str) {
        let handles = self
            .sessions
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for handle in handles {
            let _ = handle.tx.send(crate::runtime::SessionMessage::Terminate {
                reason: reason.to_string(),
            });
        }
    }

    pub(in crate::runtime) async fn maybe_reload_topology_watch(
        &self,
        reload_host: &dyn RuntimePluginHost,
    ) -> Result<TopologyReloadResult, RuntimeError> {
        let loaded = self.config_source.load()?;
        let active = self.active_topology();
        if !loaded.topology_reload_watch && !active.config.topology_reload_watch {
            return Ok(self.noop_topology_reload_result());
        }
        self.reload_topology_with_config(reload_host, loaded).await
    }

    pub(in crate::runtime) async fn reload_topology(
        &self,
        reload_host: &dyn RuntimePluginHost,
    ) -> Result<TopologyReloadResult, RuntimeError> {
        let loaded = self.config_source.load()?;
        self.reload_topology_with_config(reload_host, loaded).await
    }

    async fn reload_topology_with_config(
        &self,
        reload_host: &dyn RuntimePluginHost,
        loaded_config: crate::config::ServerConfig,
    ) -> Result<TopologyReloadResult, RuntimeError> {
        let active = self.active_topology();
        let mut candidate_config = active.config.clone();
        let applied_config_change = candidate_config.apply_topology_from(&loaded_config);

        let prepared = reload_host.prepare_protocol_topology_for_reload()?;
        let current_signature = protocol_topology_signature(&active.protocol_registry);
        let candidate_active_protocols =
            activate_protocols(&candidate_config, prepared.registry())?;
        let candidate_signature =
            protocol_topology_signature(&candidate_active_protocols.protocols);
        let current_managed_ids = reload_host.managed_protocol_ids();
        let reconfigured_adapter_ids = reconfigured_adapter_ids(
            &current_signature,
            &candidate_signature,
            &current_managed_ids,
            prepared.managed_protocol_ids(),
        );
        if !applied_config_change && current_signature == candidate_signature {
            if current_managed_ids != prepared.managed_protocol_ids() {
                reload_host.activate_protocol_topology(prepared);
                return Ok(TopologyReloadResult {
                    activated_generation_id: active.generation_id,
                    retired_generation_ids: Vec::new(),
                    applied_config_change: false,
                    reconfigured_adapter_ids,
                });
            }
            return Ok(self.noop_topology_reload_result());
        }

        let new_generation_id = self.next_topology_generation_id();
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

        let candidate_generation = Arc::new(RuntimeTopologyGeneration {
            generation_id: new_generation_id,
            config: candidate_config.clone(),
            protocol_registry: candidate_active_protocols.protocols.clone(),
            default_adapter: candidate_active_protocols.default_adapter,
            default_bedrock_adapter: candidate_active_protocols.default_bedrock_adapter,
            listener_bindings: candidate_bindings,
        });

        let workers_to_shutdown = {
            let mut topology = self
                .topology
                .write()
                .expect("runtime topology lock should not be poisoned");
            let previous_active = Arc::clone(&topology.active);
            let mut workers_to_shutdown = Vec::new();

            topology.active = Arc::clone(&candidate_generation);
            topology
                .draining
                .push(crate::runtime::DrainingTopologyGeneration {
                    generation: previous_active,
                    drain_deadline_ms: now_ms().saturating_add(
                        candidate_config
                            .topology_drain_grace_secs
                            .saturating_mul(1_000),
                    ),
                });

            for transport in [TransportKind::Tcp, TransportKind::Udp] {
                if reused_transports.contains(&transport) {
                    if let Some(worker) = topology.listener_workers.get(&transport) {
                        let _ = worker.generation_tx.send(new_generation_id);
                    }
                    continue;
                }
                if let Some(worker) = topology.listener_workers.remove(&transport) {
                    workers_to_shutdown.push(worker);
                }
            }
            for listener in new_bound_listeners {
                let worker =
                    spawn_listener_worker(listener, new_generation_id, self.accepted_tx.clone())?;
                topology.listener_workers.insert(worker.transport, worker);
            }
            workers_to_shutdown
        };

        reload_host.activate_protocol_topology(prepared);
        {
            let mut state = self.state.lock().await;
            state.core.set_max_players(candidate_config.max_players);
        }
        for mut worker in workers_to_shutdown {
            if let Some(shutdown_tx) = worker.shutdown_tx.take() {
                let _ = shutdown_tx.send(());
            }
            if let Some(join_handle) = worker.join_handle.take() {
                let _ = join_handle.await;
            }
        }
        let retired_generation_ids = self.retire_drained_topologies().await;
        Ok(TopologyReloadResult {
            activated_generation_id: new_generation_id,
            retired_generation_ids,
            applied_config_change,
            reconfigured_adapter_ids,
        })
    }

    pub(in crate::runtime) async fn enforce_topology_drains(&self) -> Result<(), RuntimeError> {
        let expired_generation_ids = {
            let topology = self
                .topology
                .read()
                .expect("runtime topology lock should not be poisoned");
            let now = now_ms();
            topology
                .draining
                .iter()
                .filter(|entry| entry.drain_deadline_ms <= now)
                .map(|entry| entry.generation.generation_id)
                .collect::<Vec<_>>()
        };
        if expired_generation_ids.is_empty() {
            let _ = self.retire_drained_topologies().await;
            return Ok(());
        }

        let session_handles = {
            let sessions = self.sessions.lock().await;
            sessions
                .values()
                .filter(|handle| expired_generation_ids.contains(&handle.topology_generation_id))
                .cloned()
                .collect::<Vec<_>>()
        };
        for handle in session_handles {
            let _ = handle.tx.send(crate::runtime::SessionMessage::Terminate {
                reason: "Server topology reloaded".to_string(),
            });
        }
        let _ = self.retire_drained_topologies().await;
        Ok(())
    }

    pub(in crate::runtime) async fn retire_drained_topologies(&self) -> Vec<TopologyGenerationId> {
        let active_generations = {
            self.sessions
                .lock()
                .await
                .values()
                .map(|handle| handle.topology_generation_id)
                .collect::<HashSet<_>>()
        };
        let mut topology = self
            .topology
            .write()
            .expect("runtime topology lock should not be poisoned");
        let mut retired = Vec::new();
        topology.draining.retain(|entry| {
            let keep = active_generations.contains(&entry.generation.generation_id);
            if !keep {
                retired.push(entry.generation.generation_id);
            }
            keep
        });
        retired
    }
}
