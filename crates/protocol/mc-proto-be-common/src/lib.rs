#![allow(clippy::multiple_crate_versions)]
use base64::Engine;
use bedrockrs_proto::info::MAGIC as BEDROCK_MAGIC;
use mc_core::{
    BlockFace, BlockPos, BlockState, CoreCommand, CoreEvent, EntityId, PlayerId, PlayerSnapshot,
    WorldMeta,
};
use mc_proto_common::{
    BedrockListenerDescriptor, ConnectionPhase, Edition, HandshakeIntent, HandshakeNextState,
    HandshakeProbe, LoginRequest, PlayEncodingContext, PlaySyncAdapter, ProtocolAdapter,
    ProtocolDescriptor, ProtocolError, RawPacketStreamWireCodec, ServerListStatus, SessionAdapter,
    StatusRequest, TransportKind, WireCodec,
};
use num_traits::ToPrimitive;
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use vek::Vec3;

const RAKNET_UNCONNECTED_PING: u8 = 0x01;
const RAKNET_OPEN_CONNECTION_REQUEST_1: u8 = 0x05;
const RAKNET_OPEN_CONNECTION_REQUEST_2: u8 = 0x07;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedBedrockLogin {
    pub display_name: String,
    pub chain_jwts: Vec<String>,
    pub client_data_jwt: String,
}

