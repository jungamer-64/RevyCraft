use super::{AdminPermission, OnlineAuthKeys};
use crate::RuntimeError;
use crate::config::ServerConfig;
use mc_core::{AdapterId, GameplayProfileId, ServerCore};
use mc_plugin_api::codec::auth::AuthMode;
use mc_plugin_api::codec::gameplay::GameplaySessionSnapshot;
use mc_plugin_host::registry::LoadedPluginSet;
use mc_plugin_host::runtime::{
    AdminSurfaceProfileHandle, AuthProfileHandle, GameplayProfileHandle, StorageProfileHandle,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock as AsyncRwLock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteAdminPrincipal {
    pub(crate) principal_id: String,
    pub(crate) permissions: Vec<AdminPermission>,
}

impl RemoteAdminPrincipal {
    #[must_use]
    pub(crate) fn new(principal_id: impl Into<String>, permissions: Vec<AdminPermission>) -> Self {
        Self {
            principal_id: principal_id.into(),
            permissions,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ResolvedRuntimeSelection {
    pub(crate) config: ServerConfig,
    pub(crate) loaded_plugins: LoadedPluginSet,
    pub(crate) auth_profile: Arc<dyn AuthProfileHandle>,
    pub(crate) bedrock_auth_profile: Option<Arc<dyn AuthProfileHandle>>,
    pub(crate) admin_surfaces: Vec<ResolvedAdminSurfaceSelection>,
    pub(crate) remote_admin_principals: HashMap<String, RemoteAdminPrincipal>,
}

#[derive(Clone)]
pub(crate) struct ResolvedAdminSurfaceSelection {
    pub(crate) instance_id: String,
    pub(crate) surface_config_path: Option<PathBuf>,
    pub(crate) profile: Arc<dyn AdminSurfaceProfileHandle>,
}

pub(crate) struct BootstrapSelectionResolution {
    pub(crate) selection: ResolvedRuntimeSelection,
    pub(crate) storage_profile: Arc<dyn StorageProfileHandle>,
    pub(crate) online_auth_keys: Option<Arc<OnlineAuthKeys>>,
    pub(crate) core: ServerCore,
}

pub(crate) struct SelectionManager {
    state: AsyncRwLock<ResolvedRuntimeSelection>,
    online_auth_keys: Option<Arc<OnlineAuthKeys>>,
}

impl SelectionManager {
    pub(crate) fn new(
        state: ResolvedRuntimeSelection,
        online_auth_keys: Option<Arc<OnlineAuthKeys>>,
    ) -> Self {
        Self {
            state: AsyncRwLock::new(state),
            online_auth_keys,
        }
    }

    pub(crate) async fn current(&self) -> ResolvedRuntimeSelection {
        self.state.read().await.clone()
    }

    pub(crate) async fn replace(&self, selection: ResolvedRuntimeSelection) {
        *self.state.write().await = selection;
    }

    pub(crate) async fn replace_config(&self, next_active_config: ServerConfig) {
        let mut selection_state = self.state.write().await;
        selection_state.config = next_active_config;
    }

    pub(crate) async fn current_admin_surfaces(&self) -> Vec<ResolvedAdminSurfaceSelection> {
        self.current().await.admin_surfaces
    }

    pub(crate) async fn auth_profile(&self) -> Arc<dyn AuthProfileHandle> {
        self.state.read().await.auth_profile.clone()
    }

    pub(crate) async fn bedrock_auth_profile(&self) -> Option<Arc<dyn AuthProfileHandle>> {
        self.state.read().await.bedrock_auth_profile.clone()
    }

    pub(crate) fn online_auth_keys(&self) -> Option<Arc<OnlineAuthKeys>> {
        self.online_auth_keys.clone()
    }

    async fn gameplay_profile_for_adapter(&self, adapter_id: &str) -> GameplayProfileId {
        let selection_state = self.state.read().await;
        selection_state
            .config
            .profiles
            .gameplay_map
            .get(&AdapterId::new(adapter_id))
            .cloned()
            .unwrap_or_else(|| selection_state.config.profiles.default_gameplay.clone())
    }

    pub(crate) async fn resolve_gameplay_for_adapter(
        &self,
        adapter_id: &str,
    ) -> Result<Arc<dyn GameplayProfileHandle>, RuntimeError> {
        let profile_id = self.gameplay_profile_for_adapter(adapter_id).await;
        self.state
            .read()
            .await
            .loaded_plugins
            .resolve_gameplay_profile(profile_id.as_str())
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "gameplay profile `{}` for adapter `{adapter_id}` is not active",
                    profile_id.as_str()
                ))
            })
    }
}

