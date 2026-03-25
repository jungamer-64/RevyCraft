use super::*;
use crate::runtime::RunningServer;
use binary_util::interfaces::{Reader, Writer};
use bedrockrs_proto::ProtoVersion;
use bedrockrs_proto::V924;
use bedrockrs_proto::compression::Compression as BedrockCompression;
use rak_rs::connection::queue::{RecvQueue, SendQueue};
use rak_rs::protocol::frame::FramePacket;
use rak_rs::protocol::packet::RakPacket;
use rak_rs::protocol::packet::offline::{
    IncompatibleProtocolVersion, OfflinePacket, OpenConnectReply, OpenConnectRequest,
    SessionInfoReply, SessionInfoRequest,
};
use rak_rs::protocol::packet::online::{
    ConnectedPing, ConnectedPong, ConnectionAccept, ConnectionRequest, Disconnect, LostConnection,
    NewConnection, OnlinePacket,
};
use rak_rs::protocol::reliability::Reliability as RakReliability;
use rak_rs::client::DEFAULT_MTU;
use rak_rs::protocol::Magic;
use rsa::rand_core::{OsRng, RngCore};
use std::sync::Arc;

pub(crate) async fn write_packet(
    stream: &mut tokio::net::TcpStream,
    codec: &MinecraftWireCodec,
    payload: &[u8],
) -> Result<(), RuntimeError> {
    let frame = codec.encode_frame(payload)?;
    stream.write_all(&frame).await?;
    Ok(())
}

pub(crate) async fn connect_tcp(addr: SocketAddr) -> Result<tokio::net::TcpStream, RuntimeError> {
    Ok(tokio::net::TcpStream::connect(addr).await?)
}

pub(crate) async fn connect_and_login_java_client(
    addr: SocketAddr,
    codec: &MinecraftWireCodec,
    protocol: TestJavaProtocol,
    username: &str,
) -> Result<(tokio::net::TcpStream, BytesMut), RuntimeError> {
    let (stream, buffer, _) = connect_and_login_java_client_until(
        addr,
        codec,
        protocol,
        username,
        TestJavaPacket::WindowItems,
    )
    .await?;
    Ok((stream, buffer))
}

pub(crate) async fn connect_and_login_java_client_until(
    addr: SocketAddr,
    codec: &MinecraftWireCodec,
    protocol: TestJavaProtocol,
    username: &str,
    wanted_packet: TestJavaPacket,
) -> Result<(tokio::net::TcpStream, BytesMut, Vec<u8>), RuntimeError> {
    let mut stream = connect_tcp(addr).await?;
    write_packet(
        &mut stream,
        codec,
        &encode_handshake(protocol.protocol_version(), 2)?,
    )
    .await?;
    write_packet(&mut stream, codec, &login_start(username)).await?;
    let mut buffer = BytesMut::new();
    let packet =
        read_until_java_packet(&mut stream, codec, &mut buffer, protocol, wanted_packet).await?;
    Ok((stream, buffer, packet))
}

pub(crate) fn listener_addr(server: &RunningServer) -> SocketAddr {
    server
        .listener_bindings()
        .iter()
        .find(|binding| binding.transport == TransportKind::Tcp)
        .expect("tcp listener binding should exist")
        .local_addr
}

pub(crate) fn udp_listener_addr(server: &RunningServer) -> SocketAddr {
    server
        .listener_bindings()
        .iter()
        .find(|binding| binding.transport == TransportKind::Udp)
        .expect("udp listener binding should exist")
        .local_addr
}

pub(crate) async fn read_packet(
    stream: &mut tokio::net::TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
) -> Result<Vec<u8>, RuntimeError> {
    loop {
        if let Some(frame) = codec.try_decode_frame(buffer)? {
            return Ok(frame);
        }
        let bytes_read = stream.read_buf(buffer).await?;
        if bytes_read == 0 {
            return Err(RuntimeError::Config("connection closed".to_string()));
        }
    }
}

