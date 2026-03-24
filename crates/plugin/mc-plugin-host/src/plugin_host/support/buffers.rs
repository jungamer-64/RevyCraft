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
    Ok(unsafe { std::slice::from_raw_parts(ptr, len) })
}

pub(crate) fn take_owned_buffer(
    free_buffer: PluginFreeBufferFn,
    buffer: OwnedBuffer,
    max_bytes: usize,
    what: &str,
) -> Result<Vec<u8>, String> {
    if buffer.ptr.is_null() {
        return if buffer.len == 0 {
            Ok(Vec::new())
        } else {
            Err(format!("{what} pointer was null with non-zero length"))
        };
    }
    let result = match checked_byte_len(buffer.len, size_of::<u8>(), max_bytes, what) {
        Ok(byte_len) => Ok(unsafe { std::slice::from_raw_parts(buffer.ptr, byte_len) }.to_vec()),
        Err(error) => Err(error),
    };
    unsafe {
        (free_buffer)(buffer);
    }
    result
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
        let _ = unsafe { Vec::from_raw_parts(buffer.ptr, buffer.len, buffer.cap) };
    }

    #[test]
    fn take_owned_buffer_frees_valid_buffers_once() {
        FREE_COUNT.with(|count| count.set(0));

        let bytes = take_owned_buffer(
            counting_free_buffer,
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
            owned_buffer(vec![1, 2, 3]),
            2,
            "test buffer",
        )
        .expect_err("oversized buffer should fail");

        assert!(error.contains("exceeded configured limit"));
        FREE_COUNT.with(|count| assert_eq!(count.get(), 1));
    }
}
