use crate::process_surfaces::{
    PausedAdminSurfaceInstance, PausedAdminSurfaceResource, PausedProcessSurfaces,
    ProcessSurfaceCommand,
};
use mc_plugin_contract::codec::admin_surface::AdminSurfaceResource;
use mc_proto_common::TransportKind;
use rand::random;
use revy_runtime_transfer::runtime_transfer_envelope_v1::Payload;
use revy_runtime_transfer::status_v1;
use revy_runtime_transfer::{
    BootstrapV1, CommitV1, CommittedStatusV1, CommittedV1, ErrorV1, ExportedSocket,
    FinalizeReportV1, HelloV1, PluginArtifactV1, PrestagePhaseV1, PrestageV1, ReadyV1,
    ResourceDescriptorV1, ResourceKindV1, RuntimeTransferEnvelopeV1, SharedTransferArena,
    SharedTransferArenaReader, SocketTransferTarget, StatusQueryV1, StatusV1, TransferErrorCodeV1,
    TransferId, TransferSequenceV1, read_envelope, write_envelope,
};
use revy_server_runtime::RuntimeError;
use revy_server_runtime::config::ServerConfigSource;
use revy_server_runtime::runtime::{
    AdminSubject, AdminUpgradeRuntimeView, ExecutableChildActivated,
    ExecutableChildRuntimePrepared, ExecutableChildSocketResources, ExecutableParentCommitHold,
    ExecutableUpgradeOutcomePending, ServerSupervisor,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot, watch};

#[cfg(unix)]
use {
    revy_runtime_transfer::UnixArenaDescriptor,
    std::ffi::OsStr,
    std::mem::{size_of, zeroed},
    std::os::fd::{AsFd, AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd},
    std::os::unix::net::UnixStream as StdUnixStream,
    std::ptr,
    tokio::net::UnixStream,
};

#[cfg(windows)]
use {
    revy_runtime_transfer::WindowsArenaName,
    std::os::windows::io::{BorrowedSocket, IntoRawSocket},
    tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    },
};

const CHILD_ARGUMENT: &str = "--runtime-transfer-child";
const AUTH_TOKEN_ENV: &str = "REVY_RUNTIME_TRANSFER_TOKEN";
const TRANSFER_ID_ENV: &str = "REVY_RUNTIME_TRANSFER_ID";
const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
const TRANSFER_ARENA_CAPACITY: u64 = revy_runtime_transfer::MAX_TRANSFER_ARENA_BYTES;
const CORE_PRESTAGE_ROUND_LIMIT: usize = 32;
const CORE_FREEZE_ADMISSION_COMMIT_LAG: u64 = 256;

#[cfg(unix)]
const CONTROL_FD_ENV: &str = "REVY_RUNTIME_TRANSFER_CONTROL_FD";
#[cfg(unix)]
const MAX_FDS_PER_RIGHTS_MESSAGE: usize = 200;

#[cfg(windows)]
const PIPE_NAME_ENV: &str = "REVY_RUNTIME_TRANSFER_PIPE_NAME";
#[cfg(windows)]
const ARENA_NAME_ENV: &str = "REVY_RUNTIME_TRANSFER_ARENA_NAME";

pub(crate) struct ExecutableHandoffCoordinator {
    server: Arc<ServerSupervisor>,
    surface_control_tx: Mutex<Option<mpsc::Sender<ProcessSurfaceCommand>>>,
    process_shutdown_tx: Mutex<Option<watch::Sender<bool>>>,
    serial: Mutex<()>,
    parent_retirement: Mutex<Option<ParentRetirementAuthority>>,
}

pub(crate) struct PendingExecutableChild {
    runtime: Option<ExecutableChildRuntimePrepared>,
    activated: Option<ExecutableChildActivated>,
    admin_surfaces: Vec<PausedAdminSurfaceInstance>,
    control: Option<ChildControl>,
    child_epoch_revision: u64,
}

pub(crate) struct ChildStatusAuthority {
    task: tokio::task::JoinHandle<()>,
}

