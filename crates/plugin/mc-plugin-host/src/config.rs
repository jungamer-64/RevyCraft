use mc_plugin_api::abi::{CURRENT_PLUGIN_ABI, PluginAbiVersion};
use revy_server_types::{
    PluginFailureAction, PluginFailureMatrix, PluginHostBootstrapSelectionView,
    PluginHostBufferLimitsView, PluginHostRuntimeSelectionView,
};
use revy_voxel_semantic::{
    AdapterId, AdminSurfaceProfileId, AuthProfileId, GameplayProfileId, StorageProfileId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginBufferLimits {
    pub protocol_response_bytes: usize,
    pub gameplay_response_bytes: usize,
    pub storage_response_bytes: usize,
    pub auth_response_bytes: usize,
    pub admin_surface_response_bytes: usize,
    pub callback_payload_bytes: usize,
    pub metadata_bytes: usize,
}

impl Default for PluginBufferLimits {
    fn default() -> Self {
        const KIB: usize = 1024;
        const MIB: usize = 1024 * KIB;
        Self {
            protocol_response_bytes: 4 * MIB,
            gameplay_response_bytes: 1 * MIB,
            storage_response_bytes: 32 * MIB,
            auth_response_bytes: 256 * KIB,
            admin_surface_response_bytes: 1 * MIB,
            callback_payload_bytes: 1 * MIB,
            metadata_bytes: 64 * KIB,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapConfig {
    pub storage_profile: StorageProfileId,
    pub plugins_dir: PathBuf,
    pub plugin_abi_min: PluginAbiVersion,
    pub plugin_abi_max: PluginAbiVersion,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            storage_profile: StorageProfileId::new("je-anvil-1_7_10"),
            plugins_dir: PathBuf::from("runtime").join("plugins"),
            plugin_abi_min: CURRENT_PLUGIN_ABI,
            plugin_abi_max: CURRENT_PLUGIN_ABI,
        }
    }
}

impl From<&PluginHostBootstrapSelectionView> for BootstrapConfig {
    fn from(view: &PluginHostBootstrapSelectionView) -> Self {
        Self {
            storage_profile: view.storage_profile.clone(),
            plugins_dir: view.plugins_dir.clone(),
            plugin_abi_min: PluginAbiVersion {
                major: view.plugin_abi_min.major,
                minor: view.plugin_abi_min.minor,
            },
            plugin_abi_max: PluginAbiVersion {
                major: view.plugin_abi_max.major,
                minor: view.plugin_abi_max.minor,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeSelectionConfig {
    pub be_enabled: bool,
    pub auth_profile: AuthProfileId,
    pub bedrock_auth_profile: AuthProfileId,
    pub default_gameplay_profile: GameplayProfileId,
    pub gameplay_profile_map: HashMap<AdapterId, GameplayProfileId>,
    pub admin_surfaces: Vec<AdminSurfaceSelectionConfig>,
    pub plugin_allowlist: Option<Vec<String>>,
    pub buffer_limits: PluginBufferLimits,
    pub plugin_failure_policy_protocol: PluginFailureAction,
    pub plugin_failure_policy_gameplay: PluginFailureAction,
    pub plugin_failure_policy_storage: PluginFailureAction,
    pub plugin_failure_policy_auth: PluginFailureAction,
    pub plugin_failure_policy_admin_surface: PluginFailureAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminSurfaceSelectionConfig {
    pub instance_id: String,
    pub profile: AdminSurfaceProfileId,
    pub config_path: Option<PathBuf>,
}

impl Default for RuntimeSelectionConfig {
    fn default() -> Self {
        let failure_matrix = PluginFailureMatrix::default();
        Self {
            be_enabled: false,
            auth_profile: AuthProfileId::new("offline-v1"),
            bedrock_auth_profile: AuthProfileId::new("bedrock-offline-v1"),
            default_gameplay_profile: GameplayProfileId::new("canonical"),
            gameplay_profile_map: HashMap::new(),
            admin_surfaces: vec![AdminSurfaceSelectionConfig {
                instance_id: "console".to_string(),
                profile: AdminSurfaceProfileId::new("console-v1"),
                config_path: None,
            }],
            plugin_allowlist: None,
            buffer_limits: PluginBufferLimits::default(),
            plugin_failure_policy_protocol: failure_matrix.protocol,
            plugin_failure_policy_gameplay: failure_matrix.gameplay,
            plugin_failure_policy_storage: failure_matrix.storage,
            plugin_failure_policy_auth: failure_matrix.auth,
            plugin_failure_policy_admin_surface: failure_matrix.admin_surface,
        }
    }
}

impl RuntimeSelectionConfig {
    #[must_use]
    pub const fn failure_matrix(&self) -> PluginFailureMatrix {
        PluginFailureMatrix {
            protocol: self.plugin_failure_policy_protocol,
            gameplay: self.plugin_failure_policy_gameplay,
            storage: self.plugin_failure_policy_storage,
            auth: self.plugin_failure_policy_auth,
            admin_surface: self.plugin_failure_policy_admin_surface,
        }
    }
}

impl From<&PluginHostBufferLimitsView> for PluginBufferLimits {
    fn from(view: &PluginHostBufferLimitsView) -> Self {
        Self {
            protocol_response_bytes: view.protocol_response_bytes,
            gameplay_response_bytes: view.gameplay_response_bytes,
            storage_response_bytes: view.storage_response_bytes,
            auth_response_bytes: view.auth_response_bytes,
            admin_surface_response_bytes: view.admin_surface_response_bytes,
            callback_payload_bytes: view.callback_payload_bytes,
            metadata_bytes: view.metadata_bytes,
        }
    }
}

impl From<&PluginHostRuntimeSelectionView> for RuntimeSelectionConfig {
    fn from(view: &PluginHostRuntimeSelectionView) -> Self {
        let mut admin_surfaces = view
            .admin_surfaces
            .iter()
            .map(|surface| AdminSurfaceSelectionConfig {
                instance_id: surface.instance_id.clone(),
                profile: surface.profile.clone(),
                config_path: surface.config_path.clone(),
            })
            .collect::<Vec<_>>();
        admin_surfaces.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        Self {
            be_enabled: view.be_enabled,
            auth_profile: view.auth_profile.clone(),
            bedrock_auth_profile: view.bedrock_auth_profile.clone(),
            default_gameplay_profile: view.default_gameplay_profile.clone(),
            gameplay_profile_map: view.gameplay_profile_map.clone(),
            admin_surfaces,
            plugin_allowlist: view.plugin_allowlist.clone(),
            buffer_limits: PluginBufferLimits::from(&view.buffer_limits),
            plugin_failure_policy_protocol: view.failure_matrix.protocol,
            plugin_failure_policy_gameplay: view.failure_matrix.gameplay,
            plugin_failure_policy_storage: view.failure_matrix.storage,
            plugin_failure_policy_auth: view.failure_matrix.auth,
            plugin_failure_policy_admin_surface: view.failure_matrix.admin_surface,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BootstrapConfig, PluginBufferLimits, RuntimeSelectionConfig};
    use revy_voxel_semantic::AdapterId;
    use revy_server_types::{
        PluginFailureAction, PluginFailureMatrix, PluginHostAdminSurfaceSelectionView,
        PluginHostBootstrapSelectionView, PluginHostBufferLimitsView,
        PluginHostRuntimeSelectionView,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn bootstrap_config_from_view_preserves_bootstrap_fields() {
        let view = PluginHostBootstrapSelectionView {
            storage_profile: "custom-storage".into(),
            plugins_dir: PathBuf::from("runtime").join("plugins"),
            plugin_abi_min: revy_server_types::PluginAbiVersionView { major: 7, minor: 0 },
            plugin_abi_max: revy_server_types::PluginAbiVersionView { major: 7, minor: 1 },
        };

        let config = BootstrapConfig::from(&view);

        assert_eq!(config.storage_profile, view.storage_profile);
        assert_eq!(config.plugins_dir, view.plugins_dir);
        assert_eq!(config.plugin_abi_min.major, view.plugin_abi_min.major);
        assert_eq!(config.plugin_abi_min.minor, view.plugin_abi_min.minor);
        assert_eq!(config.plugin_abi_max.major, view.plugin_abi_max.major);
        assert_eq!(config.plugin_abi_max.minor, view.plugin_abi_max.minor);
    }

    #[test]
    fn runtime_selection_config_from_view_sorts_admin_surfaces_and_expands_failure_matrix() {
        let view = PluginHostRuntimeSelectionView {
            be_enabled: true,
            auth_profile: "offline-v1".into(),
            bedrock_auth_profile: "bedrock-offline-v1".into(),
            default_gameplay_profile: "canonical".into(),
            gameplay_profile_map: HashMap::from([
                (AdapterId::new("je-5"), "canonical".into()),
                (AdapterId::new("be-924"), "readonly".into()),
            ]),
            admin_surfaces: vec![
                PluginHostAdminSurfaceSelectionView {
                    instance_id: "z-remote".to_string(),
                    profile: "grpc-v1".into(),
                    config_path: Some(PathBuf::from("runtime").join("grpc.toml")),
                },
                PluginHostAdminSurfaceSelectionView {
                    instance_id: "a-console".to_string(),
                    profile: "console-v1".into(),
                    config_path: None,
                },
            ],
            plugin_allowlist: Some(vec!["je-5".to_string(), "canonical".to_string()]),
            buffer_limits: PluginHostBufferLimitsView {
                protocol_response_bytes: 1234,
                gameplay_response_bytes: 2345,
                storage_response_bytes: 3456,
                auth_response_bytes: 4567,
                admin_surface_response_bytes: 5678,
                callback_payload_bytes: 6789,
                metadata_bytes: 7890,
            },
            failure_matrix: PluginFailureMatrix {
                protocol: PluginFailureAction::Skip,
                gameplay: PluginFailureAction::FailFast,
                storage: PluginFailureAction::Skip,
                auth: PluginFailureAction::FailFast,
                admin_surface: PluginFailureAction::Skip,
            },
        };

        let config = RuntimeSelectionConfig::from(&view);

        assert!(config.be_enabled);
        assert_eq!(
            config
                .admin_surfaces
                .iter()
                .map(|surface| surface.instance_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a-console", "z-remote"]
        );
        assert_eq!(
            config.buffer_limits,
            PluginBufferLimits::from(&view.buffer_limits)
        );
        assert_eq!(
            config.plugin_failure_policy_protocol,
            PluginFailureAction::Skip
        );
        assert_eq!(
            config.plugin_failure_policy_gameplay,
            PluginFailureAction::FailFast
        );
        assert_eq!(
            config.plugin_failure_policy_storage,
            PluginFailureAction::Skip
        );
        assert_eq!(
            config.plugin_failure_policy_auth,
            PluginFailureAction::FailFast
        );
        assert_eq!(
            config.plugin_failure_policy_admin_surface,
            PluginFailureAction::Skip
        );
    }
}