#[derive(Debug, Error)]
pub enum BedrockLoginError {
    #[error("bedrock login payload too short")]
    TooShort,
    #[error("bedrock login payload length overflow")]
    LengthOverflow,
    #[error("bedrock login chain json was invalid: {0}")]
    InvalidChainJson(#[from] serde_json::Error),
    #[error("bedrock login jwt payload was invalid")]
    InvalidJwtPayload,
    #[error("bedrock login did not include any jwt chain entries")]
    MissingChain,
    #[error("bedrock login did not provide a display name")]
    MissingDisplayName,
}

#[derive(Deserialize)]
struct ChainDocument {
    chain: Vec<String>,
}

fn has_magic_at(frame: &[u8], offset: usize) -> bool {
    frame
        .get(offset..offset + BEDROCK_MAGIC.len())
        .is_some_and(|slice| slice == BEDROCK_MAGIC)
}

#[must_use]
fn detects_bedrock_datagram(frame: &[u8]) -> bool {
    let Some(packet_id) = frame.first().copied() else {
        return false;
    };
    match packet_id {
        RAKNET_UNCONNECTED_PING => frame.len() >= 25 && has_magic_at(frame, 9),
        RAKNET_OPEN_CONNECTION_REQUEST_1 | RAKNET_OPEN_CONNECTION_REQUEST_2 => {
            frame.len() >= 17 && has_magic_at(frame, 1)
        }
        _ => false,
    }
}

#[must_use]
const fn bedrock_probe_intent() -> HandshakeIntent {
    HandshakeIntent {
        edition: Edition::Be,
        protocol_number: 0,
        server_host: String::new(),
        server_port: 0,
        next_state: HandshakeNextState::Login,
    }
}

/// Parses the Bedrock login payload into the JWT chain and client data token.
///
/// # Errors
///
/// Returns an error when the payload is truncated, overflows declared lengths, contains
/// invalid JSON/JWT data, or does not provide the required Bedrock display name fields.
fn parse_bedrock_login_payload(bytes: &[u8]) -> Result<ParsedBedrockLogin, BedrockLoginError> {
    if bytes.len() < 8 {
        return Err(BedrockLoginError::TooShort);
    }
    let chain_len = u32::from_le_bytes(
        bytes[0..4]
            .try_into()
            .map_err(|_| BedrockLoginError::TooShort)?,
    );
    let chain_len = usize::try_from(chain_len).map_err(|_| BedrockLoginError::LengthOverflow)?;
    let chain_end = 4_usize
        .checked_add(chain_len)
        .ok_or(BedrockLoginError::LengthOverflow)?;
    if bytes.len() < chain_end + 4 {
        return Err(BedrockLoginError::TooShort);
    }
    let chain_json = std::str::from_utf8(&bytes[4..chain_end])
        .map_err(|_| BedrockLoginError::InvalidJwtPayload)?;
    let chain: ChainDocument = serde_json::from_str(chain_json)?;

    let token_len = u32::from_le_bytes(
        bytes[chain_end..chain_end + 4]
            .try_into()
            .map_err(|_| BedrockLoginError::TooShort)?,
    );
    let token_len = usize::try_from(token_len).map_err(|_| BedrockLoginError::LengthOverflow)?;
    let token_end = chain_end
        .checked_add(4)
        .and_then(|value| value.checked_add(token_len))
        .ok_or(BedrockLoginError::LengthOverflow)?;
    if bytes.len() < token_end {
        return Err(BedrockLoginError::TooShort);
    }
    let client_data_jwt = std::str::from_utf8(&bytes[chain_end + 4..token_end])
        .map_err(|_| BedrockLoginError::InvalidJwtPayload)?
        .to_string();

    if chain.chain.is_empty() {
        return Err(BedrockLoginError::MissingChain);
    }
    let display_name = extract_display_name(&client_data_jwt).or_else(|| {
        chain
            .chain
            .last()
            .and_then(|jwt| extract_chain_display_name(jwt).ok())
    });
    let Some(display_name) = display_name else {
        return Err(BedrockLoginError::MissingDisplayName);
    };

    Ok(ParsedBedrockLogin {
        display_name,
        chain_jwts: chain.chain,
        client_data_jwt,
    })
}

fn extract_display_name(jwt: &str) -> Option<String> {
    decode_jwt_payload(jwt).ok().and_then(|payload| {
        payload
            .get("DisplayName")
            .and_then(Value::as_str)
            .or_else(|| payload.get("ThirdPartyName").and_then(Value::as_str))
            .map(ToString::to_string)
    })
}

fn extract_chain_display_name(jwt: &str) -> Result<String, BedrockLoginError> {
    let payload = decode_jwt_payload(jwt)?;
    payload
        .get("extraData")
        .and_then(|extra| extra.get("displayName"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or(BedrockLoginError::MissingDisplayName)
}

fn decode_jwt_payload(jwt: &str) -> Result<Value, BedrockLoginError> {
    let mut parts = jwt.split('.');
    let _header = parts.next().ok_or(BedrockLoginError::InvalidJwtPayload)?;
    let payload = parts.next().ok_or(BedrockLoginError::InvalidJwtPayload)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| BedrockLoginError::InvalidJwtPayload)?;
    serde_json::from_slice(&decoded).map_err(|_| BedrockLoginError::InvalidJwtPayload)
}

#[must_use]
fn block_pos_to_network(position: BlockPos) -> bedrockrs_proto::v662::types::NetworkBlockPosition {
    bedrockrs_proto::v662::types::NetworkBlockPosition {
        x: position.x,
        y: position.y.max(0).cast_unsigned(),
        z: position.z,
    }
}

#[must_use]
fn block_pos_from_network(
    position: &bedrockrs_proto::v662::types::NetworkBlockPosition,
) -> BlockPos {
    BlockPos::new(
        position.x,
        i32::try_from(position.y).unwrap_or(i32::MAX),
        position.z,
    )
}

#[must_use]
fn vec3_to_bedrock(position: mc_core::Vec3) -> Vec3<f32> {
    Vec3::new(
        f64_to_bedrock_component(position.x),
        f64_to_bedrock_component(position.y),
        f64_to_bedrock_component(position.z),
    )
}

#[must_use]
const fn protocol_error(message: &'static str) -> ProtocolError {
    ProtocolError::InvalidPacket(message)
}

#[must_use]
const fn block_face_from_i32(face: i32) -> Option<BlockFace> {
    match face {
        0 => Some(BlockFace::Bottom),
        1 => Some(BlockFace::Top),
        2 => Some(BlockFace::North),
        3 => Some(BlockFace::South),
        4 => Some(BlockFace::West),
        5 => Some(BlockFace::East),
        _ => None,
    }
}

#[must_use]
fn bedrock_actor_id(entity_id: EntityId) -> u64 {
    u64::try_from(entity_id.0).expect("bedrock entity id should be non-negative")
}

fn f64_to_bedrock_component(value: f64) -> f32 {
    value
        .to_f32()
        .expect("bedrock position component should fit into f32")
}

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
        player_id: PlayerId,
        frame: &[u8],
    ) -> Result<Option<CoreCommand>, ProtocolError>;
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
    fn encode_block_changed_packets(
        &self,
        position: BlockPos,
        block: &BlockState,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
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
        Err(protocol_error(
            "bedrock status requests are handled by the raknet listener",
        ))
    }

    fn decode_login(&self, frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
        self.profile.decode_login_request(frame)
    }

    fn encode_status_response(&self, _status: &ServerListStatus) -> Result<Vec<u8>, ProtocolError> {
        Err(protocol_error(
            "bedrock status responses are handled by the raknet listener",
        ))
    }

