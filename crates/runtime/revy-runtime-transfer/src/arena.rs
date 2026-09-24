use crate::{MAX_TRANSFER_ARENA_BYTES, TransferArenaV1, TransferId};
use std::io::{self, Write};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const ARENA_ALIGNMENT: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum ArenaError {
    #[error("transfer arena capacity must be greater than zero")]
    EmptyCapacity,
    #[error("transfer arena generation must be greater than zero")]
    InvalidGeneration,
    #[error("transfer arena capacity {capacity} exceeds policy limit {limit}")]
    CapacityLimitExceeded { capacity: u64, limit: u64 },
    #[error("transfer arena capacity {capacity} does not fit the process address space")]
    AddressSpaceExceeded { capacity: u64 },
    #[error(
        "transfer arena has {remaining} bytes remaining but an aligned reservation of {requested} bytes was requested"
    )]
    CapacityExhausted { requested: usize, remaining: usize },
    #[error("transfer arena region {offset}..{end} exceeds published length {published}")]
    InvalidRegion {
        offset: usize,
        end: usize,
        published: usize,
    },
    #[error("transfer arena reservation expected {expected} bytes but only {written} were written")]
    IncompleteReservation { expected: usize, written: usize },
    #[error("transfer arena reservation received more than its {capacity} byte capacity")]
    ReservationOverflow { capacity: usize },
    #[error("transfer arena mapping operation failed: {0}")]
    Mapping(#[source] io::Error),
    #[error("transfer arena mapping locator is invalid")]
    InvalidMappingLocator,
    #[error("transfer arena publication cannot move backwards from {current} to {requested}")]
    PublicationRegression { current: usize, requested: usize },
}

pub struct SharedTransferArena {
    mapping: Arc<Mapping>,
    next_offset: AtomicUsize,
    generation: u64,
}

