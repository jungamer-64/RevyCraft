use crate::buffers::into_owned_buffer;
use mc_plugin_api::abi::{ByteSlice, OwnedBuffer};

#[must_use]
pub const unsafe fn byte_slice_as_bytes(slice: ByteSlice) -> &'static [u8] {
    if slice.ptr.is_null() || slice.len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(slice.ptr, slice.len) }
    }
}

/// # Safety
///
/// `buffer` must have been allocated by [`into_owned_buffer`].
pub unsafe fn free_owned_buffer(buffer: OwnedBuffer) {
    if !buffer.ptr.is_null() {
        unsafe {
            let _ = Vec::from_raw_parts(buffer.ptr, buffer.len, buffer.cap);
        }
    }
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
