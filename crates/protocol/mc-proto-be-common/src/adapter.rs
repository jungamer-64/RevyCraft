use crate::probe::{bedrock_probe_intent, detects_bedrock_datagram};
use mc_content_canonical::catalog;
use mc_proto_common::{
    BedrockListenerDescriptor, ConnectionPhase, HandshakeIntent, HandshakeProbe, LoginRequest,
    PlayEncodingContext, PlaySyncAdapter, ProtocolAdapter, ProtocolDescriptor, ProtocolError,
    ProtocolSessionSnapshot, RawPacketStreamWireCodec, ServerListStatus, SessionAdapter,
    StatusRequest, TransportKind, WireCodec,
};
use revy_voxel_core::{CoreEvent, EntityId, PlayerSnapshot, RuntimeCommand};
use revy_voxel_model::{
    BlockPos, BlockState, ChunkColumn, DroppedItemSnapshot, InventorySlot, InventoryWindowContents,
    ItemStack, WorldMeta,
};
use revy_voxel_rules::{ContainerKindId, ContainerPropertyKey};

pub trait BedrockProfile: Default + Send + Sync {
    fn adapter_id(&self) -> &'static str;
    fn descriptor(&self) -> ProtocolDescriptor;
    fn listener_descriptor(&self) -> BedrockListenerDescriptor;
    fn decode_login_request(&self, frame: &[u8]) -> Result<LoginRequest, ProtocolError>;
    fn encode_disconnect_packet(
        &self,
        phase: ConnectionPhase,
        reason: &str,
    ) -> Result<Vec<u8>, ProtocolError>;
    fn encode_network_settings_packet(
        &self,
        compression_threshold: u16,
    ) -> Result<Vec<u8>, ProtocolError>;
    fn encode_login_success_packet(
        &self,
        player: &PlayerSnapshot,
    ) -> Result<Vec<u8>, ProtocolError>;
    fn decode_play_packet(
        &self,
        session: &ProtocolSessionSnapshot,
        frame: &[u8],
    ) -> Result<Option<RuntimeCommand>, ProtocolError>;
    fn encode_play_bootstrap_packets(
        &self,
        player: &PlayerSnapshot,
        entity_id: EntityId,
        world_meta: &WorldMeta,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_entity_moved_packets(
        &self,
        entity_id: EntityId,
        player: &PlayerSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_dropped_item_spawn_packets(
        &self,
        entity_id: EntityId,
        item: &DroppedItemSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_entity_despawn_packets(
        &self,
        entity_ids: &[EntityId],
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_chunk_batch_packets(
        &self,
        chunks: &[ChunkColumn],
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_block_changed_packets(
        &self,
        position: BlockPos,
        block: &BlockState,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_block_breaking_progress_packets(
        &self,
        breaker_entity_id: EntityId,
        position: BlockPos,
        stage: Option<u8>,
        duration_ms: u64,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_inventory_contents_packets(
        &self,
        window_id: u8,
        container: &ContainerKindId,
        contents: &InventoryWindowContents,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_container_opened_packets(
        &self,
        window_id: u8,
        container: &ContainerKindId,
        title: &str,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_container_closed_packets(&self, window_id: u8)
    -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_container_property_changed_packets(
        &self,
        window_id: u8,
        property: &ContainerPropertyKey,
        value: i16,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_inventory_slot_changed_packets(
        &self,
        window_id: u8,
        container: &ContainerKindId,
        slot: InventorySlot,
        stack: Option<&ItemStack>,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_selected_hotbar_slot_changed_packets(
        &self,
        slot: u8,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;

    fn session_closed(&self, _session: &ProtocolSessionSnapshot) -> Result<(), ProtocolError> {
        Ok(())
    }

    fn observe_event(
        &self,
        _session: &ProtocolSessionSnapshot,
        _event: &CoreEvent,
    ) -> Result<(), ProtocolError> {
        Ok(())
    }

    fn export_session_state(
        &self,
        _session: &ProtocolSessionSnapshot,
    ) -> Result<Vec<u8>, ProtocolError> {
        Ok(Vec::new())
    }

    fn import_session_state(
        &self,
        _session: &ProtocolSessionSnapshot,
        _blob: &[u8],
    ) -> Result<(), ProtocolError> {
        Ok(())
    }
}

fn protocol_block<'a>(block: &'a Option<BlockState>) -> std::borrow::Cow<'a, BlockState> {
    match block {
        Some(block) => std::borrow::Cow::Borrowed(block),
        None => std::borrow::Cow::Owned(BlockState::new(catalog::AIR)),
    }
}

#[derive(Default)]
pub struct BedrockAdapter<P> {
    codec: RawPacketStreamWireCodec,
    profile: P,
}

impl<P: Default> BedrockAdapter<P> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl<P: BedrockProfile> HandshakeProbe for BedrockAdapter<P> {
    fn transport_kind(&self) -> TransportKind {
        TransportKind::Udp
    }

    fn adapter_id(&self) -> Option<&'static str> {
        Some(self.profile.adapter_id())
    }

    fn try_route(&self, frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
        if detects_bedrock_datagram(frame) {
            Ok(Some(bedrock_probe_intent()))
        } else {
            Ok(None)
        }
    }
}

impl<P: BedrockProfile> SessionAdapter for BedrockAdapter<P> {
    fn wire_codec(&self) -> &dyn WireCodec {
        &self.codec
    }

    fn decode_status(&self, _frame: &[u8]) -> Result<StatusRequest, ProtocolError> {
        Err(crate::world::protocol_error(
            "bedrock status requests are handled by the raknet listener",
        ))
    }

    fn decode_login(&self, frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
        self.profile.decode_login_request(frame)
    }

    fn encode_status_response(&self, _status: &ServerListStatus) -> Result<Vec<u8>, ProtocolError> {
        Err(crate::world::protocol_error(
            "bedrock status responses are handled by the raknet listener",
        ))
    }

    fn encode_status_pong(&self, _payload: i64) -> Result<Vec<u8>, ProtocolError> {
        Err(crate::world::protocol_error(
            "bedrock status pong is handled by the raknet listener",
        ))
    }

    fn encode_disconnect(
        &self,
        phase: ConnectionPhase,
        reason: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.profile.encode_disconnect_packet(phase, reason)
    }

    fn encode_encryption_request(
        &self,
        _server_id: &str,
        _public_key_der: &[u8],
        _verify_token: &[u8],
    ) -> Result<Vec<u8>, ProtocolError> {
        Err(crate::world::protocol_error(
            "bedrock adapters do not use java edition encryption requests",
        ))
    }

    fn encode_network_settings(
        &self,
        compression_threshold: u16,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.profile
            .encode_network_settings_packet(compression_threshold)
    }

    fn encode_login_success(&self, player: &PlayerSnapshot) -> Result<Vec<u8>, ProtocolError> {
        self.profile.encode_login_success_packet(player)
    }
}

impl<P: BedrockProfile> PlaySyncAdapter for BedrockAdapter<P> {
    fn decode_play(
        &self,
        session: &ProtocolSessionSnapshot,
        frame: &[u8],
    ) -> Result<Option<RuntimeCommand>, ProtocolError> {
        self.profile.decode_play_packet(session, frame)
    }

    fn encode_play_event(
        &self,
        event: &CoreEvent,
        session: &ProtocolSessionSnapshot,
        _context: &PlayEncodingContext,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        let frames = match event {
            CoreEvent::PlayBootstrap {
                player,
                entity_id,
                world_meta,
                ..
            } => self
                .profile
                .encode_play_bootstrap_packets(player, *entity_id, world_meta),
            CoreEvent::EntityMoved { entity_id, player } => {
                self.profile.encode_entity_moved_packets(*entity_id, player)
            }
            CoreEvent::DroppedItemSpawned { entity_id, item } => self
                .profile
                .encode_dropped_item_spawn_packets(*entity_id, item),
            CoreEvent::ChunkBatch { chunks } => self.profile.encode_chunk_batch_packets(chunks),
            CoreEvent::BlockChanged { position, block } => self
                .profile
                .encode_block_changed_packets(*position, protocol_block(block).as_ref()),
            CoreEvent::BlockBreakingProgress {
                breaker_entity_id,
                position,
                stage,
                duration_ms,
            } => self.profile.encode_block_breaking_progress_packets(
                *breaker_entity_id,
                *position,
                *stage,
                *duration_ms,
            ),
            CoreEvent::InventoryContents {
                window_id,
                container,
                contents,
            } => self
                .profile
                .encode_inventory_contents_packets(*window_id, container, contents),
            CoreEvent::ContainerOpened {
                window_id,
                container,
                title,
            } => self
                .profile
                .encode_container_opened_packets(*window_id, container, title),
            CoreEvent::ContainerClosed { window_id } => {
                self.profile.encode_container_closed_packets(*window_id)
            }
            CoreEvent::ContainerPropertyChanged {
                window_id,
                property,
                value,
            } => self
                .profile
                .encode_container_property_changed_packets(*window_id, property, *value),
            CoreEvent::InventorySlotChanged {
                window_id,
                container,
                slot,
                stack,
            } => self.profile.encode_inventory_slot_changed_packets(
                *window_id,
                container,
                *slot,
                stack.as_ref(),
            ),
            CoreEvent::SelectedHotbarSlotChanged { slot } => self
                .profile
                .encode_selected_hotbar_slot_changed_packets(*slot),
            CoreEvent::EntityDespawned { entity_ids } => {
                self.profile.encode_entity_despawn_packets(entity_ids)
            }
            CoreEvent::KeepAliveRequested { .. }
            | CoreEvent::EntitySpawned { .. }
            | CoreEvent::InventoryTransactionProcessed { .. }
            | CoreEvent::CursorChanged { .. }
            | CoreEvent::LoginAccepted { .. }
            | CoreEvent::Disconnect { .. } => Ok(Vec::new()),
        }?;
        self.profile.observe_event(session, event)?;
        Ok(frames)
    }

    fn session_closed(&self, session: &ProtocolSessionSnapshot) -> Result<(), ProtocolError> {
        self.profile.session_closed(session)
    }
}

impl<P: BedrockProfile> ProtocolAdapter for BedrockAdapter<P> {
    fn descriptor(&self) -> ProtocolDescriptor {
        self.profile.descriptor()
    }

    fn bedrock_listener_descriptor(&self) -> Option<BedrockListenerDescriptor> {
        Some(self.profile.listener_descriptor())
    }

    fn export_session_state(
        &self,
        session: &ProtocolSessionSnapshot,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.profile.export_session_state(session)
    }

    fn import_session_state(
        &self,
        session: &ProtocolSessionSnapshot,
        blob: &[u8],
    ) -> Result<(), ProtocolError> {
        self.profile.import_session_state(session, blob)
    }
}