enum ParentRetirementAuthority {
    Committed(ExecutableParentCommitHold),
    OutcomeUncertain(ExecutableUpgradeOutcomePending),
    FailClosed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AdminSurfaceTransfer {
    instances: Vec<AdminSurfaceTransferInstance>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AdminSurfaceTransferInstance {
    instance_id: String,
    resume_payload: Vec<u8>,
    resources: Vec<AdminSurfaceTransferResource>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AdminSurfaceTransferResource {
    name: String,
    value: AdminSurfaceTransferValue,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum AdminSurfaceTransferValue {
    Bytes(Vec<u8>),
    NativeSocket {
        handle_kind: String,
        native_handle_index: u32,
    },
}

struct EncodedAdminSurfaces {
    region: revy_runtime_transfer::SealedArenaRegion,
    sockets: Vec<ExportedSocket>,
}

struct ParentControl {
    transport: ParentTransport,
    sequence: TransferSequenceV1,
}

struct ChildControl {
    transport: ChildTransport,
    sequence: TransferSequenceV1,
}

#[cfg(unix)]
struct ParentTransport {
    stream: UnixStream,
}

#[cfg(unix)]
struct ChildTransport {
    stream: UnixStream,
}

#[cfg(windows)]
struct ParentTransport {
    pipe: NamedPipeServer,
}

#[cfg(windows)]
struct ChildTransport {
    pipe: NamedPipeClient,
    arena_name: WindowsArenaName,
}

impl ExecutableHandoffCoordinator {
    pub(crate) fn new(server: Arc<ServerSupervisor>) -> Self {
        Self {
            server,
            surface_control_tx: Mutex::new(None),
            process_shutdown_tx: Mutex::new(None),
            serial: Mutex::new(()),
            parent_retirement: Mutex::new(None),
        }
    }

    pub(crate) async fn set_surface_control_sender(
        &self,
        surface_control_tx: mpsc::Sender<ProcessSurfaceCommand>,
    ) {
        *self.surface_control_tx.lock().await = Some(surface_control_tx);
    }

    pub(crate) async fn set_process_shutdown_sender(&self, shutdown_tx: watch::Sender<bool>) {
        *self.process_shutdown_tx.lock().await = Some(shutdown_tx);
    }

    pub(crate) async fn take_parent_retirement(&self) -> bool {
        match self.parent_retirement.lock().await.take() {
            Some(ParentRetirementAuthority::Committed(hold)) => {
                drop(hold);
                true
            }
            Some(ParentRetirementAuthority::OutcomeUncertain(pending)) => {
                drop(pending);
                true
            }
            Some(ParentRetirementAuthority::FailClosed) => true,
            None => false,
        }
    }

    pub(crate) async fn upgrade(
        &self,
        subject: AdminSubject,
        executable_path: String,
    ) -> Result<AdminUpgradeRuntimeView, RuntimeError> {
        let _serial = self.serial.lock().await;
        validate_executable(&executable_path)?;
        let transfer_id = TransferId::from_bytes(random());
        let authentication_token: [u8; revy_runtime_transfer::AUTHENTICATION_TOKEN_BYTES] =
            random();
        let arena = Arc::new(
            SharedTransferArena::create(
                TRANSFER_ARENA_CAPACITY,
                1,
                transfer_id,
                authentication_token,
            )
            .map_err(transfer_error)?,
        );
        let staged = self
            .server
            .prepare_executable_upgrade(Arc::clone(&arena))
            .await?;
        let mut preparing = Some(staged.begin_preparing().await?);
        let (mut child, mut control, child_pid) =
            match spawn_child(&executable_path, transfer_id, authentication_token, &arena).await {
                Ok(spawned) => spawned,
                Err(error) => {
                    let rollback = preparing
                        .take()
                        .expect("preparing authority is present")
                        .abort_preparation()
                        .await;
                    return Err(combine_precommit(error, rollback, Ok(())));
                }
            };
        if let Err(error) = control.receive_hello(authentication_token, child_pid).await {
            terminate_child(&mut child);
            let rollback = preparing
                .take()
                .expect("preparing authority is present")
                .abort_preparation()
                .await;
            return Err(combine_precommit(error, rollback, Ok(())));
        }

        let target = match SocketTransferTarget::new(child_pid).map_err(transfer_error) {
            Ok(target) => target,
            Err(error) => {
                terminate_child(&mut child);
                let rollback = preparing
                    .take()
                    .expect("preparing authority is present")
                    .abort_preparation()
                    .await;
                return Err(combine_precommit(error, rollback, Ok(())));
            }
        };
        let current_directory = match preparing
            .as_ref()
            .expect("preparing authority is present")
            .prestage_directory()
            .await
        {
            Ok(directory) => directory,
            Err(error) => {
                terminate_child(&mut child);
                let rollback = preparing
                    .take()
                    .expect("preparing authority is present")
                    .abort_preparation()
                    .await;
                return Err(combine_precommit(error, rollback, Ok(())));
            }
        };
        let network = match preparing
            .as_ref()
            .expect("preparing authority is present")
            .prepare_network_transfer(target)
            .await
        {
            Ok(network) => network,
            Err(error) => {
                terminate_child(&mut child);
                let rollback = preparing
                    .take()
                    .expect("preparing authority is present")
                    .abort_preparation()
                    .await;
                return Err(combine_precommit(error, rollback, Ok(())));
            }
        };
        let (network, child_sockets) = match network.transfer_sockets() {
            Ok(transfer) => transfer,
            Err(error) => {
                terminate_child(&mut child);
                let rollback = preparing
                    .take()
                    .expect("preparing authority is present")
                    .abort_preparation()
                    .await;
                return Err(combine_precommit(error, rollback, Ok(())));
            }
        };
        let paused_surfaces = match self.pause_process_surfaces().await {
            Ok(paused) => paused,
            Err(error) => {
                terminate_child(&mut child);
                let rollback = preparing
                    .take()
                    .expect("preparing authority is present")
                    .abort_preparation()
                    .await;
                return Err(combine_precommit(error, rollback, Ok(())));
            }
        };

        let transaction = self
            .prepare_child_and_freeze(
                &mut control,
                preparing.take().expect("preparing authority is present"),
                network,
                current_directory,
                child_sockets,
                &paused_surfaces,
                target,
                &arena,
            )
            .await;
        let (pending_parent, committed) = match transaction {
            Ok(committed) => committed,
            Err(PreCommitFailure::Preparing(error, preparing)) => {
                terminate_child(&mut child);
                let rollback = preparing.abort_preparation().await;
                let surfaces = self.resume_process_surfaces(paused_surfaces).await;
                return Err(combine_precommit(error, rollback, surfaces));
            }
            Err(PreCommitFailure::Frozen(error, frozen)) => {
                terminate_child(&mut child);
                let rollback = frozen.abort().await;
                let surfaces = self.resume_process_surfaces(paused_surfaces).await;
                return Err(combine_precommit(error, rollback, surfaces));
            }
            Err(PreCommitFailure::RuntimeRestored(error)) => {
                terminate_child(&mut child);
                let surfaces = self.resume_process_surfaces(paused_surfaces).await;
                return Err(combine_precommit(error, Ok(()), surfaces));
            }
            Err(PreCommitFailure::FailClosed(error)) => {
                terminate_child(&mut child);
                *self.parent_retirement.lock().await = Some(ParentRetirementAuthority::FailClosed);
                self.request_process_exit().await;
                return Err(error);
            }
        };

        let freeze_duration = pending_parent.freeze_duration();
        let cutover = match committed {
            ChildCommitOutcome::Committed {
                child_epoch_revision,
                resume_duration_us,
            } => {
                let (hold, report) = match pending_parent
                    .committed(freeze_duration, Duration::from_micros(resume_duration_us))
                    .await
                {
                    Ok(committed) => committed,
                    Err(error) => {
                        *self.parent_retirement.lock().await =
                            Some(ParentRetirementAuthority::FailClosed);
                        self.request_process_exit().await;
                        return Err(error);
                    }
                };
                *self.parent_retirement.lock().await =
                    Some(ParentRetirementAuthority::Committed(hold));
                if let Err(error) = control
                    .finalize_report(child_epoch_revision, report.freeze_us)
                    .await
                {
                    eprintln!(
                        "committed executable cutover report could not be published to child status: {error}"
                    );
                }
                report
            }
            ChildCommitOutcome::Uncertain(error) => {
                *self.parent_retirement.lock().await =
                    Some(ParentRetirementAuthority::OutcomeUncertain(pending_parent));
                self.request_process_exit().await;
                return Err(error);
            }
        };
        self.request_process_exit_after(subject).await;
        Ok(AdminUpgradeRuntimeView {
            executable_path,
            cutover,
        })
    }

    async fn prepare_child_and_freeze(
        &self,
        control: &mut ParentControl,
        preparing: revy_server_runtime::runtime::ExecutableUpgradePreparing,
        network: revy_server_runtime::runtime::ExecutableNetworkPrepared,
        current_directory: revy_server_runtime::runtime::ExecutableDirectoryPrestage,
        child_sockets: ExecutableChildSocketResources,
        paused_surfaces: &PausedProcessSurfaces,
        target: SocketTransferTarget,
        arena: &Arc<SharedTransferArena>,
    ) -> Result<(ExecutableUpgradeOutcomePending, ChildCommitOutcome), PreCommitFailure> {
        macro_rules! with_preparing_authority {
            ($result:expr) => {
                match $result {
                    Ok(value) => value,
                    Err(error) => {
                        return Err(PreCommitFailure::Preparing(error, preparing));
                    }
                }
            };
        }

        let (socket_resources, mut native_sockets, mut resource_id) =
            with_preparing_authority!(encode_socket_resources(child_sockets));
        let native_start = match u32::try_from(native_sockets.len()) {
            Ok(native_start) => native_start,
            Err(_) => {
                return Err(PreCommitFailure::Preparing(
                    RuntimeError::Config("native socket count exceeds u32".to_string()),
                    preparing,
                ));
            }
        };
        let encoded_admin =
            encode_admin_surfaces(&paused_surfaces.admin_surfaces, target, native_start, arena);
        let encoded_admin = with_preparing_authority!(encoded_admin);
        let mut resources = vec![
            arena_resource(
                next_resource_id(&mut resource_id),
                ResourceKindV1::CoreSnapshot,
                preparing.core_snapshot_region(),
            ),
            arena_resource(
                next_resource_id(&mut resource_id),
                ResourceKindV1::SessionDirectory,
                preparing.initial_directory().region(),
            ),
            arena_resource(
                next_resource_id(&mut resource_id),
                ResourceKindV1::AdminResource,
                &encoded_admin.region,
            ),
        ];
        if let Some(region) = preparing.online_auth_keys_region() {
            resources.push(arena_resource(
                next_resource_id(&mut resource_id),
                ResourceKindV1::OnlineAuthKeys,
                region,
            ));
        }
        resources.extend(socket_resources);
        for (offset, _) in encoded_admin.sockets.iter().enumerate() {
            let index = native_sockets
                .len()
                .checked_add(offset)
                .and_then(|value| u32::try_from(value).ok());
            let Some(index) = index else {
                return Err(PreCommitFailure::Preparing(
                    RuntimeError::Config("native socket index exceeds u32".to_string()),
                    preparing,
                ));
            };
            resources.push(native_resource(
                next_resource_id(&mut resource_id),
                ResourceKindV1::AdminResource,
                index,
                None,
            ));
        }
        native_sockets.extend(encoded_admin.sockets);
        let native_handle_count = match u32::try_from(native_sockets.len()) {
            Ok(count) => count,
            Err(_) => {
                return Err(PreCommitFailure::Preparing(
                    RuntimeError::Config("native socket count exceeds u32".to_string()),
                    preparing,
                ));
            }
        };
        let bootstrap = BootstrapV1 {
            validated_config_digest: preparing.validated_config_digest().to_vec(),
            plugin_artifacts: preparing
                .plugin_artifacts()
                .iter()
                .map(|artifact| PluginArtifactV1 {
                    plugin_id: artifact.plugin_id.clone(),
                    sha256: artifact.sha256.to_vec(),
                })
                .collect(),
            arena: Some(arena.descriptor()),
            resources,
            native_handle_count,
            core_snapshot_revision: preparing.core_snapshot_revision_value(),
            persisted_core_revision: preparing.persisted_core_revision_value(),
            latest_dirty_core_revision: preparing.latest_dirty_core_revision_value(),
            directory_revision: preparing.staged_directory_revision(),
            active_generation_id: preparing.active_generation_id_value(),
        };
        with_preparing_authority!(control.send(Payload::Bootstrap(bootstrap)).await);
        with_preparing_authority!(control.send_native(arena, &native_sockets).await);
        let current_prestage = PrestageV1 {
            phase: PrestagePhaseV1::Preparing as i32,
            core_revision: preparing.core_snapshot_revision_value(),
            directory_revision: current_directory.directory().revision(),
            resources: vec![arena_resource(
                next_resource_id(&mut resource_id),
                ResourceKindV1::SessionDirectory,
                current_directory.region(),
            )],
            arena_used: arena.descriptor().used,
        };
        with_preparing_authority!(control.send(Payload::Prestage(current_prestage)).await);
        with_preparing_authority!(
            control
                .expect_ready(
                    preparing.core_snapshot_revision_value(),
                    current_directory.directory().revision(),
                )
                .await
        );

        let mut core_revision = preparing.core_snapshot_revision_value();
        let mut acknowledged_core = None;
        for _ in 0..CORE_PRESTAGE_ROUND_LIMIT {
            let update = with_preparing_authority!(preparing.prestage_core(core_revision).await);
            let prestage = PrestageV1 {
                phase: PrestagePhaseV1::Preparing as i32,
                core_revision: update.revision(),
                directory_revision: current_directory.directory().revision(),
                resources: vec![arena_resource(
                    next_resource_id(&mut resource_id),
                    update.kind(),
                    update.region(),
                )],
                arena_used: arena.descriptor().used,
            };
            with_preparing_authority!(control.send(Payload::Prestage(prestage)).await);
            with_preparing_authority!(
                control
                    .expect_ready(update.revision(), current_directory.directory().revision())
                    .await
            );
            core_revision = update.revision();
            if preparing.core_prestage_lag(&update) <= CORE_FREEZE_ADMISSION_COMMIT_LAG {
                acknowledged_core = Some(update);
                break;
            }
        }
        let Some(acknowledged_core) = acknowledged_core else {
            return Err(PreCommitFailure::Preparing(
                RuntimeError::BudgetExceeded {
                    resource: "core pre-stage rounds",
                    requested: CORE_PRESTAGE_ROUND_LIMIT + 1,
                    limit: CORE_PRESTAGE_ROUND_LIMIT,
                },
                preparing,
            ));
        };
        // The acknowledged directory bounds the frozen session descriptors. Reserve their
        // complete control-plane payload before closing the data-plane gate.
        let Some(descriptor_count) = current_directory
            .directory()
            .sessions()
            .len()
            .checked_add(2)
        else {
            return Err(PreCommitFailure::Preparing(
                RuntimeError::Config("final transfer descriptor count overflows".to_string()),
                preparing,
            ));
        };
        let Some(descriptor_bytes) =
            descriptor_count.checked_mul(std::mem::size_of::<ResourceDescriptorV1>())
        else {
            return Err(PreCommitFailure::Preparing(
                RuntimeError::Config("final transfer descriptor size overflows".to_string()),
                preparing,
            ));
        };
        let mut final_resources = Vec::new();
        if let Err(_error) = final_resources.try_reserve_exact(descriptor_count) {
            return Err(PreCommitFailure::Preparing(
                RuntimeError::Allocation {
                    resource: "final transfer descriptors",
                    requested: descriptor_bytes,
                },
                preparing,
            ));
        }
        let frozen = preparing
            .freeze(network, current_directory, acknowledged_core)
            .await
            .map_err(|error| {
                // `freeze` restores parent authority on its own failure, so no frozen capability
                // remains to roll back here.
                PreCommitFailure::RuntimeRestored(error)
            })?;
        final_resources.push(arena_resource(
            next_resource_id(&mut resource_id),
            ResourceKindV1::CoreJournal,
            frozen.core_delta().region(),
        ));
        if let Some(region) = frozen.network().raknet_router_state() {
            final_resources.push(arena_resource(
                next_resource_id(&mut resource_id),
                ResourceKindV1::RakNetState,
                region,
            ));
        }
        for state in frozen.network().session_states() {
            let mut resource = arena_resource(
                next_resource_id(&mut resource_id),
                ResourceKindV1::SessionState,
                state.region(),
            );
            resource.logical_id = Some(state.connection_id().0);
            final_resources.push(resource);
        }
        let final_prestage = PrestageV1 {
            phase: PrestagePhaseV1::Frozen as i32,
            core_revision: frozen.core_delta().final_revision_value(),
            directory_revision: frozen.final_directory().directory().revision(),
            resources: final_resources,
            arena_used: arena.descriptor().used,
        };
        if let Err(error) = control.send(Payload::Prestage(final_prestage)).await {
            return Err(PreCommitFailure::Frozen(error, frozen));
        }
        if let Err(error) = control
            .expect_ready(
                frozen.core_delta().final_revision_value(),
                frozen.final_directory().directory().revision(),
            )
            .await
        {
            return Err(PreCommitFailure::Frozen(error, frozen));
        }
        let commit = CommitV1 {
            final_core_revision: frozen.core_delta().final_revision_value(),
            final_directory_revision: frozen.final_directory().directory().revision(),
            parent_epoch_revision: frozen.epoch_revision(),
            persisted_core_revision: frozen.core_delta().persisted_revision_value(),
            latest_dirty_core_revision: frozen.core_delta().latest_dirty_revision_value(),
            stage_duration_us: frozen.stage_duration_us(),
            prepare_duration_us: frozen.prepare_duration_us(),
            session_count: match u64::try_from(frozen.session_count()) {
                Ok(count) => count,
                Err(_) => {
                    return Err(PreCommitFailure::Frozen(
                        RuntimeError::Config("session count exceeds u64".to_string()),
                        frozen,
                    ));
                }
            },
            java_session_count: match u64::try_from(frozen.connection_mix().java) {
                Ok(count) => count,
                Err(_) => {
                    return Err(PreCommitFailure::Frozen(
                        RuntimeError::Config("Java session count exceeds u64".to_string()),
                        frozen,
                    ));
                }
            },
            bedrock_session_count: match u64::try_from(frozen.connection_mix().bedrock) {
                Ok(count) => count,
                Err(_) => {
                    return Err(PreCommitFailure::Frozen(
                        RuntimeError::Config("Bedrock session count exceeds u64".to_string()),
                        frozen,
                    ));
                }
            },
        };
        let Some(expected_child_revision) = commit.parent_epoch_revision.checked_add(1) else {
            return Err(PreCommitFailure::Frozen(
                RuntimeError::Config("child epoch revision overflow".to_string()),
                frozen,
            ));
        };
        let pending_parent = match frozen.commit_sent() {
            Ok(pending) => pending,
            Err(error) => return Err(PreCommitFailure::FailClosed(error)),
        };
        if let Err(error) = control.send(Payload::Commit(commit)).await {
            return Ok((pending_parent, ChildCommitOutcome::Uncertain(error)));
        }
        let outcome = control.await_committed(expected_child_revision).await;
        Ok((pending_parent, outcome))
    }

    async fn pause_process_surfaces(&self) -> Result<PausedProcessSurfaces, RuntimeError> {
        let control = self
            .surface_control_tx
            .lock()
            .await
            .clone()
            .ok_or_else(|| {
                RuntimeError::Config(
                    "executable handoff requires process-surface orchestration".to_string(),
                )
            })?;
        let (ack_tx, ack_rx) = oneshot::channel();
        control
            .send(ProcessSurfaceCommand::PauseForUpgrade { ack_tx })
            .await
            .map_err(|_| RuntimeError::Config("process surface control closed".to_string()))?;
        ack_rx
            .await
            .map_err(|_| RuntimeError::Config("process surface pause ack dropped".to_string()))?
    }

    async fn resume_process_surfaces(
        &self,
        paused: PausedProcessSurfaces,
    ) -> Result<(), RuntimeError> {
        let Some(control) = self.surface_control_tx.lock().await.clone() else {
            return Err(RuntimeError::Config(
                "process surface control closed during rollback".to_string(),
            ));
        };
        let (ack_tx, ack_rx) = oneshot::channel();
        control
            .send(ProcessSurfaceCommand::ResumeAfterUpgradeRollback { paused, ack_tx })
            .await
            .map_err(|_| RuntimeError::Config("process surface control closed".to_string()))?;
        ack_rx
            .await
            .map_err(|_| RuntimeError::Config("process surface rollback ack dropped".to_string()))?
    }

    async fn request_process_exit_after(&self, subject: AdminSubject) {
        let delay = if subject.principal_id().starts_with("console:") {
            Duration::from_millis(500)
        } else {
            Duration::from_millis(75)
        };
        tokio::time::sleep(delay).await;
        self.request_process_exit().await;
    }

    async fn request_process_exit(&self) {
        if let Some(shutdown) = self.process_shutdown_tx.lock().await.as_ref() {
            let _ = shutdown.send(true);
        }
    }
}

enum PreCommitFailure {
    Preparing(
        RuntimeError,
        revy_server_runtime::runtime::ExecutableUpgradePreparing,
    ),
    Frozen(
        RuntimeError,
        revy_server_runtime::runtime::ExecutableUpgradeFrozen,
    ),
    RuntimeRestored(RuntimeError),
    FailClosed(RuntimeError),
}

enum ChildCommitOutcome {
    Committed {
        child_epoch_revision: u64,
        resume_duration_us: u64,
    },
    Uncertain(RuntimeError),
}

impl PendingExecutableChild {
    pub(crate) fn take_admin_surfaces(&mut self) -> Vec<PausedAdminSurfaceInstance> {
        std::mem::take(&mut self.admin_surfaces)
    }

    pub(crate) async fn activate_runtime(&mut self) -> Result<Arc<ServerSupervisor>, RuntimeError> {
        let runtime = self.runtime.take().ok_or_else(|| {
            RuntimeError::Config(
                "child runtime activation authority was already consumed".to_string(),
            )
        })?;
        let activated = runtime.activate().await?;
        let server = activated.server();
        self.activated = Some(activated);
        Ok(server)
    }

    pub(crate) async fn report_error(&mut self, error: &RuntimeError) {
        if let Some(control) = self.control.as_mut() {
            let _ = control
                .send(Payload::Error(ErrorV1 {
                    code: TransferErrorCodeV1::CommitRejected as i32,
                    message: bounded_diagnostic(error),
                }))
                .await;
        }
    }

    pub(crate) async fn report_committed(mut self) -> Result<ChildStatusAuthority, RuntimeError> {
        let mut control = self.control.take().ok_or_else(|| {
            RuntimeError::Config("child commit control was already consumed".to_string())
        })?;
        let activated = self.activated.take().ok_or_else(|| {
            RuntimeError::Config(
                "child committed acknowledgement requires an activated runtime".to_string(),
            )
        })?;
        let resume_duration_us = activated.resume_duration_us();
        let child_epoch_revision = self.child_epoch_revision;
        control
            .send(Payload::Committed(CommittedV1 {
                child_epoch_revision,
                resume_duration_us,
            }))
            .await?;
        let task = tokio::spawn(async move {
            let mut report_published = false;
            while let Ok(envelope) = control.receive().await {
                let Some(Payload::Status(status)) = envelope.payload else {
                    continue;
                };
                match status.payload {
                    Some(status_v1::Payload::Query(query))
                        if query.expected_child_epoch_revision == child_epoch_revision => {}
                    Some(status_v1::Payload::FinalizeReport(report))
                        if report.child_epoch_revision == child_epoch_revision =>
                    {
                        if !report_published {
                            let _ = activated.publish_committed_report(Duration::from_micros(
                                report.freeze_duration_us,
                            ));
                            report_published = true;
                        }
                    }
                    _ => continue,
                }
                let _ = control
                    .send(Payload::Status(StatusV1 {
                        payload: Some(status_v1::Payload::Committed(CommittedStatusV1 {
                            child_epoch_revision,
                            resume_duration_us,
                            report_published,
                        })),
                    }))
                    .await;
            }
        });
        Ok(ChildStatusAuthority { task })
    }
}

impl Drop for ChildStatusAuthority {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) async fn try_prepare_executable_child(
    args: &[String],
    config_source: ServerConfigSource,
) -> Result<Option<PendingExecutableChild>, RuntimeError> {
    if !args.iter().any(|argument| argument == CHILD_ARGUMENT) {
        return Ok(None);
    }
    let (mut control, authentication_token, process_id) = open_child_control().await?;
    control
        .send(Payload::Hello(HelloV1 {
            authentication_token: authentication_token.to_vec(),
            process_id,
        }))
        .await?;
    let prepared = prepare_transferred_child(&mut control, config_source).await;
    match prepared {
        Ok(mut pending) => {
            pending.control = Some(control);
            Ok(Some(pending))
        }
        Err(error) => {
            let _ = control
                .send(Payload::Error(ErrorV1 {
                    code: TransferErrorCodeV1::ResourceImportFailed as i32,
                    message: bounded_diagnostic(&error),
                }))
                .await;
            Err(error)
        }
    }
}

async fn prepare_transferred_child(
    control: &mut ChildControl,
    config_source: ServerConfigSource,
) -> Result<PendingExecutableChild, RuntimeError> {
    let bootstrap = match control.receive_payload().await? {
        Payload::Bootstrap(bootstrap) => bootstrap,
        payload => return Err(unexpected("Bootstrap", &payload)),
    };
    let native = control
        .receive_native(
            bootstrap.native_handle_count as usize,
            bootstrap.arena.as_ref().ok_or_else(|| {
                RuntimeError::Config("child bootstrap omitted transfer arena".to_string())
            })?,
        )
        .await?;
    let (mut arena, sockets) = native;
    let mut child = ServerSupervisor::prepare_executable_child(config_source, &bootstrap, &arena)?;
    let current = match control.receive_payload().await? {
        Payload::Prestage(prestage) => prestage,
        Payload::Abort(abort) => return Err(RuntimeError::Config(abort.reason)),
        payload => return Err(unexpected("Prestage", &payload)),
    };
    if current.phase != PrestagePhaseV1::Preparing as i32 {
        return Err(RuntimeError::Config(
            "initial pre-stage must be Preparing".to_string(),
        ));
    }
    child.prestage(&current, &mut arena)?;
    let (socket_resources, admin_surfaces) = decode_native_resources(&bootstrap, &arena, sockets)?;
    let native = child.prepare_native_socket_import(socket_resources)?;
    control
        .send(Payload::Ready(ReadyV1 {
            acknowledged_core_revision: child.core_revision().value(),
            acknowledged_directory_revision: child.directory_revision(),
        }))
        .await?;

    let final_prestage = loop {
        let prestage = match control.receive_payload().await? {
            Payload::Prestage(prestage) => prestage,
            Payload::Abort(abort) => return Err(RuntimeError::Config(abort.reason)),
            payload => return Err(unexpected("Prestage", &payload)),
        };
        child.prestage(&prestage, &mut arena)?;
        match PrestagePhaseV1::try_from(prestage.phase) {
            Ok(PrestagePhaseV1::Preparing) => {
                control
                    .send(Payload::Ready(ReadyV1 {
                        acknowledged_core_revision: child.core_revision().value(),
                        acknowledged_directory_revision: child.directory_revision(),
                    }))
                    .await?;
            }
            Ok(PrestagePhaseV1::Frozen) => break prestage,
            _ => return Err(RuntimeError::Config("invalid pre-stage phase".to_string())),
        }
    };
    let child = child.prepare_session_import(&final_prestage.resources, &arena)?;
    let child = child.prepare_network_import(native).await?;
    control
        .send(Payload::Ready(ReadyV1 {
            acknowledged_core_revision: final_prestage.core_revision,
            acknowledged_directory_revision: final_prestage.directory_revision,
        }))
        .await?;
    let commit = match control.receive_payload().await? {
        Payload::Commit(commit) => commit,
        Payload::Abort(abort) => return Err(RuntimeError::Config(abort.reason)),
        payload => return Err(unexpected("Commit", &payload)),
    };
    let child_epoch_revision = commit
        .parent_epoch_revision
        .checked_add(1)
        .ok_or_else(|| RuntimeError::Config("child epoch revision overflow".to_string()))?;
    let committed = child.commit(&commit)?;
    let runtime = ServerSupervisor::boot_executable_child(committed).await?;
    Ok(PendingExecutableChild {
        runtime: Some(runtime),
        activated: None,
        admin_surfaces,
        control: None,
        child_epoch_revision,
    })
}

fn encode_socket_resources(
    sockets: ExecutableChildSocketResources,
) -> Result<(Vec<ResourceDescriptorV1>, Vec<ExportedSocket>, u64), RuntimeError> {
    let (mut listeners, mut sessions) = sockets.into_parts();
    listeners.sort_by_key(|(transport, _)| match transport {
        TransportKind::Tcp => 0,
        TransportKind::Udp => 1,
    });
    sessions.sort_by_key(|(connection_id, _)| *connection_id);
    let mut resources = Vec::with_capacity(listeners.len() + sessions.len());
    let mut native = Vec::with_capacity(resources.capacity());
    let mut resource_id = 1_u64;
    for (transport, socket) in listeners {
        let kind = match transport {
            TransportKind::Tcp => ResourceKindV1::TcpListener,
            TransportKind::Udp => ResourceKindV1::UdpListener,
        };
        let index = u32::try_from(native.len())
            .map_err(|_| RuntimeError::Config("native socket index exceeds u32".to_string()))?;
        resources.push(native_resource(
            next_resource_id(&mut resource_id),
            kind,
            index,
            None,
        ));
        native.push(socket);
    }
    for (connection_id, socket) in sessions {
        let index = u32::try_from(native.len())
            .map_err(|_| RuntimeError::Config("native socket index exceeds u32".to_string()))?;
        resources.push(native_resource(
            next_resource_id(&mut resource_id),
            ResourceKindV1::TcpStream,
            index,
            Some(connection_id),
        ));
        native.push(socket);
    }
    Ok((resources, native, resource_id))
}

fn encode_admin_surfaces(
    paused: &[PausedAdminSurfaceInstance],
    target: SocketTransferTarget,
    native_start: u32,
    arena: &SharedTransferArena,
) -> Result<EncodedAdminSurfaces, RuntimeError> {
    let mut sockets = Vec::new();
    let mut instances = Vec::with_capacity(paused.len());
    for instance in paused {
        let mut resources = Vec::with_capacity(instance.handoff_resources.len());
        for resource in &instance.handoff_resources {
            let value = match &resource.resource {
                AdminSurfaceResource::Bytes(bytes) => {
                    AdminSurfaceTransferValue::Bytes(bytes.clone())
                }
                AdminSurfaceResource::NativeHandle {
                    handle_kind,
                    raw_handle,
                } => {
                    let socket = duplicate_admin_socket(*raw_handle, target)?;
                    let offset = u32::try_from(sockets.len()).map_err(|_| {
                        RuntimeError::Config("admin native socket count exceeds u32".to_string())
                    })?;
                    let native_handle_index =
                        native_start.checked_add(offset).ok_or_else(|| {
                            RuntimeError::Config("admin native socket index overflow".to_string())
                        })?;
                    sockets.push(socket);
                    AdminSurfaceTransferValue::NativeSocket {
                        handle_kind: handle_kind.clone(),
                        native_handle_index,
                    }
                }
            };
            resources.push(AdminSurfaceTransferResource {
                name: resource.name.clone(),
                value,
            });
        }
        instances.push(AdminSurfaceTransferInstance {
            instance_id: instance.instance_id.clone(),
            resume_payload: instance.resume_payload.clone(),
            resources,
        });
    }
    let encoded = serde_json::to_vec(&AdminSurfaceTransfer { instances })
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let mut reservation = arena.reserve(encoded.len()).map_err(transfer_error)?;
    std::io::Write::write_all(&mut reservation, &encoded)?;
    Ok(EncodedAdminSurfaces {
        region: reservation.seal().map_err(transfer_error)?,
        sockets,
    })
}

fn decode_native_resources(
    bootstrap: &BootstrapV1,
    arena: &SharedTransferArenaReader,
    sockets: Vec<ExportedSocket>,
) -> Result<
    (
        ExecutableChildSocketResources,
        Vec<PausedAdminSurfaceInstance>,
    ),
    RuntimeError,
> {
    let mut sockets = sockets.into_iter().map(Some).collect::<Vec<_>>();
    let mut listeners = Vec::new();
    let mut sessions = Vec::new();
    let mut admin_native_indices = HashSet::new();
    let mut admin_metadata = None;
    for resource in &bootstrap.resources {
        let kind = ResourceKindV1::try_from(resource.kind)
            .map_err(|_| RuntimeError::Config("invalid resource kind".to_string()))?;
        match kind {
            ResourceKindV1::TcpListener | ResourceKindV1::UdpListener => {
                let socket = take_socket(&mut sockets, resource)?;
                let transport = if kind == ResourceKindV1::TcpListener {
                    TransportKind::Tcp
                } else {
                    TransportKind::Udp
                };
                listeners.push((transport, socket));
            }
            ResourceKindV1::TcpStream => {
                let socket = take_socket(&mut sockets, resource)?;
                let connection_id = resource.logical_id.ok_or_else(|| {
                    RuntimeError::Config("TCP stream resource omitted connection id".to_string())
                })?;
                sessions.push((connection_id, socket));
            }
            ResourceKindV1::AdminResource if resource.arena_length > 0 => {
                if admin_metadata.is_some() {
                    return Err(RuntimeError::Config(
                        "child received multiple admin metadata resources".to_string(),
                    ));
                }
                let bytes = arena
                    .region(
                        usize::try_from(resource.arena_offset).map_err(|_| {
                            RuntimeError::Config(
                                "admin resource offset exceeds address space".to_string(),
                            )
                        })?,
                        usize::try_from(resource.arena_length).map_err(|_| {
                            RuntimeError::Config(
                                "admin resource length exceeds address space".to_string(),
                            )
                        })?,
                    )
                    .map_err(transfer_error)?;
                admin_metadata = Some(
                    serde_json::from_slice::<AdminSurfaceTransfer>(bytes)
                        .map_err(|error| RuntimeError::Config(error.to_string()))?,
                );
            }
            ResourceKindV1::AdminResource => {
                let index = resource.native_handle_index.ok_or_else(|| {
                    RuntimeError::Config("admin native resource omitted handle index".to_string())
                })?;
                let _ = admin_native_indices.insert(index);
            }
            ResourceKindV1::CoreSnapshot
            | ResourceKindV1::CoreJournal
            | ResourceKindV1::SessionDirectory
            | ResourceKindV1::SessionState
            | ResourceKindV1::RakNetState
            | ResourceKindV1::OnlineAuthKeys => {}
        }
    }
    let metadata = admin_metadata.ok_or_else(|| {
        RuntimeError::Config("child bootstrap omitted admin surface metadata".to_string())
    })?;
    let admin_surfaces = metadata
        .instances
        .into_iter()
        .map(|instance| {
            let handoff_resources = instance
                .resources
                .into_iter()
                .map(|resource| {
                    let value = match resource.value {
                        AdminSurfaceTransferValue::Bytes(bytes) => {
                            AdminSurfaceResource::Bytes(bytes)
                        }
                        AdminSurfaceTransferValue::NativeSocket {
                            handle_kind,
                            native_handle_index,
                        } => {
                            if !admin_native_indices.remove(&native_handle_index) {
                                return Err(RuntimeError::Config(format!(
                                    "admin metadata references undescribed native socket {native_handle_index}"
                                )));
                            }
                            let socket = sockets
                                .get_mut(native_handle_index as usize)
                                .and_then(Option::take)
                                .ok_or_else(|| {
                                    RuntimeError::Config(format!(
                                        "admin native socket {native_handle_index} is unavailable"
                                    ))
                                })?;
                            AdminSurfaceResource::NativeHandle {
                                handle_kind,
                                raw_handle: import_admin_socket(socket)?,
                            }
                        }
                    };
                    Ok(PausedAdminSurfaceResource {
                        name: resource.name,
                        resource: value,
                    })
                })
                .collect::<Result<Vec<_>, RuntimeError>>()?;
            Ok(PausedAdminSurfaceInstance {
                instance_id: instance.instance_id,
                resume_payload: instance.resume_payload,
                handoff_resources,
            })
        })
        .collect::<Result<Vec<_>, RuntimeError>>()?;
    if !admin_native_indices.is_empty() || sockets.iter().any(Option::is_some) {
        return Err(RuntimeError::Config(
            "child received unclaimed native socket resources".to_string(),
        ));
    }
    Ok((
        ExecutableChildSocketResources::new(listeners, sessions),
        admin_surfaces,
    ))
}

fn take_socket(
    sockets: &mut [Option<ExportedSocket>],
    resource: &ResourceDescriptorV1,
) -> Result<ExportedSocket, RuntimeError> {
    let index = resource.native_handle_index.ok_or_else(|| {
        RuntimeError::Config(format!(
            "native resource {} omitted its handle index",
            resource.resource_id
        ))
    })?;
    sockets
        .get_mut(index as usize)
        .and_then(Option::take)
        .ok_or_else(|| {
            RuntimeError::Config(format!(
                "native resource {} references unavailable handle {index}",
                resource.resource_id
            ))
        })
}

impl ParentControl {
    async fn send(&mut self, payload: Payload) -> Result<(), RuntimeError> {
        let envelope = self
            .sequence
            .next_outbound(payload)
            .map_err(transfer_error)?;
        self.transport.write(&envelope).await
    }

    async fn receive(&mut self) -> Result<RuntimeTransferEnvelopeV1, RuntimeError> {
        let envelope = self.transport.read().await?;
        self.sequence
            .accept_inbound(&envelope)
            .map_err(transfer_error)?;
        Ok(envelope)
    }

    async fn receive_hello(
        &mut self,
        expected_token: [u8; revy_runtime_transfer::AUTHENTICATION_TOKEN_BYTES],
        expected_pid: u32,
    ) -> Result<(), RuntimeError> {
        match self.receive().await?.payload {
            Some(Payload::Hello(hello))
                if hello.authentication_token == expected_token
                    && hello.process_id == expected_pid =>
            {
                Ok(())
            }
            Some(Payload::Hello(_)) => Err(RuntimeError::Config(
                "child transfer authentication or process identity did not match".to_string(),
            )),
            payload => Err(RuntimeError::Config(format!(
                "parent expected Hello, received {}",
                payload_name(payload.as_ref())
            ))),
        }
    }

    async fn expect_ready(&mut self, core: u64, directory: u64) -> Result<(), RuntimeError> {
        let envelope = tokio::time::timeout(CONTROL_TIMEOUT, self.receive())
            .await
            .map_err(|_| RuntimeError::Config("child readiness timed out".to_string()))??;
        match envelope.payload {
            Some(Payload::Ready(ready))
                if ready.acknowledged_core_revision == core
                    && ready.acknowledged_directory_revision == directory =>
            {
                Ok(())
            }
            Some(Payload::Error(error)) => Err(RuntimeError::Config(error.message)),
            payload => Err(RuntimeError::Config(format!(
                "parent expected matching Ready, received {}",
                payload_name(payload.as_ref())
            ))),
        }
    }

    async fn await_committed(&mut self, child_epoch_revision: u64) -> ChildCommitOutcome {
        match tokio::time::timeout(CONTROL_TIMEOUT, self.receive()).await {
            Ok(Ok(envelope)) => match envelope.payload {
                Some(Payload::Committed(committed))
                    if committed.child_epoch_revision == child_epoch_revision =>
                {
                    ChildCommitOutcome::Committed {
                        child_epoch_revision,
                        resume_duration_us: committed.resume_duration_us,
                    }
                }
                Some(Payload::Error(error)) => {
                    ChildCommitOutcome::Uncertain(RuntimeError::Config(format!(
                        "child rejected commit after authority transfer: {}",
                        error.message
                    )))
                }
                payload => ChildCommitOutcome::Uncertain(RuntimeError::Config(format!(
                    "child returned {} after Commit",
                    payload_name(payload.as_ref())
                ))),
            },
            Ok(Err(error)) => ChildCommitOutcome::Uncertain(error),
            Err(_) => {
                let status = self
                    .send(Payload::Status(StatusV1 {
                        payload: Some(status_v1::Payload::Query(StatusQueryV1 {
                            expected_child_epoch_revision: child_epoch_revision,
                        })),
                    }))
                    .await;
                if status.is_ok()
                    && let Ok(Ok(envelope)) =
                        tokio::time::timeout(CONTROL_TIMEOUT, self.receive()).await
                    && let Some(Payload::Status(StatusV1 {
                        payload: Some(status_v1::Payload::Committed(committed)),
                    })) = envelope.payload
                    && committed.child_epoch_revision == child_epoch_revision
                {
                    return ChildCommitOutcome::Committed {
                        child_epoch_revision,
                        resume_duration_us: committed.resume_duration_us,
                    };
                }
                ChildCommitOutcome::Uncertain(RuntimeError::Config(
                    "child commit outcome could not be reconciled".to_string(),
                ))
            }
        }
    }

    async fn finalize_report(
        &mut self,
        child_epoch_revision: u64,
        freeze_duration_us: u64,
    ) -> Result<(), RuntimeError> {
        self.send(Payload::Status(StatusV1 {
            payload: Some(status_v1::Payload::FinalizeReport(FinalizeReportV1 {
                child_epoch_revision,
                freeze_duration_us,
            })),
        }))
        .await?;
        let envelope = tokio::time::timeout(CONTROL_TIMEOUT, self.receive())
            .await
            .map_err(|_| {
                RuntimeError::Config("child cutover report publication timed out".to_string())
            })??;
        match envelope.payload {
            Some(Payload::Status(StatusV1 {
                payload: Some(status_v1::Payload::Committed(committed)),
            })) if committed.child_epoch_revision == child_epoch_revision
                && committed.report_published =>
            {
                Ok(())
            }
            payload => Err(RuntimeError::Config(format!(
                "parent expected committed report status, received {}",
                payload_name(payload.as_ref())
            ))),
        }
    }

    async fn send_native(
        &mut self,
        arena: &SharedTransferArena,
        sockets: &[ExportedSocket],
    ) -> Result<(), RuntimeError> {
        self.transport.send_native(arena, sockets).await
    }
}

impl ChildControl {
    async fn send(&mut self, payload: Payload) -> Result<(), RuntimeError> {
        let envelope = self
            .sequence
            .next_outbound(payload)
            .map_err(transfer_error)?;
        self.transport.write(&envelope).await
    }

    async fn receive(&mut self) -> Result<RuntimeTransferEnvelopeV1, RuntimeError> {
        let envelope = self.transport.read().await?;
        self.sequence
            .accept_inbound(&envelope)
            .map_err(transfer_error)?;
        Ok(envelope)
    }

    async fn receive_payload(&mut self) -> Result<Payload, RuntimeError> {
        self.receive()
            .await?
            .payload
            .ok_or_else(|| RuntimeError::Config("transfer envelope omitted payload".to_string()))
    }

    async fn receive_native(
        &mut self,
        count: usize,
        arena: &revy_runtime_transfer::TransferArenaV1,
    ) -> Result<(SharedTransferArenaReader, Vec<ExportedSocket>), RuntimeError> {
        self.transport.receive_native(count, arena).await
    }
}

#[cfg(windows)]
impl ParentTransport {
    async fn write(&mut self, envelope: &RuntimeTransferEnvelopeV1) -> Result<(), RuntimeError> {
        write_envelope(&mut self.pipe, envelope)
            .await
            .map_err(transfer_error)
    }

    async fn read(&mut self) -> Result<RuntimeTransferEnvelopeV1, RuntimeError> {
        read_envelope(&mut self.pipe).await.map_err(transfer_error)
    }

    async fn send_native(
        &mut self,
        _arena: &SharedTransferArena,
        sockets: &[ExportedSocket],
    ) -> Result<(), RuntimeError> {
        use tokio::io::AsyncWriteExt;
        for socket in sockets {
            self.pipe.write_all(&socket.encode()).await?;
        }
        self.pipe.flush().await?;
        Ok(())
    }
}

#[cfg(windows)]
impl ChildTransport {
    async fn write(&mut self, envelope: &RuntimeTransferEnvelopeV1) -> Result<(), RuntimeError> {
        write_envelope(&mut self.pipe, envelope)
            .await
            .map_err(transfer_error)
    }

    async fn read(&mut self) -> Result<RuntimeTransferEnvelopeV1, RuntimeError> {
        read_envelope(&mut self.pipe).await.map_err(transfer_error)
    }

    async fn receive_native(
        &mut self,
        count: usize,
        arena: &revy_runtime_transfer::TransferArenaV1,
    ) -> Result<(SharedTransferArenaReader, Vec<ExportedSocket>), RuntimeError> {
        use tokio::io::AsyncReadExt;
        let mut sockets = Vec::with_capacity(count);
        for _ in 0..count {
            let mut encoded = vec![0_u8; ExportedSocket::encoded_len()];
            self.pipe.read_exact(&mut encoded).await?;
            sockets.push(ExportedSocket::decode(&encoded).map_err(transfer_error)?);
        }
        let reader = SharedTransferArenaReader::open(
            &self.arena_name,
            arena.capacity,
            arena.used,
            arena.generation,
        )
        .map_err(transfer_error)?;
        Ok((reader, sockets))
    }
}

#[cfg(unix)]
impl ParentTransport {
    async fn write(&mut self, envelope: &RuntimeTransferEnvelopeV1) -> Result<(), RuntimeError> {
        write_envelope(&mut self.stream, envelope)
            .await
            .map_err(transfer_error)
    }

    async fn read(&mut self) -> Result<RuntimeTransferEnvelopeV1, RuntimeError> {
        read_envelope(&mut self.stream)
            .await
            .map_err(transfer_error)
    }

    async fn send_native(
        &mut self,
        arena: &SharedTransferArena,
        sockets: &[ExportedSocket],
    ) -> Result<(), RuntimeError> {
        let arena_descriptor = arena.duplicate_descriptor().map_err(transfer_error)?;
        let mut descriptors = vec![arena_descriptor.as_fd().as_raw_fd()];
        descriptors.extend(sockets.iter().map(|socket| socket.as_fd().as_raw_fd()));
        send_rights_chunked(&self.stream, &descriptors).await
    }
}

#[cfg(unix)]
impl ChildTransport {
    async fn write(&mut self, envelope: &RuntimeTransferEnvelopeV1) -> Result<(), RuntimeError> {
        write_envelope(&mut self.stream, envelope)
            .await
            .map_err(transfer_error)
    }

    async fn read(&mut self) -> Result<RuntimeTransferEnvelopeV1, RuntimeError> {
        read_envelope(&mut self.stream)
            .await
            .map_err(transfer_error)
    }

    async fn receive_native(
        &mut self,
        count: usize,
        arena: &revy_runtime_transfer::TransferArenaV1,
    ) -> Result<(SharedTransferArenaReader, Vec<ExportedSocket>), RuntimeError> {
        let handles = receive_rights_chunked(&self.stream, count.saturating_add(1)).await?;
        let mut handles = handles.into_iter();
        let arena_descriptor = handles.next().ok_or_else(|| {
            RuntimeError::Config("child did not receive transfer arena descriptor".to_string())
        })?;
        let reader = SharedTransferArenaReader::from_descriptor(
            UnixArenaDescriptor::from_owned(arena_descriptor),
            arena.capacity,
            arena.used,
            arena.generation,
        )
        .map_err(transfer_error)?;
        let sockets = handles.map(ExportedSocket::from_owned).collect();
        Ok((reader, sockets))
    }
}

#[cfg(windows)]
async fn spawn_child(
    executable: &str,
    transfer_id: TransferId,
    authentication_token: [u8; revy_runtime_transfer::AUTHENTICATION_TOKEN_BYTES],
    arena: &SharedTransferArena,
) -> Result<(std::process::Child, ParentControl, u32), RuntimeError> {
    let pipe_name = format!(
        r"\\.\pipe\revy-runtime-transfer-{}-{}",
        std::process::id(),
        encode_hex(&transfer_id.as_bytes())
    );
    let pipe = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)
        .map_err(|error| {
            RuntimeError::Config(format!("failed to create transfer pipe: {error}"))
        })?;
    let mut command = std::process::Command::new(executable);
    command
        .arg(CHILD_ARGUMENT)
        .env(PIPE_NAME_ENV, &pipe_name)
        .env(TRANSFER_ID_ENV, encode_hex(&transfer_id.as_bytes()))
        .env(AUTH_TOKEN_ENV, encode_hex(&authentication_token))
        .env(ARENA_NAME_ENV, arena.mapping_name().as_str());
    let child = command.spawn().map_err(|error| {
        RuntimeError::Config(format!(
            "failed to spawn transfer child `{executable}`: {error}"
        ))
    })?;
    let child_pid = child.id();
    pipe.connect().await.map_err(|error| {
        RuntimeError::Config(format!("failed to connect transfer pipe: {error}"))
    })?;
    Ok((
        child,
        ParentControl {
            transport: ParentTransport { pipe },
            sequence: TransferSequenceV1::new(transfer_id),
        },
        child_pid,
    ))
}

#[cfg(unix)]
async fn spawn_child(
    executable: &str,
    transfer_id: TransferId,
    authentication_token: [u8; revy_runtime_transfer::AUTHENTICATION_TOKEN_BYTES],
    _arena: &SharedTransferArena,
) -> Result<(std::process::Child, ParentControl, u32), RuntimeError> {
    let (parent, child) = StdUnixStream::pair()?;
    clear_cloexec(child.as_raw_fd())?;
    let mut command = std::process::Command::new(OsStr::new(executable));
    command
        .arg(CHILD_ARGUMENT)
        .env(CONTROL_FD_ENV, child.as_raw_fd().to_string())
        .env(TRANSFER_ID_ENV, encode_hex(&transfer_id.as_bytes()))
        .env(AUTH_TOKEN_ENV, encode_hex(&authentication_token));
    let child_process = command.spawn().map_err(|error| {
        RuntimeError::Config(format!(
            "failed to spawn transfer child `{executable}`: {error}"
        ))
    })?;
    let child_pid = child_process.id();
    parent.set_nonblocking(true)?;
    Ok((
        child_process,
        ParentControl {
            transport: ParentTransport {
                stream: UnixStream::from_std(parent)?,
            },
            sequence: TransferSequenceV1::new(transfer_id),
        },
        child_pid,
    ))
}

#[cfg(windows)]
async fn open_child_control() -> Result<
    (
        ChildControl,
        [u8; revy_runtime_transfer::AUTHENTICATION_TOKEN_BYTES],
        u32,
    ),
    RuntimeError,
> {
    let transfer_id = read_transfer_id()?;
    let token = read_authentication_token()?;
    let pipe_name = required_env(PIPE_NAME_ENV)?;
    let arena_name =
        WindowsArenaName::parse(required_env(ARENA_NAME_ENV)?).map_err(transfer_error)?;
    let pipe = ClientOptions::new()
        .open(&pipe_name)
        .map_err(|error| RuntimeError::Config(format!("failed to open transfer pipe: {error}")))?;
    Ok((
        ChildControl {
            transport: ChildTransport { pipe, arena_name },
            sequence: TransferSequenceV1::new(transfer_id),
        },
        token,
        std::process::id(),
    ))
}

#[cfg(unix)]
async fn open_child_control() -> Result<
    (
        ChildControl,
        [u8; revy_runtime_transfer::AUTHENTICATION_TOKEN_BYTES],
        u32,
    ),
    RuntimeError,
> {
    let transfer_id = read_transfer_id()?;
    let token = read_authentication_token()?;
    let raw_fd = required_env(CONTROL_FD_ENV)?
        .parse::<RawFd>()
        .map_err(|error| RuntimeError::Config(format!("invalid control fd: {error}")))?;
    // SAFETY: the parent passed an inherited descriptor exclusively for this child control
    // authority, and this conversion consumes it exactly once.
    let stream = unsafe { StdUnixStream::from_raw_fd(raw_fd) };
    stream.set_nonblocking(true)?;
    Ok((
        ChildControl {
            transport: ChildTransport {
                stream: UnixStream::from_std(stream)?,
            },
            sequence: TransferSequenceV1::new(transfer_id),
        },
        token,
        std::process::id(),
    ))
}

#[cfg(windows)]
fn duplicate_admin_socket(
    raw_handle: u64,
    target: SocketTransferTarget,
) -> Result<ExportedSocket, RuntimeError> {
    let raw = usize::try_from(raw_handle)
        .map_err(|_| RuntimeError::Config("admin socket does not fit RawSocket".to_string()))?;
    // SAFETY: the admin surface returned a live borrowed socket authority for transfer.
    let borrowed = unsafe { BorrowedSocket::borrow_raw(raw as _) };
    ExportedSocket::duplicate(&borrowed, target).map_err(transfer_error)
}

#[cfg(unix)]
fn duplicate_admin_socket(
    raw_handle: u64,
    target: SocketTransferTarget,
) -> Result<ExportedSocket, RuntimeError> {
    let raw = i32::try_from(raw_handle)
        .map_err(|_| RuntimeError::Config("admin socket does not fit RawFd".to_string()))?;
    // SAFETY: the admin surface returned a live borrowed descriptor for transfer.
    let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(raw) };
    ExportedSocket::duplicate(&borrowed, target).map_err(transfer_error)
}

#[cfg(windows)]
fn import_admin_socket(socket: ExportedSocket) -> Result<u64, RuntimeError> {
    let socket = socket.import().map_err(transfer_error)?.into_raw_socket();
    Ok(socket as u64)
}

#[cfg(unix)]
fn import_admin_socket(socket: ExportedSocket) -> Result<u64, RuntimeError> {
    u64::try_from(socket.into_owned().into_raw_fd())
        .map_err(|_| RuntimeError::Config("imported admin fd is negative".to_string()))
}

#[cfg(unix)]
fn clear_cloexec(fd: RawFd) -> Result<(), RuntimeError> {
    // SAFETY: `fd` is a live control socket descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: the retrieved flags are valid for the same descriptor.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(unix)]
async fn send_rights_chunked(
    socket: &tokio::net::UnixStream,
    handles: &[RawFd],
) -> Result<(), RuntimeError> {
    // A successful one-byte send transfers exactly this chunk. Readiness retries only the
    // current unaccepted chunk; a terminal failure abandons this pre-commit control stream.
    for chunk in handles.chunks(MAX_FDS_PER_RIGHTS_MESSAGE) {
        socket
            .async_io(tokio::io::Interest::WRITABLE, || {
                send_rights_once(socket.as_raw_fd(), chunk)
            })
            .await?;
    }
    Ok(())
}

#[cfg(unix)]
async fn receive_rights_chunked(
    socket: &tokio::net::UnixStream,
    expected: usize,
) -> Result<Vec<OwnedFd>, RuntimeError> {
    let mut handles = Vec::with_capacity(expected);
    while handles.len() < expected {
        let remaining = expected - handles.len();
        let received = socket
            .async_io(tokio::io::Interest::READABLE, || {
                receive_rights_once(
                    socket.as_raw_fd(),
                    remaining.min(MAX_FDS_PER_RIGHTS_MESSAGE),
                )
            })
            .await?;
        if received.is_empty() {
            return Err(RuntimeError::Config(
                "control socket closed before native resources arrived".to_string(),
            ));
        }
        handles.extend(received);
    }
    Ok(handles)
}

#[cfg(unix)]
fn send_rights_once(socket: RawFd, handles: &[RawFd]) -> std::io::Result<()> {
    if handles.is_empty() {
        return Ok(());
    }
    let mut marker = [0_u8; 1];
    let mut iov = libc::iovec {
        iov_base: marker.as_mut_ptr().cast(),
        iov_len: marker.len(),
    };
    // SAFETY: the caller bounds each chunk to MAX_FDS_PER_RIGHTS_MESSAGE.
    let control_bytes = unsafe { libc::CMSG_SPACE(std::mem::size_of_val(handles) as _) as usize };
    let mut control = vec![0_usize; control_bytes.div_ceil(size_of::<usize>())];
    // SAFETY: zero is a valid empty `msghdr` before its pointer/length fields are initialized.
    let mut message: libc::msghdr = unsafe { zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control_bytes;
    // SAFETY: the control allocation is sized by `CMSG_SPACE`; the header and data region are
    // initialized before `sendmsg` observes them.
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&message);
        if header.is_null() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "failed to allocate SCM_RIGHTS header",
            ));
        }
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(handles) as _) as usize;
        ptr::copy_nonoverlapping(
            handles.as_ptr().cast::<u8>(),
            libc::CMSG_DATA(header),
            std::mem::size_of_val(handles),
        );
        message.msg_controllen = (*header).cmsg_len;
    }
    loop {
        // SAFETY: `message` points to live, aligned marker/control buffers during this call.
        let sent = unsafe { libc::sendmsg(socket, &message, libc::MSG_NOSIGNAL) };
        if sent == 1 {
            return Ok(());
        }
        if sent == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::WriteZero));
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(unix)]
fn receive_rights_once(socket: RawFd, expected: usize) -> std::io::Result<Vec<OwnedFd>> {
    let mut marker = [0_u8; 1];
    let mut iov = libc::iovec {
        iov_base: marker.as_mut_ptr().cast(),
        iov_len: marker.len(),
    };
    // SAFETY: the caller bounds the expected count to MAX_FDS_PER_RIGHTS_MESSAGE.
    let control_bytes = unsafe { libc::CMSG_SPACE((expected * size_of::<RawFd>()) as _) as usize };
    let mut control = vec![0_usize; control_bytes.div_ceil(size_of::<usize>())];
    // SAFETY: zero is a valid empty `msghdr` before its pointer/length fields are initialized.
    let mut message: libc::msghdr = unsafe { zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    let received = loop {
        message.msg_controllen = control_bytes;
        // SAFETY: `message` points to live, aligned, writable marker/control buffers.
        let received = unsafe { libc::recvmsg(socket, &mut message, libc::MSG_CMSG_CLOEXEC) };
        if received >= 0 {
            break received;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    };
    if received == 0 {
        return Ok(Vec::new());
    }
    let mut handles = Vec::new();
    // SAFETY: the kernel initialized the control-message chain within `message`.
    let mut current = unsafe { libc::CMSG_FIRSTHDR(&message) };
    while !current.is_null() {
        // SAFETY: `current` is part of the validated kernel-produced control chain.
        let header = unsafe { &*current };
        if header.cmsg_level == libc::SOL_SOCKET && header.cmsg_type == libc::SCM_RIGHTS {
            let bytes = header.cmsg_len as usize - unsafe { libc::CMSG_LEN(0) as usize };
            let count = bytes / size_of::<RawFd>();
            let descriptors = unsafe { libc::CMSG_DATA(current).cast::<RawFd>() };
            for index in 0..count {
                // SAFETY: `index` stays within the SCM_RIGHTS payload length.
                let descriptor = unsafe { *descriptors.add(index) };
                // SAFETY: every SCM_RIGHTS descriptor received by this process is independently
                // owned and is transferred exactly once into `OwnedFd`.
                handles.push(unsafe { OwnedFd::from_raw_fd(descriptor) });
            }
        }
        // SAFETY: the kernel-populated header chain and current pointer belong to `message`.
        current = unsafe { libc::CMSG_NXTHDR(&message, current) };
    }
    // First own every installed descriptor, so rejection also closes a partially delivered
    // ancillary message. The kernel discards descriptors that did not fit the control buffer.
    if message.msg_flags & libc::MSG_CTRUNC != 0 || handles.len() > expected || handles.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "native descriptor message was truncated or had an invalid descriptor count",
        ));
    }
    Ok(handles)
}

