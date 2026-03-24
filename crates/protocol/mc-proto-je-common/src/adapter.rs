use crate::handshake::decode_handshake_frame;
use crate::login::{encode_login_success_packet, read_login_byte_array, write_login_byte_array};
use crate::status::{encode_status_pong_packet, encode_status_response_packet};
use mc_core::{
    BlockPos, BlockState, ChunkColumn, CoreCommand, CoreEvent, EntityId, InventoryContainer,
    InventorySlot, InventoryTransactionContext, InventoryWindowContents, ItemStack, PlayerId,
    PlayerSnapshot, WorldMeta,
};
use mc_proto_common::{
    ConnectionPhase, HandshakeIntent, HandshakeProbe, LoginRequest, MinecraftWireCodec,
    PlayEncodingContext, PlaySyncAdapter, ProtocolAdapter, ProtocolDescriptor, ProtocolError,
    ServerListStatus, SessionAdapter, StatusRequest, TransportKind, WireCodec,
};

pub trait JavaEditionProfile: Default + Send + Sync {
    fn adapter_id(&self) -> &'static str;
    fn descriptor(&self) -> ProtocolDescriptor;
    fn play_disconnect_packet_id(&self) -> i32;
    fn format_disconnect_reason(&self, reason: &str) -> String;
    fn encode_play_bootstrap(
        &self,
        entity_id: EntityId,
        world_meta: &WorldMeta,
        player: &PlayerSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_chunk_batch(&self, chunks: &[ChunkColumn]) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_entity_spawn(
        &self,
        entity_id: EntityId,
        player: &PlayerSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_entity_moved(
        &self,
        entity_id: EntityId,
        player: &PlayerSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_entity_despawn(&self, entity_ids: &[EntityId]) -> Result<Vec<u8>, ProtocolError>;
    fn encode_container_opened(
        &self,
        window_id: u8,
        container: InventoryContainer,
        title: &str,
    ) -> Result<Vec<u8>, ProtocolError>;
    fn encode_container_closed(&self, window_id: u8) -> Result<Vec<u8>, ProtocolError>;
    fn encode_inventory_contents(
        &self,
        window_id: u8,
        container: InventoryContainer,
        contents: &InventoryWindowContents,
    ) -> Result<Vec<u8>, ProtocolError>;
    fn encode_container_property_changed(
        &self,
        window_id: u8,
        property_id: u8,
        value: i16,
    ) -> Result<Vec<u8>, ProtocolError>;
    fn encode_inventory_slot_changed(
        &self,
        window_id: u8,
        container: InventoryContainer,
        slot: InventorySlot,
        stack: Option<&ItemStack>,
    ) -> Result<Option<Vec<u8>>, ProtocolError>;
    fn encode_inventory_transaction_processed(
        &self,
        transaction: InventoryTransactionContext,
        accepted: bool,
    ) -> Result<Vec<u8>, ProtocolError>;
    fn encode_cursor_changed(&self, stack: Option<&ItemStack>) -> Result<Vec<u8>, ProtocolError>;
    fn encode_selected_hotbar_slot_changed(&self, slot: u8) -> Result<Vec<u8>, ProtocolError>;
    fn encode_block_changed(
        &self,
        position: BlockPos,
        block: &BlockState,
    ) -> Result<Vec<u8>, ProtocolError>;
    fn encode_keep_alive_requested(&self, keep_alive_id: i32) -> Result<Vec<u8>, ProtocolError>;
    fn decode_play(
        &self,
        player_id: PlayerId,
        frame: &[u8],
    ) -> Result<Option<CoreCommand>, ProtocolError>;
}

#[derive(Default)]
pub struct JavaEditionAdapter<P> {
    codec: MinecraftWireCodec,
    profile: P,
}

impl<P: Default> JavaEditionAdapter<P> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl<P: JavaEditionProfile> HandshakeProbe for JavaEditionAdapter<P> {
    fn transport_kind(&self) -> TransportKind {
        TransportKind::Tcp
    }

    fn adapter_id(&self) -> Option<&'static str> {
        Some(self.profile.adapter_id())
    }

    fn try_route(&self, frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
        decode_handshake_frame(frame)
    }
}

impl<P: JavaEditionProfile> SessionAdapter for JavaEditionAdapter<P> {
    fn wire_codec(&self) -> &dyn WireCodec {
        &self.codec
    }

    fn decode_status(&self, frame: &[u8]) -> Result<StatusRequest, ProtocolError> {
        let mut reader = mc_proto_common::PacketReader::new(frame);
        match reader.read_varint()? {
            0x00 => Ok(StatusRequest::Query),
            0x01 => Ok(StatusRequest::Ping {
                payload: reader.read_i64()?,
            }),
            packet_id => Err(ProtocolError::UnsupportedPacket(packet_id)),
        }
    }

    fn decode_login(&self, frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
        let mut reader = mc_proto_common::PacketReader::new(frame);
        match reader.read_varint()? {
            0x00 => Ok(LoginRequest::LoginStart {
                username: reader.read_string(16)?,
            }),
            0x01 => Ok(LoginRequest::EncryptionResponse {
                shared_secret_encrypted: read_login_byte_array(&mut reader)?,
                verify_token_encrypted: read_login_byte_array(&mut reader)?,
            }),
            packet_id => Err(ProtocolError::UnsupportedPacket(packet_id)),
        }
    }

    fn encode_status_response(&self, status: &ServerListStatus) -> Result<Vec<u8>, ProtocolError> {
        encode_status_response_packet(status)
    }

    fn encode_status_pong(&self, payload: i64) -> Result<Vec<u8>, ProtocolError> {
        Ok(encode_status_pong_packet(payload))
    }

    fn encode_disconnect(
        &self,
        phase: ConnectionPhase,
        reason: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        let packet_id = match phase {
            ConnectionPhase::Login => 0x00,
            ConnectionPhase::Play => self.profile.play_disconnect_packet_id(),
            _ => {
                return Err(ProtocolError::InvalidPacket(
                    "disconnect only valid in login/play",
                ));
            }
        };
        let mut writer = mc_proto_common::PacketWriter::default();
        writer.write_varint(packet_id);
        writer.write_string(&self.profile.format_disconnect_reason(reason))?;
        Ok(writer.into_inner())
    }

    fn encode_encryption_request(
        &self,
        server_id: &str,
        public_key_der: &[u8],
        verify_token: &[u8],
    ) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = mc_proto_common::PacketWriter::default();
        writer.write_varint(0x01);
        writer.write_string(server_id)?;
        write_login_byte_array(&mut writer, public_key_der)?;
        write_login_byte_array(&mut writer, verify_token)?;
        Ok(writer.into_inner())
    }

    fn encode_network_settings(
        &self,
        _compression_threshold: u16,
    ) -> Result<Vec<u8>, ProtocolError> {
        Err(ProtocolError::InvalidPacket(
            "java edition adapters do not support bedrock network settings",
        ))
    }

    fn encode_login_success(&self, player: &PlayerSnapshot) -> Result<Vec<u8>, ProtocolError> {
        encode_login_success_packet(player)
    }
}

impl<P: JavaEditionProfile> PlaySyncAdapter for JavaEditionAdapter<P> {
    fn decode_play(
        &self,
        player_id: PlayerId,
        frame: &[u8],
    ) -> Result<Option<CoreCommand>, ProtocolError> {
        self.profile.decode_play(player_id, frame)
    }

