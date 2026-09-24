use prost::Message;
use std::collections::HashSet;
use std::fmt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

mod arena;
mod socket;

#[cfg(unix)]
pub use arena::UnixArenaDescriptor;
#[cfg(windows)]
pub use arena::WindowsArenaName;
pub use arena::{
    ArenaError, ArenaReservation, SealedArenaRegion, SharedTransferArena, SharedTransferArenaReader,
};
pub use socket::{ExportedSocket, SocketTransferError, SocketTransferTarget};

pub const PROTOCOL_MAJOR: u32 = 1;
pub const PROTOCOL_MINOR: u32 = 2;
pub const MAX_ENVELOPE_BYTES: usize = 16 * 1024 * 1024;
pub const TRANSFER_ID_BYTES: usize = 16;
pub const ARTIFACT_HASH_BYTES: usize = 32;
pub const AUTHENTICATION_TOKEN_BYTES: usize = 32;
pub const MAX_PLUGIN_ARTIFACTS: usize = 4_096;
pub const MAX_PLUGIN_ID_BYTES: usize = 256;
pub const MAX_RESOURCE_DESCRIPTORS: usize = 16_384;
pub const MAX_TRANSFER_ARENA_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_DIAGNOSTIC_BYTES: usize = 4_096;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransferId([u8; TRANSFER_ID_BYTES]);

impl TransferId {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; TRANSFER_ID_BYTES]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(self) -> [u8; TRANSFER_ID_BYTES] {
        self.0
    }
}

impl fmt::Debug for TransferId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

pub struct TransferSequenceV1 {
    transfer_id: TransferId,
    next_outbound: u64,
    next_inbound: u64,
}

impl TransferSequenceV1 {
    #[must_use]
    pub const fn new(transfer_id: TransferId) -> Self {
        Self {
            transfer_id,
            next_outbound: 1,
            next_inbound: 1,
        }
    }

    pub fn next_outbound(
        &mut self,
        payload: runtime_transfer_envelope_v1::Payload,
    ) -> Result<RuntimeTransferEnvelopeV1, TransferProtocolError> {
        let sequence = self.next_outbound;
        self.next_outbound = self
            .next_outbound
            .checked_add(1)
            .ok_or(TransferProtocolError::InvalidSequence)?;
        Ok(RuntimeTransferEnvelopeV1::new(
            self.transfer_id,
            sequence,
            payload,
        ))
    }

    pub fn accept_inbound(
        &mut self,
        envelope: &RuntimeTransferEnvelopeV1,
    ) -> Result<(), TransferProtocolError> {
        let transfer_id = envelope.validate()?;
        if transfer_id != self.transfer_id {
            return Err(TransferProtocolError::TransferIdMismatch);
        }
        if envelope.sequence != self.next_inbound {
            return Err(TransferProtocolError::SequenceMismatch {
                expected: self.next_inbound,
                actual: envelope.sequence,
            });
        }
        self.next_inbound = self
            .next_inbound
            .checked_add(1)
            .ok_or(TransferProtocolError::InvalidSequence)?;
        Ok(())
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct RuntimeTransferEnvelopeV1 {
    #[prost(uint32, tag = "1")]
    pub protocol_major: u32,
    #[prost(uint32, tag = "2")]
    pub protocol_minor: u32,
    #[prost(bytes = "vec", tag = "3")]
    pub transfer_id: Vec<u8>,
    #[prost(uint64, tag = "4")]
    pub sequence: u64,
    #[prost(
        oneof = "runtime_transfer_envelope_v1::Payload",
        tags = "10, 11, 12, 13, 14, 15, 16, 17, 18"
    )]
    pub payload: Option<runtime_transfer_envelope_v1::Payload>,
}

