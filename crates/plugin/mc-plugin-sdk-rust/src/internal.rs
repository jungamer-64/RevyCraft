use crate::auth::RustAuthPlugin;
use crate::gameplay::RustGameplayPlugin;
use crate::protocol::RustProtocolPlugin;
use crate::storage::RustStoragePlugin;
use mc_plugin_api::abi::{ByteSlice, OwnedBuffer};
use mc_plugin_api::codec::auth::{AuthRequest, AuthResponse};
use mc_plugin_api::codec::gameplay::{GameplayRequest, GameplayResponse};
use mc_plugin_api::codec::protocol::{ProtocolRequest, ProtocolResponse};
use mc_plugin_api::codec::storage::{StorageRequest, StorageResponse};
use mc_plugin_api::host_api::HostApiTableV1;

#[doc(hidden)]
pub mod buffers {
    use super::{ByteSlice, OwnedBuffer};

    #[must_use]
    pub const unsafe fn byte_slice_as_bytes(slice: ByteSlice) -> &'static [u8] {
        unsafe { crate::buffers::byte_slice_as_bytes(slice) }
    }

    pub fn write_error_buffer(error_out: *mut OwnedBuffer, message: String) {
        crate::buffers::write_error_buffer(error_out, message);
    }

    pub fn write_output_buffer(output: *mut OwnedBuffer, bytes: Vec<u8>) {
        crate::buffers::write_output_buffer(output, bytes);
    }
}

#[doc(hidden)]
pub fn handle_protocol_request<P: RustProtocolPlugin>(
    plugin: &P,
    request: ProtocolRequest,
) -> Result<ProtocolResponse, String> {
    crate::protocol::handle_protocol_request(plugin, request)
}

#[doc(hidden)]
pub fn handle_gameplay_request<P: RustGameplayPlugin>(
    plugin: &P,
    request: GameplayRequest,
) -> Result<GameplayResponse, String> {
    crate::gameplay::handle_gameplay_request(plugin, request)
}

#[doc(hidden)]
pub fn handle_gameplay_request_with_host_api<P: RustGameplayPlugin>(
    plugin: &P,
    request: GameplayRequest,
    host_api: Option<HostApiTableV1>,
) -> Result<GameplayResponse, String> {
    crate::gameplay::handle_gameplay_request_with_host_api(plugin, request, host_api)
}

#[doc(hidden)]
pub fn handle_storage_request<P: RustStoragePlugin>(
    plugin: &P,
    request: StorageRequest,
) -> Result<StorageResponse, String> {
    crate::storage::handle_storage_request(plugin, request)
}

#[doc(hidden)]
pub fn handle_auth_request<P: RustAuthPlugin>(
    plugin: &P,
    request: AuthRequest,
) -> Result<AuthResponse, String> {
    crate::auth::handle_auth_request(plugin, request)
}