fn arena_resource(
    resource_id: u64,
    kind: ResourceKindV1,
    region: &revy_runtime_transfer::SealedArenaRegion,
) -> ResourceDescriptorV1 {
    ResourceDescriptorV1 {
        resource_id,
        kind: kind as i32,
        arena_offset: region.offset() as u64,
        arena_length: region.length() as u64,
        native_handle_index: None,
        logical_id: None,
    }
}

fn native_resource(
    resource_id: u64,
    kind: ResourceKindV1,
    native_handle_index: u32,
    logical_id: Option<u64>,
) -> ResourceDescriptorV1 {
    ResourceDescriptorV1 {
        resource_id,
        kind: kind as i32,
        arena_offset: 0,
        arena_length: 0,
        native_handle_index: Some(native_handle_index),
        logical_id,
    }
}

fn next_resource_id(next: &mut u64) -> u64 {
    let current = *next;
    *next = next
        .checked_add(1)
        .expect("resource id budget is bounded by u32");
    current
}

fn validate_executable(path: &str) -> Result<(), RuntimeError> {
    let path = Path::new(path);
    let metadata = path.metadata().map_err(|error| {
        RuntimeError::Config(format!(
            "executable `{}` is unavailable: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(RuntimeError::Config(format!(
            "executable `{}` is not a regular file",
            path.display()
        )));
    }
    Ok(())
}

