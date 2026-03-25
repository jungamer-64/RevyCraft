use super::{
    AdminUiInput, AdminUiOutput, AdminUiPluginApiV1, AuthPluginApiV1, AuthRequest, AuthResponse,
    ByteSlice, GameplayPluginApiV3, GameplayRequest, GameplayResponse, OwnedBuffer,
    PluginErrorCode, ProtocolPluginApiV2, ProtocolRequest, ProtocolResponse, RuntimeError,
    StoragePluginApiV1, StorageRequest, StorageResponse, admin_ui_host_api, decode_admin_ui_output,
    decode_auth_response, decode_gameplay_response, decode_plugin_error, decode_protocol_response,
    decode_storage_response, encode_admin_ui_input, encode_auth_request, encode_gameplay_request,
    encode_protocol_request, encode_storage_request, gameplay_host_api, take_owned_buffer,
};
use crate::config::PluginBufferLimits;

pub(crate) fn invoke_protocol(
    api: &ProtocolPluginApiV2,
    request: &ProtocolRequest,
    buffer_limits: PluginBufferLimits,
) -> Result<ProtocolResponse, RuntimeError> {
    let request_bytes = encode_protocol_request(request)
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (api.invoke)(
            ByteSlice {
                ptr: request_bytes.as_ptr(),
                len: request_bytes.len(),
            },
            &raw mut output,
            &raw mut error,
        )
    };
    if status != PluginErrorCode::Ok {
        let message = if error.ptr.is_null() {
            format!("plugin returned {status:?}")
        } else {
            let bytes = take_owned_buffer(
                api.free_buffer,
                error,
                buffer_limits.metadata_bytes,
                "plugin error buffer",
            )
            .map_err(RuntimeError::Config)?;
            String::from_utf8(bytes).unwrap_or_else(|_| "plugin returned invalid utf-8".to_string())
        };
        return Err(RuntimeError::Config(message));
    }

    let response_bytes = take_owned_buffer(
        api.free_buffer,
        output,
        buffer_limits.protocol_response_bytes,
        "protocol response buffer",
    )
    .map_err(RuntimeError::Config)?;
    decode_protocol_response(request, &response_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(crate) fn invoke_gameplay(
    plugin_id: &str,
    api: &GameplayPluginApiV3,
    request: &GameplayRequest,
    buffer_limits: PluginBufferLimits,
) -> Result<GameplayResponse, RuntimeError> {
    let request_bytes = encode_gameplay_request(request)
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let host_api = gameplay_host_api();
    let status = unsafe {
        (api.invoke)(
            ByteSlice {
                ptr: request_bytes.as_ptr(),
                len: request_bytes.len(),
            },
            &raw const host_api,
            &raw mut output,
            &raw mut error,
        )
    };
    if status != PluginErrorCode::Ok {
        return Err(RuntimeError::Config(decode_plugin_error(
            plugin_id,
            status,
            api.free_buffer,
            error,
            buffer_limits.metadata_bytes,
        )));
    }
    let response_bytes = take_owned_buffer(
        api.free_buffer,
        output,
        buffer_limits.gameplay_response_bytes,
        "gameplay response buffer",
    )
    .map_err(RuntimeError::Config)?;
    decode_gameplay_response(request, &response_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(crate) fn invoke_storage(
    plugin_id: &str,
    api: &StoragePluginApiV1,
    request: &StorageRequest,
    buffer_limits: PluginBufferLimits,
) -> Result<StorageResponse, RuntimeError> {
    let request_bytes =
        encode_storage_request(request).map_err(|error| RuntimeError::Config(error.to_string()))?;
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (api.invoke)(
            ByteSlice {
                ptr: request_bytes.as_ptr(),
                len: request_bytes.len(),
            },
            &raw mut output,
            &raw mut error,
        )
    };
    if status != PluginErrorCode::Ok {
        return Err(RuntimeError::Config(decode_plugin_error(
            plugin_id,
            status,
            api.free_buffer,
            error,
            buffer_limits.metadata_bytes,
        )));
    }

    let response_bytes = take_owned_buffer(
        api.free_buffer,
        output,
        buffer_limits.storage_response_bytes,
        "storage response buffer",
    )
    .map_err(RuntimeError::Config)?;
    decode_storage_response(request, &response_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(crate) fn invoke_auth(
    plugin_id: &str,
    api: &AuthPluginApiV1,
    request: &AuthRequest,
    buffer_limits: PluginBufferLimits,
) -> Result<AuthResponse, RuntimeError> {
    let request_bytes =
        encode_auth_request(request).map_err(|error| RuntimeError::Config(error.to_string()))?;
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (api.invoke)(
            ByteSlice {
                ptr: request_bytes.as_ptr(),
                len: request_bytes.len(),
            },
            &raw mut output,
            &raw mut error,
        )
    };
    if status != PluginErrorCode::Ok {
        return Err(RuntimeError::Config(decode_plugin_error(
            plugin_id,
            status,
            api.free_buffer,
            error,
            buffer_limits.metadata_bytes,
        )));
    }

    let response_bytes = take_owned_buffer(
        api.free_buffer,
        output,
        buffer_limits.auth_response_bytes,
        "auth response buffer",
    )
    .map_err(RuntimeError::Config)?;
    decode_auth_response(request, &response_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(crate) fn invoke_admin_ui(
    plugin_id: &str,
    api: &AdminUiPluginApiV1,
    request: &AdminUiInput,
    buffer_limits: PluginBufferLimits,
) -> Result<AdminUiOutput, RuntimeError> {
    let request_bytes =
        encode_admin_ui_input(request).map_err(|error| RuntimeError::Config(error.to_string()))?;
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let host_api = admin_ui_host_api();
    let status = unsafe {
        (api.invoke)(
            ByteSlice {
                ptr: request_bytes.as_ptr(),
                len: request_bytes.len(),
            },
            &raw const host_api,
            &raw mut output,
            &raw mut error,
        )
    };
    if status != PluginErrorCode::Ok {
        return Err(RuntimeError::Config(decode_plugin_error(
            plugin_id,
            status,
            api.free_buffer,
            error,
            buffer_limits.metadata_bytes,
        )));
    }

    let response_bytes = take_owned_buffer(
        api.free_buffer,
        output,
        buffer_limits.admin_ui_response_bytes,
        "admin-ui response buffer",
    )
    .map_err(RuntimeError::Config)?;
    decode_admin_ui_output(request, &response_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}
