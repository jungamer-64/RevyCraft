use super::*;
use crate::runtime::RunningServer;

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
    rand::rngs::OsRng.fill_bytes(&mut shared_secret);
    let shared_secret_encrypted = public_key
        .encrypt(&mut rand::rngs::OsRng, Pkcs1v15Encrypt, &shared_secret)
        .map_err(|error| {
            RuntimeError::Config(format!("failed to encrypt shared secret: {error}"))
        })?;
    let verify_token_encrypted = public_key
        .encrypt(&mut rand::rngs::OsRng, Pkcs1v15Encrypt, &verify_token)
        .map_err(|error| {
            RuntimeError::Config(format!("failed to encrypt verify token: {error}"))
        })?;
    let response = login_encryption_response(&shared_secret_encrypted, &verify_token_encrypted)?;
    write_packet(stream, codec, &response).await?;
    Ok((TestClientEncryptionState::new(shared_secret), buffer))
}
