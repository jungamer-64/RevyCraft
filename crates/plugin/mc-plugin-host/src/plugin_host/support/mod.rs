mod describe;
mod invoke;
mod manifest;
mod profiles;
mod reload;

use super::{
    AdminUiDescriptor, AdminUiInput, AdminUiOutput, AdminUiPluginApiV1, AdminUiProfileId, Arc,
    AuthPluginApiV1, AuthRequest, AuthResponse, BedrockListenerDescriptor, CapabilitySet,
    GameplayGeneration, GameplayPluginApiV2, GameplayProfileId, GameplayRequest, GameplayResponse,
    GameplaySessionSnapshot, HashMap, HashSet, ManagedGameplayPlugin, ManagedProtocolPlugin,
    OwnedBuffer, PluginAbiVersion, PluginErrorCode, PluginKind, PluginManifestV1,
    ProtocolDescriptor, ProtocolGeneration, ProtocolPluginApiV1, ProtocolRequest, ProtocolResponse,
    RuntimeError, RuntimeReloadContext, StorageGeneration, StoragePluginApiV1, StorageRequest,
    StorageResponse, admin_ui_host_api, decode_admin_ui_output, decode_auth_response,
    decode_gameplay_response, decode_plugin_error, decode_protocol_response,
    decode_storage_response, encode_admin_ui_input, encode_auth_request, encode_gameplay_request,
    encode_protocol_request, encode_storage_request, gameplay_host_api,
};
use crate::runtime::ProtocolReloadSession;
use mc_plugin_api::abi::{ByteSlice, Utf8Slice};
use mc_plugin_api::codec::auth::AuthDescriptor;
use mc_plugin_api::codec::gameplay::GameplayDescriptor;
use mc_plugin_api::codec::protocol::ProtocolSessionSnapshot;
use mc_plugin_api::codec::storage::StorageDescriptor;

pub(super) use self::describe::{
    expect_admin_ui_capabilities, expect_admin_ui_descriptor, expect_auth_capabilities,
    expect_auth_descriptor, expect_gameplay_capabilities, expect_gameplay_descriptor,
    expect_protocol_bedrock_listener_descriptor, expect_protocol_capabilities,
    expect_protocol_descriptor, expect_storage_capabilities, expect_storage_descriptor,
};
pub(super) use self::invoke::{
    invoke_admin_ui, invoke_auth, invoke_gameplay, invoke_protocol, invoke_storage,
};
pub(super) use self::manifest::{
    DecodedManifest, admin_ui_profile_id_from_manifest, decode_manifest, decode_utf8_slice,
    gameplay_profile_id_from_manifest, manifest_profile_id, require_manifest_capability,
};
pub(super) use self::profiles::{ensure_known_profiles, ensure_profile_known};
pub(super) use self::reload::{
    import_storage_runtime_state, migrate_gameplay_sessions, migrate_protocol_sessions,
    protocol_reload_compatible,
};
