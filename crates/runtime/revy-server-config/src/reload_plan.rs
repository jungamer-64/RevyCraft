use crate::error::ServerConfigError;
use crate::types::{
    AdminConfig, BootstrapConfig, NetworkConfig, PluginsConfig, ProfilesConfig, ServerConfig,
    TopologyConfig,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticConfig {
    pub bootstrap: BootstrapConfig,
}

impl StaticConfig {
    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when the candidate changes restart-only state.
    pub fn validate_reload_compatibility(&self, candidate: &Self) -> Result<(), ServerConfigError> {
        if candidate.bootstrap != self.bootstrap {
            return Err(ServerConfigError::Config(
                "bootstrap config changes require a restart".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveConfig {
    pub network: NetworkConfig,
    pub topology: TopologyConfig,
    pub plugins: PluginsConfig,
    pub profiles: ProfilesConfig,
    pub admin: AdminConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopologyReloadPlan {
    pub next_active_config: ServerConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreReloadPlan {
    pub next_active_config: ServerConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullReloadPlan {
    pub next_active_config: ServerConfig,
}

pub(crate) fn static_config(server: &ServerConfig) -> StaticConfig {
    StaticConfig {
        bootstrap: server.bootstrap.clone(),
    }
}

pub(crate) fn live_config(server: &ServerConfig) -> LiveConfig {
    LiveConfig {
        network: server.network.clone(),
        topology: server.topology.clone(),
        plugins: server.plugins.clone(),
        profiles: server.profiles.clone(),
        admin: server.admin.clone(),
    }
}

pub(crate) fn plan_topology_reload(
    active: &ServerConfig,
    candidate: &ServerConfig,
) -> Result<TopologyReloadPlan, ServerConfigError> {
    static_config(active).validate_reload_compatibility(&static_config(candidate))?;
    let mut next_active_config = active.clone();
    next_active_config.network.clone_from(&candidate.network);
    next_active_config.topology.clone_from(&candidate.topology);
    Ok(TopologyReloadPlan { next_active_config })
}

pub(crate) fn plan_core_reload(
    active: &ServerConfig,
    candidate: &ServerConfig,
) -> Result<CoreReloadPlan, ServerConfigError> {
    validate_core_reload_static_compatibility(&static_config(active), &static_config(candidate))?;
    let mut next_active_config = active.clone();
    next_active_config
        .bootstrap
        .level_name
        .clone_from(&candidate.bootstrap.level_name);
    next_active_config.bootstrap.game_mode = candidate.bootstrap.game_mode;
    next_active_config.bootstrap.difficulty = candidate.bootstrap.difficulty;
    next_active_config.bootstrap.view_distance = candidate.bootstrap.view_distance;
    next_active_config.network.max_players = candidate.network.max_players;
    Ok(CoreReloadPlan { next_active_config })
}

pub(crate) fn plan_full_reload(
    active: &ServerConfig,
    candidate: &ServerConfig,
) -> Result<FullReloadPlan, ServerConfigError> {
    validate_core_reload_static_compatibility(&static_config(active), &static_config(candidate))?;
    Ok(FullReloadPlan {
        next_active_config: candidate.clone(),
    })
}

fn validate_core_reload_static_compatibility(
    active: &StaticConfig,
    candidate: &StaticConfig,
) -> Result<(), ServerConfigError> {
    let active_bootstrap = &active.bootstrap;
    let candidate_bootstrap = &candidate.bootstrap;
    let restart_required_bootstrap_diff = active_bootstrap.online_mode
        != candidate_bootstrap.online_mode
        || active_bootstrap.level_type != candidate_bootstrap.level_type
        || active_bootstrap.world_dir != candidate_bootstrap.world_dir
        || active_bootstrap.storage_profile != candidate_bootstrap.storage_profile
        || active_bootstrap.plugins_dir != candidate_bootstrap.plugins_dir
        || active_bootstrap.plugin_abi_min != candidate_bootstrap.plugin_abi_min
        || active_bootstrap.plugin_abi_max != candidate_bootstrap.plugin_abi_max;
    if restart_required_bootstrap_diff {
        return Err(ServerConfigError::Config(
            "bootstrap config changes require a restart".to_string(),
        ));
    }
    Ok(())
}
