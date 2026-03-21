use super::{
    Arc, HashMap, HotSwappableProtocolAdapter, ManagedProtocolPlugin, PluginFailureStage,
    PluginHost, PluginKind, ProtocolRegistry, RuntimeError,
};

pub(crate) struct PreparedProtocolTopology {
    pub(crate) registry: ProtocolRegistry,
    pub(crate) adapter_ids: Vec<String>,
    pub(crate) managed: HashMap<String, ManagedProtocolPlugin>,
}

impl PluginHost {
    fn prepare_protocol_topology_with_stage(
        &self,
        stage: PluginFailureStage,
    ) -> Result<PreparedProtocolTopology, RuntimeError> {
        let catalog = self.protocol_catalog()?;
        let mut registry = ProtocolRegistry::new();
        let mut managed = HashMap::new();
        let mut adapter_ids = Vec::new();
        for package in catalog.packages() {
            if package.plugin_kind != PluginKind::Protocol {
                continue;
            }
            let modified_at = package.modified_at()?;
            let identity = package.artifact_identity(modified_at);
            if self
                .failures
                .is_artifact_quarantined(&package.plugin_id, &identity)
            {
                if let Some(reason) = self.failures.artifact_reason(&package.plugin_id, &identity) {
                    eprintln!(
                        "skipping quarantined protocol artifact `{}` during {}: {reason}",
                        package.plugin_id,
                        stage.as_str()
                    );
                }
                continue;
            }
            let generation = match self
                .loader
                .load_protocol_generation(package, self.generations.next_generation_id())
            {
                Ok(generation) => Arc::new(generation),
                Err(error) => {
                    let reason = error.to_string();
                    eprintln!(
                        "protocol {} load failed for `{}`: {reason}",
                        stage.as_str(),
                        package.plugin_id
                    );
                    self.failures.handle_candidate_failure(
                        PluginKind::Protocol,
                        stage,
                        &package.plugin_id,
                        identity,
                        &reason,
                    )?;
                    continue;
                }
            };
            let adapter = Arc::new(HotSwappableProtocolAdapter::new(
                package.plugin_id.clone(),
                generation,
                Arc::clone(&self.failures),
            ));
            registry.register_adapter(adapter.clone());
            registry.register_probe(adapter.clone());
            adapter_ids.push(package.plugin_id.clone());
            managed.insert(
                package.plugin_id.clone(),
                ManagedProtocolPlugin {
                    package: package.clone(),
                    adapter,
                    loaded_at: modified_at,
                    active_loaded_at: modified_at,
                },
            );
        }
        adapter_ids.sort();
        Ok(PreparedProtocolTopology {
            registry,
            adapter_ids,
            managed,
        })
    }

    pub(crate) fn prepare_protocol_topology_for_boot(
        &self,
    ) -> Result<PreparedProtocolTopology, RuntimeError> {
        self.prepare_protocol_topology_with_stage(PluginFailureStage::Boot)
    }

    pub(crate) fn prepare_protocol_topology_for_reload(
        &self,
    ) -> Result<PreparedProtocolTopology, RuntimeError> {
        self.prepare_protocol_topology_with_stage(PluginFailureStage::Reload)
    }

    pub(crate) fn activate_protocol_topology(&self, candidate: PreparedProtocolTopology) {
        *self
            .protocols
            .lock()
            .expect("plugin host mutex should not be poisoned") = candidate.managed;
    }
}
