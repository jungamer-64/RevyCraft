use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SocketTransferTarget {
    process_id: u32,
}

impl SocketTransferTarget {
    /// Creates a target-process capability for socket duplication.
    ///
    /// # Errors
    ///
    /// Returns [`SocketTransferError`] when `process_id` is not a valid process identifier.
    pub fn new(process_id: u32) -> Result<Self, SocketTransferError> {
        if process_id == 0 {
            return Err(SocketTransferError::InvalidProcessId);
        }
        Ok(Self { process_id })
    }

    #[must_use]
    pub const fn process_id(self) -> u32 {
        self.process_id
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SocketTransferError {
    #[error("socket transfer target process id must be non-zero")]
    InvalidProcessId,
    #[error("socket transfer descriptor has invalid length {actual}; expected {expected}")]
    InvalidDescriptorLength { actual: usize, expected: usize },
    #[error("socket transfer operation failed: {0}")]
    Io(#[from] io::Error),
}

#[cfg(unix)]
mod platform {
    use super::{SocketTransferError, SocketTransferTarget};
    use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};

    pub struct ExportedSocket(OwnedFd);

    impl ExportedSocket {
        /// Duplicates a socket descriptor into an owned capability suitable for `SCM_RIGHTS`.
        ///
        /// # Errors
        ///
        /// Returns [`SocketTransferError`] when the descriptor cannot be duplicated.
        pub fn duplicate(
            socket: &impl AsFd,
            _target: SocketTransferTarget,
        ) -> Result<Self, SocketTransferError> {
            let raw_fd =
                unsafe { libc::fcntl(socket.as_fd().as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
            if raw_fd < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            // SAFETY: `F_DUPFD_CLOEXEC` returned a new independently owned descriptor.
            Ok(Self(unsafe { OwnedFd::from_raw_fd(raw_fd) }))
        }

        #[must_use]
        pub fn into_owned(self) -> OwnedFd {
            self.0
        }

        #[must_use]
        pub fn from_owned(descriptor: OwnedFd) -> Self {
            Self(descriptor)
        }
    }

    impl AsFd for ExportedSocket {
        fn as_fd(&self) -> BorrowedFd<'_> {
            self.0.as_fd()
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::{SocketTransferError, SocketTransferTarget};
    use std::mem::{MaybeUninit, size_of};
    use std::os::windows::io::{AsRawSocket, FromRawSocket, OwnedSocket};
    use std::sync::OnceLock;
    use windows_sys::Win32::Networking::WinSock::{
        FROM_PROTOCOL_INFO, INVALID_SOCKET, SOCKET_ERROR, WSA_FLAG_OVERLAPPED, WSADATA,
        WSADuplicateSocketW, WSAGetLastError, WSAPROTOCOL_INFOW, WSASocketW, WSAStartup,
    };

    pub struct ExportedSocket {
        protocol_info: Box<WSAPROTOCOL_INFOW>,
    }

    impl ExportedSocket {
        /// Duplicates a socket for the exact target process represented by `target`.
        ///
        /// # Errors
        ///
        /// Returns [`SocketTransferError`] when Winsock initialization or duplication fails.
        pub fn duplicate(
            socket: &impl AsRawSocket,
            target: SocketTransferTarget,
        ) -> Result<Self, SocketTransferError> {
            ensure_winsock()?;
            // SAFETY: every bit pattern is valid as initial storage for the C output structure;
            // Winsock initializes it before this function publishes the value.
            let mut protocol_info: Box<WSAPROTOCOL_INFOW> =
                Box::new(unsafe { MaybeUninit::zeroed().assume_init() });
            let source_socket = usize::try_from(socket.as_raw_socket()).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "source socket does not fit the Winsock SOCKET representation",
                )
            })?;
            // SAFETY: `source_socket` is borrowed from a live socket, `protocol_info` is valid
            // writable storage, and the non-zero target process id was validated at construction.
            let result = unsafe {
                WSADuplicateSocketW(source_socket, target.process_id(), protocol_info.as_mut())
            };
            if result == SOCKET_ERROR {
                return Err(last_winsock_error().into());
            }
            Ok(Self { protocol_info })
        }

        #[must_use]
        pub fn encoded_len() -> usize {
            size_of::<WSAPROTOCOL_INFOW>()
        }

        #[must_use]
        pub fn encode(&self) -> Vec<u8> {
            // SAFETY: `protocol_info` is fully initialized and the slice is bounded by its exact
            // C layout size for immediate copying into an owned vector.
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    self.protocol_info.as_ref() as *const WSAPROTOCOL_INFOW as *const u8,
                    Self::encoded_len(),
                )
            };
            bytes.to_vec()
        }

        /// Decodes protocol information received over the authenticated V1 control channel.
        ///
        /// # Errors
        ///
        /// Returns [`SocketTransferError`] unless the descriptor has the exact platform layout.
        pub fn decode(bytes: &[u8]) -> Result<Self, SocketTransferError> {
            if bytes.len() != Self::encoded_len() {
                return Err(SocketTransferError::InvalidDescriptorLength {
                    actual: bytes.len(),
                    expected: Self::encoded_len(),
                });
            }
            // SAFETY: every bit pattern is valid as storage for the plain C protocol-info record;
            // the exact-size copy below initializes every byte before use.
            let mut protocol_info: Box<WSAPROTOCOL_INFOW> =
                Box::new(unsafe { MaybeUninit::zeroed().assume_init() });
            // SAFETY: both regions are valid for exactly `bytes.len()` bytes, and the length was
            // checked to equal the destination C structure size.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    protocol_info.as_mut() as *mut WSAPROTOCOL_INFOW as *mut u8,
                    bytes.len(),
                );
            }
            Ok(Self { protocol_info })
        }

        /// Imports the duplicated socket as a new owned child-process authority.
        ///
        /// # Errors
        ///
        /// Returns [`SocketTransferError`] when Winsock rejects the target-bound protocol info.
        pub fn import(mut self) -> Result<OwnedSocket, SocketTransferError> {
            ensure_winsock()?;
            // SAFETY: the protocol-info record was produced by Winsock for the target process (or
            // reconstructed byte-for-byte from that authenticated record) and remains writable for
            // the duration of `WSASocketW`.
            let socket = unsafe {
                WSASocketW(
                    FROM_PROTOCOL_INFO,
                    FROM_PROTOCOL_INFO,
                    FROM_PROTOCOL_INFO,
                    self.protocol_info.as_mut(),
                    0,
                    WSA_FLAG_OVERLAPPED,
                )
            };
            if socket == INVALID_SOCKET {
                return Err(last_winsock_error().into());
            }
            let raw_socket = u64::try_from(socket).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "imported Winsock SOCKET does not fit RawSocket",
                )
            })?;
            // SAFETY: `WSASocketW` returned a new valid socket authority, transferred exactly once
            // into `OwnedSocket`.
            Ok(unsafe { OwnedSocket::from_raw_socket(raw_socket) })
        }
    }

    fn ensure_winsock() -> Result<(), std::io::Error> {
        static STARTUP: OnceLock<Result<(), i32>> = OnceLock::new();
        match *STARTUP.get_or_init(|| {
            let mut data = MaybeUninit::<WSADATA>::zeroed();
            // SAFETY: `data` is valid writable storage for the duration of Winsock initialization.
            let result = unsafe { WSAStartup(0x0202, data.as_mut_ptr()) };
            if result == 0 { Ok(()) } else { Err(result) }
        }) {
            Ok(()) => Ok(()),
            Err(code) => Err(std::io::Error::from_raw_os_error(code)),
        }
    }

    fn last_winsock_error() -> std::io::Error {
        // SAFETY: `WSAGetLastError` has no preconditions and reads thread-local Winsock state.
        std::io::Error::from_raw_os_error(unsafe { WSAGetLastError() })
    }
}

