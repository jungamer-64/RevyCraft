use crate::RuntimeError;
use crate::config::ServerConfig;
use crate::runtime::OnlineAuthKeys;
use mc_core::{CoreConfig, ServerCore};
use mc_plugin_api::codec::auth::AuthMode;
use mc_plugin_host::registry::LoadedPluginSet;
use mc_plugin_host::runtime::{AuthProfileHandle, StorageProfileHandle};
use std::sync::Arc;

pub(super) struct RuntimeProfiles {
    pub(super) storage_profile: Arc<dyn StorageProfileHandle>,
    pub(super) auth_profile: Arc<dyn AuthProfileHandle>,
    pub(super) bedrock_auth_profile: Option<Arc<dyn AuthProfileHandle>>,
    pub(super) online_auth_keys: Option<Arc<OnlineAuthKeys>>,
    pub(super) core: ServerCore,
}

pub(super) fn resolve_runtime_profiles(
    config: &ServerConfig,
    loaded_plugins: &LoadedPluginSet,
) -> Result<RuntimeProfiles, RuntimeError> {
    let storage_profile = loaded_plugins
        .resolve_storage_profile(&config.storage_profile)
        .ok_or_else(|| {
            RuntimeError::Config(format!(
                "unknown storage-profile `{}`",
                config.storage_profile
            ))
        })?;
    let auth_profile = loaded_plugins
        .resolve_auth_profile(&config.auth_profile)
        .ok_or_else(|| {
            RuntimeError::Config(format!("unknown auth-profile `{}`", config.auth_profile))
        })?;
    let bedrock_auth_profile = if config.be_enabled {
        Some(
            loaded_plugins
                .resolve_auth_profile(&config.bedrock_auth_profile)
                .ok_or_else(|| {
                    RuntimeError::Config(format!(
                        "unknown bedrock-auth-profile `{}`",
                        config.bedrock_auth_profile
                    ))
                })?,
        )
    } else {
        None
    };

    match (config.online_mode, auth_profile.mode()?) {
        (true, AuthMode::Online) | (false, AuthMode::Offline) => {}
        (true, mode) => {
            return Err(RuntimeError::Config(format!(
                "online-mode=true requires an online auth profile, got {mode:?}"
            )));
        }
        (false, mode) => {
            return Err(RuntimeError::Config(format!(
                "online-mode=false requires an offline auth profile, got {mode:?}"
            )));
        }
    }
    if let Some(profile) = &bedrock_auth_profile {
        match profile.mode()? {
            AuthMode::BedrockOffline | AuthMode::BedrockXbl => {}
            mode => {
                return Err(RuntimeError::Config(format!(
                    "bedrock-auth-profile requires a bedrock auth mode, got {mode:?}"
                )));
            }
        }
    }

    let online_auth_keys = if config.online_mode {
        Some(Arc::new(OnlineAuthKeys::generate()?))
    } else {
        None
    };
    let snapshot = storage_profile.load_snapshot(&config.world_dir)?;
    let core_config = CoreConfig {
        level_name: config.level_name.clone(),
        seed: 0,
        max_players: config.max_players,
        view_distance: config.view_distance,
        game_mode: config.game_mode,
        difficulty: config.difficulty,
        ..CoreConfig::default()
    };
    let core = match snapshot {
        Some(snapshot) => ServerCore::from_snapshot(core_config, snapshot),
        None => ServerCore::new(core_config),
    };

    Ok(RuntimeProfiles {
        storage_profile,
        auth_profile,
        bedrock_auth_profile,
        online_auth_keys,
        core,
    })
}