impl SharedTransferArena {
    /// Creates an OS-backed shared transfer arena with a fixed logical budget.
    ///
    /// # Errors
    ///
    /// Returns [`ArenaError`] when the capacity or generation is invalid, exceeds policy, or
    /// the operating system cannot create and map the shared memory object.
    pub fn create(
        capacity: u64,
        generation: u64,
        transfer_id: TransferId,
        authentication_token: [u8; crate::AUTHENTICATION_TOKEN_BYTES],
    ) -> Result<Self, ArenaError> {
        let capacity = validate_parameters(capacity, generation)?;
        let mapping = Mapping::create(capacity, transfer_id, authentication_token, generation)?;
        Ok(Self {
            mapping: Arc::new(mapping),
            next_offset: AtomicUsize::new(0),
            generation,
        })
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.mapping.capacity
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn used(&self) -> usize {
        self.next_offset.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn descriptor(&self) -> TransferArenaV1 {
        TransferArenaV1 {
            capacity: self.capacity() as u64,
            used: self.used() as u64,
            generation: self.generation,
        }
    }

    /// Reserves one aligned, non-overlapping region from the arena budget.
    ///
    /// # Errors
    ///
    /// Returns [`ArenaError::CapacityExhausted`] when the requested region and alignment padding
    /// do not fit in the remaining logical budget.
    pub fn reserve(&self, length: usize) -> Result<ArenaReservation, ArenaError> {
        let mut current = self.next_offset.load(Ordering::Relaxed);
        loop {
            let aligned = align_up(current).ok_or_else(|| self.exhausted(length, current))?;
            let end = aligned
                .checked_add(length)
                .ok_or_else(|| self.exhausted(length, current))?;
            if end > self.capacity() {
                return Err(self.exhausted(length, current));
            }
            match self.next_offset.compare_exchange_weak(
                current,
                end,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    return Ok(ArenaReservation {
                        mapping: Arc::clone(&self.mapping),
                        offset: aligned,
                        capacity: length,
                        written: 0,
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn exhausted(&self, requested: usize, current: usize) -> ArenaError {
        ArenaError::CapacityExhausted {
            requested,
            remaining: self.capacity().saturating_sub(current),
        }
    }

    #[cfg(unix)]
    /// Duplicates the shared-memory descriptor for transfer over `SCM_RIGHTS`.
    ///
    /// # Errors
    ///
    /// Returns [`ArenaError`] when the operating system cannot duplicate the descriptor.
    pub fn duplicate_descriptor(&self) -> Result<UnixArenaDescriptor, ArenaError> {
        self.mapping.duplicate_descriptor()
    }

    #[cfg(windows)]
    #[must_use]
    pub fn mapping_name(&self) -> WindowsArenaName {
        self.mapping.mapping_name()
    }
}

pub struct ArenaReservation {
    mapping: Arc<Mapping>,
    offset: usize,
    capacity: usize,
    written: usize,
}

impl ArenaReservation {
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.offset
    }

    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    #[must_use]
    pub const fn written(&self) -> usize {
        self.written
    }

    /// Consumes the write authority and publishes an immutable region capability.
    ///
    /// # Errors
    ///
    /// Returns [`ArenaError::IncompleteReservation`] unless every reserved byte was initialized.
    pub fn seal(self) -> Result<SealedArenaRegion, ArenaError> {
        if self.written != self.capacity {
            return Err(ArenaError::IncompleteReservation {
                expected: self.capacity,
                written: self.written,
            });
        }
        std::sync::atomic::fence(Ordering::Release);
        Ok(SealedArenaRegion {
            mapping: self.mapping,
            offset: self.offset,
            length: self.capacity,
        })
    }

    /// Consumes the write authority and publishes only the initialized prefix.
    ///
    /// This is intended for a staging-time maximum-size reservation whose exact final length is
    /// known only when a bounded delta is sealed. The unused tail remains unavailable to later
    /// reservations, so disjoint ownership is retained without exposing uninitialized memory.
    #[must_use]
    pub fn seal_written(self) -> SealedArenaRegion {
        std::sync::atomic::fence(Ordering::Release);
        SealedArenaRegion {
            mapping: self.mapping,
            offset: self.offset,
            length: self.written,
        }
    }
}

impl Write for ArenaReservation {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let remaining = self.capacity - self.written;
        if buffer.len() > remaining {
            return Err(io::Error::other(ArenaError::ReservationOverflow {
                capacity: self.capacity,
            }));
        }
        if buffer.is_empty() {
            return Ok(0);
        }
        // SAFETY: reservations are allocated as disjoint ranges by the arena's atomic bump
        // allocator. This reservation exclusively owns its range until `seal` consumes it,
        // and the checked bounds keep the copy within the stable mapping.
        unsafe {
            std::ptr::copy_nonoverlapping(
                buffer.as_ptr(),
                self.mapping.base.as_ptr().add(self.offset + self.written),
                buffer.len(),
            );
        }
        self.written += buffer.len();
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct SealedArenaRegion {
    mapping: Arc<Mapping>,
    offset: usize,
    length: usize,
}

impl SealedArenaRegion {
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.offset
    }

    #[must_use]
    pub const fn length(&self) -> usize {
        self.length
    }

    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        std::sync::atomic::fence(Ordering::Acquire);
        // SAFETY: this range was bounds checked when it was reserved and cannot be written
        // through the safe API after sealing. The Arc keeps the stable mapping alive.
        unsafe {
            std::slice::from_raw_parts(self.mapping.base.as_ptr().add(self.offset), self.length)
        }
    }
}

pub struct SharedTransferArenaReader {
    mapping: Mapping,
    published: usize,
    generation: u64,
}

impl SharedTransferArenaReader {
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub const fn published(&self) -> usize {
        self.published
    }

    /// Advances the immutable prefix that the parent has published into this mapping.
    ///
    /// # Errors
    ///
    /// Returns [`ArenaError`] when the new boundary moves backwards, exceeds the mapping, or does
    /// not fit the child address space.
    pub fn advance_published(&mut self, published: u64) -> Result<(), ArenaError> {
        let requested =
            usize::try_from(published).map_err(|_| ArenaError::AddressSpaceExceeded {
                capacity: published,
            })?;
        if requested < self.published {
            return Err(ArenaError::PublicationRegression {
                current: self.published,
                requested,
            });
        }
        if requested > self.mapping.capacity {
            return Err(ArenaError::InvalidRegion {
                offset: 0,
                end: requested,
                published: self.mapping.capacity,
            });
        }
        std::sync::atomic::fence(Ordering::Acquire);
        self.published = requested;
        Ok(())
    }

    /// Borrows a previously published region from the read-only mapping.
    ///
    /// # Errors
    ///
    /// Returns [`ArenaError::InvalidRegion`] when the range is outside the published prefix.
    pub fn region(&self, offset: usize, length: usize) -> Result<&[u8], ArenaError> {
        let end = offset
            .checked_add(length)
            .ok_or(ArenaError::InvalidRegion {
                offset,
                end: usize::MAX,
                published: self.published,
            })?;
        if end > self.published {
            return Err(ArenaError::InvalidRegion {
                offset,
                end,
                published: self.published,
            });
        }
        std::sync::atomic::fence(Ordering::Acquire);
        // SAFETY: the requested region is within the read-only mapping and `self` keeps it
        // alive for the returned slice lifetime.
        Ok(unsafe { std::slice::from_raw_parts(self.mapping.base.as_ptr().add(offset), length) })
    }

    #[cfg(unix)]
    /// Maps a descriptor received over `SCM_RIGHTS` as a read-only arena.
    ///
    /// # Errors
    ///
    /// Returns [`ArenaError`] when metadata is invalid or the mapping operation fails.
    pub fn from_descriptor(
        descriptor: UnixArenaDescriptor,
        capacity: u64,
        published: u64,
        generation: u64,
    ) -> Result<Self, ArenaError> {
        let capacity = validate_import(capacity, published, generation)?;
        let published =
            usize::try_from(published).map_err(|_| ArenaError::AddressSpaceExceeded {
                capacity: published,
            })?;
        let mapping = Mapping::open_read_only(descriptor, capacity)?;
        Ok(Self {
            mapping,
            published,
            generation,
        })
    }

    #[cfg(windows)]
    /// Opens a named file mapping as a read-only arena.
    ///
    /// # Errors
    ///
    /// Returns [`ArenaError`] when metadata is invalid or the mapping operation fails.
    pub fn open(
        name: &WindowsArenaName,
        capacity: u64,
        published: u64,
        generation: u64,
    ) -> Result<Self, ArenaError> {
        let capacity = validate_import(capacity, published, generation)?;
        let published =
            usize::try_from(published).map_err(|_| ArenaError::AddressSpaceExceeded {
                capacity: published,
            })?;
        let mapping = Mapping::open_read_only(name, capacity)?;
        Ok(Self {
            mapping,
            published,
            generation,
        })
    }
}

fn validate_parameters(capacity: u64, generation: u64) -> Result<usize, ArenaError> {
    if capacity == 0 {
        return Err(ArenaError::EmptyCapacity);
    }
    if generation == 0 {
        return Err(ArenaError::InvalidGeneration);
    }
    if capacity > MAX_TRANSFER_ARENA_BYTES {
        return Err(ArenaError::CapacityLimitExceeded {
            capacity,
            limit: MAX_TRANSFER_ARENA_BYTES,
        });
    }
    usize::try_from(capacity).map_err(|_| ArenaError::AddressSpaceExceeded { capacity })
}

fn validate_import(capacity: u64, published: u64, generation: u64) -> Result<usize, ArenaError> {
    let capacity_usize = validate_parameters(capacity, generation)?;
    if published > capacity {
        let published_end = usize::try_from(published).unwrap_or(usize::MAX);
        return Err(ArenaError::InvalidRegion {
            offset: 0,
            end: published_end,
            published: capacity_usize,
        });
    }
    Ok(capacity_usize)
}

fn align_up(value: usize) -> Option<usize> {
    value
        .checked_add(ARENA_ALIGNMENT - 1)
        .map(|value| value & !(ARENA_ALIGNMENT - 1))
}

#[cfg(unix)]
mod platform {
    use super::{ArenaError, Mapping};
    use crate::TransferId;
    use std::ffi::CString;
    use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ARENA_NAME: AtomicU64 = AtomicU64::new(1);

    pub struct UnixArenaDescriptor(OwnedFd);

    impl UnixArenaDescriptor {
        #[must_use]
        pub fn from_owned(descriptor: OwnedFd) -> Self {
            Self(descriptor)
        }

        #[must_use]
        pub fn into_owned(self) -> OwnedFd {
            self.0
        }
    }

    impl AsFd for UnixArenaDescriptor {
        fn as_fd(&self) -> BorrowedFd<'_> {
            self.0.as_fd()
        }
    }

    impl Mapping {
        pub(super) fn create(
            capacity: usize,
            transfer_id: TransferId,
            _authentication_token: [u8; crate::AUTHENTICATION_TOKEN_BYTES],
            generation: u64,
        ) -> Result<Self, ArenaError> {
            let sequence = NEXT_ARENA_NAME.fetch_add(1, Ordering::Relaxed);
            let name = format!(
                "/revycraft-{}-{generation}-{sequence}",
                transfer_id
                    .as_bytes()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            );
            let name = CString::new(name).expect("arena name has no interior NUL");
            // SAFETY: `name` is NUL-terminated and flags/mode are valid for `shm_open`.
            let raw_fd = unsafe {
                libc::shm_open(
                    name.as_ptr(),
                    libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                    0o600,
                )
            };
            if raw_fd < 0 {
                return Err(ArenaError::Mapping(std::io::Error::last_os_error()));
            }
            // SAFETY: `shm_open` returned a new owned file descriptor.
            let descriptor = unsafe { OwnedFd::from_raw_fd(raw_fd) };
            // The descriptor remains authoritative after unlink, and is transferred via
            // SCM_RIGHTS; no global shared-memory name remains usable by another process.
            // SAFETY: `name` remains a valid NUL-terminated string for this call.
            let unlink_result = unsafe { libc::shm_unlink(name.as_ptr()) };
            if unlink_result != 0 {
                return Err(ArenaError::Mapping(std::io::Error::last_os_error()));
            }
            let length =
                libc::off_t::try_from(capacity).map_err(|_| ArenaError::AddressSpaceExceeded {
                    capacity: capacity as u64,
                })?;
            // SAFETY: descriptor is valid and the requested length was range checked.
            if unsafe { libc::ftruncate(descriptor.as_raw_fd(), length) } != 0 {
                return Err(ArenaError::Mapping(std::io::Error::last_os_error()));
            }
            let base = map(
                descriptor.as_raw_fd(),
                capacity,
                libc::PROT_READ | libc::PROT_WRITE,
            )?;
            Ok(Self {
                base,
                capacity,
                resource: descriptor,
            })
        }

        pub(super) fn duplicate_descriptor(&self) -> Result<UnixArenaDescriptor, ArenaError> {
            // SAFETY: the source descriptor is valid for the lifetime of `self`; F_DUPFD_CLOEXEC
            // creates an independently owned descriptor suitable for SCM_RIGHTS transfer.
            let raw_fd =
                unsafe { libc::fcntl(self.resource.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
            if raw_fd < 0 {
                return Err(ArenaError::Mapping(std::io::Error::last_os_error()));
            }
            // SAFETY: fcntl returned a new owned descriptor.
            Ok(UnixArenaDescriptor(unsafe { OwnedFd::from_raw_fd(raw_fd) }))
        }

        pub(super) fn open_read_only(
            descriptor: UnixArenaDescriptor,
            capacity: usize,
        ) -> Result<Self, ArenaError> {
            let base = map(descriptor.0.as_raw_fd(), capacity, libc::PROT_READ)?;
            Ok(Self {
                base,
                capacity,
                resource: descriptor.0,
            })
        }
    }

    fn map(fd: i32, capacity: usize, protection: i32) -> Result<NonNull<u8>, ArenaError> {
        // SAFETY: the descriptor has been sized to `capacity`; arguments request a shared
        // mapping and no fixed address. The returned mapping is checked before use.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                capacity,
                protection,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(ArenaError::Mapping(std::io::Error::last_os_error()));
        }
        NonNull::new(base.cast()).ok_or_else(|| {
            ArenaError::Mapping(std::io::Error::other("mmap returned a null address"))
        })
    }

    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: base/capacity describe the live mapping owned by this value.
            unsafe {
                libc::munmap(self.base.as_ptr().cast(), self.capacity);
            }
        }
    }
}

#[cfg(unix)]
pub use platform::UnixArenaDescriptor;

#[cfg(windows)]
mod platform {
    use super::{ArenaError, Mapping};
    use crate::TransferId;
    use std::fmt::Write as _;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::ptr::NonNull;
    use std::sync::Arc;
    use windows_sys::Win32::Foundation::{
        ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_READ, MapViewOfFile, OpenFileMappingW, PAGE_READWRITE,
        UnmapViewOfFile,
    };

    #[derive(Clone)]
    pub struct WindowsArenaName(Arc<str>);

    impl WindowsArenaName {
        #[must_use]
        pub fn as_str(&self) -> &str {
            &self.0
        }

        #[must_use]
        /// Reconstructs a mapping capability received over the authenticated control channel.
        ///
        /// # Errors
        ///
        /// Returns [`ArenaError::InvalidMappingLocator`] if the locator is not the exact bounded
        /// form emitted by [`SharedTransferArena`](super::SharedTransferArena).
        pub fn parse(name: String) -> Result<Self, ArenaError> {
            let Some(suffix) = name.strip_prefix("Local\\RevyCraft-") else {
                return Err(ArenaError::InvalidMappingLocator);
            };
            let Some((capability, generation)) = suffix.split_once('-') else {
                return Err(ArenaError::InvalidMappingLocator);
            };
            if capability.len()
                != (crate::TRANSFER_ID_BYTES + crate::AUTHENTICATION_TOKEN_BYTES) * 2
                || !capability.bytes().all(|byte| byte.is_ascii_hexdigit())
                || generation
                    .parse::<u64>()
                    .ok()
                    .filter(|value| *value > 0)
                    .is_none()
            {
                return Err(ArenaError::InvalidMappingLocator);
            }
            Ok(Self(name.into()))
        }
    }

    pub(super) struct WindowsResource {
        _handle: OwnedHandle,
        name: WindowsArenaName,
    }

    impl Mapping {
        pub(super) fn create(
            capacity: usize,
            transfer_id: TransferId,
            authentication_token: [u8; crate::AUTHENTICATION_TOKEN_BYTES],
            generation: u64,
        ) -> Result<Self, ArenaError> {
            let name = arena_name(transfer_id, authentication_token, generation);
            let wide_name = wide_name(&name);
            let size = capacity as u64;
            let size_low = u32::try_from(size & u64::from(u32::MAX))
                .expect("masked mapping size always fits in u32");
            // SAFETY: arguments describe a pagefile-backed mapping; the UTF-16 name is
            // NUL-terminated and remains alive for the call.
            let raw_handle = unsafe {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    std::ptr::null(),
                    PAGE_READWRITE,
                    (size >> 32) as u32,
                    size_low,
                    wide_name.as_ptr(),
                )
            };
            if raw_handle.is_null() {
                return Err(ArenaError::Mapping(std::io::Error::last_os_error()));
            }
            // SAFETY: CreateFileMappingW returned a newly owned handle.
            let handle = unsafe { OwnedHandle::from_raw_handle(raw_handle.cast()) };
            // A capability collision must not attach this transfer to an arena owned by another
            // transfer. CreateFileMappingW reports that case by returning the existing object and
            // setting the thread error code rather than by returning null.
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                return Err(ArenaError::Mapping(std::io::Error::from_raw_os_error(
                    ERROR_ALREADY_EXISTS as i32,
                )));
            }
            let base = map(
                handle.as_raw_handle().cast(),
                capacity,
                windows_sys::Win32::System::Memory::FILE_MAP_ALL_ACCESS,
            )?;
            Ok(Self {
                base,
                capacity,
                resource: WindowsResource {
                    _handle: handle,
                    name,
                },
            })
        }

        pub(super) fn mapping_name(&self) -> WindowsArenaName {
            self.resource.name.clone()
        }

        pub(super) fn open_read_only(
            name: &WindowsArenaName,
            capacity: usize,
        ) -> Result<Self, ArenaError> {
            let wide_name = wide_name(name);
            // SAFETY: the UTF-16 name is NUL-terminated and remains alive for the call.
            let raw_handle = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, wide_name.as_ptr()) };
            if raw_handle.is_null() {
                return Err(ArenaError::Mapping(std::io::Error::last_os_error()));
            }
            // SAFETY: OpenFileMappingW returned a newly owned handle.
            let handle = unsafe { OwnedHandle::from_raw_handle(raw_handle.cast()) };
            let base = map(handle.as_raw_handle().cast(), capacity, FILE_MAP_READ)?;
            Ok(Self {
                base,
                capacity,
                resource: WindowsResource {
                    _handle: handle,
                    name: name.clone(),
                },
            })
        }
    }

    fn arena_name(
        transfer_id: TransferId,
        authentication_token: [u8; crate::AUTHENTICATION_TOKEN_BYTES],
        generation: u64,
    ) -> WindowsArenaName {
        let mut name = String::from("Local\\RevyCraft-");
        for byte in transfer_id
            .as_bytes()
            .into_iter()
            .chain(authentication_token)
        {
            write!(name, "{byte:02x}").expect("writing to a String cannot fail");
        }
        write!(name, "-{generation}").expect("writing to a String cannot fail");
        WindowsArenaName(name.into())
    }

    fn wide_name(name: &WindowsArenaName) -> Vec<u16> {
        name.as_str()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    }

    fn map(
        handle: HANDLE,
        capacity: usize,
        access: windows_sys::Win32::System::Memory::FILE_MAP,
    ) -> Result<NonNull<u8>, ArenaError> {
        // SAFETY: handle refers to a mapping at least `capacity` bytes long. The returned view
        // is checked before it is exposed.
        let view = unsafe { MapViewOfFile(handle, access, 0, 0, capacity) };
        NonNull::new(view.Value.cast())
            .ok_or_else(|| ArenaError::Mapping(std::io::Error::last_os_error()))
    }

    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: base is the live view owned by this value and is unmapped exactly once.
            unsafe {
                UnmapViewOfFile(
                    windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
                        Value: self.base.as_ptr().cast(),
                    },
                );
            }
        }
    }
}