pub(crate) struct SelectionResolver;

impl SelectionResolver {
    pub(crate) fn gameplay_profile_for_adapter(
        config: &ServerConfig,
        adapter_id: &str,
    ) -> GameplayProfileId {
        config
            .profiles
            .gameplay_map
            .get(&AdapterId::new(adapter_id))
            .cloned()
            .unwrap_or_else(|| config.profiles.default_gameplay.clone())
    }

    pub(crate) fn core_config(config: &ServerConfig) -> mc_core::CoreConfig {
        mc_core::CoreConfig {
            level_name: config.bootstrap.level_name.clone(),
            seed: 0,
            max_players: config.network.max_players,
            view_distance: config.bootstrap.view_distance,
            game_mode: config.bootstrap.game_mode,
            difficulty: config.bootstrap.difficulty,
            ..mc_core::CoreConfig::default()
        }
    }

    pub(crate) fn resolve_bootstrap(
        config: &ServerConfig,
        loaded_plugins: LoadedPluginSet,
    ) -> Result<BootstrapSelectionResolution, RuntimeError> {
        let storage_profile = Self::resolve_storage_profile(config, &loaded_plugins)?;
        let selection = Self::resolve(config.clone(), loaded_plugins, &[])?;
        let online_auth_keys = if config.bootstrap.online_mode {
            Some(Arc::new(OnlineAuthKeys::generate()?))
        } else {
            None
        };
        let snapshot = storage_profile.load_snapshot(&config.bootstrap.world_dir)?;
        let core_config = Self::core_config(config);
        let core = match snapshot {
            Some(snapshot) => ServerCore::from_snapshot(core_config, snapshot),
            None => ServerCore::new(core_config),
        };
        Ok(BootstrapSelectionResolution {
            selection,
            storage_profile,
            online_auth_keys,
            core,
        })
    }