    fn encode_play_event(
        &self,
        event: &CoreEvent,
        _context: &PlayEncodingContext,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        match event {
            CoreEvent::PlayBootstrap {
                player,
                entity_id,
                world_meta,
                ..
            } => self
                .profile
                .encode_play_bootstrap(*entity_id, world_meta, player),
            CoreEvent::ChunkBatch { chunks } => self.profile.encode_chunk_batch(chunks),
            CoreEvent::EntitySpawned { entity_id, player } => {
                self.profile.encode_entity_spawn(*entity_id, player)
            }
            CoreEvent::EntityMoved { entity_id, player } => {
                self.profile.encode_entity_moved(*entity_id, player)
            }
            CoreEvent::EntityDespawned { entity_ids } => {
                Ok(vec![self.profile.encode_entity_despawn(entity_ids)?])
            }
            CoreEvent::InventoryContents {
                window_id,
                container,
                contents,
            } => {
                Ok(vec![self.profile.encode_inventory_contents(
                    *window_id, *container, contents,
                )?])
            }
            CoreEvent::ContainerOpened {
                window_id,
                container,
                title,
            } => Ok(vec![
                self.profile
                    .encode_container_opened(*window_id, *container, title)?,
            ]),
            CoreEvent::ContainerClosed { window_id } => {
                Ok(vec![self.profile.encode_container_closed(*window_id)?])
            }
            CoreEvent::ContainerPropertyChanged {
                window_id,
                property_id,
                value,
            } => Ok(vec![self.profile.encode_container_property_changed(
                *window_id,
                *property_id,
                *value,
            )?]),
            CoreEvent::InventorySlotChanged {
                window_id,
                container,
                slot,
                stack,
            } => Ok(self
                .profile
                .encode_inventory_slot_changed(*window_id, *container, *slot, stack.as_ref())?
                .into_iter()
                .collect()),
            CoreEvent::InventoryTransactionProcessed {
                transaction,
                accepted,
            } => Ok(vec![self.profile.encode_inventory_transaction_processed(
                *transaction,
                *accepted,
            )?]),
            CoreEvent::CursorChanged { stack } => {
                Ok(vec![self.profile.encode_cursor_changed(stack.as_ref())?])
            }
            CoreEvent::SelectedHotbarSlotChanged { slot } => Ok(vec![
                self.profile.encode_selected_hotbar_slot_changed(*slot)?,
            ]),
            CoreEvent::BlockChanged { position, block } => {
                Ok(vec![self.profile.encode_block_changed(*position, block)?])
            }
            CoreEvent::KeepAliveRequested { keep_alive_id } => Ok(vec![
                self.profile.encode_keep_alive_requested(*keep_alive_id)?,
            ]),
            CoreEvent::LoginAccepted { .. } | CoreEvent::Disconnect { .. } => Err(
                ProtocolError::InvalidPacket("session event cannot be encoded as play sync"),
            ),
        }
    }
}

impl<P: JavaEditionProfile> ProtocolAdapter for JavaEditionAdapter<P> {
    fn descriptor(&self) -> ProtocolDescriptor {
        self.profile.descriptor()
    }
}