#[cfg(windows)]
pub use platform::WindowsArenaName;

struct Mapping {
    base: NonNull<u8>,
    capacity: usize,
    #[cfg(unix)]
    resource: std::os::fd::OwnedFd,
    #[cfg(windows)]
    resource: platform::WindowsResource,
}

// SAFETY: the mapping address is stable for `Mapping`'s lifetime. Mutable access is available
// only through atomically allocated, non-overlapping `ArenaReservation` ranges; published and
// imported regions expose shared read-only slices.
unsafe impl Send for Mapping {}
// SAFETY: see the `Send` justification. Concurrent operations cannot obtain overlapping mutable
// ranges through the safe API.
unsafe impl Sync for Mapping {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_TRANSFER: AtomicU64 = AtomicU64::new(1);

    fn arena(capacity: u64) -> SharedTransferArena {
        let mut transfer_id = [3; crate::TRANSFER_ID_BYTES];
        transfer_id[crate::TRANSFER_ID_BYTES - size_of::<u64>()..].copy_from_slice(
            &NEXT_TEST_TRANSFER
                .fetch_add(1, Ordering::Relaxed)
                .to_le_bytes(),
        );
        SharedTransferArena::create(
            capacity,
            7,
            TransferId::from_bytes(transfer_id),
            [9; crate::AUTHENTICATION_TOKEN_BYTES],
        )
        .unwrap()
    }

