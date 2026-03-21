use crate::runtime_ids::{
    BEDROCK_26_3_RUNTIME_ID_AIR, BEDROCK_26_3_RUNTIME_ID_BEDROCK, BEDROCK_26_3_RUNTIME_ID_BRICKS,
    BEDROCK_26_3_RUNTIME_ID_COBBLESTONE, BEDROCK_26_3_RUNTIME_ID_DIRT,
    BEDROCK_26_3_RUNTIME_ID_GLASS, BEDROCK_26_3_RUNTIME_ID_GRASS_BLOCK,
    BEDROCK_26_3_RUNTIME_ID_OAK_PLANKS, BEDROCK_26_3_RUNTIME_ID_SAND,
    BEDROCK_26_3_RUNTIME_ID_STONE, block_runtime_id,
};
use crate::{BE_26_3_PROTOCOL_NUMBER, Bedrock263Adapter};
use base64::Engine;
use bedrockrs_proto::V924;
use bedrockrs_proto::codec::encode_packets;
use bedrockrs_proto::v662::packets::{LoginPacket, RequestNetworkSettingsPacket};
use mc_core::BlockState;
use mc_proto_common::{HandshakeProbe, LoginRequest, SessionAdapter};
use serde_json::json;

fn test_jwt(payload: &serde_json::Value) -> String {
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
    let payload =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
    format!("{header}.{payload}.")
}

#[test]
fn request_network_settings_maps_to_login_request() {
    let adapter = Bedrock263Adapter::new();
    let frame = encode_packets(
        &[V924::RequestNetworkSettingsPacket(
            RequestNetworkSettingsPacket {
                client_network_version: BE_26_3_PROTOCOL_NUMBER,
            },
        )],
        None,
        None,
    )
    .expect("request should encode");
    let request = adapter.decode_login(&frame).expect("request should decode");
    assert_eq!(
        request,
        LoginRequest::BedrockNetworkSettingsRequest {
            protocol_number: BE_26_3_PROTOCOL_NUMBER
        }
    );
}

#[test]
fn login_packet_maps_to_bedrock_login_request() {
    let adapter = Bedrock263Adapter::new();
    let chain_entry = test_jwt(&json!({"extraData":{"displayName":"Builder"}}));
    let chain = json!({ "chain": [chain_entry] }).to_string();
    let client_jwt = test_jwt(&json!({"DisplayName":"Builder"}));
    let mut connection_request = Vec::new();
    let chain_len = u32::try_from(chain.len()).expect("test chain jwt should fit in u32");
    connection_request.extend_from_slice(&chain_len.to_le_bytes());
    connection_request.extend_from_slice(chain.as_bytes());
    let client_jwt_len =
        u32::try_from(client_jwt.len()).expect("test client jwt should fit in u32");
    connection_request.extend_from_slice(&client_jwt_len.to_le_bytes());
    connection_request.extend_from_slice(client_jwt.as_bytes());
    let frame = encode_packets(
        &[V924::LoginPacket(LoginPacket {
            client_network_version: BE_26_3_PROTOCOL_NUMBER,
            connection_request,
        })],
        None,
        None,
    )
    .expect("login packet should encode");
    let request = adapter.decode_login(&frame).expect("login should decode");
    match request {
        LoginRequest::BedrockLogin {
            protocol_number,
            display_name,
            ..
        } => {
            assert_eq!(protocol_number, BE_26_3_PROTOCOL_NUMBER);
            assert_eq!(display_name, "Builder");
        }
        other => panic!("unexpected request: {other:?}"),
    }
}

#[test]
fn probe_matches_raknet_datagram() {
    let adapter = Bedrock263Adapter::new();
    let mut datagram = Vec::new();
    datagram.push(0x01);
    datagram.extend_from_slice(&123_i64.to_be_bytes());
    datagram.extend_from_slice(&bedrockrs_proto::info::MAGIC);
    datagram.extend_from_slice(&456_i64.to_be_bytes());
    assert!(
        adapter
            .try_route(&datagram)
            .expect("probe should succeed")
            .is_some()
    );
}

#[test]
fn supported_block_runtime_ids_match_bedrock_1_26_0_palette() {
    assert_eq!(
        block_runtime_id(&BlockState::stone()),
        BEDROCK_26_3_RUNTIME_ID_STONE
    );
    assert_eq!(
        block_runtime_id(&BlockState::cobblestone()),
        BEDROCK_26_3_RUNTIME_ID_COBBLESTONE
    );
    assert_eq!(
        block_runtime_id(&BlockState::sand()),
        BEDROCK_26_3_RUNTIME_ID_SAND
    );
    assert_eq!(
        block_runtime_id(&BlockState::bricks()),
        BEDROCK_26_3_RUNTIME_ID_BRICKS
    );
    assert_eq!(
        block_runtime_id(&BlockState::dirt()),
        BEDROCK_26_3_RUNTIME_ID_DIRT
    );
    assert_eq!(
        block_runtime_id(&BlockState::grass_block()),
        BEDROCK_26_3_RUNTIME_ID_GRASS_BLOCK
    );
    assert_eq!(
        block_runtime_id(&BlockState::glass()),
        BEDROCK_26_3_RUNTIME_ID_GLASS
    );
    assert_eq!(
        block_runtime_id(&BlockState::air()),
        BEDROCK_26_3_RUNTIME_ID_AIR
    );
    assert_eq!(
        block_runtime_id(&BlockState::bedrock()),
        BEDROCK_26_3_RUNTIME_ID_BEDROCK
    );
    assert_eq!(
        block_runtime_id(&BlockState::oak_planks()),
        BEDROCK_26_3_RUNTIME_ID_OAK_PLANKS
    );
}
