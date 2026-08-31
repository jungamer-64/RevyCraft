mod buffers;
mod describe;
mod manifest;
mod profiles;
mod reload;

use super::{
    AdminSurfaceCapability, AdminSurfaceDescriptor, AdminSurfaceResponse, Arc, AuthCapability,
    AuthResponse, BedrockListenerDescriptor, GameplayCapability, GameplayGeneration,
    GameplayProfileId, GameplayRequest, GameplayResponse, GameplaySessionSnapshot, HashMap,
    HashSet, ManagedGameplayPlugin, ManagedProtocolPlugin, OwnedBuffer, PluginAbiVersion,
    PluginFreeBufferFn, PluginKind, PluginManifestV9, ProtocolCapability, ProtocolDescriptor,
    ProtocolGeneration, ProtocolRequest, ProtocolResponse, RuntimeError, RuntimeReloadContext,
    StorageCapability, StorageGeneration, StorageRequest, StorageResponse,
};
use crate::runtime::ProtocolReloadSession;
use mc_plugin_abi::raw::{ByteSlice, Utf8Slice};
use mc_plugin_contract::codec::auth::AuthDescriptor;
use mc_plugin_contract::codec::gameplay::GameplayDescriptor;
use mc_plugin_contract::codec::protocol::ProtocolSessionSnapshot;
use mc_plugin_contract::codec::storage::StorageDescriptor;

pub(super) use self::buffers::{
    decode_utf8_slice_with_limit, read_byte_slice, read_checked_slice, release_owned_buffer,
    take_owned_buffer,
};
pub(super) use self::describe::{
    expect_admin_surface_capabilities, expect_admin_surface_descriptor, expect_auth_capabilities,
    expect_auth_descriptor, expect_gameplay_capabilities, expect_gameplay_descriptor,
    expect_protocol_bedrock_listener_descriptor, expect_protocol_capabilities,
    expect_protocol_descriptor, expect_storage_capabilities, expect_storage_descriptor,
};
pub(super) use self::manifest::{
    DecodedManifest, ManifestCapabilities, decode_manifest, decode_utf8_slice,
};
pub(super) use self::profiles::{ensure_known_profiles, ensure_profile_known};
pub(super) use self::reload::{
    import_storage_runtime_state, protocol_reload_compatible, validate_gameplay_session_migration,
    validate_protocol_session_migration,
};