pub(crate) async fn read_until_packet_id(
    stream: &mut tokio::net::TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
    wanted_packet_id: i32,
    max_attempts: usize,
) -> Result<Vec<u8>, RuntimeError> {
    let max_attempts = max_attempts.max(64);
    for _ in 0..max_attempts {
        let packet = tokio::time::timeout(
            Duration::from_millis(250),
            read_packet(stream, codec, buffer),
        )
        .await
        .map_err(|_| {
            RuntimeError::Config(format!(
                "timed out waiting for packet id 0x{wanted_packet_id:02x}"
            ))
        })??;
        if packet_id(&packet) == wanted_packet_id {
            return Ok(packet);
        }
    }
    Err(RuntimeError::Config(format!(
        "did not receive packet id 0x{wanted_packet_id:02x}"
    )))
}

pub(crate) async fn read_until_java_packet(
    stream: &mut tokio::net::TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
    protocol: TestJavaProtocol,
    packet: TestJavaPacket,
) -> Result<Vec<u8>, RuntimeError> {
    let wanted_packet_id = protocol
        .clientbound_packet_id(packet)
        .ok_or_else(|| RuntimeError::Config(format!("packet {packet:?} is unsupported")))?;
    read_until_packet_id(stream, codec, buffer, wanted_packet_id, 1).await
}

pub(crate) async fn read_until_set_slot(
    stream: &mut tokio::net::TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
    protocol: TestJavaProtocol,
    wanted_window_id: i8,
    wanted_slot: i16,
    max_attempts: usize,
) -> Result<Vec<u8>, RuntimeError> {
    let max_attempts = max_attempts.max(64);
    for _ in 0..max_attempts {
        let packet = tokio::time::timeout(
            Duration::from_millis(250),
            read_packet(stream, codec, buffer),
        )
        .await
        .map_err(|_| {
            RuntimeError::Config(format!(
                "timed out waiting for set slot window {wanted_window_id} slot {wanted_slot}"
            ))
        })??;
        if let Ok((window_id, slot, _)) = decode_set_slot(protocol, &packet)
            && window_id == wanted_window_id
            && slot == wanted_slot
        {
            return Ok(packet);
        }
    }
    Err(RuntimeError::Config(format!(
        "did not receive set slot window {wanted_window_id} slot {wanted_slot}"
    )))
}

pub(crate) async fn read_until_confirm_transaction(
    stream: &mut tokio::net::TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
    protocol: TestJavaProtocol,
    wanted_window_id: u8,
    wanted_action_number: i16,
    max_attempts: usize,
) -> Result<Vec<u8>, RuntimeError> {
    let max_attempts = max_attempts.max(64);
    for _ in 0..max_attempts {
        let packet = tokio::time::timeout(
            Duration::from_millis(250),
            read_packet(stream, codec, buffer),
        )
        .await
        .map_err(|_| {
            RuntimeError::Config(format!(
                "timed out waiting for confirm transaction window {wanted_window_id} action {wanted_action_number}"
            ))
        })??;
        if let Ok((window_id, action_number, _)) = decode_confirm_transaction(protocol, &packet)
            && window_id == wanted_window_id
            && action_number == wanted_action_number
        {
            return Ok(packet);
        }
    }
    Err(RuntimeError::Config(format!(
        "did not receive confirm transaction window {wanted_window_id} action {wanted_action_number}"
    )))
}

pub(crate) async fn read_until_window_property(
    stream: &mut tokio::net::TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
    protocol: TestJavaProtocol,
    wanted_window_id: u8,
    wanted_property_id: i16,
    max_attempts: usize,
) -> Result<Vec<u8>, RuntimeError> {
    let max_attempts = max_attempts.max(64);
    for _ in 0..max_attempts {
        let packet = tokio::time::timeout(
            Duration::from_millis(250),
            read_packet(stream, codec, buffer),
        )
        .await
        .map_err(|_| {
            RuntimeError::Config(format!(
                "timed out waiting for window property window {wanted_window_id} property {wanted_property_id}"
            ))
        })??;
        if let Ok((window_id, property_id, _)) = decode_window_property(protocol, &packet)
            && window_id == wanted_window_id
            && property_id == wanted_property_id
        {
            return Ok(packet);
        }
    }
    Err(RuntimeError::Config(format!(
        "did not receive window property window {wanted_window_id} property {wanted_property_id}"
    )))
}

