use crate::abi::{CapabilityDescriptorV1, PluginAbiVersion, PluginKind, Utf8Slice};

pub const PLUGIN_MANIFEST_SYMBOL_V1: &[u8] = b"mc_plugin_manifest_v1\0";
pub const PLUGIN_PROTOCOL_API_SYMBOL_V1: &[u8] = b"mc_plugin_protocol_api_v1\0";
pub const PLUGIN_STORAGE_API_SYMBOL_V1: &[u8] = b"mc_plugin_storage_api_v1\0";
pub const PLUGIN_AUTH_API_SYMBOL_V1: &[u8] = b"mc_plugin_auth_api_v1\0";
pub const PLUGIN_GAMEPLAY_API_SYMBOL_V2: &[u8] = b"mc_plugin_gameplay_api_v2\0";

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PluginManifestV1 {
    pub plugin_id: Utf8Slice,
    pub display_name: Utf8Slice,
    pub plugin_kind: PluginKind,
    pub plugin_abi: PluginAbiVersion,
    pub min_host_abi: PluginAbiVersion,
    pub max_host_abi: PluginAbiVersion,
    pub capabilities: *const CapabilityDescriptorV1,
    pub capabilities_len: usize,
}

unsafe impl Send for PluginManifestV1 {}
unsafe impl Sync for PluginManifestV1 {}
