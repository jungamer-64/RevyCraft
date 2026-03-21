use super::{
    AuthPluginApiV1, AuthRequest, AuthResponse, ByteSlice, GameplayPluginApiV1, GameplayRequest,
    GameplayResponse, OwnedBuffer, PluginErrorCode, ProtocolPluginApiV1, ProtocolRequest,
    ProtocolResponse, RuntimeError, StoragePluginApiV1, StorageRequest, StorageResponse,
    decode_auth_response, decode_gameplay_response, decode_plugin_error, decode_protocol_response,
    decode_storage_response, encode_auth_request, encode_gameplay_request, encode_protocol_request,
    encode_storage_request,
};

pub(crate) fn invoke_protocol(
    api: &ProtocolPluginApiV1,
    request: &ProtocolRequest,
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
            let bytes = unsafe { std::slice::from_raw_parts(error.ptr, error.len) }.to_vec();
            unsafe {
                (api.free_buffer)(error);
            }
            String::from_utf8(bytes).unwrap_or_else(|_| "plugin returned invalid utf-8".to_string())
        };
        return Err(RuntimeError::Config(message));
    }

    let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
    unsafe {
        (api.free_buffer)(output);
    }
    decode_protocol_response(request, &response_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(crate) fn invoke_gameplay(
    plugin_id: &str,
    api: &GameplayPluginApiV1,
    request: &GameplayRequest,
) -> Result<GameplayResponse, RuntimeError> {
    let request_bytes = encode_gameplay_request(request)
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
        return Err(RuntimeError::Config(decode_plugin_error(
            plugin_id,
            status,
            api.free_buffer,
            error,
        )));
    }
    let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
    unsafe {
        (api.free_buffer)(output);
    }
    decode_gameplay_response(request, &response_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(crate) fn invoke_storage(
    plugin_id: &str,
    api: &StoragePluginApiV1,
    request: &StorageRequest,
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
        )));
    }

    let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
    unsafe {
        (api.free_buffer)(output);
    }
    decode_storage_response(request, &response_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(crate) fn invoke_auth(
    plugin_id: &str,
    api: &AuthPluginApiV1,
    request: &AuthRequest,
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
        )));
    }

    let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
    unsafe {
        (api.free_buffer)(output);
    }
    decode_auth_response(request, &response_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}
