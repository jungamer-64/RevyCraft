use crate::PluginFailureMatrix;
use mc_plugin_api::{CURRENT_PLUGIN_ABI, PluginAbiVersion};
use std::collections::HashMap;
use std::path::PathBuf;

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
    pub plugin_failure_policy_protocol: crate::PluginFailureAction,
    pub plugin_failure_policy_gameplay: crate::PluginFailureAction,
    pub plugin_failure_policy_storage: crate::PluginFailureAction,
    pub plugin_failure_policy_auth: crate::PluginFailureAction,
    pub plugin_abi_min: PluginAbiVersion,
    pub plugin_abi_max: PluginAbiVersion,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let failure_matrix = PluginFailureMatrix::default();
        Self {
            be_enabled: false,
            storage_profile: "je-anvil-1_7_10".to_string(),
            auth_profile: "offline-v1".to_string(),
            bedrock_auth_profile: "bedrock-offline-v1".to_string(),
            default_gameplay_profile: "canonical".to_string(),
            gameplay_profile_map: HashMap::new(),
            plugins_dir: PathBuf::from("runtime").join("plugins"),
            plugin_allowlist: None,
            plugin_failure_policy_protocol: failure_matrix.protocol,
            plugin_failure_policy_gameplay: failure_matrix.gameplay,
            plugin_failure_policy_storage: failure_matrix.storage,
            plugin_failure_policy_auth: failure_matrix.auth,
            plugin_abi_min: CURRENT_PLUGIN_ABI,
            plugin_abi_max: CURRENT_PLUGIN_ABI,
        }
    }
}