    fn encode_status_pong(&self, _payload: i64) -> Result<Vec<u8>, ProtocolError> {
        Err(protocol_error(
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
        Err(protocol_error(
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
        player_id: PlayerId,
        frame: &[u8],
    ) -> Result<Option<CoreCommand>, ProtocolError> {
        self.profile.decode_play_packet(player_id, frame)
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
                .encode_play_bootstrap_packets(player, *entity_id, world_meta),
            CoreEvent::EntityMoved { entity_id, player } => {
                self.profile.encode_entity_moved_packets(*entity_id, player)
            }
            CoreEvent::BlockChanged { position, block } => {
                self.profile.encode_block_changed_packets(*position, block)
            }
            CoreEvent::KeepAliveRequested { .. }
            | CoreEvent::ChunkBatch { .. }
            | CoreEvent::EntitySpawned { .. }
            | CoreEvent::EntityDespawned { .. }
            | CoreEvent::InventoryContents { .. }
            | CoreEvent::InventorySlotChanged { .. }
            | CoreEvent::SelectedHotbarSlotChanged { .. }
            | CoreEvent::LoginAccepted { .. }
            | CoreEvent::Disconnect { .. } => Ok(Vec::new()),
        }
    }
}

impl<P: BedrockProfile> ProtocolAdapter for BedrockAdapter<P> {
    fn descriptor(&self) -> ProtocolDescriptor {
        self.profile.descriptor()
    }

    fn bedrock_listener_descriptor(&self) -> Option<BedrockListenerDescriptor> {
        Some(self.profile.listener_descriptor())
    }
}

#[doc(hidden)]
pub mod internal {
    use super::*;

    pub use super::{BedrockLoginError, ParsedBedrockLogin};

    pub const RAKNET_OPEN_CONNECTION_REQUEST_1: u8 = super::RAKNET_OPEN_CONNECTION_REQUEST_1;
    pub const RAKNET_OPEN_CONNECTION_REQUEST_2: u8 = super::RAKNET_OPEN_CONNECTION_REQUEST_2;
    pub const RAKNET_UNCONNECTED_PING: u8 = super::RAKNET_UNCONNECTED_PING;

    pub fn bedrock_actor_id(entity_id: EntityId) -> u64 {
        super::bedrock_actor_id(entity_id)
    }

    pub const fn bedrock_probe_intent() -> HandshakeIntent {
        super::bedrock_probe_intent()
    }

    pub const fn block_face_from_i32(face: i32) -> Option<BlockFace> {
        super::block_face_from_i32(face)
    }

    pub fn block_pos_from_network(
        position: &bedrockrs_proto::v662::types::NetworkBlockPosition,
    ) -> BlockPos {
        super::block_pos_from_network(position)
    }

    pub fn block_pos_to_network(
        position: BlockPos,
    ) -> bedrockrs_proto::v662::types::NetworkBlockPosition {
        super::block_pos_to_network(position)
    }

    pub fn detects_bedrock_datagram(frame: &[u8]) -> bool {
        super::detects_bedrock_datagram(frame)
    }

    pub fn parse_bedrock_login_payload(
        bytes: &[u8],
    ) -> Result<ParsedBedrockLogin, BedrockLoginError> {
        super::parse_bedrock_login_payload(bytes)
    }

    pub const fn protocol_error(message: &'static str) -> ProtocolError {
        super::protocol_error(message)
    }

    pub fn vec3_to_bedrock(position: mc_core::Vec3) -> Vec3<f32> {
        super::vec3_to_bedrock(position)
    }
}

#[cfg(test)]
mod tests {
    use super::{ParsedBedrockLogin, detects_bedrock_datagram, parse_bedrock_login_payload};
    use base64::Engine;
    use serde_json::json;

    fn test_jwt(payload: &serde_json::Value) -> String {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
        format!("{header}.{payload}.")
    }

    #[test]
    fn parses_connection_request_blob() {
        let chain = json!({
            "chain": [
                test_jwt(&json!({"extraData":{"displayName":"ChainName"}}))
            ]
        })
        .to_string();
        let client_jwt = test_jwt(&json!({"DisplayName":"ClientName"}));
        let mut bytes = Vec::new();
        bytes.extend_from_slice(
            &u32::try_from(chain.len())
                .expect("chain length should fit into u32")
                .to_le_bytes(),
        );
        bytes.extend_from_slice(chain.as_bytes());
        bytes.extend_from_slice(
            &u32::try_from(client_jwt.len())
                .expect("client jwt length should fit into u32")
                .to_le_bytes(),
        );
        bytes.extend_from_slice(client_jwt.as_bytes());

        let parsed = parse_bedrock_login_payload(&bytes).expect("login payload should parse");
        assert_eq!(
            parsed,
            ParsedBedrockLogin {
                display_name: "ClientName".to_string(),
                chain_jwts: vec![test_jwt(&json!({"extraData":{"displayName":"ChainName"}}))],
                client_data_jwt: client_jwt,
            }
        );
    }

    #[test]
    fn recognises_raknet_bedrock_probe() {
        let mut datagram = Vec::new();
        datagram.push(0x01);
        datagram.extend_from_slice(&123_i64.to_be_bytes());
        datagram.extend_from_slice(&bedrockrs_proto::info::MAGIC);
        datagram.extend_from_slice(&456_i64.to_be_bytes());
        assert!(detects_bedrock_datagram(&datagram));
    }
}
