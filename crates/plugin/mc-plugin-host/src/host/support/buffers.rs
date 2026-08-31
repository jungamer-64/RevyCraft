use super::{ByteSlice, OwnedBuffer, PluginFreeBufferFn, RuntimeError, Utf8Slice};
use std::mem::size_of;

fn checked_byte_len(
    len: usize,
    element_size: usize,
    max_bytes: usize,
    what: &str,
) -> Result<usize, String> {
    let byte_len = len
        .checked_mul(element_size)
        .ok_or_else(|| format!("{what} length overflowed"))?;
    if byte_len > max_bytes {
        return Err(format!(
            "{what} exceeded configured limit: {byte_len} bytes > {max_bytes} bytes"
        ));
    }
    if byte_len > isize::MAX as usize {
        return Err(format!(
            "{what} exceeded the maximum addressable slice length"
        ));
    }
    Ok(byte_len)
}

pub(crate) fn read_byte_slice<'a>(
    slice: ByteSlice,
    max_bytes: usize,
    what: &str,
) -> Result<&'a [u8], String> {
    if slice.ptr.is_null() {
        return if slice.len == 0 {
            Ok(&[])
        } else {
            Err(format!("{what} pointer was null with non-zero length"))
        };
    }
    checked_byte_len(slice.len, size_of::<u8>(), max_bytes, what)?;
    Ok(unsafe { std::slice::from_raw_parts(slice.ptr, slice.len) })
}

pub(crate) fn decode_utf8_slice_with_limit(
    slice: Utf8Slice,
    max_bytes: usize,
    what: &str,
) -> Result<String, RuntimeError> {
    if slice.ptr.is_null() {
        return Err(RuntimeError::Config(format!("{what} pointer was null")));
    }
    checked_byte_len(slice.len, size_of::<u8>(), max_bytes, what).map_err(RuntimeError::Config)?;
    let bytes = unsafe { std::slice::from_raw_parts(slice.ptr, slice.len) };
    String::from_utf8(bytes.to_vec()).map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(crate) fn read_checked_slice<'a, T>(
    ptr: *const T,
    len: usize,
    max_bytes: usize,
    what: &str,
) -> Result<&'a [T], RuntimeError> {
    if ptr.is_null() {
        return if len == 0 {
            Ok(&[])
        } else {
            Err(RuntimeError::Config(format!(
                "{what} pointer was null with non-zero length"
            )))
        };
    }
    checked_byte_len(len, size_of::<T>(), max_bytes, what).map_err(RuntimeError::Config)?;
    if !(ptr as usize).is_multiple_of(std::mem::align_of::<T>()) {
        return Err(RuntimeError::Config(format!(
            "{what} pointer was not properly aligned"
        )));
    }
    Ok(unsafe { std::slice::from_raw_parts(ptr, len) })
}

struct ForeignOwnedBuffer<L> {
    buffer: Option<OwnedBuffer>,
    free_buffer: PluginFreeBufferFn,
    _generation_lease: L,
}

impl<L> ForeignOwnedBuffer<L> {
    fn new(buffer: OwnedBuffer, free_buffer: PluginFreeBufferFn, generation_lease: L) -> Self {
        Self {
            buffer: Some(buffer),
            free_buffer,
            _generation_lease: generation_lease,
        }
    }

    fn copy_with_limit(&self, max_bytes: usize, what: &str) -> Result<Vec<u8>, String> {
        let buffer = self
            .buffer
            .as_ref()
            .expect("foreign buffer is present until its guard is dropped");
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
        if buffer.cap > isize::MAX as usize {
            return Err(format!(
                "{what} capacity exceeded the addressable allocation limit"
            ));
        }
        let byte_len = checked_byte_len(buffer.len, size_of::<u8>(), max_bytes, what)?;
        Ok(unsafe { std::slice::from_raw_parts(buffer.ptr, byte_len) }.to_vec())
    }
}

impl<L> Drop for ForeignOwnedBuffer<L> {
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

pub(crate) fn take_owned_buffer<L>(
    free_buffer: PluginFreeBufferFn,
    generation_lease: L,
    buffer: OwnedBuffer,
    max_bytes: usize,
    what: &str,
) -> Result<Vec<u8>, String> {
    ForeignOwnedBuffer::new(buffer, free_buffer, generation_lease).copy_with_limit(max_bytes, what)
}

pub(crate) fn release_owned_buffer<L>(
    free_buffer: PluginFreeBufferFn,
    generation_lease: L,
    buffer: OwnedBuffer,
) {
    drop(ForeignOwnedBuffer::new(
        buffer,
        free_buffer,
        generation_lease,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        static FREE_COUNT: Cell<usize> = const { Cell::new(0) };
    }

    fn owned_buffer(bytes: Vec<u8>) -> OwnedBuffer {
        let mut bytes = bytes;
        let buffer = OwnedBuffer {
            ptr: bytes.as_mut_ptr(),
            len: bytes.len(),
            cap: bytes.capacity(),
        };
        std::mem::forget(bytes);
        buffer
    }

    unsafe extern "C" fn counting_free_buffer(buffer: OwnedBuffer) {
        FREE_COUNT.with(|count| count.set(count.get() + 1));
        if buffer.ptr.is_null() {
            return;
        }
        let _ = unsafe { Vec::from_raw_parts(buffer.ptr, buffer.len.min(buffer.cap), buffer.cap) };
    }

    #[test]
    fn take_owned_buffer_frees_valid_buffers_once() {
        FREE_COUNT.with(|count| count.set(0));

        let bytes = take_owned_buffer(
            counting_free_buffer,
            (),
            owned_buffer(vec![1, 2, 3]),
            16,
            "test buffer",
        )
        .expect("buffer should be returned");

        assert_eq!(bytes, vec![1, 2, 3]);
        FREE_COUNT.with(|count| assert_eq!(count.get(), 1));
    }

    #[test]
    fn take_owned_buffer_frees_oversized_buffers_once() {
        FREE_COUNT.with(|count| count.set(0));

        let error = take_owned_buffer(
            counting_free_buffer,
            (),
            owned_buffer(vec![1, 2, 3]),
            2,
            "test buffer",
        )
        .expect_err("oversized buffer should fail");

        assert!(error.contains("exceeded configured limit"));
        FREE_COUNT.with(|count| assert_eq!(count.get(), 1));
    }

    #[test]
    fn take_owned_buffer_rejects_length_larger_than_capacity_and_frees_once() {
        FREE_COUNT.with(|count| count.set(0));
        let mut buffer = owned_buffer(vec![1, 2, 3]);
        buffer.len = buffer.cap + 1;

        let error = take_owned_buffer(counting_free_buffer, (), buffer, 16, "test buffer")
            .expect_err("invalid length and capacity must fail");

        assert!(error.contains("exceeded capacity"));
        FREE_COUNT.with(|count| assert_eq!(count.get(), 1));
    }
}