pub use platform::ExportedSocket;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_target_process() {
        assert!(matches!(
            SocketTransferTarget::new(0),
            Err(SocketTransferError::InvalidProcessId)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn current_process_can_import_a_target_bound_listener_duplicate() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let target = SocketTransferTarget::new(std::process::id()).unwrap();
        let encoded = ExportedSocket::duplicate(&listener, target)
            .unwrap()
            .encode();
        let imported = ExportedSocket::decode(&encoded).unwrap().import().unwrap();
        let imported = std::net::TcpListener::from(imported);
        let client = std::net::TcpStream::connect(address).unwrap();
        let (accepted, _) = imported.accept().unwrap();
        assert_eq!(client.peer_addr().unwrap(), accepted.local_addr().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn duplicated_listener_descriptor_is_independently_owned() {
        use std::os::fd::OwnedFd;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let target = SocketTransferTarget::new(std::process::id()).unwrap();
        let descriptor: OwnedFd = ExportedSocket::duplicate(&listener, target)
            .unwrap()
            .into_owned();
        let imported = std::net::TcpListener::from(descriptor);
        let client = std::net::TcpStream::connect(address).unwrap();
        let (accepted, _) = imported.accept().unwrap();
        assert_eq!(client.peer_addr().unwrap(), accepted.local_addr().unwrap());
    }
}
