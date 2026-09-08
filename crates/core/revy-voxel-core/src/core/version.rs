use super::{
    ActiveMiningState, DroppedItemState, EntityKind, EntityStore, GameplayEffectApplyResult,
    GameplayLoginPreview, GameplayLoginPreviewError, PlayerIdentity, PlayerSessionState,
    PlayerTransform, PlayerVitals, ServerCore, SessionStore, SystemScheduler, VersionComponent,
    WorldContainerViewers, WorldStore,
};
use crate::{
    BlockEntityState, BlockPos, BlockState, ChunkColumn, ChunkPos, CoreCommand, EntityId,
    GameplayEffectBatch, PlayerId, PlayerInventory, PlayerSnapshot, PlayerSummary, TargetedEvent,
    WorldMeta, WorldSnapshot,
};
use revy_voxel_semantic::ContentBehavior;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;

const CORE_TRANSFER_FORMAT_MAJOR: u16 = 1;
const MAX_CORE_TRANSFER_BYTES: usize = 512 * 1024 * 1024;
const CORE_DELTA_MAGIC: [u8; 8] = *b"RVCDCORE";
const CORE_DELTA_HEADER_BYTES: usize = CORE_DELTA_MAGIC.len()
    + std::mem::size_of::<u16>()
    + std::mem::size_of::<u64>() * 2
    + std::mem::size_of::<u32>();

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CoreRevision(u64);

impl CoreRevision {
    #[must_use]
    pub const fn initial() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn from_value(value: u64) -> Self {
        Self(value)
    }

    fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[derive(Debug)]
pub struct CoreVersion {
    revision: CoreRevision,
    state: ServerCore,
}

impl CoreVersion {
    #[must_use]
    pub fn initial(state: ServerCore) -> Arc<Self> {
        Arc::new(Self {
            revision: CoreRevision::initial(),
            state,
        })
    }

    #[must_use]
    pub const fn revision(&self) -> CoreRevision {
        self.revision
    }

    #[must_use]
    pub fn snapshot(&self) -> WorldSnapshot {
        self.state().snapshot()
    }

    #[must_use]
    pub fn player_summary(&self) -> PlayerSummary {
        self.state().player_summary()
    }

    #[must_use]
    pub fn session_resync_events(&self, player_id: PlayerId) -> Vec<TargetedEvent> {
        self.state().session_resync_events(player_id)
    }

    #[must_use]
    pub fn world_meta(&self) -> &WorldMeta {
        self.state().world_meta()
    }

    #[must_use]
    pub fn player_snapshot(&self, player_id: PlayerId) -> Option<PlayerSnapshot> {
        self.state().player_snapshot(player_id)
    }

    #[must_use]
    pub fn player_session_state(&self, player_id: PlayerId) -> Option<super::PlayerSessionState> {
        self.state().player_session_state(player_id)
    }

    #[must_use]
    pub fn block_state(&self, position: BlockPos) -> Option<BlockState> {
        self.state().block_state(position)
    }

    #[must_use]
    pub fn block_entity(&self, position: BlockPos) -> Option<BlockEntityState> {
        self.state().block_entity(position)
    }

    #[must_use]
    pub fn can_edit_block(&self, player_id: PlayerId, position: BlockPos) -> bool {
        self.state().can_edit_block(player_id, position)
    }

    pub fn login_preview(
        &self,
        connection_id: crate::ConnectionId,
        username: String,
        player_id: PlayerId,
        now_ms: u64,
    ) -> Result<GameplayLoginPreview, GameplayLoginPreviewError> {
        GameplayLoginPreview::new(
            self.state().fork_components(),
            connection_id,
            username,
            player_id,
            now_ms,
        )
    }

    /// Materializes a complete process-transfer snapshot of this immutable revision.
    ///
    /// This serializes the full core and must therefore run during pre-copy, before the
    /// data-plane freeze.
    ///
    /// # Errors
    ///
    /// Returns [`CoreTransferError`] when the snapshot cannot be encoded.
    pub fn prepare_process_transfer(&self) -> Result<CoreTransferSnapshot, CoreTransferError> {
        let snapshot = CoreTransferSnapshotV1::from_version(self);
        Ok(CoreTransferSnapshot {
            revision: self.revision,
            bytes: encode_transfer(&snapshot, MAX_CORE_TRANSFER_BYTES)?,
        })
    }

    /// Restores a complete immutable core revision using the candidate executable's content
    /// behavior.
    ///
    /// # Errors
    ///
    /// Returns [`CoreTransferError`] when the payload is malformed, uses an unsupported format,
    /// or violates core ownership invariants.
    pub fn import_process_transfer(
        bytes: &[u8],
        content_behavior: Arc<dyn ContentBehavior>,
    ) -> Result<Arc<Self>, CoreTransferError> {
        let snapshot: CoreTransferSnapshotV1 = decode_transfer(bytes)?;
        snapshot.into_version(content_behavior)
    }