fn terminate_child(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn combine_precommit(
    cause: RuntimeError,
    rollback: Result<(), RuntimeError>,
    surfaces: Result<(), RuntimeError>,
) -> RuntimeError {
    match (rollback, surfaces) {
        (Ok(()), Ok(())) => cause,
        (Err(rollback), Ok(())) => {
            RuntimeError::Config(format!("{cause}; runtime rollback failed: {rollback}"))
        }
        (Ok(()), Err(surfaces)) => RuntimeError::Config(format!(
            "{cause}; process surface rollback failed: {surfaces}"
        )),
        (Err(rollback), Err(surfaces)) => RuntimeError::Config(format!(
            "{cause}; runtime rollback failed: {rollback}; process surface rollback failed: {surfaces}"
        )),
    }
}

fn bounded_diagnostic(error: &RuntimeError) -> String {
    let message = error.to_string();
    if message.len() <= revy_runtime_transfer::MAX_DIAGNOSTIC_BYTES {
        message
    } else {
        "child commit failed; diagnostic exceeded transfer policy".to_string()
    }
}

fn unexpected(expected: &str, payload: &Payload) -> RuntimeError {
    RuntimeError::Config(format!(
        "child expected {expected}, received {}",
        payload_name(Some(payload))
    ))
}

fn payload_name(payload: Option<&Payload>) -> &'static str {
    match payload {
        Some(Payload::Hello(_)) => "Hello",
        Some(Payload::Bootstrap(_)) => "Bootstrap",
        Some(Payload::Prestage(_)) => "Prestage",
        Some(Payload::Ready(_)) => "Ready",
        Some(Payload::Commit(_)) => "Commit",
        Some(Payload::Committed(_)) => "Committed",
        Some(Payload::Status(_)) => "Status",
        Some(Payload::Abort(_)) => "Abort",
        Some(Payload::Error(_)) => "Error",
        None => "missing payload",
    }
}