pub(crate) async fn assert_no_java_packet(
    stream: &mut tokio::net::TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
    protocol: TestJavaProtocol,
    packet: TestJavaPacket,
) -> Result<(), RuntimeError> {
    let wanted_packet_id = protocol
        .clientbound_packet_id(packet)
        .ok_or_else(|| RuntimeError::Config(format!("packet {packet:?} is unsupported")))?;
    assert_no_packet_id(stream, codec, buffer, wanted_packet_id).await
}

pub(crate) async fn assert_no_packet_id(
    stream: &mut tokio::net::TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
    wanted_packet_id: i32,
) -> Result<(), RuntimeError> {
    match tokio::time::timeout(
        Duration::from_millis(200),
        read_until_packet_id(stream, codec, buffer, wanted_packet_id, 2),
    )
    .await
    {
        Err(_) | Ok(Err(RuntimeError::Config(_))) => Ok(()),
        Ok(Err(error)) => Err(error),
        Ok(Ok(packet)) => Err(RuntimeError::Config(format!(
            "unexpected packet id 0x{wanted_packet_id:02x}: got 0x{:02x}",
            packet_id(&packet),
        ))),
    }
}

pub(crate) struct BedrockTestClient {
    socket: Arc<tokio::net::UdpSocket>,
    send_queue: SendQueue,
    recv_queue: RecvQueue,
    compression: Option<BedrockCompression>,
    server_addr: SocketAddr,
}

impl BedrockTestClient {
    pub(crate) async fn connect(addr: SocketAddr) -> Result<Self, RuntimeError> {
        let socket = Arc::new(tokio::net::UdpSocket::bind("0.0.0.0:0").await?);
        socket.connect(addr).await?;

        let mtu = Self::perform_mtu_discovery(&socket).await?;
        let mut client_id_bytes = [0_u8; 8];
        OsRng.fill_bytes(&mut client_id_bytes);
        let client_id = i64::from_le_bytes(client_id_bytes);

        Self::send_rak_packet(
            &socket,
            SessionInfoRequest {
                magic: Magic::new(),
                address: addr,
                mtu_size: mtu,
                client_id,
            }
            .into(),
        )
        .await?;

        let session_reply = Self::recv_offline_packet(&socket).await?;
        let OfflinePacket::SessionInfoReply(SessionInfoReply {
            mtu_size,
            security,
            ..
        }) = session_reply
        else {
            return Err(RuntimeError::Config(
                "expected RakNet session info reply".to_string(),
            ));
        };
        if security {
            return Err(RuntimeError::Config(
                "test bedrock client does not support RakNet security".to_string(),
            ));
        }

        let mut client = Self {
            socket: Arc::clone(&socket),
            send_queue: SendQueue::new(mtu_size, 12_000, 5, Arc::clone(&socket), addr),
            recv_queue: RecvQueue::new(),
            compression: None,
            server_addr: addr,
        };

        client
            .send_queue
            .send_packet(
                ConnectionRequest {
                    client_id,
                    time: epoch_seconds_i64(),
                    security: false,
                }
                .into(),
                RakReliability::Reliable,
                true,
            )
            .await
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        client.finish_online_handshake().await?;
        Ok(client)
    }

    pub(crate) async fn login(&mut self, username: &str) -> Result<(), RuntimeError> {
        self.send_bedrock_payload(&bedrock_transport_payload(&bedrock_network_settings_request()?))
            .await?;
        let packet = read_until_bedrock_packet(self, TestBedrockPacket::NetworkSettings, 16).await?;
        let V924::NetworkSettingsPacket(settings) = packet else {
            unreachable!("network settings packet classification should match");
        };
        self.compression = Some(BedrockCompression::Zlib {
            threshold: settings.compression_threshold,
            compression_level: 6,
        });

        self.send_bedrock_payload(&bedrock_transport_payload(&bedrock_login_packet(
            username,
            self.compression.as_ref(),
        )?))
        .await?;
        Ok(())
    }

