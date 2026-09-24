//! Owned plugin objects at the ABI boundary. Immutable code and manifest storage may
//! be shared; mutable plugin state belongs to an object returned by `create_instance`.

use crate::__macro_support::buffers::write_error_buffer;
use mc_plugin_abi::raw::{OwnedBuffer, PluginStatus};
use std::ffi::c_void;

/// Allocates a plugin object for one host-owned generation.
/// Constructor panics map to `INTERNAL`; allocator aborts are not recoverable.
///
/// # Safety
///
/// `output` must be writable and aligned for one pointer; `error_out`, if non-null,
/// must be writable and aligned for an `OwnedBuffer`. Success transfers the object
/// to the caller, which must eventually use `destroy_instance::<T>` exactly once.
pub unsafe extern "C" fn create_instance<T: Default + Send + Sync>(
    output: *mut *mut c_void,
    error_out: *mut OwnedBuffer,
) -> PluginStatus {
    if output.is_null()
        || !output
            .addr()
            .is_multiple_of(std::mem::align_of::<*mut c_void>())
    {
        write_error_buffer(
            error_out,
            "invalid plugin instance output pointer".to_string(),
        );
        return PluginStatus::INVALID_INPUT;
    }
    // SAFETY: the caller provides writable pointer storage, validated for null/alignment.
    unsafe { output.write(std::ptr::null_mut()) };
    match std::panic::catch_unwind(|| Box::new(T::default())) {
        Ok(instance) => {
            // SAFETY: output remains exclusively writable for this call. Only success
            // transfers the allocation, which the matching destroy callback consumes.
            unsafe { output.write(Box::into_raw(instance).cast()) };
            PluginStatus::OK
        }
        Err(_) => {
            write_error_buffer(error_out, "plugin object constructor panicked".to_string());
            PluginStatus::INTERNAL
        }
    }
}

/// Borrows a plugin object only for the invocation closure.
///
/// # Safety
///
/// `instance` must be an unconsumed result of `create_instance::<T>` in this library.
/// It must remain alive throughout `invoke`; no destroy call may overlap it.
///
/// # Errors
///
/// Rejects null or misaligned handles before constructing a reference. Allocation
/// provenance and the concrete type remain the ABI caller's responsibility.
pub unsafe fn with_instance<T: Send + Sync, R>(
    instance: *mut c_void,
    invoke: impl FnOnce(&T) -> R,
) -> Result<R, String> {
    if instance.is_null() || !instance.addr().is_multiple_of(std::mem::align_of::<T>()) {
        return Err("invalid plugin instance pointer".to_string());
    }
    // SAFETY: caller guarantees the matching live allocation and no concurrent destroy;
    // T: Sync permits shared calls, and the reference cannot outlive the closure.
    Ok(invoke(unsafe { &*instance.cast::<T>() }))
}

/// Consumes the object with its allocating library's destructor and allocator.
/// A destructor panic is terminal (the C ABI does not unwind).
///
/// # Safety
///
/// `instance` is an unconsumed result of `create_instance::<T>` in this library.
/// All invocations and plugin-owned asynchronous work must already have completed.
pub unsafe extern "C" fn destroy_instance<T: Send + Sync>(instance: *mut c_void) {
    // SAFETY: the caller transfers the unique allocation back after all borrowers and
    // workers have finished. No other path reconstructs or frees this Box.
    drop(unsafe { Box::from_raw(instance.cast::<T>()) });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn objects_have_independent_lifetimes() -> Result<(), String> {
        static DROPS: AtomicUsize = AtomicUsize::new(0);
        #[derive(Default)]
        struct Object(AtomicUsize);
        impl Drop for Object {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::SeqCst);
            }
        }
        let mut first = std::ptr::null_mut();
        let mut second = std::ptr::null_mut();
        let mut error = OwnedBuffer::empty();
        // SAFETY: each constructor receives live, exclusively writable output storage.
        assert_eq!(
            unsafe { create_instance::<Object>(&raw mut first, &raw mut error) },
            PluginStatus::OK
        );
        // SAFETY: same as above, with distinct output storage for the second object.
        assert_eq!(
            unsafe { create_instance::<Object>(&raw mut second, &raw mut error) },
            PluginStatus::OK
        );
        // SAFETY: both handles came from the matching constructor and remain owned here.
        unsafe { with_instance::<Object, _>(first, |object| object.0.store(7, Ordering::SeqCst)) }?;
        // SAFETY: second is still live and has not been consumed.
        assert_eq!(
            unsafe {
                with_instance::<Object, _>(second, |object| object.0.load(Ordering::SeqCst))
            }?,
            0
        );
        // SAFETY: no calls remain in flight; consume first exactly once.
        unsafe { destroy_instance::<Object>(first) };
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
        // SAFETY: first's destruction did not consume second.
        assert_eq!(
            unsafe {
                with_instance::<Object, _>(second, |object| object.0.load(Ordering::SeqCst))
            }?,
            0
        );
        // SAFETY: consume the remaining object exactly once after its last borrower.
        unsafe { destroy_instance::<Object>(second) };
        assert_eq!(DROPS.load(Ordering::SeqCst), 2);
        Ok(())
    }

    #[test]
    fn constructor_panic_returns_error_without_transferring_an_object() {
        struct Failing;
        impl Default for Failing {
            fn default() -> Self {
                panic!("constructor failure fixture")
            }
        }
        let mut instance = std::ptr::null_mut();
        let mut error = OwnedBuffer::empty();
        // SAFETY: outputs refer to live, aligned, exclusively writable storage.
        assert_eq!(
            unsafe { create_instance::<Failing>(&raw mut instance, &raw mut error) },
            PluginStatus::INTERNAL
        );
        assert!(instance.is_null());
        assert!(error.len > 0);
        // SAFETY: the error buffer was allocated by this SDK boundary exactly once.
        unsafe { crate::__macro_support::buffers::free_owned_buffer(error) };
    }
}