fn transfer_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Config(error.to_string())
}

fn required_env(name: &str) -> Result<String, RuntimeError> {
    std::env::var(name)
        .map_err(|_| RuntimeError::Config(format!("required transfer env `{name}` is missing")))
}

fn read_transfer_id() -> Result<TransferId, RuntimeError> {
    decode_hex::<{ revy_runtime_transfer::TRANSFER_ID_BYTES }>(&required_env(TRANSFER_ID_ENV)?)
        .map(TransferId::from_bytes)
}

fn read_authentication_token()
-> Result<[u8; revy_runtime_transfer::AUTHENTICATION_TOKEN_BYTES], RuntimeError> {
    decode_hex(&required_env(AUTH_TOKEN_ENV)?)
}

fn encode_hex<const N: usize>(bytes: &[u8; N]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex<const N: usize>(encoded: &str) -> Result<[u8; N], RuntimeError> {
    if encoded.len() != N * 2 || !encoded.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(RuntimeError::Config(
            "transfer capability has invalid hex encoding".to_string(),
        ));
    }
    let mut bytes = [0_u8; N];
    for (index, output) in bytes.iter_mut().enumerate() {
        let start = index * 2;
        *output = u8::from_str_radix(&encoded[start..start + 2], 16)
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
    }
    Ok(bytes)
}