    async fn perform_mtu_discovery(
        socket: &Arc<tokio::net::UdpSocket>,
    ) -> Result<u16, RuntimeError> {
        for mtu in [DEFAULT_MTU, 1506, 1492, 1400, 1200, 576] {
            Self::send_rak_packet(
                socket,
                OpenConnectRequest {
                    protocol: V924::RAKNET_VERSION,
                    mtu_size: mtu,
                }
                .into(),
            )
            .await?;

            let response = match tokio::time::timeout(Duration::from_millis(500), Self::recv_udp(socket)).await {
                Ok(result) => result?,
                Err(_) => continue,
            };

            match OfflinePacket::read_from_slice(&response)
                .map_err(|error| RuntimeError::Config(error.to_string()))?
            {
                OfflinePacket::OpenConnectReply(OpenConnectReply { mtu_size, .. }) => {
                    return Ok(mtu_size);
                }
                OfflinePacket::IncompatibleProtocolVersion(IncompatibleProtocolVersion {
                    protocol,
                    ..
                }) => {
                    return Err(RuntimeError::Config(format!(
                        "RakNet version mismatch: server expects {protocol}, client uses {}",
                        V924::RAKNET_VERSION
                    )));
                }
                _ => continue,
            }
        }

        Err(RuntimeError::Config(
            "failed to negotiate RakNet MTU with test server".to_string(),
        ))
    }

    async fn finish_online_handshake(&mut self) -> Result<(), RuntimeError> {
        loop {
            let payload = self.recv_raknet_payload().await?;
            match OnlinePacket::read_from_slice(&payload)
                .map_err(|error| RuntimeError::Config(error.to_string()))?
            {
                OnlinePacket::ConnectedPing(ConnectedPing { time }) => {
                    self.send_queue
                        .send_packet(
                            ConnectedPong {
                                ping_time: time,
                                pong_time: epoch_seconds_i64(),
                            }
                            .into(),
                            RakReliability::Reliable,
                            true,
                        )
                        .await
                        .map_err(|error| RuntimeError::Config(error.to_string()))?;
                }
                OnlinePacket::ConnectionAccept(ConnectionAccept {
                    request_time,
                    timestamp,
                    ..
                }) => {
                    let server_addr = self.server_addr;
                    self.send_queue
                        .send_packet(
                            NewConnection {
                                server_address: server_addr,
                                system_address: vec![server_addr; 10],
                                request_time,
                                timestamp,
                            }
                            .into(),
                            RakReliability::Reliable,
                            true,
                        )
                        .await
                        .map_err(|error| RuntimeError::Config(error.to_string()))?;
                    return Ok(());
                }
                other => {
                    return Err(RuntimeError::Config(format!(
                        "unexpected RakNet online packet during handshake: {other:?}"
                    )));
                }
            }
        }
    }

    async fn send_bedrock_payload(&mut self, payload: &[u8]) -> Result<(), RuntimeError> {
        self.send_queue
            .insert(payload, RakReliability::Reliable, true, Some(0))
            .await
            .map_err(|error| RuntimeError::Config(error.to_string()))
    }

    async fn recv_bedrock_payload(&mut self) -> Result<Vec<u8>, RuntimeError> {
        loop {
            let payload = self.recv_raknet_payload().await?;
            if let Ok(packet) = OnlinePacket::read_from_slice(&payload) {
                match packet {
                    OnlinePacket::ConnectedPing(ConnectedPing { time }) => {
                        self.send_queue
                            .send_packet(
                                ConnectedPong {
                                    ping_time: time,
                                    pong_time: epoch_seconds_i64(),
                                }
                                .into(),
                                RakReliability::Reliable,
                                true,
                            )
                            .await
                            .map_err(|error| RuntimeError::Config(error.to_string()))?;
                        continue;
                    }
                    OnlinePacket::ConnectedPong(_) => continue,
                    OnlinePacket::Disconnect(Disconnect {})
                    | OnlinePacket::LostConnection(LostConnection {}) => {
                        return Err(RuntimeError::Config(
                            "RakNet test client disconnected".to_string(),
                        ));
                    }
                    other => {
                        return Err(RuntimeError::Config(format!(
                            "unexpected RakNet online packet during Bedrock receive: {other:?}"
                        )));
                    }
                }
            }
            return Ok(payload);
        }
    }