    /// Applies the final bounded mutation delta to a previously imported pre-copy revision.
    ///
    /// Plugin callbacks are not invoked while replaying this delta; the journal contains the
    /// already validated semantic effects committed by the parent process.
    ///
    /// # Errors
    ///
    /// Returns [`CoreTransferError`] if the delta is malformed, non-adjacent, based on another
    /// revision, or conflicts with the imported state.
    pub fn apply_process_transfer_delta(
        &self,
        bytes: &[u8],
    ) -> Result<Arc<Self>, CoreTransferError> {
        let mut reader = CoreDeltaReader::new(bytes)?;
        if reader.read_array::<8>()? != CORE_DELTA_MAGIC {
            return Err(CoreTransferError::InvalidState(
                "core delta magic is invalid".to_string(),
            ));
        }
        let format_major = reader.read_u16()?;
        if format_major != CORE_TRANSFER_FORMAT_MAJOR {
            return Err(CoreTransferError::UnsupportedFormat {
                major: format_major,
            });
        }
        let base_revision = CoreRevision(reader.read_u64()?);
        let final_revision = CoreRevision(reader.read_u64()?);
        let commit_count = usize::try_from(reader.read_u32()?).map_err(|_error| {
            CoreTransferError::InvalidState("core delta commit count is invalid".to_string())
        })?;
        if commit_count > CoreTransferDelta::MAX_COMMITS {
            return Err(CoreTransferError::BudgetExceeded {
                actual: commit_count,
                limit: CoreTransferDelta::MAX_COMMITS,
            });
        }
        if base_revision != self.revision {
            return Err(CoreTransferError::InvalidState(format!(
                "core delta base revision {} does not match imported revision {}",
                base_revision.value(),
                self.revision.value()
            )));
        }
        let mut state = self.state.fork_components();
        let mut revision = self.revision;
        for _ in 0..commit_count {
            let frame_len = usize::try_from(reader.read_u32()?).map_err(|_error| {
                CoreTransferError::InvalidState("core delta frame length is invalid".to_string())
            })?;
            let commit: CoreTransferCommit = decode_transfer(reader.read_bytes(frame_len)?)?;
            if commit.base_revision != revision || commit.next_revision != revision.next() {
                return Err(CoreTransferError::InvalidState(format!(
                    "core delta commit {} -> {} is not adjacent to revision {}",
                    commit.base_revision.value(),
                    commit.next_revision.value(),
                    revision.value()
                )));
            }
            for mutation in commit.mutations {
                mutation.apply(&mut state)?;
            }
            revision = commit.next_revision;
        }
        reader.finish()?;
        if revision != final_revision {
            return Err(CoreTransferError::InvalidState(format!(
                "core delta ended at revision {} instead of declared revision {}",
                revision.value(),
                final_revision.value()
            )));
        }
        Ok(Arc::new(Self { revision, state }))
    }

