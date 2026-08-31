use crate::PluginAbiVersion;
use crate::raw::{CapabilityDescriptorV9, PluginKindTag, Utf8Slice};

pub const PLUGIN_MANIFEST_SYMBOL_V9: &[u8] = b"mc_plugin_manifest_v9\0";
pub const PLUGIN_PROTOCOL_API_SYMBOL_V9: &[u8] = b"mc_plugin_protocol_api_v9\0";
pub const PLUGIN_STORAGE_API_SYMBOL_V9: &[u8] = b"mc_plugin_storage_api_v9\0";
pub const PLUGIN_AUTH_API_SYMBOL_V9: &[u8] = b"mc_plugin_auth_api_v9\0";
pub const PLUGIN_GAMEPLAY_API_SYMBOL_V9: &[u8] = b"mc_plugin_gameplay_api_v9\0";
pub const PLUGIN_ADMIN_SURFACE_API_SYMBOL_V9: &[u8] = b"mc_plugin_admin_surface_api_v9\0";

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PluginManifestHeaderV9 {
    pub plugin_abi: PluginAbiVersion,
    pub struct_size: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PluginManifestV9 {
    pub plugin_abi: PluginAbiVersion,
    pub struct_size: usize,
    pub plugin_id: Utf8Slice,
    pub display_name: Utf8Slice,
    pub plugin_kind: PluginKindTag,
    pub min_host_abi: PluginAbiVersion,
    pub max_host_abi: PluginAbiVersion,
    pub capabilities: *const CapabilityDescriptorV9,
    pub capabilities_len: usize,
    pub max_session_handoff_bytes: usize,
}