    async fn recv_raknet_payload(&mut self) -> Result<Vec<u8>, RuntimeError> {
        loop {
            let payload = tokio::time::timeout(Duration::from_secs(2), Self::recv_udp(&self.socket))
                .await
                .map_err(|_| RuntimeError::Config("timed out waiting for RakNet payload".to_string()))??;

            match payload.first().copied() {
                Some(0x80..=0x8d) => {
                    let frame =
                        FramePacket::read_from_slice(&payload).map_err(|error| RuntimeError::Config(error.to_string()))?;
                    if self.recv_queue.insert(frame).is_err() {
                        continue;
                    }
                    if let Some(raw) = self.recv_queue.flush().into_iter().next() {
                        return Ok(raw);
                    }
                }
                _ => {}
            }
        }
    }

    async fn recv_offline_packet(
        socket: &Arc<tokio::net::UdpSocket>,
    ) -> Result<OfflinePacket, RuntimeError> {
        let payload = tokio::time::timeout(Duration::from_secs(2), Self::recv_udp(socket))
            .await
            .map_err(|_| RuntimeError::Config("timed out waiting for offline RakNet packet".to_string()))??;
        OfflinePacket::read_from_slice(&payload)
            .map_err(|error| RuntimeError::Config(error.to_string()))
    }

    async fn recv_udp(socket: &Arc<tokio::net::UdpSocket>) -> Result<Vec<u8>, RuntimeError> {
        let mut buffer = [0_u8; 4096];
        let len = socket.recv(&mut buffer).await?;
        Ok(buffer[..len].to_vec())
    }

    async fn send_rak_packet(
        socket: &Arc<tokio::net::UdpSocket>,
        packet: RakPacket,
    ) -> Result<(), RuntimeError> {
        let payload = packet
            .write_to_bytes()
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        socket.send(payload.as_slice()).await?;
        Ok(())
    }
}

pub(crate) async fn read_until_bedrock_packet(
    client: &mut BedrockTestClient,
    wanted_packet: TestBedrockPacket,
    max_attempts: usize,
) -> Result<V924, RuntimeError> {
    let max_attempts = max_attempts.max(64);
    let mut last_decode_error = None;
    for _ in 0..max_attempts {
        let payload = client.recv_bedrock_payload().await?;
        let Ok(packets) = decode_bedrock_packets(&payload, client.compression.as_ref()).map_err(
            |error| {
                RuntimeError::Config(format!(
                    "{error}; wanted={wanted_packet:?}; compression={:?}; payload_len={}; payload_prefix={:02x?}",
                    client.compression,
                    payload.len(),
                    &payload.iter().take(24).copied().collect::<Vec<_>>(),
                ))
            },
        ) else {
            last_decode_error = Some(format!(
                "wanted={wanted_packet:?}; compression={:?}; payload_len={}; payload_prefix={:02x?}",
                client.compression,
                payload.len(),
                &payload.iter().take(24).copied().collect::<Vec<_>>(),
            ));
            continue;
        };
        for packet in packets {
            if test_bedrock_packet(&packet) == Some(wanted_packet) {
                return Ok(packet);
            }
        }
    }
    Err(RuntimeError::Config(format!(
        "did not receive bedrock packet {wanted_packet:?}; last_decode_error={last_decode_error:?}"
    )))
}

fn epoch_seconds_i64() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_secs(),
    )
    .expect("epoch seconds should fit in i64")
}

pub(crate) struct TestClientEncryptionState {
    pub(crate) encrypt: MinecraftStreamCipher,
    pub(crate) decrypt: MinecraftStreamCipher,
}

impl TestClientEncryptionState {
    pub(crate) fn new(shared_secret: [u8; 16]) -> Self {
        Self {
            encrypt: MinecraftStreamCipher::new(shared_secret),
            decrypt: MinecraftStreamCipher::new(shared_secret),
        }
    }
}

pub(crate) async fn write_packet_encrypted(
    stream: &mut tokio::net::TcpStream,
    codec: &MinecraftWireCodec,
    payload: &[u8],
    encryption: &mut TestClientEncryptionState,
) -> Result<(), RuntimeError> {
    let mut frame = codec.encode_frame(payload)?;
    encryption.encrypt.apply_encrypt(&mut frame);
    stream.write_all(&frame).await?;
    Ok(())
}