    pub(super) fn state(&self) -> &ServerCore {
        &self.state
    }
}

#[derive(Clone, Debug)]
pub struct CoreTransferSnapshot {
    revision: CoreRevision,
    bytes: Vec<u8>,
}

impl CoreTransferSnapshot {
    #[must_use]
    pub const fn revision(&self) -> CoreRevision {
        self.revision
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum CoreTransferError {
    #[error("core transfer payload codec failed: {0}")]
    Codec(String),
    #[error("core transfer payload length {actual} exceeds limit {limit}")]
    BudgetExceeded { actual: usize, limit: usize },
    #[error("core transfer allocation of {requested} bytes failed")]
    Allocation { requested: usize },
    #[error("core transfer output failed: {0}")]
    Output(String),
    #[error("core transfer payload contains {remaining} trailing bytes")]
    TrailingBytes { remaining: usize },
    #[error("core transfer format major {major} is unsupported")]
    UnsupportedFormat { major: u16 },
    #[error("core transfer state is invalid: {0}")]
    InvalidState(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CoreTransferMutation {
    Command {
        command: CoreCommand,
        now_ms: u64,
    },
    BuiltinGameplayCommand {
        command: crate::GameplayCommand,
        now_ms: u64,
    },
    BuiltinTick {
        now_ms: u64,
    },
    GameplayEffects {
        now_ms: u64,
        effects: Vec<crate::GameplayEffect>,
    },
    LoginEffects {
        connection_id: crate::ConnectionId,
        username: String,
        player_id: PlayerId,
        batch: GameplayEffectBatch,
    },
    SetMaxPlayers {
        max_players: u32,
    },
    Reconfigure {
        config: crate::CoreConfig,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoreTransferCommit {
    base_revision: CoreRevision,
    next_revision: CoreRevision,
    requires_persistence: bool,
    mutations: Vec<CoreTransferMutation>,
}

impl CoreTransferCommit {
    #[must_use]
    pub const fn base_revision(&self) -> CoreRevision {
        self.base_revision
    }

    #[must_use]
    pub const fn next_revision(&self) -> CoreRevision {
        self.next_revision
    }

    #[must_use]
    pub const fn requires_persistence(&self) -> bool {
        self.requires_persistence
    }

    /// Encodes this committed semantic mutation once for the bounded process-transfer journal.
    ///
    /// # Errors
    ///
    /// Returns [`CoreTransferError`] if encoding fails or exceeds the caller's retention budget.
    /// The byte limit is enforced before growing the output, and cannot exceed the wire limit.
    pub fn encode(self, byte_limit: usize) -> Result<EncodedCoreTransferCommit, CoreTransferError> {
        let base_revision = self.base_revision;
        let next_revision = self.next_revision;
        let requires_persistence = self.requires_persistence;
        let bytes = Arc::<[u8]>::from(encode_transfer(
            &self,
            byte_limit.min(MAX_CORE_TRANSFER_BYTES),
        )?);
        Ok(EncodedCoreTransferCommit {
            base_revision,
            next_revision,
            requires_persistence,
            bytes,
        })
    }
}

/// A semantic core commit serialized before the data-plane freeze.
#[derive(Clone, Debug)]
pub struct EncodedCoreTransferCommit {
    base_revision: CoreRevision,
    next_revision: CoreRevision,
    requires_persistence: bool,
    bytes: Arc<[u8]>,
}

impl EncodedCoreTransferCommit {
    #[must_use]
    pub const fn base_revision(&self) -> CoreRevision {
        self.base_revision
    }

    #[must_use]
    pub const fn next_revision(&self) -> CoreRevision {
        self.next_revision
    }

    #[must_use]
    pub const fn requires_persistence(&self) -> bool {
        self.requires_persistence
    }

    #[must_use]
    pub fn encoded_len(&self) -> usize {
        self.bytes.len()
    }
}

#[derive(Clone, Debug)]
pub struct CoreTransferDelta {
    base_revision: CoreRevision,
    final_revision: CoreRevision,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreTransferDeltaDescriptor {
    base_revision: CoreRevision,
    final_revision: CoreRevision,
    encoded_len: usize,
}

impl CoreTransferDeltaDescriptor {
    #[must_use]
    pub const fn base_revision(self) -> CoreRevision {
        self.base_revision
    }

    #[must_use]
    pub const fn final_revision(self) -> CoreRevision {
        self.final_revision
    }

    #[must_use]
    pub const fn encoded_len(self) -> usize {
        self.encoded_len
    }
}

impl CoreTransferDelta {
    /// Maximum adjacent commits in one pre-copy update. The runtime retains at most this many
    /// entries, independently of its byte budget. This overlap window covers ongoing gameplay
    /// while a child imports a snapshot; it does not define the smaller freeze admission budget.
    pub const MAX_COMMITS: usize = 16_384;

    /// Encodes a bounded sequence of adjacent committed mutations.
    ///
    /// # Errors
    ///
    /// Returns [`CoreTransferError`] if revisions are not adjacent or the delta cannot be encoded.
    pub fn prepare(
        base_revision: CoreRevision,
        commits: Vec<CoreTransferCommit>,
    ) -> Result<Self, CoreTransferError> {
        let commits = commits
            .into_iter()
            .map(|commit| commit.encode(MAX_CORE_TRANSFER_BYTES))
            .collect::<Result<Vec<_>, _>>()?;
        Self::seal_preencoded(base_revision, commits)
    }

    /// Seals commits serialized during normal operation into the final transfer frame.
    ///
    /// This performs no semantic serialization and is suitable for the bounded final-delta
    /// portion of a data-plane freeze.
    ///
    /// # Errors
    ///
    /// Returns [`CoreTransferError`] if revisions are not adjacent, the commit count or bytes
    /// exceed policy, or the output frame cannot be allocated.
    pub fn seal_preencoded(
        base_revision: CoreRevision,
        commits: Vec<EncodedCoreTransferCommit>,
    ) -> Result<Self, CoreTransferError> {
        let descriptor = validate_preencoded(base_revision, commits.iter())?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(descriptor.encoded_len)
            .map_err(|_error| CoreTransferError::Allocation {
                requested: descriptor.encoded_len,
            })?;
        write_preencoded(descriptor, commits.iter(), &mut bytes)?;
        Ok(Self {
            base_revision,
            final_revision: descriptor.final_revision,
            bytes,
        })
    }

    /// Seals pre-encoded adjacent commits directly into caller-owned storage.
    ///
    /// The iterator is traversed once for validation and exact length calculation, then once for
    /// bounded copying. It performs no semantic serialization and no heap allocation.
    ///
    /// # Errors
    ///
    /// Returns [`CoreTransferError`] if revisions are not adjacent, a transfer budget is exceeded,
    /// or the destination rejects the complete frame.
    pub fn seal_preencoded_into<'a, I, W>(
        base_revision: CoreRevision,
        commits: I,
        writer: &mut W,
    ) -> Result<CoreTransferDeltaDescriptor, CoreTransferError>
    where
        I: Iterator<Item = &'a EncodedCoreTransferCommit> + Clone,
        W: Write,
    {
        let descriptor = validate_preencoded(base_revision, commits.clone())?;
        write_preencoded(descriptor, commits, writer)?;
        Ok(descriptor)
    }

    #[must_use]
    pub const fn base_revision(&self) -> CoreRevision {
        self.base_revision
    }

    #[must_use]
    pub const fn final_revision(&self) -> CoreRevision {
        self.final_revision
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

fn validate_preencoded<'a>(
    base_revision: CoreRevision,
    commits: impl Iterator<Item = &'a EncodedCoreTransferCommit>,
) -> Result<CoreTransferDeltaDescriptor, CoreTransferError> {
    let mut expected = base_revision;
    let mut encoded_len = CORE_DELTA_HEADER_BYTES;
    let mut commit_count = 0_usize;
    for commit in commits {
        commit_count = commit_count
            .checked_add(1)
            .ok_or(CoreTransferError::BudgetExceeded {
                actual: usize::MAX,
                limit: CoreTransferDelta::MAX_COMMITS,
            })?;
        if commit_count > CoreTransferDelta::MAX_COMMITS {
            return Err(CoreTransferError::BudgetExceeded {
                actual: commit_count,
                limit: CoreTransferDelta::MAX_COMMITS,
            });
        }
        if commit.base_revision != expected || commit.next_revision != expected.next() {
            return Err(CoreTransferError::InvalidState(format!(
                "core transfer commit {} -> {} is not adjacent to revision {}",
                commit.base_revision.value(),
                commit.next_revision.value(),
                expected.value()
            )));
        }
        expected = commit.next_revision;
        encoded_len = encoded_len
            .checked_add(std::mem::size_of::<u32>())
            .and_then(|length| length.checked_add(commit.bytes.len()))
            .ok_or(CoreTransferError::BudgetExceeded {
                actual: usize::MAX,
                limit: MAX_CORE_TRANSFER_BYTES,
            })?;
    }
    if encoded_len > MAX_CORE_TRANSFER_BYTES {
        return Err(CoreTransferError::BudgetExceeded {
            actual: encoded_len,
            limit: MAX_CORE_TRANSFER_BYTES,
        });
    }
    Ok(CoreTransferDeltaDescriptor {
        base_revision,
        final_revision: expected,
        encoded_len,
    })
}

fn write_preencoded<'a>(
    descriptor: CoreTransferDeltaDescriptor,
    commits: impl Iterator<Item = &'a EncodedCoreTransferCommit> + Clone,
    writer: &mut impl Write,
) -> Result<(), CoreTransferError> {
    let commit_count = commits.clone().count();
    let commit_count =
        u32::try_from(commit_count).map_err(|_error| CoreTransferError::BudgetExceeded {
            actual: commit_count,
            limit: CoreTransferDelta::MAX_COMMITS,
        })?;
    writer
        .write_all(&CORE_DELTA_MAGIC)
        .and_then(|()| writer.write_all(&CORE_TRANSFER_FORMAT_MAJOR.to_le_bytes()))
        .and_then(|()| writer.write_all(&descriptor.base_revision.value().to_le_bytes()))
        .and_then(|()| writer.write_all(&descriptor.final_revision.value().to_le_bytes()))
        .and_then(|()| writer.write_all(&commit_count.to_le_bytes()))
        .map_err(|error| CoreTransferError::Output(error.to_string()))?;
    for commit in commits {
        let frame_len = u32::try_from(commit.bytes.len()).map_err(|_error| {
            CoreTransferError::BudgetExceeded {
                actual: commit.bytes.len(),
                limit: u32::MAX as usize,
            }
        })?;
        writer
            .write_all(&frame_len.to_le_bytes())
            .and_then(|()| writer.write_all(&commit.bytes))
            .map_err(|error| CoreTransferError::Output(error.to_string()))?;
    }
    Ok(())
}

fn encode_transfer<T: Serialize>(
    value: &T,
    byte_limit: usize,
) -> Result<Vec<u8>, CoreTransferError> {
    let mut writer = BoundedTransferWriter::new(byte_limit);
    if let Err(error) = ciborium::ser::into_writer(value, &mut writer) {
        return match writer.failure {
            Some(TransferWriteFailure::BudgetExceeded { actual }) => {
                Err(CoreTransferError::BudgetExceeded {
                    actual,
                    limit: byte_limit,
                })
            }
            Some(TransferWriteFailure::Allocation { requested }) => {
                Err(CoreTransferError::Allocation { requested })
            }
            None => Err(CoreTransferError::Codec(error.to_string())),
        };
    }
    Ok(writer.bytes)
}

fn decode_transfer<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CoreTransferError> {
    if bytes.len() > MAX_CORE_TRANSFER_BYTES {
        return Err(CoreTransferError::BudgetExceeded {
            actual: bytes.len(),
            limit: MAX_CORE_TRANSFER_BYTES,
        });
    }
    let mut remaining = bytes;
    let value = ciborium::de::from_reader(&mut remaining)
        .map_err(|error| CoreTransferError::Codec(error.to_string()))?;
    if !remaining.is_empty() {
        return Err(CoreTransferError::TrailingBytes {
            remaining: remaining.len(),
        });
    }
    Ok(value)
}

struct BoundedTransferWriter {
    bytes: Vec<u8>,
    limit: usize,
    failure: Option<TransferWriteFailure>,
}

#[derive(Clone, Copy)]
enum TransferWriteFailure {
    BudgetExceeded { actual: usize },
    Allocation { requested: usize },
}

impl BoundedTransferWriter {
    const fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            failure: None,
        }
    }
}

impl Write for BoundedTransferWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let actual = self.bytes.len().checked_add(data.len()).ok_or_else(|| {
            self.failure = Some(TransferWriteFailure::BudgetExceeded { actual: usize::MAX });
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "core transfer length overflow",
            )
        })?;
        if actual > self.limit {
            self.failure = Some(TransferWriteFailure::BudgetExceeded { actual });
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "core transfer budget exceeded",
            ));
        }
        if self.bytes.try_reserve(data.len()).is_err() {
            self.failure = Some(TransferWriteFailure::Allocation { requested: actual });
            return Err(std::io::Error::new(
                std::io::ErrorKind::OutOfMemory,
                "core transfer allocation failed",
            ));
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct CoreDeltaReader<'a> {
    remaining: &'a [u8],
}

impl<'a> CoreDeltaReader<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, CoreTransferError> {
        if bytes.len() > MAX_CORE_TRANSFER_BYTES {
            return Err(CoreTransferError::BudgetExceeded {
                actual: bytes.len(),
                limit: MAX_CORE_TRANSFER_BYTES,
            });
        }
        if bytes.len() < CORE_DELTA_HEADER_BYTES {
            return Err(CoreTransferError::InvalidState(
                "core delta header is truncated".to_string(),
            ));
        }
        Ok(Self { remaining: bytes })
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], CoreTransferError> {
        let bytes = self.read_bytes(N)?;
        let mut value = [0_u8; N];
        value.copy_from_slice(bytes);
        Ok(value)
    }

