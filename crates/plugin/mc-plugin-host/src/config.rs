use crate::host::{PluginFailureAction, PluginFailureMatrix};
use mc_plugin_api::abi::{CURRENT_PLUGIN_ABI, PluginAbiVersion};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapConfig {
    pub storage_profile: String,
    pub plugins_dir: PathBuf,
    pub plugin_abi_min: PluginAbiVersion,
    pub plugin_abi_max: PluginAbiVersion,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            storage_profile: "je-anvil-1_7_10".to_string(),
            plugins_dir: PathBuf::from("runtime").join("plugins"),
            plugin_abi_min: CURRENT_PLUGIN_ABI,
            plugin_abi_max: CURRENT_PLUGIN_ABI,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeSelectionConfig {
    pub be_enabled: bool,
    pub auth_profile: String,
    pub bedrock_auth_profile: String,
    pub default_gameplay_profile: String,
    pub gameplay_profile_map: HashMap<String, String>,
    pub plugin_allowlist: Option<Vec<String>>,
    pub plugin_failure_policy_protocol: PluginFailureAction,
    pub plugin_failure_policy_gameplay: PluginFailureAction,
    pub plugin_failure_policy_storage: PluginFailureAction,
    pub plugin_failure_policy_auth: PluginFailureAction,
}

impl Default for RuntimeSelectionConfig {
    fn default() -> Self {
        let failure_matrix = PluginFailureMatrix::default();
        Self {
            be_enabled: false,
            auth_profile: "offline-v1".to_string(),
            bedrock_auth_profile: "bedrock-offline-v1".to_string(),
            default_gameplay_profile: "canonical".to_string(),
            gameplay_profile_map: HashMap::new(),
            plugin_allowlist: None,
            plugin_failure_policy_protocol: failure_matrix.protocol,
            plugin_failure_policy_gameplay: failure_matrix.gameplay,
            plugin_failure_policy_storage: failure_matrix.storage,
            plugin_failure_policy_auth: failure_matrix.auth,
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
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    pub be_enabled: bool,
    pub storage_profile: String,
    pub auth_profile: String,
    pub bedrock_auth_profile: String,
    pub default_gameplay_profile: String,
    pub gameplay_profile_map: HashMap<String, String>,
    pub plugins_dir: PathBuf,
    pub plugin_allowlist: Option<Vec<String>>,
    pub plugin_failure_policy_protocol: PluginFailureAction,
    pub plugin_failure_policy_gameplay: PluginFailureAction,
    pub plugin_failure_policy_storage: PluginFailureAction,
    pub plugin_failure_policy_auth: PluginFailureAction,
    pub plugin_abi_min: PluginAbiVersion,
    pub plugin_abi_max: PluginAbiVersion,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let bootstrap = BootstrapConfig::default();
        let runtime = RuntimeSelectionConfig::default();
        Self {
            be_enabled: runtime.be_enabled,
            storage_profile: bootstrap.storage_profile,
            auth_profile: runtime.auth_profile,
            bedrock_auth_profile: runtime.bedrock_auth_profile,
            default_gameplay_profile: runtime.default_gameplay_profile,
            gameplay_profile_map: runtime.gameplay_profile_map,
            plugins_dir: bootstrap.plugins_dir,
            plugin_allowlist: runtime.plugin_allowlist,
            plugin_failure_policy_protocol: runtime.plugin_failure_policy_protocol,
            plugin_failure_policy_gameplay: runtime.plugin_failure_policy_gameplay,
            plugin_failure_policy_storage: runtime.plugin_failure_policy_storage,
            plugin_failure_policy_auth: runtime.plugin_failure_policy_auth,
            plugin_abi_min: bootstrap.plugin_abi_min,
            plugin_abi_max: bootstrap.plugin_abi_max,
        }
    }
}

impl ServerConfig {
    #[must_use]
    pub fn bootstrap_config(&self) -> BootstrapConfig {
        BootstrapConfig {
            storage_profile: self.storage_profile.clone(),
            plugins_dir: self.plugins_dir.clone(),
            plugin_abi_min: self.plugin_abi_min,
            plugin_abi_max: self.plugin_abi_max,
        }
    }

    #[must_use]
    pub fn runtime_selection_config(&self) -> RuntimeSelectionConfig {
        RuntimeSelectionConfig {
            be_enabled: self.be_enabled,
            auth_profile: self.auth_profile.clone(),
            bedrock_auth_profile: self.bedrock_auth_profile.clone(),
            default_gameplay_profile: self.default_gameplay_profile.clone(),
            gameplay_profile_map: self.gameplay_profile_map.clone(),
            plugin_allowlist: self.plugin_allowlist.clone(),
            plugin_failure_policy_protocol: self.plugin_failure_policy_protocol,
            plugin_failure_policy_gameplay: self.plugin_failure_policy_gameplay,
            plugin_failure_policy_storage: self.plugin_failure_policy_storage,
            plugin_failure_policy_auth: self.plugin_failure_policy_auth,
        }
    }
}