pub(crate) async fn read_packet_encrypted(
    stream: &mut tokio::net::TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
    encryption: &mut TestClientEncryptionState,
) -> Result<Vec<u8>, RuntimeError> {
    loop {
        if let Some(frame) = codec.try_decode_frame(buffer)? {
            return Ok(frame);
        }
        let mut chunk = [0_u8; 8192];
        let bytes_read = stream.read(&mut chunk).await?;
        if bytes_read == 0 {
            return Err(RuntimeError::Config("connection closed".to_string()));
        }
        let bytes = &mut chunk[..bytes_read];
        encryption.decrypt.apply_decrypt(bytes);
        buffer.extend_from_slice(bytes);
    }
}

pub(crate) async fn read_until_packet_id_encrypted(
    stream: &mut tokio::net::TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
    wanted_packet_id: i32,
    max_attempts: usize,
    encryption: &mut TestClientEncryptionState,
) -> Result<Vec<u8>, RuntimeError> {
    let max_attempts = max_attempts.max(64);
    for _ in 0..max_attempts {
        let packet = tokio::time::timeout(
            Duration::from_millis(250),
            read_packet_encrypted(stream, codec, buffer, encryption),
        )
        .await
        .map_err(|_| {
            RuntimeError::Config(format!(
                "timed out waiting for encrypted packet id 0x{wanted_packet_id:02x}"
            ))
        })??;
        if packet_id(&packet) == wanted_packet_id {
            return Ok(packet);
        }
    }
    Err(RuntimeError::Config(format!(
        "did not receive encrypted packet id 0x{wanted_packet_id:02x}"
    )))
}

pub(crate) async fn read_until_java_packet_encrypted(
    stream: &mut tokio::net::TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
    protocol: TestJavaProtocol,
    wanted_packet: TestJavaPacket,
    max_attempts: usize,
    encryption: &mut TestClientEncryptionState,
) -> Result<Vec<u8>, RuntimeError> {
    let wanted_packet_id = protocol
        .clientbound_packet_id(wanted_packet)
        .ok_or_else(|| {
            RuntimeError::Config(format!(
                "packet {wanted_packet:?} is not available for protocol {protocol:?}"
            ))
        })?;
    read_until_packet_id_encrypted(
        stream,
        codec,
        buffer,
        wanted_packet_id,
        max_attempts,
        encryption,
    )
    .await
}

pub(crate) async fn perform_online_login(
    stream: &mut tokio::net::TcpStream,
    codec: &MinecraftWireCodec,
    protocol: TestJavaProtocol,
    username: &str,
) -> Result<(TestClientEncryptionState, BytesMut), RuntimeError> {
    let mut buffer = BytesMut::new();
    write_packet(
        stream,
        codec,
        &encode_handshake(protocol.protocol_version(), 2)?,
    )
    .await?;
    write_packet(stream, codec, &login_start(username)).await?;
    let request = read_packet(stream, codec, &mut buffer).await?;
    let (server_id, public_key_der, verify_token) = parse_encryption_request(&request)?;
    assert_eq!(server_id, super::super::super::LOGIN_SERVER_ID);
    let public_key = RsaPublicKey::from_public_key_der(&public_key_der)
        .map_err(|error| RuntimeError::Config(format!("invalid test public key: {error}")))?;
    let mut shared_secret = [0_u8; 16];
    OsRng.fill_bytes(&mut shared_secret);
    let shared_secret_encrypted = public_key
        .encrypt(&mut OsRng, Pkcs1v15Encrypt, &shared_secret)
        .map_err(|error| {
            RuntimeError::Config(format!("failed to encrypt shared secret: {error}"))
        })?;
    let verify_token_encrypted = public_key
        .encrypt(&mut OsRng, Pkcs1v15Encrypt, &verify_token)
        .map_err(|error| {
            RuntimeError::Config(format!("failed to encrypt verify token: {error}"))
        })?;
    let response = login_encryption_response(&shared_secret_encrypted, &verify_token_encrypted)?;
    write_packet(stream, codec, &response).await?;
    Ok((TestClientEncryptionState::new(shared_secret), buffer))
}
