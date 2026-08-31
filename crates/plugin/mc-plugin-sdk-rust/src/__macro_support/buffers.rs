use crate::buffers::into_owned_buffer;
use mc_plugin_abi::CURRENT_PLUGIN_ABI;
use mc_plugin_abi::host::{HostFreeBufferFn, PluginApiHeaderV9};
use mc_plugin_abi::raw::{ByteSlice, OwnedBuffer};

/// # Safety
///
/// A non-null pointer must reference `len` readable bytes for the duration of the call.
pub unsafe fn byte_slice_as_bytes(slice: ByteSlice) -> Result<&'static [u8], String> {
    if slice.ptr.is_null() {
        return if slice.len == 0 {
            Ok(&[])
        } else {
            Err("byte slice pointer was null with non-zero length".to_string())
        };
    }
    if slice.len > isize::MAX as usize {
        return Err("byte slice exceeded the maximum addressable length".to_string());
    }
    Ok(unsafe { std::slice::from_raw_parts(slice.ptr, slice.len) })
}

/// # Safety
///
/// `api` must either be null or point to a readable ABI header. When the header declares a
/// compatible full layout, it must point to that complete layout for the duration of the call.
pub unsafe fn read_host_api<T: Copy>(api: *const T, what: &str) -> Result<T, String> {
    if api.is_null() {
        return Err(format!("{what} pointer was null"));
    }
    if !(api as usize).is_multiple_of(std::mem::align_of::<T>()) {
        return Err(format!("{what} pointer was not properly aligned"));
    }
    let header = unsafe { api.cast::<PluginApiHeaderV9>().read() };
    if header.abi != CURRENT_PLUGIN_ABI {
        return Err(format!(
            "{what} ABI {} did not match {}",
            header.abi, CURRENT_PLUGIN_ABI
        ));
    }
    if header.struct_size < std::mem::size_of::<T>() {
        return Err(format!(
            "{what} size {} was smaller than {}",
            header.struct_size,
            std::mem::size_of::<T>()
        ));
    }
    Ok(unsafe { api.read() })
}

/// # Safety
///
/// `buffer` must have been allocated by [`into_owned_buffer`].
pub unsafe fn free_owned_buffer(buffer: OwnedBuffer) {
    if !buffer.ptr.is_null() {
        unsafe {
            let _ = Vec::from_raw_parts(buffer.ptr, buffer.len.min(buffer.cap), buffer.cap);
        }
    }
}

struct HostOwnedBuffer {
    buffer: Option<OwnedBuffer>,
    free_buffer: HostFreeBufferFn,
}

impl HostOwnedBuffer {
    fn new(buffer: OwnedBuffer, free_buffer: HostFreeBufferFn) -> Self {
        Self {
            buffer: Some(buffer),
            free_buffer,
        }
    }

    fn copy(&self, what: &str) -> Result<Vec<u8>, String> {
        let buffer = self
            .buffer
            .as_ref()
            .expect("host buffer remains owned until its guard is dropped");
        if buffer.ptr.is_null() {
            return if buffer.len == 0 && buffer.cap == 0 {
                Ok(Vec::new())
            } else {
                Err(format!(
                    "{what} pointer was null with non-zero length or capacity"
                ))
            };
        }
        if buffer.len > buffer.cap {
            return Err(format!(
                "{what} length {} exceeded capacity {}",
                buffer.len, buffer.cap
            ));
        }
        if buffer.cap > isize::MAX as usize || buffer.len > isize::MAX as usize {
            return Err(format!("{what} exceeded the maximum addressable length"));
        }
        Ok(unsafe { std::slice::from_raw_parts(buffer.ptr, buffer.len) }.to_vec())
    }
}

impl Drop for HostOwnedBuffer {
    fn drop(&mut self) {
        let Some(buffer) = self.buffer.take() else {
            return;
        };
        if buffer.ptr.is_null() && buffer.len == 0 && buffer.cap == 0 {
            return;
        }
        unsafe {
            (self.free_buffer)(buffer);
        }
    }
}

pub fn copy_host_owned_buffer(
    buffer: OwnedBuffer,
    free_buffer: HostFreeBufferFn,
    what: &str,
) -> Result<Vec<u8>, String> {
    HostOwnedBuffer::new(buffer, free_buffer).copy(what)
}

pub fn release_host_owned_buffer(buffer: OwnedBuffer, free_buffer: HostFreeBufferFn) {
    drop(HostOwnedBuffer::new(buffer, free_buffer));
}

fn write_owned_buffer_ptr(output: *mut OwnedBuffer, bytes: Vec<u8>) {
    unsafe {
        *output = into_owned_buffer(bytes);
    }
}

pub fn write_error_buffer(error_out: *mut OwnedBuffer, message: String) {
    if error_out.is_null() {
        return;
    }
    write_owned_buffer_ptr(error_out, message.into_bytes());
}

pub fn write_output_buffer(output: *mut OwnedBuffer, bytes: Vec<u8>) {
    if output.is_null() {
        return;
    }
    write_owned_buffer_ptr(output, bytes);
}
