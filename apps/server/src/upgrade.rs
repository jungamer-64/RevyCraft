use crate::process_surfaces::{PausedProcessSurfaces, ProcessSurfaceCommand};
use rand::random;
use server_runtime::RuntimeError;
use server_runtime::runtime::{
    AdminSubject, AdminUpgradeRuntimeView, RuntimeUpgradeCommitHold, RuntimeUpgradeImport,
    RuntimeUpgradePayload, RuntimeUpgradeSessionHandle, ServerSupervisor,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot, watch};

#[cfg(any(unix, windows))]
use {
    serde::{Deserialize, Serialize},
    tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
};

#[cfg(unix)]
use {
    std::ffi::OsStr,
    std::mem::{size_of, zeroed},
    std::os::fd::{AsRawFd, FromRawFd, RawFd},
    std::os::unix::net::UnixStream as StdUnixStream,
    std::ptr,
    tokio::net::UnixStream,
};

#[cfg(windows)]
use {
    std::mem::{size_of, zeroed},
    std::os::windows::io::{AsRawHandle, AsRawSocket, FromRawSocket, RawSocket},
    std::sync::OnceLock,
    tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions},
    windows_sys::Win32::Networking::WinSock::{
        FROM_PROTOCOL_INFO, INVALID_SOCKET, SOCKET, WSADATA, WSADuplicateSocketW, WSAGetLastError,
        WSAPROTOCOL_INFOW, WSAStartup, WSASocketW, WSA_FLAG_OVERLAPPED,
    },
};

#[cfg(any(unix, windows))]
const UPGRADE_CHILD_ARG: &str = "--upgrade-child";
#[cfg(any(unix, windows))]
const UPGRADE_AUTH_TOKEN_ENV: &str = "REVY_UPGRADE_AUTH_TOKEN";
#[cfg(unix)]
const UPGRADE_CONTROL_FD_ENV: &str = "REVY_UPGRADE_CONTROL_FD";
#[cfg(unix)]
const MAX_FDS_PER_RIGHTS_MESSAGE: usize = 200;
#[cfg(windows)]
const UPGRADE_PIPE_NAME_ENV: &str = "REVY_UPGRADE_PIPE_NAME";

#[cfg(debug_assertions)]
const UPGRADE_TEST_FAULT_ENV: &str = "REVY_UPGRADE_TEST_FAULT";
#[cfg(debug_assertions)]
const UPGRADE_TEST_READY_TIMEOUT_MS_ENV: &str = "REVY_UPGRADE_TEST_READY_TIMEOUT_MS";

pub(crate) struct UpgradeCoordinator {
    server: Arc<ServerSupervisor>,
    surface_control_tx: Mutex<Option<mpsc::Sender<ProcessSurfaceCommand>>>,
    process_shutdown_tx: Mutex<Option<watch::Sender<bool>>>,
    upgrade_lock: Mutex<()>,
    committed: Mutex<Option<CommittedUpgrade>>,
}

struct CommittedUpgrade {
    _hold: RuntimeUpgradeCommitHold,
}

pub(crate) struct PendingUpgradeChild {
    pub(crate) server: Arc<ServerSupervisor>,
    pub(crate) grpc_listener_override: Option<std::net::TcpListener>,
    commit_gate: UpgradeChildCommitGate,
}

