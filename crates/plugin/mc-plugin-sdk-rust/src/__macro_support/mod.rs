use crate::admin_surface::RustAdminSurfacePlugin;
use crate::auth::RustAuthPlugin;
use crate::gameplay::RustGameplayPlugin;
use crate::protocol::RustProtocolPlugin;
use crate::storage::RustStoragePlugin;
#[doc(hidden)]
pub use mc_plugin_abi::CURRENT_PLUGIN_ABI;
#[doc(hidden)]
pub use mc_plugin_abi::host::{
    AdminSurfaceHostApiV9, AdminSurfacePluginApiV9, AuthPluginApiV9, GameplayHostApiV9,
    GameplayPluginApiV9, ProtocolPluginApiV9, StoragePluginApiV9,
};
#[doc(hidden)]
pub use mc_plugin_abi::manifest::PluginManifestV9;
#[doc(hidden)]
pub use mc_plugin_abi::raw::{ByteSlice, OwnedBuffer, PluginStatus};
#[doc(hidden)]
pub use mc_plugin_contract::codec;
use mc_plugin_contract::codec::admin_surface::{AdminSurfaceRequest, AdminSurfaceResponse};
use mc_plugin_contract::codec::auth::{AuthRequest, AuthResponse};
use mc_plugin_contract::codec::gameplay::{GameplayRequest, GameplayResponse};
use mc_plugin_contract::codec::protocol::{ProtocolRequest, ProtocolResponse};
use mc_plugin_contract::codec::storage::{StorageRequest, StorageResponse};

pub mod admin_surface;
#[doc(hidden)]
pub mod auth;
#[doc(hidden)]
pub mod buffers;
#[doc(hidden)]
pub mod gameplay;
#[doc(hidden)]
pub mod protocol;
#[doc(hidden)]
pub mod storage;

#[doc(hidden)]
pub fn handle_admin_surface_request<P: RustAdminSurfacePlugin>(
    plugin: &P,
    request: AdminSurfaceRequest,
) -> Result<AdminSurfaceResponse, String> {
    admin_surface::handle_admin_surface_request(plugin, request)
}

#[doc(hidden)]
pub fn handle_admin_surface_request_with_host_api<P: RustAdminSurfacePlugin>(
    plugin: &P,
    request: AdminSurfaceRequest,
    host_api: Option<AdminSurfaceHostApiV9>,
) -> Result<AdminSurfaceResponse, String> {
    admin_surface::handle_admin_surface_request_with_host_api(plugin, request, host_api)
}

#[doc(hidden)]
pub fn handle_protocol_request<P: RustProtocolPlugin>(
    plugin: &P,
    request: ProtocolRequest,
) -> Result<ProtocolResponse, String> {
    protocol::handle_protocol_request(plugin, request)
}

#[doc(hidden)]
pub fn handle_gameplay_request<P: RustGameplayPlugin>(
    plugin: &P,
    request: GameplayRequest,
) -> Result<GameplayResponse, String> {
    gameplay::handle_gameplay_request(plugin, request)
}

#[doc(hidden)]
pub fn handle_gameplay_request_with_host_api<P: RustGameplayPlugin>(
    plugin: &P,
    request: GameplayRequest,
    host_api: Option<GameplayHostApiV9>,
) -> Result<GameplayResponse, String> {
    gameplay::handle_gameplay_request_with_host_api(plugin, request, host_api)
}

#[doc(hidden)]
pub fn handle_storage_request<P: RustStoragePlugin>(
    plugin: &P,
    request: StorageRequest,
) -> Result<StorageResponse, String> {
    storage::handle_storage_request(plugin, request)
}

#[doc(hidden)]
pub fn handle_auth_request<P: RustAuthPlugin>(
    plugin: &P,
    request: AuthRequest,
) -> Result<AuthResponse, String> {
    auth::handle_auth_request(plugin, request)
}
