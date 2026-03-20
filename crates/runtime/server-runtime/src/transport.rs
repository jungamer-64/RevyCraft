use crate::RuntimeError;
use crate::config::ServerConfig;
use crate::registry::{ListenerBinding, ProtocolRegistry};
use aes::Aes128;
use aes::cipher::{BlockEncrypt, KeyInit};
use bedrockrs_network::connection::Connection as BedrockConnection;
use bedrockrs_network::listener::Listener as BedrockListener;
use bedrockrs_proto::compression::Compression as BedrockCompression;
use bytes::BytesMut;
use mc_proto_common::{MinecraftWireCodec, TransportKind, WireCodec};
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListenerPlan {
    pub transport: TransportKind,
    pub bind_addr: SocketAddr,
    pub adapter_ids: Vec<String>,
    pub bedrock_bind_metadata: Option<BedrockBindMetadata>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BedrockBindMetadata {
    pub game_version: String,
    pub protocol_number: i32,
    pub raknet_version: u8,
}

pub struct AcceptedTransportSession {
    pub transport: TransportKind,
    pub io: TransportSessionIo,
}

pub enum TransportSessionIo {
    Tcp {
        stream: TcpStream,
        encryption: Box<Option<TransportEncryptionState>>,
    },
    Bedrock {
        connection: BedrockConnection,
        compression: Option<BedrockCompression>,
    },
}

impl TransportSessionIo {
    pub async fn read_into(&mut self, buffer: &mut BytesMut) -> Result<usize, std::io::Error> {
        match self {
            Self::Tcp { stream, encryption } => {
                let mut chunk = [0_u8; 8192];
                let bytes_read = stream.read(&mut chunk).await?;
                if bytes_read == 0 {
                    return Ok(0);
                }
                let bytes = &mut chunk[..bytes_read];
                if let Some(encryption) = encryption.as_mut().as_mut() {
                    encryption.decrypt.apply_decrypt(bytes);
                }
                buffer.extend_from_slice(bytes);
                Ok(bytes_read)
            }
            Self::Bedrock {
                connection,
                compression,
            } => {
                let mut packet_stream =
                    connection.recv_raw().await.map_err(std::io::Error::other)?;
                if let Some(compression) = compression.as_ref() {
                    packet_stream = compression
                        .decompress(packet_stream)
                        .map_err(std::io::Error::other)?;
                }
                let bytes_read = packet_stream.len();
                buffer.extend_from_slice(&packet_stream);
                Ok(bytes_read)
            }
        }
    }

    pub async fn write_all(&mut self, bytes: &[u8]) -> Result<(), std::io::Error> {
        match self {
            Self::Tcp { stream, encryption } => {
                let mut encrypted = bytes.to_vec();
                if let Some(encryption) = encryption.as_mut().as_mut() {
                    encryption.encrypt.apply_encrypt(&mut encrypted);
                }
                stream.write_all(&encrypted).await
            }
            Self::Bedrock {
                connection,
                compression,
            } => {
                let packet_stream = if let Some(compression) = compression.as_ref() {
                    compression
                        .compress(bytes.to_vec())
                        .map_err(std::io::Error::other)?
                } else {
                    bytes.to_vec()
                };
                connection
                    .send_raw(&packet_stream)
                    .await
                    .map_err(std::io::Error::other)
            }
        }
    }

    pub fn enable_encryption(&mut self, shared_secret: [u8; 16]) {
        match self {
            Self::Tcp { encryption, .. } => {
                encryption
                    .as_mut()
                    .replace(TransportEncryptionState::new(shared_secret));
            }
            Self::Bedrock { .. } => {}
        }
    }

    pub const fn enable_bedrock_compression(&mut self, compression_threshold: u16) {
        if let Self::Bedrock { compression, .. } = self {
            *compression = Some(BedrockCompression::Zlib {
                threshold: compression_threshold,
                compression_level: 6,
            });
        }
    }
}

pub enum BoundTransportListener {
    Tcp {
        listener: TcpListener,
        adapter_ids: Vec<String>,
    },
    Bedrock {
        listener: Box<BedrockListener>,
        adapter_ids: Vec<String>,
        bind_addr: SocketAddr,
    },
}

impl BoundTransportListener {
    pub fn listener_binding(&self) -> Result<ListenerBinding, RuntimeError> {
        match self {
            Self::Tcp {
                listener,
                adapter_ids,
            } => Ok(ListenerBinding {
                transport: TransportKind::Tcp,
                local_addr: listener.local_addr()?,
                adapter_ids: adapter_ids.clone(),
            }),
            Self::Bedrock {
                adapter_ids,
                bind_addr,
                ..
            } => Ok(ListenerBinding {
                transport: TransportKind::Udp,
                local_addr: *bind_addr,
                adapter_ids: adapter_ids.clone(),
            }),
        }
    }
}

pub struct TransportEncryptionState {
    encrypt: MinecraftStreamCipher,
    decrypt: MinecraftStreamCipher,
}

impl TransportEncryptionState {
    fn new(shared_secret: [u8; 16]) -> Self {
        Self {
            encrypt: MinecraftStreamCipher::new(shared_secret),
            decrypt: MinecraftStreamCipher::new(shared_secret),
        }
    }
}

pub struct MinecraftStreamCipher {
    cipher: Aes128,
    shift_register: [u8; 16],
}

impl MinecraftStreamCipher {
    pub fn new(shared_secret: [u8; 16]) -> Self {
        Self {
            cipher: Aes128::new_from_slice(&shared_secret)
                .expect("AES-128 key length should be exactly 16 bytes"),
            shift_register: shared_secret,
        }
    }

    pub fn apply_encrypt(&mut self, bytes: &mut [u8]) {
        for byte in bytes {
            let mut block = aes::Block::default();
            block.copy_from_slice(&self.shift_register);
            self.cipher.encrypt_block(&mut block);
            let ciphertext = *byte ^ block[0];
            self.shift_register.copy_within(1.., 0);
            self.shift_register[15] = ciphertext;
            *byte = ciphertext;
        }
    }

    pub fn apply_decrypt(&mut self, bytes: &mut [u8]) {
        for byte in bytes {
            let ciphertext = *byte;
            let mut block = aes::Block::default();
            block.copy_from_slice(&self.shift_register);
            self.cipher.encrypt_block(&mut block);
            let plaintext = ciphertext ^ block[0];
            self.shift_register.copy_within(1.., 0);
            self.shift_register[15] = ciphertext;
            *byte = plaintext;
        }
    }
}

pub fn build_listener_plans(
    config: &ServerConfig,
    protocols: &ProtocolRegistry,
) -> Result<Vec<ListenerPlan>, RuntimeError> {
    let tcp_adapter_ids = protocols.adapter_ids_for_transport(TransportKind::Tcp);
    if tcp_adapter_ids.is_empty() {
        return Err(RuntimeError::Config(
            "no tcp protocol adapters registered".to_string(),
        ));
    }
    let mut plans = vec![ListenerPlan {
        transport: TransportKind::Tcp,
        bind_addr: config.bind_addr(),
        adapter_ids: tcp_adapter_ids,
        bedrock_bind_metadata: None,
    }];
    if config.be_enabled {
        let udp_adapter_ids = protocols.adapter_ids_for_transport(TransportKind::Udp);
        if udp_adapter_ids.is_empty() {
            return Err(RuntimeError::Config(
                "be-enabled=true requires at least one udp protocol adapter".to_string(),
            ));
        }
        let default_bedrock_adapter = protocols
            .resolve_adapter(&config.default_bedrock_adapter)
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "default-bedrock-adapter `{}` is not registered",
                    config.default_bedrock_adapter
                ))
            })?;
        let descriptor = default_bedrock_adapter.descriptor();
        let bedrock_listener_descriptor = default_bedrock_adapter
            .bedrock_listener_descriptor()
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "default-bedrock-adapter `{}` must provide bedrock listener metadata",
                    config.default_bedrock_adapter
                ))
            })?;
        plans.push(ListenerPlan {
            transport: TransportKind::Udp,
            bind_addr: config.bind_addr(),
            adapter_ids: udp_adapter_ids,
            bedrock_bind_metadata: Some(BedrockBindMetadata {
                game_version: bedrock_listener_descriptor.game_version,
                protocol_number: descriptor.protocol_number,
                raknet_version: bedrock_listener_descriptor.raknet_version,
            }),
        });
    }
    Ok(plans)
}

