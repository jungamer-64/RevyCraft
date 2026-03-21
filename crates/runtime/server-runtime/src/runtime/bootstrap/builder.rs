use super::listeners::{bind_runtime_listeners, spawn_listener_workers};
use super::r#loop::spawn_runtime_loop;
use super::profiles::{RuntimeProfiles, resolve_runtime_profiles};
use super::protocols::{ActiveProtocols, activate_protocols};
use crate::RuntimeError;
use crate::config::ServerConfigSource;
use crate::runtime::{
    RuntimeServer, RuntimeState, RuntimeTopologyGeneration, RuntimeTopologyState,
    TopologyGenerationId,
};
use mc_plugin_host::registry::LoadedPluginSet;
use mc_plugin_host::runtime::RuntimePluginHost;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, oneshot};

pub struct ServerBuilder {
    config_source: ServerConfigSource,
    loaded_plugins: LoadedPluginSet,
}

pub struct ReloadableServerBuilder {
    config_source: ServerConfigSource,
    loaded_plugins: LoadedPluginSet,
    reload_host: Arc<dyn RuntimePluginHost>,
}

impl ServerBuilder {
    #[must_use]
    pub fn new(config_source: ServerConfigSource, loaded_plugins: LoadedPluginSet) -> Self {
        Self {
            config_source,
            loaded_plugins,
        }
    }

    #[must_use]
    pub fn with_reload_host(
        self,
        reload_host: Arc<dyn RuntimePluginHost>,
    ) -> ReloadableServerBuilder {
        ReloadableServerBuilder {
            config_source: self.config_source,
            loaded_plugins: self.loaded_plugins,
            reload_host,
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the server cannot bind, load its persisted
    /// world state, or starts with unsupported configuration such as an auth
    /// profile mode mismatch.
    pub async fn build(self) -> Result<crate::runtime::RunningServer, RuntimeError> {
        build_server(self.config_source, self.loaded_plugins, None).await
    }
}

impl ReloadableServerBuilder {
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the server cannot bind, load its persisted
    /// world state, or starts with unsupported configuration such as an auth
    /// profile mode mismatch.
    pub async fn build(self) -> Result<crate::runtime::ReloadableRunningServer, RuntimeError> {
        let Self {
            config_source,
            loaded_plugins,
            reload_host,
        } = self;
        let running = build_server(
            config_source,
            loaded_plugins,
            Some(Arc::clone(&reload_host)),
        )
        .await?;
        Ok(crate::runtime::ReloadableRunningServer {
            running,
            reload_host,
        })
    }
}

async fn build_server(
    config_source: ServerConfigSource,
    loaded_plugins: LoadedPluginSet,
    reload_host: Option<Arc<dyn RuntimePluginHost>>,
) -> Result<crate::runtime::RunningServer, RuntimeError> {
    let config = config_source.load()?;
    if reload_host.is_none() {
        if config.plugin_reload_watch {
            return Err(RuntimeError::Config(
                "plugin-reload-watch requires ServerBuilder::with_reload_host(...)".to_string(),
            ));
        }
        if config.topology_reload_watch {
            return Err(RuntimeError::Config(
                "topology-reload-watch requires ServerBuilder::with_reload_host(...)".to_string(),
            ));
        }
    }

    let ActiveProtocols {
        protocols,
        default_adapter,
        default_bedrock_adapter,
    } = activate_protocols(&config, loaded_plugins.protocols())?;
    let RuntimeProfiles {
        storage_profile,
        auth_profile,
        bedrock_auth_profile,
        online_auth_keys,
        core,
    } = resolve_runtime_profiles(&config, &loaded_plugins)?;
    let super::listeners::BoundListeners {
        listener_bindings,
        bound_listeners,
    } = bind_runtime_listeners(&config, &protocols).await?;
    let initial_generation_id = TopologyGenerationId(1);
    let topology_generation = Arc::new(RuntimeTopologyGeneration {
        generation_id: initial_generation_id,
        config: config.clone(),
        protocol_registry: protocols,
        default_adapter,
        default_bedrock_adapter,
        listener_bindings: listener_bindings.clone(),
    });
    let (accepted_tx, accepted_rx) = mpsc::unbounded_channel();
    let listener_workers =
        spawn_listener_workers(bound_listeners, initial_generation_id, accepted_tx.clone())?;

    let server = Arc::new(RuntimeServer {
        config,
        config_source,
        loaded_plugins,
        reload_host,
        consistency_gate: tokio::sync::RwLock::new(()),
        topology: std::sync::RwLock::new(RuntimeTopologyState {
            active: topology_generation,
            draining: Vec::new(),
            listener_workers,
            next_generation_id: 2,
        }),
        auth_profile,
        bedrock_auth_profile,
        online_auth_keys,
        storage_profile,
        state: Mutex::new(RuntimeState { core, dirty: false }),
        sessions: Mutex::new(HashMap::new()),
        next_connection_id: Mutex::new(1),
        accepted_tx,
    });

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let run_server = Arc::clone(&server);
    let join_handle = spawn_runtime_loop(run_server, shutdown_rx, accepted_rx);

    Ok(crate::runtime::RunningServer {
        runtime: server,
        shutdown_tx: Some(shutdown_tx),
        join_handle,
    })
}