#[cfg(all(test, unix))]
mod tests {
    use super::{
        receive_rights_chunked, receive_rights_once, send_rights_chunked, send_rights_once,
    };
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, OwnedFd};
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    use tokio::net::UnixStream;

    #[tokio::test]
    async fn native_transfer_preserves_all_resources_under_backpressure()
    -> Result<(), Box<dyn std::error::Error>> {
        let (sender, mut receiver) = UnixStream::pair()?;
        let send_buffer: libc::c_int = 1_024;
        // SAFETY: the socket remains borrowed and the option points to a live c_int.
        let configured = unsafe {
            libc::setsockopt(
                sender.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&raw const send_buffer).cast(),
                size_of_val(&send_buffer) as _,
            )
        };
        assert_eq!(configured, 0);
        sender.writable().await?;
        let mut prefix_bytes = 0;
        loop {
            match sender.try_write(&[0_u8; 4_096]) {
                Ok(written) => prefix_bytes += written,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error.into()),
            }
            assert!(prefix_bytes < 1024 * 1024);
        }
        let mut resources = Vec::new();
        for index in 0_u32..1_000 {
            let (resource, mut peer) = std::os::unix::net::UnixStream::pair()?;
            peer.write_all(&index.to_le_bytes())?;
            resources.push(resource);
        }
        let descriptors = resources.iter().map(AsRawFd::as_raw_fd).collect::<Vec<_>>();
        let sending = send_rights_chunked(&sender, &descriptors);
        tokio::pin!(sending);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut sending)
                .await
                .is_err()
        );
        let (_, received) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::try_join!(sending, async {
                let mut prefix = vec![0_u8; prefix_bytes];
                receiver.read_exact(&mut prefix).await?;
                receive_rights_chunked(&receiver, resources.len()).await
            })
        })
        .await??;
        assert_eq!(received.len(), resources.len());
        for (index, descriptor) in received.into_iter().enumerate() {
            // SAFETY: F_GETFD only inspects this live, borrowed descriptor.
            let flags = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) };
            assert!(flags >= 0, "received descriptor must remain open");
            assert_ne!(flags & libc::FD_CLOEXEC, 0);
            let mut stream = std::os::unix::net::UnixStream::from(descriptor);
            let mut marker = [0_u8; 4];
            stream.read_exact(&mut marker)?;
            assert_eq!(u32::from_le_bytes(marker), u32::try_from(index)?);
        }
        Ok(())
    }

    #[tokio::test]
    async fn native_receive_waits_for_sender_and_rejects_truncated_rights()
    -> Result<(), Box<dyn std::error::Error>> {
        let (sender, receiver) = UnixStream::pair()?;
        let resource: OwnedFd = std::fs::File::open("/dev/null")?.into();
        let receiving = receive_rights_chunked(&receiver, 1);
        tokio::pin!(receiving);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut receiving)
                .await
                .is_err()
        );
        send_rights_chunked(&sender, &[resource.as_raw_fd()]).await?;
        let received = tokio::time::timeout(Duration::from_secs(5), receiving).await??;
        assert_eq!(received.len(), 1);

        send_rights_once(sender.as_raw_fd(), &[resource.as_raw_fd(); 8])?;
        let error = receiver
            .async_io(tokio::io::Interest::READABLE, || {
                receive_rights_once(receiver.as_raw_fd(), 1)
            })
            .await
            .expect_err("truncated resource transfer must not become a complete import");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        Ok(())
    }
}