pub mod runtime_transfer_envelope_v1 {
    use super::{
        AbortV1, BootstrapV1, CommitV1, CommittedV1, ErrorV1, HelloV1, PrestageV1, ReadyV1,
        StatusV1,
    };
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Payload {
        #[prost(message, tag = "10")]
        Hello(HelloV1),
        #[prost(message, tag = "11")]
        Bootstrap(BootstrapV1),
        #[prost(message, tag = "12")]
        Prestage(PrestageV1),
        #[prost(message, tag = "13")]
        Ready(ReadyV1),
        #[prost(message, tag = "14")]
        Commit(CommitV1),
        #[prost(message, tag = "15")]
        Committed(CommittedV1),
        #[prost(message, tag = "16")]
        Status(StatusV1),
        #[prost(message, tag = "17")]
        Abort(AbortV1),
        #[prost(message, tag = "18")]
        Error(ErrorV1),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct HelloV1 {
    #[prost(bytes = "vec", tag = "1")]
    pub authentication_token: Vec<u8>,
    #[prost(uint32, tag = "2")]
    pub process_id: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct BootstrapV1 {
    #[prost(bytes = "vec", tag = "1")]
    pub validated_config_digest: Vec<u8>,
    #[prost(message, repeated, tag = "2")]
    pub plugin_artifacts: Vec<PluginArtifactV1>,
    #[prost(message, optional, tag = "3")]
    pub arena: Option<TransferArenaV1>,
    #[prost(message, repeated, tag = "4")]
    pub resources: Vec<ResourceDescriptorV1>,
    #[prost(uint32, tag = "5")]
    pub native_handle_count: u32,
    #[prost(uint64, tag = "6")]
    pub core_snapshot_revision: u64,
    #[prost(uint64, tag = "7")]
    pub persisted_core_revision: u64,
    #[prost(uint64, optional, tag = "8")]
    pub latest_dirty_core_revision: Option<u64>,
    #[prost(uint64, tag = "9")]
    pub directory_revision: u64,
    #[prost(uint64, tag = "10")]
    pub active_generation_id: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct PluginArtifactV1 {
    #[prost(string, tag = "1")]
    pub plugin_id: String,
    #[prost(bytes = "vec", tag = "2")]
    pub sha256: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TransferArenaV1 {
    #[prost(uint64, tag = "1")]
    pub capacity: u64,
    #[prost(uint64, tag = "2")]
    pub used: u64,
    #[prost(uint64, tag = "3")]
    pub generation: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct ResourceDescriptorV1 {
    #[prost(uint64, tag = "1")]
    pub resource_id: u64,
    #[prost(enumeration = "ResourceKindV1", tag = "2")]
    pub kind: i32,
    #[prost(uint64, tag = "3")]
    pub arena_offset: u64,
    #[prost(uint64, tag = "4")]
    pub arena_length: u64,
    #[prost(uint32, optional, tag = "5")]
    pub native_handle_index: Option<u32>,
    #[prost(uint64, optional, tag = "6")]
    pub logical_id: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
pub enum ResourceKindV1 {
    CoreSnapshot = 0,
    CoreJournal = 1,
    SessionDirectory = 2,
    SessionState = 3,
    TcpStream = 4,
    TcpListener = 5,
    UdpListener = 6,
    RakNetState = 7,
    AdminResource = 8,
    OnlineAuthKeys = 9,
}

#[derive(Clone, PartialEq, Message)]
pub struct PrestageV1 {
    #[prost(uint64, tag = "1")]
    pub core_revision: u64,
    #[prost(uint64, tag = "2")]
    pub directory_revision: u64,
    #[prost(message, repeated, tag = "3")]
    pub resources: Vec<ResourceDescriptorV1>,
    #[prost(uint64, tag = "4")]
    pub arena_used: u64,
    #[prost(enumeration = "PrestagePhaseV1", tag = "5")]
    pub phase: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
pub enum PrestagePhaseV1 {
    Unspecified = 0,
    Preparing = 1,
    Frozen = 2,
}

#[derive(Clone, PartialEq, Message)]
pub struct ReadyV1 {
    #[prost(uint64, tag = "1")]
    pub acknowledged_core_revision: u64,
    #[prost(uint64, tag = "2")]
    pub acknowledged_directory_revision: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct CommitV1 {
    #[prost(uint64, tag = "1")]
    pub final_core_revision: u64,
    #[prost(uint64, tag = "2")]
    pub final_directory_revision: u64,
    #[prost(uint64, tag = "3")]
    pub parent_epoch_revision: u64,
    #[prost(uint64, tag = "4")]
    pub persisted_core_revision: u64,
    #[prost(uint64, optional, tag = "5")]
    pub latest_dirty_core_revision: Option<u64>,
    #[prost(uint64, tag = "6")]
    pub stage_duration_us: u64,
    #[prost(uint64, tag = "7")]
    pub prepare_duration_us: u64,
    #[prost(uint64, tag = "8")]
    pub session_count: u64,
    #[prost(uint64, tag = "9")]
    pub java_session_count: u64,
    #[prost(uint64, tag = "10")]
    pub bedrock_session_count: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct CommittedV1 {
    #[prost(uint64, tag = "1")]
    pub child_epoch_revision: u64,
    #[prost(uint64, tag = "2")]
    pub resume_duration_us: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct StatusV1 {
    #[prost(oneof = "status_v1::Payload", tags = "1, 2, 3")]
    pub payload: Option<status_v1::Payload>,
}

pub mod status_v1 {
    use super::{CommittedStatusV1, FinalizeReportV1, StatusQueryV1};
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Payload {
        #[prost(message, tag = "1")]
        Query(StatusQueryV1),
        #[prost(message, tag = "2")]
        Committed(CommittedStatusV1),
        #[prost(message, tag = "3")]
        FinalizeReport(FinalizeReportV1),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct StatusQueryV1 {
    #[prost(uint64, tag = "1")]
    pub expected_child_epoch_revision: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct CommittedStatusV1 {
    #[prost(uint64, tag = "1")]
    pub child_epoch_revision: u64,
    #[prost(uint64, tag = "2")]
    pub resume_duration_us: u64,
    #[prost(bool, tag = "3")]
    pub report_published: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct FinalizeReportV1 {
    #[prost(uint64, tag = "1")]
    pub child_epoch_revision: u64,
    #[prost(uint64, tag = "2")]
    pub freeze_duration_us: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct AbortV1 {
    #[prost(string, tag = "1")]
    pub reason: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct ErrorV1 {
    #[prost(enumeration = "TransferErrorCodeV1", tag = "1")]
    pub code: i32,
    #[prost(string, tag = "2")]
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
pub enum TransferErrorCodeV1 {
    InvalidEnvelope = 0,
    VersionMismatch = 1,
    AuthenticationFailed = 2,
    ArtifactMismatch = 3,
    BudgetExceeded = 4,
    ResourceImportFailed = 5,
    CommitRejected = 6,
    OutcomeUncertain = 7,
}

#[derive(Debug, thiserror::Error)]
pub enum TransferProtocolError {
    #[error("transfer envelope length {actual} exceeds limit {limit}")]
    EnvelopeTooLarge { actual: usize, limit: usize },
    #[error("transfer envelope ended unexpectedly")]
    UnexpectedEnd,
    #[error("transfer protocol version {major}.{minor} is unsupported")]
    VersionMismatch { major: u32, minor: u32 },
    #[error("transfer id must contain exactly {TRANSFER_ID_BYTES} bytes")]
    InvalidTransferId,
    #[error("transfer envelope has no payload")]
    MissingPayload,
    #[error("transfer pre-stage phase tag {phase} is invalid")]
    InvalidPrestagePhase { phase: i32 },
    #[error("a frozen pre-stage cannot replace the full core snapshot")]
    FrozenCoreSnapshot,
    #[error("transfer sequence must be greater than zero")]
    InvalidSequence,
    #[error("transfer sequence {actual} did not match expected sequence {expected}")]
    SequenceMismatch { expected: u64, actual: u64 },
    #[error("transfer id did not match the active transfer session")]
    TransferIdMismatch,
    #[error("validated config digest must contain exactly {ARTIFACT_HASH_BYTES} bytes")]
    InvalidConfigDigest,
    #[error(
        "transfer authentication token must contain exactly {AUTHENTICATION_TOKEN_BYTES} bytes"
    )]
    InvalidAuthenticationToken,
    #[error("transfer process id must be non-zero")]
    InvalidProcessId,
    #[error("persisted core revision {persisted} exceeds transferred core revision {transferred}")]
    PersistedRevisionAhead { persisted: u64, transferred: u64 },
    #[error("dirty core revision {dirty} exceeds transferred core revision {transferred}")]
    DirtyRevisionAhead { dirty: u64, transferred: u64 },
    #[error("runtime epoch revision must be non-zero")]
    InvalidEpochRevision,
    #[error("active runtime generation id must be non-zero")]
    InvalidGenerationId,
    #[error("plugin artifact count {actual} exceeds limit {limit}")]
    TooManyPluginArtifacts { actual: usize, limit: usize },
    #[error("plugin artifact id length {actual} is outside 1..={limit} bytes")]
    InvalidPluginIdLength { actual: usize, limit: usize },
    #[error("plugin artifact id `{plugin_id}` occurs more than once")]
    DuplicatePluginArtifact { plugin_id: String },
    #[error("plugin artifact `{plugin_id}` has an invalid SHA-256 digest length")]
    InvalidArtifactHash { plugin_id: String },
    #[error("transfer arena is required")]
    MissingArena,
    #[error("transfer arena generation must be non-zero")]
    InvalidArenaGeneration,
    #[error("transfer arena capacity {capacity} exceeds limit {limit}")]
    ArenaCapacityExceeded { capacity: u64, limit: u64 },
    #[error("transfer arena used length {used} exceeds capacity {capacity}")]
    InvalidArenaRange { used: u64, capacity: u64 },
    #[error("resource {resource_id} range exceeds the transfer arena")]
    InvalidResourceRange { resource_id: u64 },
    #[error("resource descriptor count {actual} exceeds limit {limit}")]
    TooManyResources { actual: usize, limit: usize },
    #[error("resource id must be non-zero")]
    InvalidResourceId,
    #[error("resource id {resource_id} occurs more than once")]
    DuplicateResourceId { resource_id: u64 },
    #[error("resource {resource_id} has unknown kind tag {kind}")]
    InvalidResourceKind { resource_id: u64, kind: i32 },
    #[error(
        "resource {resource_id} refers to native handle index {index}, but only {count} handles were declared"
    )]
    InvalidNativeHandleIndex {
        resource_id: u64,
        index: u32,
        count: u32,
    },
    #[error("native handle index {index} is assigned to more than one resource")]
    DuplicateNativeHandleIndex { index: u32 },
    #[error("native socket resource {resource_id} is missing a native handle")]
    MissingNativeSocketHandle { resource_id: u64 },
    #[error("tcp stream resource {resource_id} requires a non-zero connection id")]
    MissingConnectionId { resource_id: u64 },
    #[error("resource {resource_id} carries a connection id outside a tcp stream descriptor")]
    UnexpectedConnectionId { resource_id: u64 },
    #[error("{declared} native handles were declared but only {assigned} are described")]
    UnassignedNativeHandles { declared: u32, assigned: usize },
    #[error(
        "resource {first_resource_id} overlaps resource {second_resource_id} in the transfer arena"
    )]
    OverlappingResourceRange {
        first_resource_id: u64,
        second_resource_id: u64,
    },
    #[error("transfer diagnostic length {actual} exceeds limit {limit}")]
    DiagnosticTooLong { actual: usize, limit: usize },
    #[error("transfer status message has no payload")]
    MissingStatusPayload,
    #[error("transfer error code tag {code} is invalid")]
    InvalidTransferErrorCode { code: i32 },
    #[error(
        "transfer session count {session_count} does not equal java {java_session_count} plus bedrock {bedrock_session_count}"
    )]
    InvalidSessionCount {
        session_count: u64,
        java_session_count: u64,
        bedrock_session_count: u64,
    },
    #[error("protobuf transfer envelope is invalid: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("transfer transport failed: {0}")]
    Io(#[from] std::io::Error),
}

impl RuntimeTransferEnvelopeV1 {
    pub fn new(
        transfer_id: TransferId,
        sequence: u64,
        payload: runtime_transfer_envelope_v1::Payload,
    ) -> Self {
        Self {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            transfer_id: transfer_id.as_bytes().to_vec(),
            sequence,
            payload: Some(payload),
        }
    }

    pub fn validate(&self) -> Result<TransferId, TransferProtocolError> {
        if self.protocol_major != PROTOCOL_MAJOR || self.protocol_minor != PROTOCOL_MINOR {
            return Err(TransferProtocolError::VersionMismatch {
                major: self.protocol_major,
                minor: self.protocol_minor,
            });
        }
        let transfer_id = self
            .transfer_id
            .as_slice()
            .try_into()
            .map(TransferId::from_bytes)
            .map_err(|_| TransferProtocolError::InvalidTransferId)?;
        if self.sequence == 0 {
            return Err(TransferProtocolError::InvalidSequence);
        }
        let Some(payload) = self.payload.as_ref() else {
            return Err(TransferProtocolError::MissingPayload);
        };
        match payload {
            runtime_transfer_envelope_v1::Payload::Hello(hello) => hello.validate()?,
            runtime_transfer_envelope_v1::Payload::Bootstrap(bootstrap) => bootstrap.validate()?,
            runtime_transfer_envelope_v1::Payload::Prestage(prestage) => prestage.validate()?,
            runtime_transfer_envelope_v1::Payload::Status(status) => status.validate()?,
            runtime_transfer_envelope_v1::Payload::Abort(abort) => {
                validate_diagnostic(&abort.reason)?;
            }
            runtime_transfer_envelope_v1::Payload::Error(error) => error.validate()?,
            runtime_transfer_envelope_v1::Payload::Commit(commit) => commit.validate()?,
            runtime_transfer_envelope_v1::Payload::Committed(committed) => committed.validate()?,
            runtime_transfer_envelope_v1::Payload::Ready(_) => {}
        }
        Ok(transfer_id)
    }
}

impl HelloV1 {
    fn validate(&self) -> Result<(), TransferProtocolError> {
        if self.authentication_token.len() != AUTHENTICATION_TOKEN_BYTES {
            return Err(TransferProtocolError::InvalidAuthenticationToken);
        }
        if self.process_id == 0 {
            return Err(TransferProtocolError::InvalidProcessId);
        }
        Ok(())
    }
}

impl BootstrapV1 {
    /// Validates all bounded bootstrap metadata before any resource is imported.
    ///
    /// # Errors
    ///
    /// Returns [`TransferProtocolError`] for invalid hashes, revisions, arena bounds, or resource
    /// descriptors.
    pub fn validate(&self) -> Result<(), TransferProtocolError> {
        if self.validated_config_digest.len() != ARTIFACT_HASH_BYTES {
            return Err(TransferProtocolError::InvalidConfigDigest);
        }
        if self.plugin_artifacts.len() > MAX_PLUGIN_ARTIFACTS {
            return Err(TransferProtocolError::TooManyPluginArtifacts {
                actual: self.plugin_artifacts.len(),
                limit: MAX_PLUGIN_ARTIFACTS,
            });
        }
        let mut plugin_ids = HashSet::with_capacity(self.plugin_artifacts.len());
        for artifact in &self.plugin_artifacts {
            let plugin_id_len = artifact.plugin_id.len();
            if plugin_id_len == 0 || plugin_id_len > MAX_PLUGIN_ID_BYTES {
                return Err(TransferProtocolError::InvalidPluginIdLength {
                    actual: plugin_id_len,
                    limit: MAX_PLUGIN_ID_BYTES,
                });
            }
            if !plugin_ids.insert(artifact.plugin_id.as_str()) {
                return Err(TransferProtocolError::DuplicatePluginArtifact {
                    plugin_id: artifact.plugin_id.clone(),
                });
            }
            if artifact.sha256.len() != ARTIFACT_HASH_BYTES {
                return Err(TransferProtocolError::InvalidArtifactHash {
                    plugin_id: artifact.plugin_id.clone(),
                });
            }
        }
        let arena = self
            .arena
            .as_ref()
            .ok_or(TransferProtocolError::MissingArena)?;
        validate_arena(arena)?;
        validate_resources(
            &self.resources,
            Some(arena.used),
            Some(self.native_handle_count),
        )?;
        validate_core_revisions(
            self.core_snapshot_revision,
            self.persisted_core_revision,
            self.latest_dirty_core_revision,
        )?;
        if self.active_generation_id == 0 {
            return Err(TransferProtocolError::InvalidGenerationId);
        }
        Ok(())
    }
}

impl CommitV1 {
    /// Validates the final revision and persistence relationship carried by a commit.
    ///
    /// # Errors
    ///
    /// Returns [`TransferProtocolError`] when the parent epoch is invalid or persistence metadata
    /// is ahead of the transferred core.
    pub fn validate(&self) -> Result<(), TransferProtocolError> {
        if self.parent_epoch_revision == 0 {
            return Err(TransferProtocolError::InvalidEpochRevision);
        }
        validate_core_revisions(
            self.final_core_revision,
            self.persisted_core_revision,
            self.latest_dirty_core_revision,
        )?;
        if self
            .java_session_count
            .checked_add(self.bedrock_session_count)
            != Some(self.session_count)
        {
            return Err(TransferProtocolError::InvalidSessionCount {
                session_count: self.session_count,
                java_session_count: self.java_session_count,
                bedrock_session_count: self.bedrock_session_count,
            });
        }
        Ok(())
    }
}

fn validate_core_revisions(
    transferred: u64,
    persisted: u64,
    dirty: Option<u64>,
) -> Result<(), TransferProtocolError> {
    if persisted > transferred {
        return Err(TransferProtocolError::PersistedRevisionAhead {
            persisted,
            transferred,
        });
    }
    if let Some(dirty) = dirty
        && dirty > transferred
    {
        return Err(TransferProtocolError::DirtyRevisionAhead { dirty, transferred });
    }
    Ok(())
}

impl PrestageV1 {
    /// Validates the bounded resources referenced by a pre-stage update.
    ///
    /// # Errors
    ///
    /// Returns [`TransferProtocolError`] for invalid, duplicate, or overlapping descriptors.
    pub fn validate(&self) -> Result<(), TransferProtocolError> {
        let phase = PrestagePhaseV1::try_from(self.phase)
            .map_err(|_| TransferProtocolError::InvalidPrestagePhase { phase: self.phase })?;
        if phase == PrestagePhaseV1::Unspecified {
            return Err(TransferProtocolError::InvalidPrestagePhase { phase: self.phase });
        }
        if phase == PrestagePhaseV1::Frozen
            && self
                .resources
                .iter()
                .any(|resource| resource.kind == ResourceKindV1::CoreSnapshot as i32)
        {
            return Err(TransferProtocolError::FrozenCoreSnapshot);
        }
        validate_resources(&self.resources, Some(self.arena_used), None)
    }
}

impl StatusV1 {
    fn validate(&self) -> Result<(), TransferProtocolError> {
        let epoch_revision = match self.payload.as_ref() {
            Some(status_v1::Payload::Query(query)) => query.expected_child_epoch_revision,
            Some(status_v1::Payload::Committed(committed)) => committed.child_epoch_revision,
            Some(status_v1::Payload::FinalizeReport(report)) => report.child_epoch_revision,
            None => return Err(TransferProtocolError::MissingStatusPayload),
        };
        if epoch_revision == 0 {
            return Err(TransferProtocolError::InvalidEpochRevision);
        }
        Ok(())
    }
}

impl CommittedV1 {
    fn validate(&self) -> Result<(), TransferProtocolError> {
        if self.child_epoch_revision == 0 {
            return Err(TransferProtocolError::InvalidEpochRevision);
        }
        Ok(())
    }
}

impl ErrorV1 {
    fn validate(&self) -> Result<(), TransferProtocolError> {
        TransferErrorCodeV1::try_from(self.code)
            .map_err(|_| TransferProtocolError::InvalidTransferErrorCode { code: self.code })?;
        validate_diagnostic(&self.message)
    }
}

fn validate_arena(arena: &TransferArenaV1) -> Result<(), TransferProtocolError> {
    if arena.generation == 0 {
        return Err(TransferProtocolError::InvalidArenaGeneration);
    }
    if arena.capacity > MAX_TRANSFER_ARENA_BYTES {
        return Err(TransferProtocolError::ArenaCapacityExceeded {
            capacity: arena.capacity,
            limit: MAX_TRANSFER_ARENA_BYTES,
        });
    }
    if arena.used > arena.capacity {
        return Err(TransferProtocolError::InvalidArenaRange {
            used: arena.used,
            capacity: arena.capacity,
        });
    }
    Ok(())
}

fn validate_resources(
    resources: &[ResourceDescriptorV1],
    arena_used: Option<u64>,
    native_handle_count: Option<u32>,
) -> Result<(), TransferProtocolError> {
    if resources.len() > MAX_RESOURCE_DESCRIPTORS {
        return Err(TransferProtocolError::TooManyResources {
            actual: resources.len(),
            limit: MAX_RESOURCE_DESCRIPTORS,
        });
    }
    let mut resource_ids = HashSet::with_capacity(resources.len());
    let mut native_handle_indices = HashSet::new();
    let mut ranges = Vec::new();
    for resource in resources {
        if resource.resource_id == 0 {
            return Err(TransferProtocolError::InvalidResourceId);
        }
        if !resource_ids.insert(resource.resource_id) {
            return Err(TransferProtocolError::DuplicateResourceId {
                resource_id: resource.resource_id,
            });
        }
        let kind = ResourceKindV1::try_from(resource.kind).map_err(|_| {
            TransferProtocolError::InvalidResourceKind {
                resource_id: resource.resource_id,
                kind: resource.kind,
            }
        })?;
        match kind {
            ResourceKindV1::TcpStream => {
                if resource.native_handle_index.is_none() {
                    return Err(TransferProtocolError::MissingNativeSocketHandle {
                        resource_id: resource.resource_id,
                    });
                }
                if resource
                    .logical_id
                    .is_none_or(|connection_id| connection_id == 0)
                {
                    return Err(TransferProtocolError::MissingConnectionId {
                        resource_id: resource.resource_id,
                    });
                }
            }
            ResourceKindV1::TcpListener | ResourceKindV1::UdpListener => {
                if resource.native_handle_index.is_none() {
                    return Err(TransferProtocolError::MissingNativeSocketHandle {
                        resource_id: resource.resource_id,
                    });
                }
                if resource.logical_id.is_some() {
                    return Err(TransferProtocolError::UnexpectedConnectionId {
                        resource_id: resource.resource_id,
                    });
                }
            }
            ResourceKindV1::SessionState => {
                if resource
                    .logical_id
                    .is_none_or(|connection_id| connection_id == 0)
                {
                    return Err(TransferProtocolError::MissingConnectionId {
                        resource_id: resource.resource_id,
                    });
                }
            }
            ResourceKindV1::CoreSnapshot
            | ResourceKindV1::CoreJournal
            | ResourceKindV1::SessionDirectory
            | ResourceKindV1::RakNetState
            | ResourceKindV1::AdminResource
            | ResourceKindV1::OnlineAuthKeys => {
                if resource.logical_id.is_some() {
                    return Err(TransferProtocolError::UnexpectedConnectionId {
                        resource_id: resource.resource_id,
                    });
                }
            }
        }
        let end = resource
            .arena_offset
            .checked_add(resource.arena_length)
            .ok_or(TransferProtocolError::InvalidResourceRange {
                resource_id: resource.resource_id,
            })?;
        if arena_used.is_some_and(|used| end > used) {
            return Err(TransferProtocolError::InvalidResourceRange {
                resource_id: resource.resource_id,
            });
        }
        if resource.arena_length > 0 {
            ranges.push((resource.arena_offset, end, resource.resource_id));
        }
        if let Some(index) = resource.native_handle_index {
            if native_handle_count.is_some_and(|count| index >= count) {
                return Err(TransferProtocolError::InvalidNativeHandleIndex {
                    resource_id: resource.resource_id,
                    index,
                    count: native_handle_count.unwrap_or_default(),
                });
            }
            if !native_handle_indices.insert(index) {
                return Err(TransferProtocolError::DuplicateNativeHandleIndex { index });
            }
        }
    }
    if let Some(declared) = native_handle_count
        && usize::try_from(declared).ok() != Some(native_handle_indices.len())
    {
        return Err(TransferProtocolError::UnassignedNativeHandles {
            declared,
            assigned: native_handle_indices.len(),
        });
    }
    ranges.sort_unstable_by_key(|range| range.0);
    for pair in ranges.windows(2) {
        let first = pair[0];
        let second = pair[1];
        if second.0 < first.1 {
            return Err(TransferProtocolError::OverlappingResourceRange {
                first_resource_id: first.2,
                second_resource_id: second.2,
            });
        }
    }
    Ok(())
}

fn validate_diagnostic(value: &str) -> Result<(), TransferProtocolError> {
    if value.len() > MAX_DIAGNOSTIC_BYTES {
        return Err(TransferProtocolError::DiagnosticTooLong {
            actual: value.len(),
            limit: MAX_DIAGNOSTIC_BYTES,
        });
    }
    Ok(())
}

pub async fn write_envelope<W>(
    writer: &mut W,
    envelope: &RuntimeTransferEnvelopeV1,
) -> Result<(), TransferProtocolError>
where
    W: AsyncWrite + Unpin,
{
    envelope.validate()?;
    let encoded = envelope.encode_to_vec();
    if encoded.len() > MAX_ENVELOPE_BYTES {
        return Err(TransferProtocolError::EnvelopeTooLarge {
            actual: encoded.len(),
            limit: MAX_ENVELOPE_BYTES,
        });
    }
    let length =
        u32::try_from(encoded.len()).map_err(|_| TransferProtocolError::EnvelopeTooLarge {
            actual: encoded.len(),
            limit: MAX_ENVELOPE_BYTES,
        })?;
    writer.write_all(&length.to_be_bytes()).await?;
    writer.write_all(&encoded).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_envelope<R>(
    reader: &mut R,
) -> Result<RuntimeTransferEnvelopeV1, TransferProtocolError>
where
    R: AsyncRead + Unpin,
{
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            TransferProtocolError::UnexpectedEnd
        } else {
            TransferProtocolError::Io(error)
        }
    })?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_ENVELOPE_BYTES {
        return Err(TransferProtocolError::EnvelopeTooLarge {
            actual: length,
            limit: MAX_ENVELOPE_BYTES,
        });
    }
    let mut encoded = vec![0_u8; length];
    reader.read_exact(&mut encoded).await?;
    let envelope = RuntimeTransferEnvelopeV1::decode(encoded.as_slice())?;
    envelope.validate()?;
    Ok(envelope)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_checkpoint_descriptors_require_connection_identity() {
        let mut update = PrestageV1 {
            core_revision: 5,
            directory_revision: 2,
            resources: vec![ResourceDescriptorV1 {
                resource_id: 7,
                kind: ResourceKindV1::SessionState as i32,
                arena_offset: 0,
                arena_length: 16,
                native_handle_index: None,
                logical_id: Some(42),
            }],
            arena_used: 16,
            phase: PrestagePhaseV1::Frozen as i32,
        };
        update.validate().unwrap();
        for identity in [None, Some(0)] {
            update.resources[0].logical_id = identity;
            assert!(matches!(
                update.validate(),
                Err(TransferProtocolError::MissingConnectionId { resource_id: 7 })
            ));
        }
    }

    #[test]
    fn prestage_limits_full_snapshot_replacement_to_preparing() {
        let mut update = PrestageV1 {
            core_revision: 5,
            directory_revision: 2,
            resources: vec![ResourceDescriptorV1 {
                resource_id: 1,
                kind: ResourceKindV1::CoreSnapshot as i32,
                arena_offset: 0,
                arena_length: 16,
                native_handle_index: None,
                logical_id: None,
            }],
            arena_used: 16,
            phase: PrestagePhaseV1::Preparing as i32,
        };
        update.validate().unwrap();
        update.phase = PrestagePhaseV1::Frozen as i32;
        assert!(matches!(
            update.validate(),
            Err(TransferProtocolError::FrozenCoreSnapshot)
        ));
        update.resources[0].kind = ResourceKindV1::CoreJournal as i32;
        update.validate().unwrap();
        update.phase = 31;
        assert!(matches!(
            update.validate(),
            Err(TransferProtocolError::InvalidPrestagePhase { phase: 31 })
        ));
    }

    #[test]
    fn hello_requires_the_negotiated_protocol_revision() {
        let mut envelope = RuntimeTransferEnvelopeV1::new(
            TransferId::from_bytes([1; TRANSFER_ID_BYTES]),
            1,
            runtime_transfer_envelope_v1::Payload::Hello(HelloV1 {
                authentication_token: vec![2; AUTHENTICATION_TOKEN_BYTES],
                process_id: 5,
            }),
        );
        envelope.validate().unwrap();
        envelope.protocol_minor = PROTOCOL_MINOR + 1;
        assert!(matches!(
            envelope.validate(),
            Err(TransferProtocolError::VersionMismatch { .. })
        ));
    }

    #[tokio::test]
    async fn framed_round_trip_preserves_validated_envelope() {
        let transfer_id = TransferId::from_bytes([7; TRANSFER_ID_BYTES]);
        let envelope = RuntimeTransferEnvelopeV1::new(
            transfer_id,
            1,
            runtime_transfer_envelope_v1::Payload::Ready(ReadyV1 {
                acknowledged_core_revision: 4,
                acknowledged_directory_revision: 9,
            }),
        );
        let (mut writer, mut reader) = tokio::io::duplex(1_024);
        let write = tokio::spawn(async move { write_envelope(&mut writer, &envelope).await });
        let decoded = read_envelope(&mut reader).await.unwrap();
        write.await.unwrap().unwrap();
        assert_eq!(decoded.validate().unwrap(), transfer_id);
        assert_eq!(decoded.sequence, 1);
    }

    #[test]
    fn bootstrap_rejects_resource_outside_bounded_arena() {
        let envelope = RuntimeTransferEnvelopeV1::new(
            TransferId::from_bytes([1; TRANSFER_ID_BYTES]),
            1,
            runtime_transfer_envelope_v1::Payload::Bootstrap(BootstrapV1 {
                validated_config_digest: vec![0; ARTIFACT_HASH_BYTES],
                plugin_artifacts: Vec::new(),
                arena: Some(TransferArenaV1 {
                    capacity: 32,
                    used: 16,
                    generation: 1,
                }),
                resources: vec![ResourceDescriptorV1 {
                    resource_id: 3,
                    kind: ResourceKindV1::CoreSnapshot as i32,
                    arena_offset: 12,
                    arena_length: 8,
                    native_handle_index: None,
                    logical_id: None,
                }],
                native_handle_count: 0,
                core_snapshot_revision: 7,
                persisted_core_revision: 6,
                latest_dirty_core_revision: Some(7),
                directory_revision: 3,
                active_generation_id: 1,
            }),
        );
        assert!(matches!(
            envelope.validate(),
            Err(TransferProtocolError::InvalidResourceRange { resource_id: 3 })
        ));
    }

    #[test]
    fn bootstrap_rejects_duplicate_artifact_identity() {
        let envelope = RuntimeTransferEnvelopeV1::new(
            TransferId::from_bytes([2; TRANSFER_ID_BYTES]),
            1,
            runtime_transfer_envelope_v1::Payload::Bootstrap(BootstrapV1 {
                validated_config_digest: vec![0; ARTIFACT_HASH_BYTES],
                plugin_artifacts: vec![
                    PluginArtifactV1 {
                        plugin_id: "duplicate".to_string(),
                        sha256: vec![1; ARTIFACT_HASH_BYTES],
                    },
                    PluginArtifactV1 {
                        plugin_id: "duplicate".to_string(),
                        sha256: vec![1; ARTIFACT_HASH_BYTES],
                    },
                ],
                arena: Some(TransferArenaV1 {
                    capacity: 0,
                    used: 0,
                    generation: 1,
                }),
                resources: Vec::new(),
                native_handle_count: 0,
                core_snapshot_revision: 7,
                persisted_core_revision: 6,
                latest_dirty_core_revision: Some(7),
                directory_revision: 3,
                active_generation_id: 1,
            }),
        );
        assert!(matches!(
            envelope.validate(),
            Err(TransferProtocolError::DuplicatePluginArtifact { plugin_id })
                if plugin_id == "duplicate"
        ));
    }

    #[test]
    fn commit_rejects_persistence_revision_ahead_of_final_core() {
        let envelope = RuntimeTransferEnvelopeV1::new(
            TransferId::from_bytes([5; TRANSFER_ID_BYTES]),
            1,
            runtime_transfer_envelope_v1::Payload::Commit(CommitV1 {
                final_core_revision: 8,
                final_directory_revision: 3,
                parent_epoch_revision: 4,
                persisted_core_revision: 9,
                latest_dirty_core_revision: None,
                stage_duration_us: 10,
                prepare_duration_us: 20,
                session_count: 0,
                java_session_count: 0,
                bedrock_session_count: 0,
            }),
        );
        assert!(matches!(
            envelope.validate(),
            Err(TransferProtocolError::PersistedRevisionAhead {
                persisted: 9,
                transferred: 8
            })
        ));
    }

    #[test]
    fn commit_rejects_session_mix_that_does_not_match_total() {
        let envelope = RuntimeTransferEnvelopeV1::new(
            TransferId::from_bytes([6; TRANSFER_ID_BYTES]),
            1,
            runtime_transfer_envelope_v1::Payload::Commit(CommitV1 {
                final_core_revision: 8,
                final_directory_revision: 3,
                parent_epoch_revision: 4,
                persisted_core_revision: 8,
                latest_dirty_core_revision: None,
                stage_duration_us: 10,
                prepare_duration_us: 20,
                session_count: 3,
                java_session_count: 2,
                bedrock_session_count: 2,
            }),
        );
        assert!(matches!(
            envelope.validate(),
            Err(TransferProtocolError::InvalidSessionCount {
                session_count: 3,
                java_session_count: 2,
                bedrock_session_count: 2,
            })
        ));
    }

    #[test]
    fn transfer_sequence_rejects_replay_and_cross_transfer_messages() {
        let transfer_id = TransferId::from_bytes([3; TRANSFER_ID_BYTES]);
        let mut sender = TransferSequenceV1::new(transfer_id);
        let mut receiver = TransferSequenceV1::new(transfer_id);
        let envelope = sender
            .next_outbound(runtime_transfer_envelope_v1::Payload::Ready(ReadyV1 {
                acknowledged_core_revision: 1,
                acknowledged_directory_revision: 2,
            }))
            .unwrap();
        receiver.accept_inbound(&envelope).unwrap();
        assert!(matches!(
            receiver.accept_inbound(&envelope),
            Err(TransferProtocolError::SequenceMismatch {
                expected: 2,
                actual: 1
            })
        ));

        let foreign = RuntimeTransferEnvelopeV1::new(
            TransferId::from_bytes([4; TRANSFER_ID_BYTES]),
            2,
            runtime_transfer_envelope_v1::Payload::Ready(ReadyV1 {
                acknowledged_core_revision: 1,
                acknowledged_directory_revision: 2,
            }),
        );
        assert!(matches!(
            receiver.accept_inbound(&foreign),
            Err(TransferProtocolError::TransferIdMismatch)
        ));
    }
}