    #[test]
    fn reservations_are_disjoint_aligned_and_sealed_before_reading() {
        let arena = arena(128);
        let mut first = arena.reserve(3).unwrap();
        let mut second = arena.reserve(5).unwrap();
        assert_eq!(first.offset(), 0);
        assert_eq!(second.offset(), ARENA_ALIGNMENT);
        first.write_all(b"one").unwrap();
        second.write_all(b"three").unwrap();
        let first = first.seal().unwrap();
        let second = second.seal().unwrap();
        assert_eq!(first.as_slice(), b"one");
        assert_eq!(second.as_slice(), b"three");
        assert_eq!(arena.descriptor().used, 13);
    }

    #[test]
    fn policy_exhaustion_is_distinct_from_mapping_failure() {
        let arena = arena(16);
        let _reservation = arena.reserve(9).unwrap();
        assert!(matches!(
            arena.reserve(1),
            Err(ArenaError::CapacityExhausted {
                requested: 1,
                remaining: 7
            })
        ));
        assert!(matches!(
            SharedTransferArena::create(
                MAX_TRANSFER_ARENA_BYTES + 1,
                1,
                TransferId::from_bytes([1; crate::TRANSFER_ID_BYTES]),
                [2; crate::AUTHENTICATION_TOKEN_BYTES],
            ),
            Err(ArenaError::CapacityLimitExceeded { .. })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_mapping_capability_collision_is_rejected() {
        let transfer_id = TransferId::from_bytes([8; crate::TRANSFER_ID_BYTES]);
        let authentication_token = [7; crate::AUTHENTICATION_TOKEN_BYTES];
        let _owner =
            SharedTransferArena::create(128, 3, transfer_id, authentication_token).unwrap();
        assert!(matches!(
            SharedTransferArena::create(128, 3, transfer_id, authentication_token),
            Err(ArenaError::Mapping(error))
                if error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS as i32)
        ));
    }

    #[test]
    fn imported_read_only_mapping_observes_sealed_bytes() {
        let arena = arena(128);
        let mut reservation = arena.reserve(11).unwrap();
        reservation.write_all(b"shared-data").unwrap();
        let region = reservation.seal().unwrap();
        let descriptor = arena.descriptor();

        #[cfg(unix)]
        let reader = SharedTransferArenaReader::from_descriptor(
            arena.duplicate_descriptor().unwrap(),
            descriptor.capacity,
            descriptor.used,
            descriptor.generation,
        )
        .unwrap();
        #[cfg(windows)]
        let reader = SharedTransferArenaReader::open(
            &arena.mapping_name(),
            descriptor.capacity,
            descriptor.used,
            descriptor.generation,
        )
        .unwrap();

        assert_eq!(
            reader.region(region.offset(), region.length()).unwrap(),
            b"shared-data"
        );
        assert!(matches!(
            reader.region(usize::try_from(descriptor.used).unwrap(), 1),
            Err(ArenaError::InvalidRegion { .. })
        ));
    }

    #[test]
    fn imported_publication_advances_monotonically() {
        let arena = arena(128);
        let initial = arena.descriptor();
        #[cfg(unix)]
        let mut reader = SharedTransferArenaReader::from_descriptor(
            arena.duplicate_descriptor().unwrap(),
            initial.capacity,
            initial.used,
            initial.generation,
        )
        .unwrap();
        #[cfg(windows)]
        let mut reader = SharedTransferArenaReader::open(
            &arena.mapping_name(),
            initial.capacity,
            initial.used,
            initial.generation,
        )
        .unwrap();

        let mut reservation = arena.reserve(7).unwrap();
        reservation.write_all(b"publish").unwrap();
        let region = reservation.seal().unwrap();
        assert!(matches!(
            reader.region(region.offset(), region.length()),
            Err(ArenaError::InvalidRegion { .. })
        ));
        reader.advance_published(arena.descriptor().used).unwrap();
        assert_eq!(
            reader.region(region.offset(), region.length()).unwrap(),
            b"publish"
        );
        assert!(matches!(
            reader.advance_published(0),
            Err(ArenaError::PublicationRegression { .. })
        ));
    }
}