pub async fn bind_transport_listener(
    plan: ListenerPlan,
    config: &ServerConfig,
) -> Result<BoundTransportListener, RuntimeError> {
    match plan.transport {
        TransportKind::Tcp => Ok(BoundTransportListener::Tcp {
            listener: TcpListener::bind(plan.bind_addr).await?,
            adapter_ids: plan.adapter_ids,
        }),
        TransportKind::Udp => {
            let metadata = plan.bedrock_bind_metadata.ok_or_else(|| {
                RuntimeError::Config(
                    "udp listener plan is missing bedrock listener metadata".to_string(),
                )
            })?;
            let mut listener = BedrockListener::new_raknet(
                plan.bind_addr,
                config.motd.clone(),
                "RevyCraft".to_string(),
                metadata.game_version,
                u32::try_from(metadata.protocol_number).map_err(|_| {
                    RuntimeError::Config(format!(
                        "bedrock protocol number {} must be non-negative",
                        metadata.protocol_number
                    ))
                })?,
                metadata.raknet_version,
                u32::from(config.max_players),
                0,
                false,
            )
            .await
            .map_err(|error| {
                RuntimeError::Unsupported(format!("failed to bind bedrock listener: {error}"))
            })?;
            listener.start().await.map_err(|error| {
                RuntimeError::Unsupported(format!("failed to start bedrock listener: {error}"))
            })?;
            Ok(BoundTransportListener::Bedrock {
                listener: Box::new(listener),
                bind_addr: plan.bind_addr,
                adapter_ids: plan.adapter_ids,
            })
        }
    }
}

pub fn default_wire_codec(transport: TransportKind) -> &'static dyn WireCodec {
    static TCP_CODEC: MinecraftWireCodec = MinecraftWireCodec;
    match transport {
        TransportKind::Tcp => &TCP_CODEC,
        TransportKind::Udp => unreachable!("udp transport sessions are not implemented"),
    }
}

pub async fn write_payload(
    transport_io: &mut TransportSessionIo,
    codec: &dyn WireCodec,
    payload: &[u8],
) -> Result<(), RuntimeError> {
    let frame = codec.encode_frame(payload)?;
    transport_io.write_all(&frame).await?;
    Ok(())
}
