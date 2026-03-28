use crate::admin_transport::RustAdminTransportPlugin;
use crate::admin_ui::RustAdminUiPlugin;
use crate::auth::RustAuthPlugin;
use crate::gameplay::RustGameplayPlugin;
use crate::protocol::RustProtocolPlugin;
use crate::storage::RustStoragePlugin;
use mc_plugin_api::codec::admin_transport::{AdminTransportRequest, AdminTransportResponse};
use mc_plugin_api::codec::admin_ui::{AdminUiInput, AdminUiOutput};
use mc_plugin_api::codec::auth::{AuthRequest, AuthResponse};
use mc_plugin_api::codec::gameplay::{GameplayRequest, GameplayResponse};
use mc_plugin_api::codec::protocol::{ProtocolRequest, ProtocolResponse};
use mc_plugin_api::codec::storage::{StorageRequest, StorageResponse};
use mc_plugin_api::host_api::{AdminTransportHostApiV1, GameplayHostApiV2, HostApiTableV1};

pub mod admin_transport;
pub mod admin_ui;
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
pub fn handle_admin_transport_request<P: RustAdminTransportPlugin>(
    plugin: &P,
    request: AdminTransportRequest,
) -> Result<AdminTransportResponse, String> {
    admin_transport::handle_admin_transport_request(plugin, request)
}

#[doc(hidden)]
pub fn handle_admin_transport_request_with_host_api<P: RustAdminTransportPlugin>(
    plugin: &P,
    request: AdminTransportRequest,
    host_api: Option<AdminTransportHostApiV1>,
) -> Result<AdminTransportResponse, String> {
    admin_transport::handle_admin_transport_request_with_host_api(plugin, request, host_api)
}

#[doc(hidden)]
pub fn handle_admin_ui_request<P: RustAdminUiPlugin>(
    plugin: &P,
    request: AdminUiInput,
) -> Result<AdminUiOutput, String> {
    admin_ui::handle_admin_ui_request(plugin, request)
}

#[doc(hidden)]
pub fn handle_admin_ui_request_with_host_api<P: RustAdminUiPlugin>(
    plugin: &P,
    request: AdminUiInput,
    host_api: Option<HostApiTableV1>,
) -> Result<AdminUiOutput, String> {
    admin_ui::handle_admin_ui_request_with_host_api(plugin, request, host_api)
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
    host_api: Option<GameplayHostApiV2>,
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