    pub(crate) fn resolve(
        config: ServerConfig,
        loaded_plugins: LoadedPluginSet,
        active_gameplay_sessions: &[GameplaySessionSnapshot],
    ) -> Result<ResolvedRuntimeSelection, RuntimeError> {
        Self::ensure_candidate_gameplay_profiles_active(&config, active_gameplay_sessions)?;
        let auth_profile = loaded_plugins
            .resolve_auth_profile(config.profiles.auth.as_str())
            .ok_or_else(|| {
                RuntimeError::Config(format!("unknown auth-profile `{}`", config.profiles.auth))
            })?;
        match (config.bootstrap.online_mode, auth_profile.mode()?) {
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

        let bedrock_auth_profile = if config.topology.be_enabled {
            let profile = loaded_plugins
                .resolve_auth_profile(config.profiles.bedrock_auth.as_str())
                .ok_or_else(|| {
                    RuntimeError::Config(format!(
                        "unknown bedrock-auth-profile `{}`",
                        config.profiles.bedrock_auth
                    ))
                })?;
            match profile.mode()? {
                AuthMode::BedrockOffline | AuthMode::BedrockXbl => {}
                mode => {
                    return Err(RuntimeError::Config(format!(
                        "bedrock-auth-profile requires a bedrock auth mode, got {mode:?}"
                    )));
                }
            }
            Some(profile)
        } else {
            None
        };
        let admin_surfaces = Self::resolve_admin_surface_selections(&config, &loaded_plugins)?;
        let remote_admin_principals =
            Self::materialize_remote_admin_principals(&config, &admin_surfaces)?;

        Ok(ResolvedRuntimeSelection {
            config,
            loaded_plugins,
            auth_profile,
            bedrock_auth_profile,
            admin_surfaces,
            remote_admin_principals,
        })
    }

    pub(crate) fn materialize_remote_admin_principals(
        config: &ServerConfig,
        surfaces: &[ResolvedAdminSurfaceSelection],
    ) -> Result<HashMap<String, RemoteAdminPrincipal>, RuntimeError> {
        let mut principals = config
            .admin
            .principals
            .iter()
            .map(|(principal_id, principal)| {
                (
                    principal_id.clone(),
                    RemoteAdminPrincipal::new(
                        principal_id.clone(),
                        principal
                            .permissions
                            .iter()
                            .copied()
                            .map(runtime_permission_from_config)
                            .collect(),
                    ),
                )
            })
            .collect::<HashMap<_, _>>();
        for surface in surfaces {
            let declaration = surface
                .profile
                .declare_instance(&surface.instance_id, surface.surface_config_path.as_deref())
                .map_err(|error| RuntimeError::Config(error.to_string()))?;
            for principal in declaration.principals {
                let permissions = principal
                    .permissions
                    .into_iter()
                    .map(runtime_permission_from_plugin)
                    .collect::<Vec<_>>();
                match principals.get(&principal.principal_id) {
                    Some(existing) if existing.permissions != permissions => {
                        return Err(RuntimeError::Config(format!(
                            "admin principal `{}` permissions conflicted between host config and admin surface `{}`",
                            principal.principal_id, surface.instance_id
                        )));
                    }
                    Some(_) => {}
                    None => {
                        principals.insert(
                            principal.principal_id.clone(),
                            RemoteAdminPrincipal::new(principal.principal_id, permissions),
                        );
                    }
                }
            }
        }
        Ok(principals)
    }

    fn resolve_admin_surface_selections(
        config: &ServerConfig,
        loaded_plugins: &LoadedPluginSet,
    ) -> Result<Vec<ResolvedAdminSurfaceSelection>, RuntimeError> {
        let mut surfaces = config.admin.surfaces.iter().collect::<Vec<_>>();
        surfaces.sort_by(|left, right| left.0.cmp(right.0));
        surfaces
            .into_iter()
            .map(|(instance_id, surface)| {
                let profile_id = surface.profile.to_string();
                let profile = loaded_plugins
                    .resolve_admin_surface_profile(&profile_id)
                    .ok_or_else(|| {
                        RuntimeError::Config(format!(
                            "unknown admin-surface profile `{profile_id}` for surface `{instance_id}`"
                        ))
                    })?;
                Ok(ResolvedAdminSurfaceSelection {
                    instance_id: instance_id.clone(),
                    surface_config_path: surface.config.clone(),
                    profile,
                })
            })
            .collect()
    }

    pub(crate) fn ensure_candidate_gameplay_profiles_active(
        candidate: &ServerConfig,
        active_gameplay_sessions: &[GameplaySessionSnapshot],
    ) -> Result<(), RuntimeError> {
        let mut active_profiles = HashSet::new();
        let _ = active_profiles.insert(candidate.profiles.default_gameplay.clone());
        active_profiles.extend(candidate.profiles.gameplay_map.values().cloned());
        for session in active_gameplay_sessions {
            if !active_profiles.contains(&session.gameplay_profile) {
                return Err(RuntimeError::Config(format!(
                    "cannot remove gameplay profile `{}` while sessions are still using it",
                    session.gameplay_profile.as_str()
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn resolve_storage_profile(
        config: &ServerConfig,
        loaded_plugins: &LoadedPluginSet,
    ) -> Result<Arc<dyn StorageProfileHandle>, RuntimeError> {
        loaded_plugins
            .resolve_storage_profile(config.bootstrap.storage_profile.as_str())
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "unknown storage-profile `{}`",
                    config.bootstrap.storage_profile.as_str()
                ))
            })
    }
}

const fn runtime_permission_from_config(
    permission: crate::config::AdminPermission,
) -> AdminPermission {
    match permission {
        crate::config::AdminPermission::Status => AdminPermission::Status,
        crate::config::AdminPermission::Sessions => AdminPermission::Sessions,
        crate::config::AdminPermission::ReloadRuntime => AdminPermission::ReloadRuntime,
        crate::config::AdminPermission::UpgradeRuntime => AdminPermission::UpgradeRuntime,
        crate::config::AdminPermission::Shutdown => AdminPermission::Shutdown,
    }
}

const fn runtime_permission_from_plugin(
    permission: mc_plugin_api::codec::admin::AdminPermission,
) -> AdminPermission {
    match permission {
        mc_plugin_api::codec::admin::AdminPermission::Status => AdminPermission::Status,
        mc_plugin_api::codec::admin::AdminPermission::Sessions => AdminPermission::Sessions,
        mc_plugin_api::codec::admin::AdminPermission::ReloadRuntime => {
            AdminPermission::ReloadRuntime
        }
        mc_plugin_api::codec::admin::AdminPermission::UpgradeRuntime => {
            AdminPermission::UpgradeRuntime
        }
        mc_plugin_api::codec::admin::AdminPermission::Shutdown => AdminPermission::Shutdown,
    }
}
