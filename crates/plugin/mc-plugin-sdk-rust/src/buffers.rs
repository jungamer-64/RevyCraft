use super::*;

#[must_use]
pub const fn into_owned_buffer(mut buffer: Vec<u8>) -> OwnedBuffer {
    let owned = OwnedBuffer {
        ptr: buffer.as_mut_ptr(),
        len: buffer.len(),
        cap: buffer.capacity(),
    };
    std::mem::forget(buffer);
    owned
}