    fn read_u16(&mut self) -> Result<u16, CoreTransferError> {
        Ok(u16::from_le_bytes(self.read_array()?))
    }

    fn read_u32(&mut self) -> Result<u32, CoreTransferError> {
        Ok(u32::from_le_bytes(self.read_array()?))
    }

    fn read_u64(&mut self) -> Result<u64, CoreTransferError> {
        Ok(u64::from_le_bytes(self.read_array()?))
    }

    fn read_bytes(&mut self, length: usize) -> Result<&'a [u8], CoreTransferError> {
        if length > self.remaining.len() {
            return Err(CoreTransferError::InvalidState(
                "core delta frame is truncated".to_string(),
            ));
        }
        let (value, remaining) = self.remaining.split_at(length);
        self.remaining = remaining;
        Ok(value)
    }

    fn finish(self) -> Result<(), CoreTransferError> {
        if self.remaining.is_empty() {
            Ok(())
        } else {
            Err(CoreTransferError::TrailingBytes {
                remaining: self.remaining.len(),
            })
        }
    }
}

impl CoreTransferMutation {
    fn apply(self, state: &mut ServerCore) -> Result<(), CoreTransferError> {
        match self {
            Self::Command { command, now_ms } => {
                let _events = state.apply_command(command, now_ms);
            }
            Self::BuiltinGameplayCommand { command, now_ms } => {
                let _events = state.apply_builtin_gameplay_command(command, now_ms);
            }
            Self::BuiltinTick { now_ms } => {
                let _events = state.tick(now_ms);
            }
            Self::GameplayEffects { batch } => {
                if state.validate_and_apply_gameplay_effects(batch)
                    == GameplayEffectApplyResult::Conflict
                {
                    return Err(CoreTransferError::InvalidState(
                        "committed gameplay effects conflicted during core delta replay"
                            .to_string(),
                    ));
                }
            }
            Self::LoginEffects {
                connection_id,
                username,
                player_id,
                batch,
            } => {
                if state.validate_and_apply_login_effects(connection_id, username, player_id, batch)
                    == GameplayEffectApplyResult::Conflict
                {
                    return Err(CoreTransferError::InvalidState(
                        "committed login effects conflicted during core delta replay".to_string(),
                    ));
                }
            }
            Self::SetMaxPlayers { max_players } => state.set_max_players(max_players),
            Self::Reconfigure { config } => state.reconfigure(config),
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreTransferSnapshotV1 {
    format_major: u16,
    revision: CoreRevision,
    world: CoreTransferWorldV1,
    entities: CoreTransferEntitiesV1,
    sessions: CoreTransferSessionsV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreTransferWorldV1 {
    config: crate::CoreConfig,
    world_meta: WorldMeta,
    chunks: BTreeMap<ChunkPos, ChunkColumn>,
    block_entities: BTreeMap<BlockPos, BlockEntityState>,
    container_viewers: BTreeMap<BlockPos, WorldContainerViewers>,
    saved_players: BTreeMap<PlayerId, PlayerSnapshot>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreTransferEntitiesV1 {
    entity_kinds: BTreeMap<EntityId, EntityKind>,
    players_by_player_id: BTreeMap<PlayerId, EntityId>,
    player_identity: BTreeMap<EntityId, PlayerIdentity>,
    player_transform: BTreeMap<EntityId, PlayerTransform>,
    player_vitals: BTreeMap<EntityId, PlayerVitals>,
    player_inventory: BTreeMap<EntityId, PlayerInventory>,
    player_selected_hotbar: BTreeMap<EntityId, u8>,
    player_active_mining: BTreeMap<EntityId, ActiveMiningState>,
    dropped_items: BTreeMap<EntityId, DroppedItemState>,
    next_entity_id: i32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreTransferSessionsV1 {
    player_sessions: BTreeMap<PlayerId, PlayerSessionState>,
    next_keep_alive_id: i32,
    keepalive_interval_ms: u64,
    keepalive_timeout_ms: u64,
}

impl CoreTransferSnapshotV1 {
    fn from_version(version: &CoreVersion) -> Self {
        let state = version.state();
        Self {
            format_major: CORE_TRANSFER_FORMAT_MAJOR,
            revision: version.revision(),
            world: CoreTransferWorldV1 {
                config: state.world.config.clone(),
                world_meta: state.world.world_meta.clone(),
                chunks: state.world.snapshot_chunks(),
                block_entities: (*state.world.block_entities).clone(),
                container_viewers: (*state.world.container_viewers).clone(),
                saved_players: (*state.world.saved_players).clone(),
            },
            entities: CoreTransferEntitiesV1 {
                entity_kinds: (*state.entities.entity_kinds).clone(),
                players_by_player_id: (*state.entities.players_by_player_id).clone(),
                player_identity: (*state.entities.player_identity).clone(),
                player_transform: (*state.entities.player_transform).clone(),
                player_vitals: (*state.entities.player_vitals).clone(),
                player_inventory: (*state.entities.player_inventory).clone(),
                player_selected_hotbar: (*state.entities.player_selected_hotbar).clone(),
                player_active_mining: (*state.entities.player_active_mining).clone(),
                dropped_items: (*state.entities.dropped_items).clone(),
                next_entity_id: state.entities.next_entity_id,
            },
            sessions: CoreTransferSessionsV1 {
                player_sessions: (*state.sessions.player_sessions).clone(),
                next_keep_alive_id: state.sessions.next_keep_alive_id,
                keepalive_interval_ms: state.sessions.keepalive_interval_ms,
                keepalive_timeout_ms: state.sessions.keepalive_timeout_ms,
            },
        }
    }

    fn into_version(
        self,
        content_behavior: Arc<dyn ContentBehavior>,
    ) -> Result<Arc<CoreVersion>, CoreTransferError> {
        if self.format_major != CORE_TRANSFER_FORMAT_MAJOR {
            return Err(CoreTransferError::UnsupportedFormat {
                major: self.format_major,
            });
        }
        self.validate()?;
        Ok(Arc::new(CoreVersion {
            revision: self.revision,
            state: ServerCore {
                content_behavior,
                world: VersionComponent::new(WorldStore {
                    config: self.world.config,
                    world_meta: self.world.world_meta,
                    chunks: VersionComponent::new(
                        self.world
                            .chunks
                            .into_iter()
                            .map(|(position, chunk)| (position, VersionComponent::new(chunk)))
                            .collect(),
                    ),
                    block_entities: VersionComponent::new(self.world.block_entities),
                    container_viewers: VersionComponent::new(self.world.container_viewers),
                    saved_players: VersionComponent::new(self.world.saved_players),
                }),
                entities: VersionComponent::new(EntityStore {
                    entity_kinds: VersionComponent::new(self.entities.entity_kinds),
                    players_by_player_id: VersionComponent::new(self.entities.players_by_player_id),
                    player_identity: VersionComponent::new(self.entities.player_identity),
                    player_transform: VersionComponent::new(self.entities.player_transform),
                    player_vitals: VersionComponent::new(self.entities.player_vitals),
                    player_inventory: VersionComponent::new(self.entities.player_inventory),
                    player_selected_hotbar: VersionComponent::new(
                        self.entities.player_selected_hotbar,
                    ),
                    player_active_mining: VersionComponent::new(self.entities.player_active_mining),
                    dropped_items: VersionComponent::new(self.entities.dropped_items),
                    next_entity_id: self.entities.next_entity_id,
                }),
                sessions: VersionComponent::new(SessionStore {
                    player_sessions: VersionComponent::new(self.sessions.player_sessions),
                    next_keep_alive_id: self.sessions.next_keep_alive_id,
                    keepalive_interval_ms: self.sessions.keepalive_interval_ms,
                    keepalive_timeout_ms: self.sessions.keepalive_timeout_ms,
                }),
                scheduler: SystemScheduler,
            },
        }))
    }

    fn validate(&self) -> Result<(), CoreTransferError> {
        for (player_id, entity_id) in &self.entities.players_by_player_id {
            if self.entities.entity_kinds.get(entity_id) != Some(&EntityKind::Player) {
                return Err(CoreTransferError::InvalidState(format!(
                    "player {player_id:?} references non-player entity {entity_id:?}"
                )));
            }
            if self
                .entities
                .player_identity
                .get(entity_id)
                .is_none_or(|identity| identity.player_id != *player_id)
            {
                return Err(CoreTransferError::InvalidState(format!(
                    "player {player_id:?} has no matching identity for entity {entity_id:?}"
                )));
            }
            if !self.entities.player_transform.contains_key(entity_id)
                || !self.entities.player_vitals.contains_key(entity_id)
                || !self.entities.player_inventory.contains_key(entity_id)
                || !self.entities.player_selected_hotbar.contains_key(entity_id)
            {
                return Err(CoreTransferError::InvalidState(format!(
                    "player entity {entity_id:?} is missing a required component"
                )));
            }
            if self
                .sessions
                .player_sessions
                .get(player_id)
                .is_none_or(|session| session.entity_id != *entity_id)
            {
                return Err(CoreTransferError::InvalidState(format!(
                    "player {player_id:?} has no matching live session"
                )));
            }
        }
        for (entity_id, identity) in &self.entities.player_identity {
            if self.entities.players_by_player_id.get(&identity.player_id) != Some(entity_id) {
                return Err(CoreTransferError::InvalidState(format!(
                    "player identity for entity {entity_id:?} is not indexed"
                )));
            }
        }
        for (player_id, session) in &self.sessions.player_sessions {
            if self.entities.players_by_player_id.get(player_id) != Some(&session.entity_id) {
                return Err(CoreTransferError::InvalidState(format!(
                    "session for player {player_id:?} is not indexed by its entity"
                )));
            }
        }
        for entity_id in self.entities.dropped_items.keys() {
            if self.entities.entity_kinds.get(entity_id) != Some(&EntityKind::DroppedItem) {
                return Err(CoreTransferError::InvalidState(format!(
                    "dropped item {entity_id:?} has no dropped-item entity kind"
                )));
            }
        }
        if self
            .entities
            .entity_kinds
            .contains_key(&EntityId(self.entities.next_entity_id))
        {
            return Err(CoreTransferError::InvalidState(
                "next entity id is already occupied".to_string(),
            ));
        }
        Ok(())
    }
}

pub struct CoreMutation {
    base_revision: CoreRevision,
    state: ServerCore,
    transfer_mutations: Vec<CoreTransferMutation>,
}

impl CoreMutation {
    #[must_use]
    pub fn from_version(base: &Arc<CoreVersion>) -> Self {
        Self {
            base_revision: base.revision,
            state: base.state.fork_components(),
            transfer_mutations: Vec::new(),
        }
    }

    pub fn apply_command(&mut self, command: CoreCommand, now_ms: u64) -> Vec<TargetedEvent> {
        let events = self.state.apply_command(command.clone(), now_ms);
        self.transfer_mutations
            .push(CoreTransferMutation::Command { command, now_ms });
        events
    }

    pub fn apply_builtin_gameplay_command(
        &mut self,
        command: crate::GameplayCommand,
        now_ms: u64,
    ) -> Vec<TargetedEvent> {
        let events = self
            .state
            .apply_builtin_gameplay_command(command.clone(), now_ms);
        self.transfer_mutations
            .push(CoreTransferMutation::BuiltinGameplayCommand { command, now_ms });
        events
    }

    pub fn tick(&mut self, now_ms: u64) -> Vec<TargetedEvent> {
        let events = self.state.tick(now_ms);
        self.transfer_mutations
            .push(CoreTransferMutation::BuiltinTick { now_ms });
        events
    }

    pub fn validate_and_apply_gameplay_effects(
        &mut self,
        batch: GameplayEffectBatch,
    ) -> GameplayEffectApplyResult {
        let has_effects = !batch.effects.is_empty();
        let result = self
            .state
            .validate_and_apply_gameplay_effects(batch.clone());
        if has_effects && result != GameplayEffectApplyResult::Conflict {
            self.transfer_mutations
                .push(CoreTransferMutation::GameplayEffects { batch });
        }
        result
    }

    pub fn validate_and_apply_login_effects(
        &mut self,
        connection_id: crate::ConnectionId,
        username: String,
        player_id: PlayerId,
        batch: GameplayEffectBatch,
    ) -> GameplayEffectApplyResult {
        let result = self.state.validate_and_apply_login_effects(
            connection_id,
            username.clone(),
            player_id,
            batch.clone(),
        );
        if result != GameplayEffectApplyResult::Conflict {
            self.transfer_mutations
                .push(CoreTransferMutation::LoginEffects {
                    connection_id,
                    username,
                    player_id,
                    batch,
                });
        }
        result
    }

    pub fn set_max_players(&mut self, max_players: u32) {
        self.state.set_max_players(max_players);
        self.transfer_mutations
            .push(CoreTransferMutation::SetMaxPlayers { max_players });
    }

    pub fn reconfigure(&mut self, config: crate::CoreConfig) {
        self.state.reconfigure(config.clone());
        self.transfer_mutations
            .push(CoreTransferMutation::Reconfigure { config });
    }

    /// Seals the mutation log and its outgoing events at one revision boundary.
    ///
    /// An applied mutation advances the revision even when it emits no events (for example,
    /// acknowledging a keepalive). Events cannot establish whether core state changed. A
    /// mutation attempt that leaves the same state may still advance the revision; only an
    /// empty mutation log retains the base revision.
    #[must_use]
    pub fn prepare(
        self,
        events: Vec<TargetedEvent>,
        requires_persistence: bool,
    ) -> PreparedCoreCommit {
        let next_revision = if self.transfer_mutations.is_empty() {
            self.base_revision
        } else {
            self.base_revision.next()
        };
        PreparedCoreCommit {
            base_revision: self.base_revision,
            next_version: Arc::new(CoreVersion {
                revision: next_revision,
                state: self.state,
            }),
            events,
            requires_persistence,
            transfer_mutations: self.transfer_mutations,
        }
    }
}

pub struct PreparedCoreCommit {
    base_revision: CoreRevision,
    next_version: Arc<CoreVersion>,
    events: Vec<TargetedEvent>,
    requires_persistence: bool,
    transfer_mutations: Vec<CoreTransferMutation>,
}

impl PreparedCoreCommit {
    #[must_use]
    pub const fn base_revision(&self) -> CoreRevision {
        self.base_revision
    }

    #[must_use]
    pub fn next_version(&self) -> &Arc<CoreVersion> {
        &self.next_version
    }

    #[must_use]
    pub fn events(&self) -> &[TargetedEvent] {
        &self.events
    }

    #[must_use]
    pub const fn requires_persistence(&self) -> bool {
        self.requires_persistence
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        Arc<CoreVersion>,
        Vec<TargetedEvent>,
        bool,
        CoreTransferCommit,
    ) {
        let transfer = CoreTransferCommit {
            base_revision: self.base_revision,
            next_revision: self.next_version.revision,
            requires_persistence: self.requires_persistence,
            mutations: self.transfer_mutations,
        };
        (
            self.next_version,
            self.events,
            self.requires_persistence,
            transfer,
        )
    }
}

#[derive(Clone)]
pub struct CoreHandoff {
    version: Arc<CoreVersion>,
}

impl CoreHandoff {
    #[must_use]
    pub fn from_committed(version: Arc<CoreVersion>) -> Self {
        Self { version }
    }

    #[must_use]
    pub fn version(&self) -> Arc<CoreVersion> {
        Arc::clone(&self.version)
    }
}
