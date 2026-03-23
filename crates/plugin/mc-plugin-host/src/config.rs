use crate::host::{PluginFailureAction, PluginFailureMatrix};
use mc_core::{AdapterId, AdminUiProfileId, AuthProfileId, GameplayProfileId, StorageProfileId};
use mc_plugin_api::abi::{CURRENT_PLUGIN_ABI, PluginAbiVersion};
use std::collections::HashMap;
use std::path::PathBuf;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeSelectionConfig {
    pub be_enabled: bool,
    pub auth_profile: AuthProfileId,
    pub bedrock_auth_profile: AuthProfileId,
    pub default_gameplay_profile: GameplayProfileId,
    pub gameplay_profile_map: HashMap<AdapterId, GameplayProfileId>,
    pub admin_ui_profile: AdminUiProfileId,
    pub plugin_allowlist: Option<Vec<String>>,
    pub plugin_failure_policy_protocol: PluginFailureAction,
    pub plugin_failure_policy_gameplay: PluginFailureAction,
    pub plugin_failure_policy_storage: PluginFailureAction,
    pub plugin_failure_policy_auth: PluginFailureAction,
    pub plugin_failure_policy_admin_ui: PluginFailureAction,
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
            admin_ui_profile: AdminUiProfileId::new("console-v1"),
            plugin_allowlist: None,
            plugin_failure_policy_protocol: failure_matrix.protocol,
            plugin_failure_policy_gameplay: failure_matrix.gameplay,
            plugin_failure_policy_storage: failure_matrix.storage,
            plugin_failure_policy_auth: failure_matrix.auth,
            plugin_failure_policy_admin_ui: failure_matrix.admin_ui,
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
            admin_ui: self.plugin_failure_policy_admin_ui,
        }
    }
}
