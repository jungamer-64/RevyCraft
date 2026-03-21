use mc_core::{EntityId, PlayerId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionPhase {
    Handshaking,
    Status,
    Login,
    Play,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HandshakeNextState {
    Status,
    Login,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransportKind {
    Tcp,
    Udp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WireFormatKind {
    MinecraftFramed,
    RawPacketStream,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Edition {
    Je,
    Be,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BedrockListenerDescriptor {
    pub game_version: String,
    pub raknet_version: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProtocolDescriptor {
    pub adapter_id: String,
    pub transport: TransportKind,
    pub wire_format: WireFormatKind,
    pub edition: Edition,
    pub version_name: String,
    pub protocol_number: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeIntent {
    pub edition: Edition,
    pub protocol_number: i32,
    pub server_host: String,
    pub server_port: u16,
    pub next_state: HandshakeNextState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatusRequest {
    Query,
    Ping { payload: i64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoginRequest {
    LoginStart {
        username: String,
    },
    EncryptionResponse {
        shared_secret_encrypted: Vec<u8>,
        verify_token_encrypted: Vec<u8>,
    },
    BedrockNetworkSettingsRequest {
        protocol_number: i32,
    },
    BedrockLogin {
        protocol_number: i32,
        display_name: String,
        chain_jwts: Vec<String>,
        client_data_jwt: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerListStatus {
    pub version: ProtocolDescriptor,
    pub players_online: usize,
    pub max_players: usize,
    pub description: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayEncodingContext {
    pub player_id: PlayerId,
    pub entity_id: EntityId,
}