enum UpgradeChildCommitGate {
    #[cfg(unix)]
    Unix(UnixChildUpgradeTransport),
    #[cfg(windows)]
    Windows(WindowsChildUpgradeTransport),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum UpgradeControlMessage {
    ChildHello { auth_token: String },
    Bootstrap {
        payload: RuntimeUpgradePayload,
        has_admin_listener: bool,
        transferred_handle_count: usize,
    },
    Ready,
    Commit,
    Error { message: String },
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UpgradeTestFault {
    SessionTransferFailure,
    ChildImportFailure,
    ChildReadyTimeout,
    GrpcTakeoverFailure,
}

#[cfg(debug_assertions)]
impl UpgradeTestFault {
    fn current() -> Option<Self> {
        match std::env::var(UPGRADE_TEST_FAULT_ENV).ok()?.as_str() {
            "session-transfer-failure" => Some(Self::SessionTransferFailure),
            "child-import-failure" => Some(Self::ChildImportFailure),
            "child-ready-timeout" => Some(Self::ChildReadyTimeout),
            "grpc-takeover-failure" => Some(Self::GrpcTakeoverFailure),
            _ => None,
        }
    }
}

impl UpgradeCoordinator {
    pub(crate) fn new(server: Arc<ServerSupervisor>) -> Self {
        Self {
            server,
            surface_control_tx: Mutex::new(None),
            process_shutdown_tx: Mutex::new(None),
            upgrade_lock: Mutex::new(()),
            committed: Mutex::new(None),
        }
    }

    pub(crate) async fn set_surface_control_sender(
        &self,
        surface_control_tx: mpsc::Sender<ProcessSurfaceCommand>,
    ) {
        *self.surface_control_tx.lock().await = Some(surface_control_tx);
    }

    pub(crate) async fn set_process_shutdown_sender(
        &self,
        shutdown_tx: watch::Sender<bool>,
    ) {
        *self.process_shutdown_tx.lock().await = Some(shutdown_tx);
    }

    #[cfg(not(any(unix, windows)))]
    pub(crate) async fn upgrade(
        &self,
        _subject: AdminSubject,
        _executable_path: String,
    ) -> Result<AdminUpgradeRuntimeView, RuntimeError> {
        Err(RuntimeError::Unsupported(
            "runtime binary self-upgrade is not supported on this platform".to_string(),
        ))
    }

    #[cfg(any(unix, windows))]
    pub(crate) async fn upgrade(
        &self,
        subject: AdminSubject,
        executable_path: String,
    ) -> Result<AdminUpgradeRuntimeView, RuntimeError> {
        let _upgrade_guard = self.upgrade_lock.lock().await;
        validate_upgrade_executable(&executable_path)?;

        let paused_surfaces = self.pause_process_surfaces(subject.is_local_console()).await?;
        let runtime_upgrade = self.server.begin_runtime_upgrade().await;
        let guard = match runtime_upgrade {
            Ok(guard) => guard,
            Err(error) => {
                self.resume_process_surfaces(paused_surfaces).await?;
                return Err(error);
            }
        };

        #[cfg(debug_assertions)]
        if UpgradeTestFault::current() == Some(UpgradeTestFault::SessionTransferFailure) {
            guard.rollback().await?;
            self.resume_process_surfaces(paused_surfaces).await?;
            return Err(RuntimeError::Config(
                "injected upgrade session transfer failure".to_string(),
            ));
        }

        let result = self
            .run_upgrade_transaction(&executable_path, &guard, &paused_surfaces)
            .await;
        match result {
            Ok(mut transport) => {
                transport.write_message(&UpgradeControlMessage::Commit).await?;
                if let Some(shutdown_tx) = self.process_shutdown_tx.lock().await.as_ref() {
                    let _ = shutdown_tx.send(true);
                }
                *self.committed.lock().await = Some(CommittedUpgrade {
                    _hold: guard.commit(),
                });
                let _ = self.server.request_shutdown();
                Ok(AdminUpgradeRuntimeView { executable_path })
            }
            Err(error) => {
                guard.rollback().await?;
                self.resume_process_surfaces(paused_surfaces).await?;
                Err(error)
            }
        }
    }

    #[cfg(any(unix, windows))]
    async fn run_upgrade_transaction(
        &self,
        executable_path: &str,
        guard: &server_runtime::runtime::RuntimeUpgradeGuard,
        paused_surfaces: &PausedProcessSurfaces,
    ) -> Result<ParentUpgradeTransport, RuntimeError> {
        let game_listener = guard.clone_game_listener()?;
        let sessions = guard.clone_sessions_for_child()?;
        let payload = guard.payload().clone();
        let auth_token = upgrade_auth_token();
        let (mut child, mut transport) =
            spawn_parent_upgrade_transport(executable_path, &auth_token).await?;
        let transaction = async {
            transport.await_child_hello(&auth_token).await?;
            transport
                .write_message(&UpgradeControlMessage::Bootstrap {
                    payload,
                    has_admin_listener: paused_surfaces.admin_listener_for_child.is_some(),
                    transferred_handle_count: 1
                        + usize::from(paused_surfaces.admin_listener_for_child.is_some())
                        + sessions.len(),
                })
                .await?;
            transport.send_handles(
                &game_listener,
                paused_surfaces.admin_listener_for_child.as_ref(),
                &sessions,
            )?;
            let child_ready = tokio::time::timeout(upgrade_ready_timeout(), transport.read_message())
                .await;
            match child_ready {
                Ok(Ok(UpgradeControlMessage::Ready)) => Ok(transport),
                Ok(Ok(UpgradeControlMessage::Error { message })) => {
                    Err(RuntimeError::Config(message))
                }
                Ok(Ok(UpgradeControlMessage::ChildHello { .. }
                    | UpgradeControlMessage::Bootstrap { .. }
                    | UpgradeControlMessage::Commit)) => Err(RuntimeError::Config(
                    "upgrade child returned an unexpected control message".to_string(),
                )),
                Ok(Err(error)) => Err(error),
                Err(_) => Err(RuntimeError::Config(
                    "upgrade child did not report readiness before timeout".to_string(),
                )),
            }
        }
        .await;
        if transaction.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        transaction
    }

    #[cfg(any(unix, windows))]
    async fn pause_process_surfaces(
        &self,
        skip_console: bool,
    ) -> Result<PausedProcessSurfaces, RuntimeError> {
        let surface_control_tx = self
            .surface_control_tx
            .lock()
            .await
            .clone()
            .ok_or_else(|| {
                RuntimeError::Config(
                    "runtime upgrade is unavailable without process-surface orchestration"
                        .to_string(),
                )
            })?;
        let (ack_tx, ack_rx) = oneshot::channel();
        surface_control_tx
            .send(ProcessSurfaceCommand::PauseForUpgrade {
                skip_console,
                ack_tx,
            })
            .await
            .map_err(|_| {
                RuntimeError::Config("failed to pause process surfaces for upgrade".to_string())
            })?;
        ack_rx.await.map_err(|_| {
            RuntimeError::Config("process surface pause acknowledgement was dropped".to_string())
        })?
    }

    #[cfg(any(unix, windows))]
    async fn resume_process_surfaces(
        &self,
        paused: PausedProcessSurfaces,
    ) -> Result<(), RuntimeError> {
        let surface_control_tx = match self.surface_control_tx.lock().await.clone() {
            Some(tx) => tx,
            None => return Ok(()),
        };
        let (ack_tx, ack_rx) = oneshot::channel();
        surface_control_tx
            .send(ProcessSurfaceCommand::ResumeAfterUpgradeRollback { paused, ack_tx })
            .await
            .map_err(|_| {
                RuntimeError::Config(
                    "failed to resume process surfaces after upgrade rollback".to_string(),
                )
            })?;
        ack_rx.await.map_err(|_| {
            RuntimeError::Config("process surface resume acknowledgement was dropped".to_string())
        })?
    }
}

impl PendingUpgradeChild {
    pub(crate) fn server(&self) -> Arc<ServerSupervisor> {
        Arc::clone(&self.server)
    }

    pub(crate) fn take_grpc_listener_override(&mut self) -> Option<std::net::TcpListener> {
        self.grpc_listener_override.take()
    }

    pub(crate) async fn report_ready_and_wait_for_commit(&mut self) -> Result<(), RuntimeError> {
        self.commit_gate.report_ready_and_wait_for_commit().await
    }

    pub(crate) async fn report_error(&mut self, message: String) -> Result<(), RuntimeError> {
        self.commit_gate.report_error(message).await
    }
}

impl UpgradeChildCommitGate {
    async fn report_ready_and_wait_for_commit(&mut self) -> Result<(), RuntimeError> {
        match self {
            #[cfg(unix)]
            Self::Unix(transport) => {
                transport.write_message(&UpgradeControlMessage::Ready).await?;
                match transport.read_message().await? {
                    UpgradeControlMessage::Commit => Ok(()),
                    UpgradeControlMessage::ChildHello { .. }
                    | UpgradeControlMessage::Bootstrap { .. }
                    | UpgradeControlMessage::Ready
                    | UpgradeControlMessage::Error { .. } => Err(RuntimeError::Config(
                        "upgrade child expected commit after readiness handshake".to_string(),
                    )),
                }
            }
            #[cfg(windows)]
            Self::Windows(transport) => {
                transport.write_message(&UpgradeControlMessage::Ready).await?;
                match transport.read_message().await? {
                    UpgradeControlMessage::Commit => Ok(()),
                    UpgradeControlMessage::ChildHello { .. }
                    | UpgradeControlMessage::Bootstrap { .. }
                    | UpgradeControlMessage::Ready
                    | UpgradeControlMessage::Error { .. } => Err(RuntimeError::Config(
                        "upgrade child expected commit after readiness handshake".to_string(),
                    )),
                }
            }
        }
    }

    async fn report_error(&mut self, message: String) -> Result<(), RuntimeError> {
        match self {
            #[cfg(unix)]
            Self::Unix(transport) => {
                transport
                    .write_message(&UpgradeControlMessage::Error { message })
                    .await
            }
            #[cfg(windows)]
            Self::Windows(transport) => {
                transport
                    .write_message(&UpgradeControlMessage::Error { message })
                    .await
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) async fn try_boot_upgrade_child(
    _args: &[String],
) -> Result<Option<PendingUpgradeChild>, RuntimeError> {
    Ok(None)
}

#[cfg(any(unix, windows))]
pub(crate) async fn try_boot_upgrade_child(
    args: &[String],
) -> Result<Option<PendingUpgradeChild>, RuntimeError> {
    if !args.iter().any(|arg| arg == UPGRADE_CHILD_ARG) {
        return Ok(None);
    }

    let mut transport = open_child_upgrade_transport().await?;
    let bootstrap = transport.read_message().await?;
    let (payload, has_admin_listener, transferred_handle_count) = match bootstrap {
        UpgradeControlMessage::Bootstrap {
            payload,
            has_admin_listener,
            transferred_handle_count,
        } => (payload, has_admin_listener, transferred_handle_count),
        UpgradeControlMessage::ChildHello { .. }
        | UpgradeControlMessage::Ready
        | UpgradeControlMessage::Commit
        | UpgradeControlMessage::Error { .. } => {
            transport
                .write_message(&UpgradeControlMessage::Error {
                    message: "upgrade child expected bootstrap payload on control socket"
                        .to_string(),
                })
                .await?;
            return Err(RuntimeError::Config(
                "upgrade child expected bootstrap payload on control socket".to_string(),
            ));
        }
    };

    #[cfg(debug_assertions)]
    if UpgradeTestFault::current() == Some(UpgradeTestFault::ChildImportFailure) {
        let message = "injected child import failure".to_string();
        transport
            .write_message(&UpgradeControlMessage::Error {
                message: message.clone(),
            })
            .await?;
        return Err(RuntimeError::Config(message));
    }

    let received = transport.receive_handles(transferred_handle_count, has_admin_listener)?;
    let sessions = payload
        .sessions
        .iter()
        .cloned()
        .zip(received.session_streams)
        .map(|(state, stream)| RuntimeUpgradeSessionHandle { state, stream })
        .collect::<Vec<_>>();
    if sessions.len() != payload.sessions.len() {
        let message =
            "upgrade child session handle count did not match serialized session payload count"
                .to_string();
        transport
            .write_message(&UpgradeControlMessage::Error {
                message: message.clone(),
            })
            .await
            .ok();
        return Err(RuntimeError::Config(message));
    }

    let server = match ServerSupervisor::boot_from_runtime_upgrade(
        resolve_server_config_source(),
        RuntimeUpgradeImport {
            payload,
            game_listener: received.game_listener,
            sessions,
        },
    )
    .await
    {
        Ok(server) => server,
        Err(error) => {
            transport
                .write_message(&UpgradeControlMessage::Error {
                    message: error.to_string(),
                })
                .await
                .ok();
            return Err(error);
        }
    };

    Ok(Some(PendingUpgradeChild {
        server: Arc::new(server),
        grpc_listener_override: received.admin_listener,
        commit_gate: transport.into_commit_gate(),
    }))
}

#[cfg(any(unix, windows))]
pub(crate) fn child_upgrade_fault_before_ready() -> Option<RuntimeError> {
    #[cfg(debug_assertions)]
    {
        if UpgradeTestFault::current() == Some(UpgradeTestFault::GrpcTakeoverFailure) {
            return Some(RuntimeError::Config(
                "injected child gRPC takeover failure".to_string(),
            ));
        }
    }
    None
}

#[cfg(any(unix, windows))]
pub(crate) async fn child_upgrade_ready_delay_if_needed() {
    #[cfg(debug_assertions)]
    if UpgradeTestFault::current() == Some(UpgradeTestFault::ChildReadyTimeout) {
        tokio::time::sleep(upgrade_ready_timeout() + Duration::from_millis(250)).await;
    }
}

#[cfg(any(unix, windows))]
fn resolve_server_config_source() -> server_runtime::config::ServerConfigSource {
    let config_path = std::env::var_os(crate::SERVER_CONFIG_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(crate::DEFAULT_SERVER_CONFIG_PATH));
    server_runtime::config::ServerConfigSource::Toml(config_path)
}

#[cfg(any(unix, windows))]
fn validate_upgrade_executable(path: &str) -> Result<(), RuntimeError> {
    let executable = Path::new(path);
    let metadata = executable.metadata().map_err(|error| {
        RuntimeError::Config(format!(
            "upgrade executable `{}` could not be accessed: {error}",
            executable.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(RuntimeError::Config(format!(
            "upgrade executable `{}` is not a regular file",
            executable.display()
        )));
    }
    Ok(())
}

#[cfg(any(unix, windows))]
fn upgrade_auth_token() -> String {
    format!("{:032x}", random::<u128>())
}

#[cfg(any(unix, windows))]
fn upgrade_ready_timeout() -> Duration {
    #[cfg(debug_assertions)]
    if let Ok(value) = std::env::var(UPGRADE_TEST_READY_TIMEOUT_MS_ENV)
        && let Ok(parsed) = value.parse::<u64>()
    {
        return Duration::from_millis(parsed);
    }
    Duration::from_secs(30)
}

struct ReceivedUpgradeHandles {
    game_listener: std::net::TcpListener,
    admin_listener: Option<std::net::TcpListener>,
    session_streams: Vec<std::net::TcpStream>,
}

enum ParentUpgradeTransport {
    #[cfg(unix)]
    Unix(UnixParentUpgradeTransport),
    #[cfg(windows)]
    Windows(WindowsParentUpgradeTransport),
}

enum ChildUpgradeTransport {
    #[cfg(unix)]
    Unix(UnixChildUpgradeTransport),
    #[cfg(windows)]
    Windows(WindowsChildUpgradeTransport),
}

impl ParentUpgradeTransport {
    async fn await_child_hello(&mut self, expected_token: &str) -> Result<(), RuntimeError> {
        match self.read_message().await? {
            UpgradeControlMessage::ChildHello { auth_token } if auth_token == expected_token => {
                Ok(())
            }
            UpgradeControlMessage::ChildHello { .. } => Err(RuntimeError::Config(
                "upgrade child presented an invalid control-channel auth token".to_string(),
            )),
            UpgradeControlMessage::Error { message } => Err(RuntimeError::Config(message)),
            UpgradeControlMessage::Bootstrap { .. }
            | UpgradeControlMessage::Ready
            | UpgradeControlMessage::Commit => Err(RuntimeError::Config(
                "upgrade child returned an unexpected hello message".to_string(),
            )),
        }
    }

    async fn write_message(&mut self, message: &UpgradeControlMessage) -> Result<(), RuntimeError> {
        match self {
            #[cfg(unix)]
            Self::Unix(transport) => transport.write_message(message).await,
            #[cfg(windows)]
            Self::Windows(transport) => transport.write_message(message).await,
        }
    }

    async fn read_message(&mut self) -> Result<UpgradeControlMessage, RuntimeError> {
        match self {
            #[cfg(unix)]
            Self::Unix(transport) => transport.read_message().await,
            #[cfg(windows)]
            Self::Windows(transport) => transport.read_message().await,
        }
    }

    fn send_handles(
        &self,
        game_listener: &std::net::TcpListener,
        admin_listener: Option<&std::net::TcpListener>,
        sessions: &[RuntimeUpgradeSessionHandle],
    ) -> Result<(), RuntimeError> {
        match self {
            #[cfg(unix)]
            Self::Unix(transport) => transport.send_handles(game_listener, admin_listener, sessions),
            #[cfg(windows)]
            Self::Windows(transport) => {
                transport.send_handles(game_listener, admin_listener, sessions)
            }
        }
    }
}

impl ChildUpgradeTransport {
    async fn read_message(&mut self) -> Result<UpgradeControlMessage, RuntimeError> {
        match self {
            #[cfg(unix)]
            Self::Unix(transport) => transport.read_message().await,
            #[cfg(windows)]
            Self::Windows(transport) => transport.read_message().await,
        }
    }

    async fn write_message(&mut self, message: &UpgradeControlMessage) -> Result<(), RuntimeError> {
        match self {
            #[cfg(unix)]
            Self::Unix(transport) => transport.write_message(message).await,
            #[cfg(windows)]
            Self::Windows(transport) => transport.write_message(message).await,
        }
    }

    fn receive_handles(
        &self,
        expected: usize,
        has_admin_listener: bool,
    ) -> Result<ReceivedUpgradeHandles, RuntimeError> {
        match self {
            #[cfg(unix)]
            Self::Unix(transport) => transport.receive_handles(expected, has_admin_listener),
            #[cfg(windows)]
            Self::Windows(transport) => transport.receive_handles(expected, has_admin_listener),
        }
    }

    fn into_commit_gate(self) -> UpgradeChildCommitGate {
        match self {
            #[cfg(unix)]
            Self::Unix(transport) => UpgradeChildCommitGate::Unix(transport),
            #[cfg(windows)]
            Self::Windows(transport) => UpgradeChildCommitGate::Windows(transport),
        }
    }
}

#[cfg(any(unix, windows))]
async fn write_control_message<IO>(
    io: &mut IO,
    message: &UpgradeControlMessage,
) -> Result<(), RuntimeError>
where
    IO: AsyncWrite + Unpin,
{
    let bytes = serde_json::to_vec(message)
        .map_err(|error| RuntimeError::Config(format!("failed to serialize upgrade message: {error}")))?;
    io.write_u32(bytes.len() as u32).await?;
    io.write_all(&bytes).await?;
    io.flush().await?;
    Ok(())
}

#[cfg(any(unix, windows))]
async fn read_control_message<IO>(io: &mut IO) -> Result<UpgradeControlMessage, RuntimeError>
where
    IO: AsyncRead + Unpin,
{
    let len = io.read_u32().await?;
    let mut bytes = vec![0_u8; len as usize];
    io.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes)
        .map_err(|error| RuntimeError::Config(format!("failed to decode upgrade message: {error}")))
}

#[cfg(unix)]
struct UnixParentUpgradeTransport {
    stream: UnixStream,
}

#[cfg(unix)]
struct UnixChildUpgradeTransport {
    stream: UnixStream,
}

#[cfg(unix)]
impl UnixParentUpgradeTransport {
    async fn write_message(&mut self, message: &UpgradeControlMessage) -> Result<(), RuntimeError> {
        write_control_message(&mut self.stream, message).await
    }

    async fn read_message(&mut self) -> Result<UpgradeControlMessage, RuntimeError> {
        read_control_message(&mut self.stream).await
    }

    fn send_handles(
        &self,
        game_listener: &std::net::TcpListener,
        admin_listener: Option<&std::net::TcpListener>,
        sessions: &[RuntimeUpgradeSessionHandle],
    ) -> Result<(), RuntimeError> {
        let mut handles = vec![game_listener.as_raw_fd()];
        if let Some(listener) = admin_listener {
            handles.push(listener.as_raw_fd());
        }
        handles.extend(sessions.iter().map(|session| session.stream.as_raw_fd()));
        send_rights_chunked(self.stream.as_raw_fd(), &handles)
    }
}

#[cfg(unix)]
impl UnixChildUpgradeTransport {
    async fn write_message(&mut self, message: &UpgradeControlMessage) -> Result<(), RuntimeError> {
        write_control_message(&mut self.stream, message).await
    }

    async fn read_message(&mut self) -> Result<UpgradeControlMessage, RuntimeError> {
        read_control_message(&mut self.stream).await
    }

    fn receive_handles(
        &self,
        expected: usize,
        has_admin_listener: bool,
    ) -> Result<ReceivedUpgradeHandles, RuntimeError> {
        let handles = receive_rights_chunked(self.stream.as_raw_fd(), expected)?;
        let mut handles = handles.into_iter();
        let game_listener = unsafe {
            std::net::TcpListener::from_raw_fd(handles.next().ok_or_else(|| {
                RuntimeError::Config("upgrade child did not receive a game listener handle".to_string())
            })?)
        };
        let admin_listener = if has_admin_listener {
            Some(unsafe {
                std::net::TcpListener::from_raw_fd(handles.next().ok_or_else(|| {
                    RuntimeError::Config(
                        "upgrade child did not receive an admin gRPC listener handle".to_string(),
                    )
                })?)
            })
        } else {
            None
        };
        let session_streams = handles
            .map(|fd| unsafe { std::net::TcpStream::from_raw_fd(fd) })
            .collect::<Vec<_>>();
        Ok(ReceivedUpgradeHandles {
            game_listener,
            admin_listener,
            session_streams,
        })
    }
}

#[cfg(unix)]
async fn spawn_parent_upgrade_transport(
    executable_path: &str,
    auth_token: &str,
) -> Result<(std::process::Child, ParentUpgradeTransport), RuntimeError> {
    let (parent_control, child_control) = StdUnixStream::pair()?;
    clear_fd_cloexec(child_control.as_raw_fd())?;

    let mut command = std::process::Command::new(OsStr::new(executable_path));
    command.arg(UPGRADE_CHILD_ARG);
    command.env(UPGRADE_CONTROL_FD_ENV, child_control.as_raw_fd().to_string());
    command.env(UPGRADE_AUTH_TOKEN_ENV, auth_token);
    let child = command.spawn().map_err(|error| {
        RuntimeError::Config(format!(
            "failed to spawn upgrade child `{}`: {error}",
            executable_path
        ))
    })?;
    Ok((
        child,
        ParentUpgradeTransport::Unix(UnixParentUpgradeTransport {
            stream: into_tokio_unix_stream(parent_control)?,
        }),
    ))
}

#[cfg(unix)]
async fn open_child_upgrade_transport() -> Result<ChildUpgradeTransport, RuntimeError> {
    let control_fd = read_env_fd(UPGRADE_CONTROL_FD_ENV)?;
    let auth_token = std::env::var(UPGRADE_AUTH_TOKEN_ENV)
        .map_err(|_| RuntimeError::Config(format!("missing required upgrade env `{UPGRADE_AUTH_TOKEN_ENV}`")))?;
    let mut transport = UnixChildUpgradeTransport {
        stream: into_tokio_unix_stream(unsafe { StdUnixStream::from_raw_fd(control_fd) })?,
    };
    transport
        .write_message(&UpgradeControlMessage::ChildHello { auth_token })
        .await?;
    Ok(ChildUpgradeTransport::Unix(transport))
}

#[cfg(unix)]
fn into_tokio_unix_stream(stream: StdUnixStream) -> Result<UnixStream, RuntimeError> {
    stream.set_nonblocking(true)?;
    UnixStream::from_std(stream).map_err(Into::into)
}

#[cfg(unix)]
fn read_env_fd(name: &str) -> Result<i32, RuntimeError> {
    let value = std::env::var(name)
        .map_err(|_| RuntimeError::Config(format!("missing required upgrade env `{name}`")))?;
    parse_fd_env(name, &value)
}

#[cfg(unix)]
fn parse_fd_env(name: &str, value: &str) -> Result<i32, RuntimeError> {
    value.parse::<i32>().map_err(|error| {
        RuntimeError::Config(format!("invalid fd value for `{name}` (`{value}`): {error}"))
    })
}

#[cfg(unix)]
fn clear_fd_cloexec(fd: i32) -> Result<(), RuntimeError> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let result = unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) };
    if result < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(unix)]
fn send_rights_chunked(socket_fd: RawFd, handles: &[RawFd]) -> Result<(), RuntimeError> {
    if handles.is_empty() {
        return Ok(());
    }
    for chunk in handles.chunks(MAX_FDS_PER_RIGHTS_MESSAGE) {
        send_rights_once(socket_fd, chunk)?;
    }
    Ok(())
}

#[cfg(unix)]
fn receive_rights_chunked(socket_fd: RawFd, expected: usize) -> Result<Vec<RawFd>, RuntimeError> {
    let mut received = Vec::with_capacity(expected);
    while received.len() < expected {
        let remaining = expected - received.len();
        let chunk = receive_rights_once(socket_fd, remaining.min(MAX_FDS_PER_RIGHTS_MESSAGE))?;
        if chunk.is_empty() {
            return Err(RuntimeError::Config(
                "upgrade control socket closed before all handles were transferred".to_string(),
            ));
        }
        received.extend(chunk);
    }
    Ok(received)
}

#[cfg(unix)]
fn send_rights_once(socket_fd: RawFd, handles: &[RawFd]) -> Result<(), RuntimeError> {
    let mut data = [0_u8; 1];
    let mut iov = libc::iovec {
        iov_base: data.as_mut_ptr().cast(),
        iov_len: data.len(),
    };
    let mut control =
        vec![0_u8; unsafe { libc::CMSG_SPACE((std::mem::size_of_val(handles)) as _) as usize }];
    let mut message: libc::msghdr = unsafe { zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len();

    unsafe {
        let header = libc::CMSG_FIRSTHDR(&message)
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("failed to allocate upgrade rights header".to_string()))?;
        let header = header as *const libc::cmsghdr as *mut libc::cmsghdr;
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN((std::mem::size_of_val(handles)) as _) as usize;
        ptr::copy_nonoverlapping(
            handles.as_ptr().cast::<u8>(),
            libc::CMSG_DATA(header),
            std::mem::size_of_val(handles),
        );
        message.msg_controllen = (*header).cmsg_len;
    }

    let sent = unsafe { libc::sendmsg(socket_fd, &message, 0) };
    if sent < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(unix)]
fn receive_rights_once(socket_fd: RawFd, expected: usize) -> Result<Vec<RawFd>, RuntimeError> {
    let mut data = [0_u8; 1];
    let mut iov = libc::iovec {
        iov_base: data.as_mut_ptr().cast(),
        iov_len: data.len(),
    };
    let mut control = vec![0_u8; unsafe { libc::CMSG_SPACE((expected * size_of::<RawFd>()) as _) as usize }];
    let mut message: libc::msghdr = unsafe { zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len();

    let received_len = unsafe { libc::recvmsg(socket_fd, &mut message, 0) };
    if received_len < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if received_len == 0 {
        return Ok(Vec::new());
    }

    let mut handles = Vec::new();
    let mut current = unsafe { libc::CMSG_FIRSTHDR(&message) };
    while !current.is_null() {
        let header = unsafe { &*current };
        if header.cmsg_level == libc::SOL_SOCKET && header.cmsg_type == libc::SCM_RIGHTS {
            let data_len = header.cmsg_len as usize - unsafe { libc::CMSG_LEN(0) as usize };
            let count = data_len / size_of::<RawFd>();
            let ptr = unsafe { libc::CMSG_DATA(current).cast::<RawFd>() };
            for index in 0..count {
                handles.push(unsafe { *ptr.add(index) });
            }
        }
        current = unsafe { libc::CMSG_NXTHDR(&message, current) };
    }
    Ok(handles)
}

#[cfg(windows)]
struct WindowsParentUpgradeTransport {
    pipe: NamedPipeServer,
    child_pid: u32,
}

#[cfg(windows)]
struct WindowsChildUpgradeTransport {
    pipe: NamedPipeClient,
}

#[cfg(windows)]
impl WindowsParentUpgradeTransport {
    async fn write_message(&mut self, message: &UpgradeControlMessage) -> Result<(), RuntimeError> {
        write_control_message(&mut self.pipe, message).await
    }

    async fn read_message(&mut self) -> Result<UpgradeControlMessage, RuntimeError> {
        read_control_message(&mut self.pipe).await
    }

    fn send_handles(
        &self,
        game_listener: &std::net::TcpListener,
        admin_listener: Option<&std::net::TcpListener>,
        sessions: &[RuntimeUpgradeSessionHandle],
    ) -> Result<(), RuntimeError> {
        ensure_winsock_started()?;
        let mut infos = Vec::with_capacity(1 + usize::from(admin_listener.is_some()) + sessions.len());
        infos.push(duplicate_socket_protocol_info(
            game_listener.as_raw_socket(),
            self.child_pid,
        )?);
        if let Some(listener) = admin_listener {
            infos.push(duplicate_socket_protocol_info(
                listener.as_raw_socket(),
                self.child_pid,
            )?);
        }
        for session in sessions {
            infos.push(duplicate_socket_protocol_info(
                session.stream.as_raw_socket(),
                self.child_pid,
            )?);
        }
        send_protocol_infos(&self.pipe, &infos)
    }
}

#[cfg(windows)]
impl WindowsChildUpgradeTransport {
    async fn write_message(&mut self, message: &UpgradeControlMessage) -> Result<(), RuntimeError> {
        write_control_message(&mut self.pipe, message).await
    }

    async fn read_message(&mut self) -> Result<UpgradeControlMessage, RuntimeError> {
        read_control_message(&mut self.pipe).await
    }

    fn receive_handles(
        &self,
        expected: usize,
        has_admin_listener: bool,
    ) -> Result<ReceivedUpgradeHandles, RuntimeError> {
        ensure_winsock_started()?;
        let infos = receive_protocol_infos(&self.pipe, expected)?;
        let mut infos = infos.into_iter();
        let game_listener = unsafe {
            std::net::TcpListener::from_raw_socket(socket_from_protocol_info(
                &infos.next().ok_or_else(|| {
                    RuntimeError::Config(
                        "upgrade child did not receive a game listener handle".to_string(),
                    )
                })?,
            )?)
        };
        let admin_listener = if has_admin_listener {
            Some(unsafe {
                std::net::TcpListener::from_raw_socket(socket_from_protocol_info(
                    &infos.next().ok_or_else(|| {
                        RuntimeError::Config(
                            "upgrade child did not receive an admin gRPC listener handle"
                                .to_string(),
                        )
                    })?,
                )?)
            })
        } else {
            None
        };
        let session_streams = infos
            .map(|info| unsafe {
                socket_from_protocol_info(&info)
                    .map(|socket| std::net::TcpStream::from_raw_socket(socket))
            })
            .collect::<Result<Vec<_>, RuntimeError>>()?;
        Ok(ReceivedUpgradeHandles {
            game_listener,
            admin_listener,
            session_streams,
        })
    }
}

#[cfg(windows)]
async fn spawn_parent_upgrade_transport(
    executable_path: &str,
    auth_token: &str,
) -> Result<(std::process::Child, ParentUpgradeTransport), RuntimeError> {
    let pipe_name = format!(
        r"\\.\pipe\revy-runtime-upgrade-{}-{:032x}",
        std::process::id(),
        random::<u128>(),
    );
    let mut command = std::process::Command::new(executable_path);
    command.arg(UPGRADE_CHILD_ARG);
    command.env(UPGRADE_PIPE_NAME_ENV, &pipe_name);
    command.env(UPGRADE_AUTH_TOKEN_ENV, auth_token);
    let pipe = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)
        .map_err(|error| RuntimeError::Config(format!("failed to create upgrade pipe: {error}")))?;
    let child = command.spawn().map_err(|error| {
        RuntimeError::Config(format!(
            "failed to spawn upgrade child `{}`: {error}",
            executable_path
        ))
    })?;
    let child_pid = child.id();
    pipe.connect()
        .await
        .map_err(|error| RuntimeError::Config(format!("failed to connect upgrade pipe: {error}")))?;
    Ok((
        child,
        ParentUpgradeTransport::Windows(WindowsParentUpgradeTransport {
            pipe,
            child_pid,
        }),
    ))
}

#[cfg(windows)]
async fn open_child_upgrade_transport() -> Result<ChildUpgradeTransport, RuntimeError> {
    let pipe_name = std::env::var(UPGRADE_PIPE_NAME_ENV).map_err(|_| {
        RuntimeError::Config(format!("missing required upgrade env `{UPGRADE_PIPE_NAME_ENV}`"))
    })?;
    let auth_token = std::env::var(UPGRADE_AUTH_TOKEN_ENV)
        .map_err(|_| RuntimeError::Config(format!("missing required upgrade env `{UPGRADE_AUTH_TOKEN_ENV}`")))?;
    let mut transport = WindowsChildUpgradeTransport {
        pipe: ClientOptions::new()
            .open(&pipe_name)
            .map_err(|error| RuntimeError::Config(format!("failed to open upgrade pipe: {error}")))?,
    };
    transport
        .write_message(&UpgradeControlMessage::ChildHello { auth_token })
        .await?;
    Ok(ChildUpgradeTransport::Windows(transport))
}

#[cfg(windows)]
fn send_protocol_infos(
    pipe: &NamedPipeServer,
    infos: &[WSAPROTOCOL_INFOW],
) -> Result<(), RuntimeError> {
    for info in infos {
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (info as *const WSAPROTOCOL_INFOW).cast::<u8>(),
                size_of::<WSAPROTOCOL_INFOW>(),
            )
        };
        let written = unsafe {
            windows_sys::Win32::Storage::FileSystem::WriteFile(
                pipe.as_raw_handle() as _,
                bytes.as_ptr().cast(),
                size_of::<WSAPROTOCOL_INFOW>() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if written == 0 {
            return Err(RuntimeError::Config(format!(
                "failed to write duplicated socket info: {}",
                unsafe { WSAGetLastError() }
            )));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn receive_protocol_infos(
    pipe: &NamedPipeClient,
    expected: usize,
) -> Result<Vec<WSAPROTOCOL_INFOW>, RuntimeError> {
    let mut infos = Vec::with_capacity(expected);
    for _ in 0..expected {
        let mut info: WSAPROTOCOL_INFOW = unsafe { zeroed() };
        let read_ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::ReadFile(
                pipe.as_raw_handle() as _,
                (&mut info as *mut WSAPROTOCOL_INFOW).cast(),
                size_of::<WSAPROTOCOL_INFOW>() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if read_ok == 0 {
            return Err(RuntimeError::Config(format!(
                "failed to read duplicated socket info: {}",
                unsafe { WSAGetLastError() }
            )));
        }
        infos.push(info);
    }
    Ok(infos)
}

#[cfg(windows)]
fn ensure_winsock_started() -> Result<(), RuntimeError> {
    static STARTUP: OnceLock<Result<(), String>> = OnceLock::new();
    STARTUP
        .get_or_init(|| {
            let mut data: WSADATA = unsafe { zeroed() };
            let result = unsafe { WSAStartup(make_word(2, 2), &mut data) };
            if result == 0 {
                Ok(())
            } else {
                Err(format!("WSAStartup failed with code {result}"))
            }
        })
        .as_ref()
        .map(|_| ())
        .map_err(|message| RuntimeError::Config(message.clone()))
}

#[cfg(windows)]
const fn make_word(low: u8, high: u8) -> u16 {
    (low as u16) | ((high as u16) << 8)
}

#[cfg(windows)]
fn duplicate_socket_protocol_info(
    socket: RawSocket,
    child_pid: u32,
) -> Result<WSAPROTOCOL_INFOW, RuntimeError> {
    let mut info: WSAPROTOCOL_INFOW = unsafe { zeroed() };
    let result = unsafe { WSADuplicateSocketW(socket as SOCKET, child_pid, &mut info) };
    if result != 0 {
        return Err(RuntimeError::Config(format!(
            "WSADuplicateSocketW failed with code {}",
            unsafe { WSAGetLastError() }
        )));
    }
    Ok(info)
}

#[cfg(windows)]
unsafe fn socket_from_protocol_info(info: &WSAPROTOCOL_INFOW) -> Result<RawSocket, RuntimeError> {
    let socket = unsafe {
        WSASocketW(
            FROM_PROTOCOL_INFO,
            FROM_PROTOCOL_INFO,
            FROM_PROTOCOL_INFO,
            info as *const WSAPROTOCOL_INFOW as *mut WSAPROTOCOL_INFOW,
            0,
            WSA_FLAG_OVERLAPPED,
        )
    };
    if socket == INVALID_SOCKET {
        return Err(RuntimeError::Config(format!(
            "WSASocketW failed with code {}",
            unsafe { WSAGetLastError() }
        )));
    }
    Ok(socket as RawSocket)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::error::Error;
    use std::net::{TcpListener, TcpStream};
    use std::os::fd::{AsRawFd, FromRawFd};

    #[test]
    fn rights_transfer_round_trips_listener_and_stream() -> Result<(), Box<dyn Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let listener_addr = listener.local_addr()?;
        let client = TcpStream::connect(listener_addr)?;
        let (server_stream, _) = listener.accept()?;
        let server_local_addr = server_stream.local_addr()?;
        let (sender, receiver) = StdUnixStream::pair()?;

        send_rights_chunked(
            sender.as_raw_fd(),
            &[listener.as_raw_fd(), server_stream.as_raw_fd()],
        )?;
        let received = receive_rights_chunked(receiver.as_raw_fd(), 2)?;

        assert_eq!(received.len(), 2);
        let received_listener = unsafe { TcpListener::from_raw_fd(received[0]) };
        let received_stream = unsafe { TcpStream::from_raw_fd(received[1]) };
        assert_eq!(received_listener.local_addr()?, listener_addr);
        assert_eq!(received_stream.local_addr()?, server_local_addr);
        drop(client);
        Ok(())
    }

    #[test]
    fn rights_transfer_chunks_large_handle_sets() -> Result<(), Box<dyn Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let (sender, receiver) = StdUnixStream::pair()?;
        let duplicate_count = MAX_FDS_PER_RIGHTS_MESSAGE + 3;
        let duplicated = (0..duplicate_count)
            .map(|_| {
                let fd = unsafe { libc::dup(listener.as_raw_fd()) };
                if fd < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(fd)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        send_rights_chunked(sender.as_raw_fd(), &duplicated)?;
        let received = receive_rights_chunked(receiver.as_raw_fd(), duplicate_count)?;

        assert_eq!(received.len(), duplicate_count);
        for fd in duplicated.into_iter().chain(received.into_iter()) {
            unsafe {
                libc::close(fd);
            }
        }
        Ok(())
    }
}
