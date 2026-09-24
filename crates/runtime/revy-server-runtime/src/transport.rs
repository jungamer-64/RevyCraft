use crate::config::ServerConfig;
use crate::{ListenerBinding, RuntimeError};
use aes::Aes128;
use aes::cipher::{BlockCipherEncrypt, KeyInit};
use bytes::BytesMut;
use mc_plugin_host::registry::ProtocolRegistry;
use mc_proto_be_common::{BEDROCK_GAME_PACKET_ID, BedrockCompression};
use mc_proto_common::{MinecraftWireCodec, TransportKind, WireCodec};
use revy_raknet::{
    Bound as RakNetBound, FrozenPeer as FrozenRakNetPeer, PeerSnapshot, RakNetBudgets, RakNetPeer,
    RakNetSender, RakNetServer, ReceivePaused as RakNetReceivePaused,
    RunningPeer as RunningRakNetPeer, ServerConfig as RakNetServerConfig,
};
use revy_runtime_transfer::{ExportedSocket, SocketTransferTarget};
use revy_voxel_core::AdapterId;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::collections::VecDeque;
use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

const TCP_LISTENER_BACKLOG: i32 = 1024;
const TRANSPORT_WRITE_QUEUE_CAPACITY: usize = 1_024;
const TRANSPORT_WRITE_BATCH_FRAME_LIMIT: usize = 4_096;
const TRANSPORT_WRITE_BATCH_BYTE_LIMIT: usize = 16 * 1024 * 1024;
// Aggregation target, not a new packet-admission limit: a larger individual frame
// retains its existing RakNet budget. Only already queued frames are combined.
const BEDROCK_BATCH_TARGET_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListenerPlan {
    pub transport: TransportKind,
    pub bind_addr: SocketAddr,
    pub adapter_ids: Vec<AdapterId>,
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

enum TransportReader {
    Tcp {
        stream: OwnedReadHalf,
        decrypt: Option<MinecraftStreamCipher>,
    },
    Bedrock {
        connection: BedrockPeerAuthority,
        compression: Option<BedrockCompression>,
    },
    Transitioning,
}

enum BedrockPeerAuthority {
    Running(RakNetPeer<RunningRakNetPeer>),
    Frozen(RakNetPeer<FrozenRakNetPeer>),
    Transitioning,
}

enum TransportWriter {
    Tcp {
        stream: OwnedWriteHalf,
        encrypt: Option<MinecraftStreamCipher>,
    },
    Bedrock {
        sender: RakNetSender,
        compression: Option<BedrockCompression>,
    },
}

enum TransportWriterCommand {
    Write {
        frames: Vec<Vec<u8>>,
        completion: Option<oneshot::Sender<Result<(), String>>>,
    },
    EnableEncryption([u8; 16]),
    RestoreEncryption(MinecraftStreamCipherSnapshot),
    EnableBedrockCompression(u16),
    TakeForProcessTransfer {
        completion: oneshot::Sender<TransportWriter>,
    },
}

enum TransportWriterControl {
    Pause {
        completion: oneshot::Sender<Result<(), String>>,
    },
    ResumeWithFront {
        frames: Vec<Vec<u8>>,
    },
}

pub struct TransportSessionIo {
    reader: TransportReader,
    writer_tx: Option<mpsc::Sender<TransportWriterCommand>>,
    writer_control_tx: mpsc::Sender<TransportWriterControl>,
    writer_failure: watch::Receiver<Option<String>>,
    writer_task: Option<JoinHandle<()>>,
    writer_paused: bool,
}

pub(crate) enum FrozenProcessTransport {
    Tcp {
        stream: TcpStream,
        decrypt: Option<MinecraftStreamCipherSnapshot>,
        encrypt: Option<MinecraftStreamCipherSnapshot>,
    },
    Bedrock {
        peer: RakNetPeer<FrozenRakNetPeer>,
        reader_compression: Option<BedrockCompression>,
        writer_compression: Option<BedrockCompression>,
    },
}

pub(crate) enum FrozenProcessTransportState {
    Tcp {
        decrypt: Option<MinecraftStreamCipherSnapshot>,
        encrypt: Option<MinecraftStreamCipherSnapshot>,
    },
    Bedrock {
        peer: PeerSnapshot,
        reader_compression_threshold: Option<u16>,
        writer_compression_threshold: Option<u16>,
    },
}

pub(crate) enum PreparedProcessTransportSocket {
    Tcp(ExportedSocket),
    Bedrock,
}

#[derive(Clone, Copy)]
pub(crate) struct MinecraftStreamCipherSnapshot {
    shared_secret: [u8; 16],
    shift_register: [u8; 16],
}

impl MinecraftStreamCipherSnapshot {
    pub(crate) const fn from_parts(shared_secret: [u8; 16], shift_register: [u8; 16]) -> Self {
        Self {
            shared_secret,
            shift_register,
        }
    }

    pub(crate) const fn shared_secret(self) -> [u8; 16] {
        self.shared_secret
    }

    pub(crate) const fn shift_register(self) -> [u8; 16] {
        self.shift_register
    }
}

impl TransportSessionIo {
    #[must_use]
    pub fn tcp(stream: TcpStream) -> Self {
        let (reader, writer) = stream.into_split();
        Self::from_parts(
            TransportReader::Tcp {
                stream: reader,
                decrypt: None,
            },
            TransportWriter::Tcp {
                stream: writer,
                encrypt: None,
            },
        )
    }

    #[must_use]
    pub fn bedrock(connection: RakNetPeer<RunningRakNetPeer>) -> Self {
        let sender = connection.sender();
        Self::from_parts(
            TransportReader::Bedrock {
                connection: BedrockPeerAuthority::Running(connection),
                compression: None,
            },
            TransportWriter::Bedrock {
                sender,
                compression: None,
            },
        )
    }

    pub(crate) fn prestaged_imported_tcp(stream: TcpStream) -> Self {
        let (reader, writer) = stream.into_split();
        Self::from_parts(
            TransportReader::Tcp {
                stream: reader,
                decrypt: None,
            },
            TransportWriter::Tcp {
                stream: writer,
                encrypt: None,
            },
        )
    }

    pub(crate) fn imported_bedrock(
        peer: RakNetPeer<FrozenRakNetPeer>,
        reader_compression_threshold: Option<u16>,
        writer_compression_threshold: Option<u16>,
    ) -> Self {
        let sender = peer.sender();
        Self::from_parts(
            TransportReader::Bedrock {
                connection: BedrockPeerAuthority::Frozen(peer),
                compression: reader_compression_threshold.map(BedrockCompression::zlib),
            },
            TransportWriter::Bedrock {
                sender,
                compression: writer_compression_threshold.map(BedrockCompression::zlib),
            },
        )
    }

    fn from_parts(reader: TransportReader, mut writer: TransportWriter) -> Self {
        let (writer_tx, mut writer_rx) = mpsc::channel(TRANSPORT_WRITE_QUEUE_CAPACITY);
        let (writer_control_tx, mut writer_control_rx) = mpsc::channel(4);
        let (failure_tx, writer_failure) = watch::channel(None);
        let writer_task = tokio::spawn(async move {
            let mut paused = false;
            let mut front = VecDeque::new();
            loop {
                if paused {
                    let Some(control) = writer_control_rx.recv().await else {
                        break;
                    };
                    match control {
                        TransportWriterControl::Pause { completion } => {
                            let _ = completion.send(Ok(()));
                        }
                        TransportWriterControl::ResumeWithFront { frames } => {
                            Self::resume_writer_control(frames, &mut paused, &mut front);
                        }
                    }
                    continue;
                }
                if !front.is_empty() {
                    if let Err(error) = writer.write_next(&mut front).await {
                        failure_tx.send_replace(Some(error.to_string()));
                        break;
                    }
                    continue;
                }
                tokio::select! {
                    biased;
                    control = writer_control_rx.recv() => {
                        let Some(control) = control else { break; };
                        match control {
                            TransportWriterControl::Pause { completion } => {
                                paused = true;
                                let _ = completion.send(Ok(()));
                            }
                            TransportWriterControl::ResumeWithFront { frames } => {
                                Self::resume_writer_control(frames, &mut paused, &mut front);
                            }
                        }
                    }
                    command = writer_rx.recv() => {
                        let Some(command) = command else { break; };
                        match command {
                            TransportWriterCommand::Write { frames, completion } => {
                                let mut frames = VecDeque::from(frames);
                                let mut result = Ok(());
                                while !frames.is_empty() {
                                    if let Err(error) = writer.write_next(&mut frames).await {
                                        result = Err(error);
                                        break;
                                    }
                                }
                                let diagnostic = result.as_ref().err().map(ToString::to_string);
                                if let Some(completion) = completion {
                                    let _ = completion.send(result.map_err(|error| error.to_string()));
                                }
                                if let Some(diagnostic) = diagnostic {
                                    failure_tx.send_replace(Some(diagnostic));
                                    break;
                                }
                            }
                            TransportWriterCommand::EnableEncryption(shared_secret) => {
                                if let TransportWriter::Tcp { encrypt, .. } = &mut writer {
                                    *encrypt = Some(MinecraftStreamCipher::new(shared_secret));
                                }
                            }
                            TransportWriterCommand::RestoreEncryption(snapshot) => {
                                if let TransportWriter::Tcp { encrypt, .. } = &mut writer {
                                    *encrypt = Some(MinecraftStreamCipher::restore(snapshot));
                                }
                            }
                            TransportWriterCommand::EnableBedrockCompression(threshold) => {
                                if let TransportWriter::Bedrock { compression, .. } = &mut writer {
                                    *compression = Some(BedrockCompression::zlib(threshold));
                                }
                            }
                            TransportWriterCommand::TakeForProcessTransfer { completion } => {
                                let _ = completion.send(writer);
                                return;
                            }
                        }
                    }
                }
            }
        });
        Self {
            reader,
            writer_tx: Some(writer_tx),
            writer_control_tx,
            writer_failure,
            writer_task: Some(writer_task),
            writer_paused: false,
        }
    }

    pub async fn read_into(&mut self, buffer: &mut BytesMut) -> Result<usize, std::io::Error> {
        match &mut self.reader {
            TransportReader::Tcp { stream, decrypt } => {
                let mut chunk = [0_u8; 8192];
                let bytes_read = stream.read(&mut chunk).await?;
                if bytes_read == 0 {
                    return Ok(0);
                }
                let bytes = &mut chunk[..bytes_read];
                if let Some(decrypt) = decrypt.as_mut() {
                    decrypt.apply_decrypt(bytes);
                }
                buffer.extend_from_slice(bytes);
                Ok(bytes_read)
            }
            TransportReader::Bedrock {
                connection,
                compression,
            } => {
                let BedrockPeerAuthority::Running(connection) = connection else {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "bedrock transport is frozen for cutover",
                    ));
                };
                let packet_stream = connection.recv_raw().await.map_err(std::io::Error::other)?;
                let Some((&packet_id, packet_stream)) = packet_stream.split_first() else {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "empty RakNet application payload",
                    ));
                };
                if packet_id != BEDROCK_GAME_PACKET_ID {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("invalid Bedrock RakNet payload id 0x{packet_id:02x}"),
                    ));
                }
                let mut packet_stream = packet_stream.to_vec();
                if let Some(compression) = compression.as_ref() {
                    packet_stream = compression
                        .decompress(&packet_stream)
                        .map_err(std::io::Error::other)?;
                }
                let bytes_read = packet_stream.len();
                buffer.extend_from_slice(&packet_stream);
                Ok(bytes_read)
            }
            TransportReader::Transitioning => Err(std::io::Error::other(
                "transport authority is transitioning",
            )),
        }
    }

    fn writer(&self) -> Result<&mpsc::Sender<TransportWriterCommand>, RuntimeError> {
        self.writer_tx.as_ref().ok_or_else(|| {
            RuntimeError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "transport writer is closed",
            ))
        })
    }

    fn enqueue(&self, command: TransportWriterCommand) -> Result<(), RuntimeError> {
        match self.writer()?.try_send(command) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(RuntimeError::BudgetExceeded {
                resource: "session transport writer queue",
                requested: TRANSPORT_WRITE_QUEUE_CAPACITY.saturating_add(1),
                limit: TRANSPORT_WRITE_QUEUE_CAPACITY,
            }),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(RuntimeError::Io(
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "transport writer closed"),
            )),
        }
    }

    fn enqueue_write(
        &self,
        bytes: Vec<u8>,
        completion: Option<oneshot::Sender<Result<(), String>>>,
    ) -> Result<(), RuntimeError> {
        self.enqueue_write_batch(vec![bytes], completion)
    }

    fn enqueue_write_batch(
        &self,
        frames: Vec<Vec<u8>>,
        completion: Option<oneshot::Sender<Result<(), String>>>,
    ) -> Result<(), RuntimeError> {
        if frames.len() > TRANSPORT_WRITE_BATCH_FRAME_LIMIT {
            return Err(RuntimeError::BudgetExceeded {
                resource: "session transport writer batch frames",
                requested: frames.len(),
                limit: TRANSPORT_WRITE_BATCH_FRAME_LIMIT,
            });
        }
        let bytes = frames
            .iter()
            .try_fold(0_usize, |total, frame| total.checked_add(frame.len()));
        let Some(bytes) = bytes else {
            return Err(RuntimeError::BudgetExceeded {
                resource: "session transport writer batch bytes",
                requested: usize::MAX,
                limit: TRANSPORT_WRITE_BATCH_BYTE_LIMIT,
            });
        };
        if bytes > TRANSPORT_WRITE_BATCH_BYTE_LIMIT {
            return Err(RuntimeError::BudgetExceeded {
                resource: "session transport writer batch bytes",
                requested: bytes,
                limit: TRANSPORT_WRITE_BATCH_BYTE_LIMIT,
            });
        }
        self.enqueue(TransportWriterCommand::Write { frames, completion })
    }

    fn resume_writer_control(
        frames: Vec<Vec<u8>>,
        paused: &mut bool,
        front: &mut VecDeque<Vec<u8>>,
    ) {
        for frame in frames.into_iter().rev() {
            front.push_front(frame);
        }
        *paused = false;
    }

    pub async fn pause_writer(&mut self) -> Result<(), RuntimeError> {
        if self.writer_paused {
            return Ok(());
        }
        let (completion_tx, completion_rx) = oneshot::channel();
        self.writer_control_tx
            .send(TransportWriterControl::Pause {
                completion: completion_tx,
            })
            .await
            .map_err(|_| {
                RuntimeError::Io(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "transport writer closed before cutover freeze",
                ))
            })?;
        completion_rx
            .await
            .map_err(|_| {
                RuntimeError::Io(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "transport writer dropped cutover freeze acknowledgement",
                ))
            })?
            .map_err(|error| RuntimeError::Io(std::io::Error::other(error)))?;
        self.writer_paused = true;
        Ok(())
    }

    pub fn resume_writer_with_front(&mut self, frames: Vec<Vec<u8>>) -> Result<(), RuntimeError> {
        if !self.writer_paused && frames.is_empty() {
            return Ok(());
        }
        let result = match self
            .writer_control_tx
            .try_send(TransportWriterControl::ResumeWithFront { frames })
        {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(RuntimeError::BudgetExceeded {
                resource: "session transport writer control queue",
                requested: 5,
                limit: 4,
            }),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(RuntimeError::Io(
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "transport writer closed"),
            )),
        };
        if result.is_ok() {
            self.writer_paused = false;
        }
        result
    }

    pub fn enable_encryption(&mut self, shared_secret: [u8; 16]) -> Result<(), RuntimeError> {
        self.enqueue(TransportWriterCommand::EnableEncryption(shared_secret))?;
        if let TransportReader::Tcp { decrypt, .. } = &mut self.reader {
            *decrypt = Some(MinecraftStreamCipher::new(shared_secret));
            Ok(())
        } else {
            Err(RuntimeError::Config(
                "tcp encryption cannot be enabled for a bedrock transport".to_string(),
            ))
        }
    }

    pub(crate) fn restore_imported_tcp_state(
        &mut self,
        decrypt: Option<MinecraftStreamCipherSnapshot>,
        encrypt: Option<MinecraftStreamCipherSnapshot>,
    ) -> Result<(), RuntimeError> {
        if let Some(encrypt) = encrypt {
            self.enqueue(TransportWriterCommand::RestoreEncryption(encrypt))?;
        }
        let TransportReader::Tcp {
            decrypt: active, ..
        } = &mut self.reader
        else {
            return Err(RuntimeError::Config(
                "TCP cipher state cannot be restored into a Bedrock transport".to_string(),
            ));
        };
        *active = decrypt.map(MinecraftStreamCipher::restore);
        Ok(())
    }

    pub fn enable_bedrock_compression(
        &mut self,
        compression_threshold: u16,
    ) -> Result<(), RuntimeError> {
        self.enqueue(TransportWriterCommand::EnableBedrockCompression(
            compression_threshold,
        ))?;
        if let TransportReader::Bedrock { compression, .. } = &mut self.reader {
            *compression = Some(BedrockCompression::zlib(compression_threshold));
            Ok(())
        } else {
            Err(RuntimeError::Config(
                "bedrock compression cannot be enabled for a tcp transport".to_string(),
            ))
        }
    }

    pub fn subscribe_writer_failure(&self) -> watch::Receiver<Option<String>> {
        self.writer_failure.clone()
    }

    pub async fn freeze_transport(&mut self) -> Result<(), RuntimeError> {
        let TransportReader::Bedrock { connection, .. } = &mut self.reader else {
            return Ok(());
        };
        let current = std::mem::replace(connection, BedrockPeerAuthority::Transitioning);
        match current {
            BedrockPeerAuthority::Running(peer) => match peer.freeze().await {
                Ok(peer) => {
                    *connection = BedrockPeerAuthority::Frozen(peer);
                    Ok(())
                }
                Err(error) => Err(RuntimeError::from(error)),
            },
            frozen @ BedrockPeerAuthority::Frozen(_) => {
                *connection = frozen;
                Ok(())
            }
            BedrockPeerAuthority::Transitioning => Err(RuntimeError::Config(
                "bedrock transport entered an overlapping authority transition".to_string(),
            )),
        }
    }

    pub(crate) async fn freeze_for_process_transfer(
        &mut self,
    ) -> Result<FrozenProcessTransport, RuntimeError> {
        if self.writer_paused {
            return Err(RuntimeError::Config(
                "process transfer requires a running transport writer".to_string(),
            ));
        }
        let (completion_tx, completion_rx) = oneshot::channel();
        self.writer_tx
            .as_ref()
            .ok_or_else(|| {
                RuntimeError::Io(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "transport writer authority was already consumed",
                ))
            })?
            .send(TransportWriterCommand::TakeForProcessTransfer {
                completion: completion_tx,
            })
            .await
            .map_err(|_| {
                RuntimeError::Io(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "transport writer closed before process transfer",
                ))
            })?;
        let writer = completion_rx.await.map_err(|_| {
            RuntimeError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "transport writer dropped process-transfer authority",
            ))
        })?;
        self.writer_tx.take();
        if let Some(task) = self.writer_task.take() {
            task.await?;
        }
        self.writer_paused = true;
        if let Err(error) = self.freeze_transport().await {
            let reader = std::mem::replace(&mut self.reader, TransportReader::Transitioning);
            *self = Self::from_parts(reader, writer);
            return Err(error);
        }
        if let Err(error) = self.seal_bedrock_for_process_transfer().await {
            let reader = std::mem::replace(&mut self.reader, TransportReader::Transitioning);
            *self = Self::from_parts(reader, writer);
            return Err(error);
        }
        let reader = std::mem::replace(&mut self.reader, TransportReader::Transitioning);
        match (reader, writer) {
            (
                TransportReader::Tcp { stream, decrypt },
                TransportWriter::Tcp {
                    stream: writer,
                    encrypt,
                },
            ) => {
                let stream = stream.reunite(writer).map_err(|_error| {
                    RuntimeError::Config(
                        "TCP transport halves did not belong to the same session".to_string(),
                    )
                })?;
                Ok(FrozenProcessTransport::Tcp {
                    stream,
                    decrypt: decrypt.as_ref().map(MinecraftStreamCipher::snapshot),
                    encrypt: encrypt.as_ref().map(MinecraftStreamCipher::snapshot),
                })
            }
            (
                TransportReader::Bedrock {
                    connection: BedrockPeerAuthority::Frozen(peer),
                    compression: reader_compression,
                },
                TransportWriter::Bedrock {
                    sender: _,
                    compression: writer_compression,
                },
            ) => Ok(FrozenProcessTransport::Bedrock {
                peer,
                reader_compression,
                writer_compression,
            }),
            (reader, writer) => {
                *self = Self::from_parts(reader, writer);
                Err(RuntimeError::Config(
                    "transport reader and writer authorities did not match during process transfer"
                        .to_string(),
                ))
            }
        }
    }

    /// Duplicates only the OS socket while the parent retains all read/write authority.
    ///
    /// TCP sessions have an independently transferable socket. Bedrock peers share the UDP
    /// listener socket, so their per-peer transport state is sealed later without another handle.
    pub(crate) fn duplicate_socket_for_process_transfer(
        &self,
        target: SocketTransferTarget,
    ) -> Result<PreparedProcessTransportSocket, RuntimeError> {
        match &self.reader {
            TransportReader::Tcp { stream, .. } => {
                duplicate_process_socket(stream.as_ref(), target)
                    .map(PreparedProcessTransportSocket::Tcp)
            }
            TransportReader::Bedrock { .. } => Ok(PreparedProcessTransportSocket::Bedrock),
            TransportReader::Transitioning => Err(RuntimeError::Config(
                "transport socket cannot be duplicated during an authority transition".to_string(),
            )),
        }
    }

    async fn seal_bedrock_for_process_transfer(&mut self) -> Result<(), RuntimeError> {
        let TransportReader::Bedrock { connection, .. } = &mut self.reader else {
            return Ok(());
        };
        let current = std::mem::replace(connection, BedrockPeerAuthority::Transitioning);
        match current {
            BedrockPeerAuthority::Frozen(peer) => match peer.seal_for_transfer().await {
                Ok(peer) => {
                    *connection = BedrockPeerAuthority::Frozen(peer);
                    Ok(())
                }
                Err(error) => Err(RuntimeError::from(error)),
            },
            running @ BedrockPeerAuthority::Running(_) => {
                *connection = running;
                Err(RuntimeError::Config(
                    "bedrock transport must be frozen before transfer seal".to_string(),
                ))
            }
            BedrockPeerAuthority::Transitioning => Err(RuntimeError::Config(
                "bedrock transport entered an overlapping authority transition".to_string(),
            )),
        }
    }

    pub fn resume_transport(&mut self) -> Result<(), RuntimeError> {
        let TransportReader::Bedrock { connection, .. } = &mut self.reader else {
            return Ok(());
        };
        let current = std::mem::replace(connection, BedrockPeerAuthority::Transitioning);
        match current {
            BedrockPeerAuthority::Frozen(peer) => match peer.resume() {
                Ok(peer) => {
                    *connection = BedrockPeerAuthority::Running(peer);
                    Ok(())
                }
                Err(error) => Err(RuntimeError::from(error)),
            },
            running @ BedrockPeerAuthority::Running(_) => {
                *connection = running;
                Ok(())
            }
            BedrockPeerAuthority::Transitioning => Err(RuntimeError::Config(
                "bedrock transport entered an overlapping authority transition".to_string(),
            )),
        }
    }

    pub async fn shutdown_writer(&mut self) {
        self.writer_tx.take();
        if let Some(task) = self.writer_task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl FrozenProcessTransport {
    pub(crate) fn transfer_state(&self) -> FrozenProcessTransportState {
        match self {
            Self::Tcp {
                decrypt, encrypt, ..
            } => FrozenProcessTransportState::Tcp {
                decrypt: *decrypt,
                encrypt: *encrypt,
            },
            Self::Bedrock {
                peer,
                reader_compression,
                writer_compression,
            } => FrozenProcessTransportState::Bedrock {
                peer: peer.snapshot().clone(),
                reader_compression_threshold: reader_compression.map(BedrockCompression::threshold),
                writer_compression_threshold: writer_compression.map(BedrockCompression::threshold),
            },
        }
    }

    pub(crate) async fn rollback(self) -> Result<TransportSessionIo, RuntimeError> {
        match self {
            Self::Tcp {
                stream,
                decrypt,
                encrypt,
            } => {
                let (reader, writer) = stream.into_split();
                Ok(TransportSessionIo::from_parts(
                    TransportReader::Tcp {
                        stream: reader,
                        decrypt: decrypt.map(MinecraftStreamCipher::restore),
                    },
                    TransportWriter::Tcp {
                        stream: writer,
                        encrypt: encrypt.map(MinecraftStreamCipher::restore),
                    },
                ))
            }
            Self::Bedrock {
                peer,
                reader_compression,
                writer_compression,
            } => {
                let peer = peer.resume()?;
                let sender = peer.sender();
                Ok(TransportSessionIo::from_parts(
                    TransportReader::Bedrock {
                        connection: BedrockPeerAuthority::Running(peer),
                        compression: reader_compression,
                    },
                    TransportWriter::Bedrock {
                        sender,
                        compression: writer_compression,
                    },
                ))
            }
        }
    }

    pub(crate) async fn commit(self) -> Result<(), RuntimeError> {
        match self {
            Self::Tcp { stream, .. } => {
                drop(stream);
                Ok(())
            }
            Self::Bedrock { peer, .. } => {
                let _transferred = peer.mark_transferred().await?;
                Ok(())
            }
        }
    }
}

#[cfg(unix)]
fn duplicate_process_socket(
    socket: &impl std::os::fd::AsFd,
    target: SocketTransferTarget,
) -> Result<ExportedSocket, RuntimeError> {
    ExportedSocket::duplicate(socket, target).map_err(RuntimeError::from)
}

#[cfg(windows)]
fn duplicate_process_socket(
    socket: &impl std::os::windows::io::AsRawSocket,
    target: SocketTransferTarget,
) -> Result<ExportedSocket, RuntimeError> {
    ExportedSocket::duplicate(socket, target).map_err(RuntimeError::from)
}

impl TransportWriter {
    /// Preserves packet order and command/compression boundaries. Bedrock frames already
    /// contain packet lengths, so concatenation forms one wire batch with one compressor.
    /// Completion is acknowledged by the owner only after every group has been sent.
    async fn write_next(&mut self, frames: &mut VecDeque<Vec<u8>>) -> Result<(), std::io::Error> {
        let Some(bytes) = frames.pop_front() else {
            return Ok(());
        };
        match self {
            Self::Tcp { stream, encrypt } => {
                let mut bytes = bytes;
                if let Some(encrypt) = encrypt.as_mut() {
                    encrypt.apply_encrypt(&mut bytes);
                }
                stream.write_all(&bytes).await
            }
            Self::Bedrock {
                sender,
                compression,
            } => {
                let bytes = coalesce_bedrock_frames(bytes, frames)?;
                let packet_stream = if let Some(compression) = compression.as_ref() {
                    compression
                        .compress(&bytes)
                        .map_err(std::io::Error::other)?
                } else {
                    bytes
                };
                let mut transport_payload = Vec::with_capacity(packet_stream.len() + 1);
                transport_payload.push(BEDROCK_GAME_PACKET_ID);
                transport_payload.extend_from_slice(&packet_stream);
                sender
                    .send_raw(&transport_payload)
                    .await
                    .map_err(std::io::Error::other)
            }
        }
    }
}

fn coalesce_bedrock_frames(
    mut first: Vec<u8>,
    remaining: &mut VecDeque<Vec<u8>>,
) -> io::Result<Vec<u8>> {
    let mut total = first.len();
    let mut count = 0;
    for next in remaining.iter() {
        let Some(combined) = total.checked_add(next.len()) else {
            break;
        };
        if combined > BEDROCK_BATCH_TARGET_BYTES {
            break;
        }
        total = combined;
        count += 1;
    }
    first
        .try_reserve_exact(total - first.len())
        .map_err(io::Error::other)?;
    for frame in remaining.drain(..count) {
        first.extend_from_slice(&frame);
    }
    Ok(first)
}

pub enum BoundTransportListener {
    Tcp {
        listener: TcpListener,
        adapter_ids: Vec<AdapterId>,
    },
    Bedrock {
        listener: BedrockListenerSocket,
        adapter_ids: Vec<AdapterId>,
        bind_addr: SocketAddr,
    },
}

pub(crate) enum BedrockListenerSocket {
    Bound(Box<RakNetServer<RakNetBound>>),
    ReceivePaused(Box<RakNetServer<RakNetReceivePaused>>),
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

pub struct MinecraftStreamCipher {
    cipher: Aes128,
    shared_secret: [u8; 16],
    shift_register: [u8; 16],
}

fn dual_stack_bind_addr(bind_addr: SocketAddr) -> Option<SocketAddr> {
    match bind_addr {
        SocketAddr::V4(addr) if addr.ip().is_unspecified() => Some(SocketAddr::new(
            IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            addr.port(),
        )),
        SocketAddr::V4(_) | SocketAddr::V6(_) => None,
    }
}

fn should_fallback_from_dual_stack(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::AddrNotAvailable | io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported
    ) || matches!(error.raw_os_error(), Some(22 | 92 | 97 | 99))
}

fn bind_dual_stack_tcp_listener(bind_addr: SocketAddr) -> Result<TcpListener, io::Error> {
    let socket = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
    #[cfg(not(windows))]
    socket.set_reuse_address(true)?;
    socket.set_only_v6(false)?;
    socket.bind(&SockAddr::from(bind_addr))?;
    socket.listen(TCP_LISTENER_BACKLOG)?;
    socket.set_nonblocking(true)?;
    let std_listener: std::net::TcpListener = socket.into();
    TcpListener::from_std(std_listener)
}

async fn bind_tcp_listener(bind_addr: SocketAddr) -> Result<TcpListener, io::Error> {
    if let Some(dual_stack_addr) = dual_stack_bind_addr(bind_addr) {
        match bind_dual_stack_tcp_listener(dual_stack_addr) {
            Ok(listener) => return Ok(listener),
            Err(error) if should_fallback_from_dual_stack(&error) => {}
            Err(error) => return Err(error),
        }
    }
    TcpListener::bind(bind_addr).await
}

impl MinecraftStreamCipher {
    pub fn new(shared_secret: [u8; 16]) -> Self {
        Self::from_parts(shared_secret, shared_secret)
    }

    pub fn from_parts(shared_secret: [u8; 16], shift_register: [u8; 16]) -> Self {
        Self {
            cipher: Aes128::new_from_slice(&shared_secret)
                .expect("AES-128 key length should be exactly 16 bytes"),
            shared_secret,
            shift_register,
        }
    }

    fn snapshot(&self) -> MinecraftStreamCipherSnapshot {
        MinecraftStreamCipherSnapshot {
            shared_secret: self.shared_secret,
            shift_register: self.shift_register,
        }
    }

    fn restore(snapshot: MinecraftStreamCipherSnapshot) -> Self {
        Self::from_parts(snapshot.shared_secret, snapshot.shift_register)
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
    if config.topology.be_enabled {
        let udp_adapter_ids = protocols.adapter_ids_for_transport(TransportKind::Udp);
        if udp_adapter_ids.is_empty() {
            return Err(RuntimeError::Config(
                "be-enabled=true requires at least one udp protocol adapter".to_string(),
            ));
        }
        let default_bedrock_adapter = protocols
            .resolve_adapter(config.topology.default_bedrock_adapter.as_str())
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "default-bedrock-adapter `{}` is not registered",
                    config.topology.default_bedrock_adapter
                ))
            })?;
        let descriptor = default_bedrock_adapter.descriptor();
        let bedrock_listener_descriptor = default_bedrock_adapter
            .bedrock_listener_descriptor()
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "default-bedrock-adapter `{}` must provide bedrock listener metadata",
                    config.topology.default_bedrock_adapter
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
            listener: bind_tcp_listener(plan.bind_addr).await?,
            adapter_ids: plan.adapter_ids,
        }),
        TransportKind::Udp => {
            let metadata = plan.bedrock_bind_metadata.ok_or_else(|| {
                RuntimeError::Config(
                    "udp listener plan is missing bedrock listener metadata".to_string(),
                )
            })?;
            let listener = RakNetServer::<RakNetBound>::bind(RakNetServerConfig {
                bind_addr: plan.bind_addr,
                motd: config.network.motd.clone(),
                server_name: "RevyCraft".to_string(),
                game_version: metadata.game_version,
                protocol_number: u32::try_from(metadata.protocol_number).map_err(|_| {
                    RuntimeError::Config(format!(
                        "bedrock protocol number {} must be non-negative",
                        metadata.protocol_number
                    ))
                })?,
                raknet_version: metadata.raknet_version,
                max_players: config.network.max_players,
                online_players: 0,
                server_guid: 0,
                budgets: RakNetBudgets::default(),
            })
            .await
            .map_err(RuntimeError::from)?;
            let bind_addr = listener.local_addr().map_err(RuntimeError::from)?;
            Ok(BoundTransportListener::Bedrock {
                listener: BedrockListenerSocket::Bound(Box::new(listener)),
                bind_addr,
                adapter_ids: plan.adapter_ids,
            })
        }
    }
}

pub fn default_wire_codec(
    transport: TransportKind,
) -> Result<&'static dyn WireCodec, RuntimeError> {
    static TCP_CODEC: MinecraftWireCodec = MinecraftWireCodec;
    match transport {
        TransportKind::Tcp => Ok(&TCP_CODEC),
        TransportKind::Udp => Err(RuntimeError::Config(
            "udp sessions require an active protocol adapter".to_string(),
        )),
    }
}

pub async fn write_payload_batch(
    transport_io: &mut TransportSessionIo,
    codec: &dyn WireCodec,
    payloads: Vec<Vec<u8>>,
) -> Result<(), RuntimeError> {
    let frames = payloads
        .into_iter()
        .map(|payload| codec.encode_frame(&payload))
        .collect::<Result<Vec<_>, _>>()?;
    transport_io.enqueue_write_batch(frames, None)
}

pub async fn write_payload_confirmed(
    transport_io: &mut TransportSessionIo,
    codec: &dyn WireCodec,
    payload: &[u8],
) -> Result<(), RuntimeError> {
    let frame = codec.encode_frame(payload)?;
    let (completion_tx, completion_rx) = oneshot::channel();
    transport_io.enqueue_write(frame, Some(completion_tx))?;
    completion_rx
        .await
        .map_err(|_| {
            RuntimeError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "transport writer dropped a write completion",
            ))
        })?
        .map_err(|error| RuntimeError::Io(std::io::Error::other(error)))
}

#[cfg(test)]
mod tests {
    use super::{
        BEDROCK_BATCH_TARGET_BYTES, FrozenProcessTransport, TransportSessionIo,
        coalesce_bedrock_frames,
    };
    use std::collections::VecDeque;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    use tokio::net::{TcpListener, TcpStream};

    #[test]
    fn bedrock_batches_preserve_framing_order_and_do_not_split_large_packets() -> std::io::Result<()>
    {
        // Independently framed hotbar packets: length, packet id, slot, container, selected.
        let mut frames = VecDeque::from([vec![4, 48, 2, 0, 1], vec![4, 48, 3, 0, 1]]);
        assert_eq!(
            coalesce_bedrock_frames(vec![4, 48, 1, 0, 1], &mut frames)?,
            [4, 48, 1, 0, 1, 4, 48, 2, 0, 1, 4, 48, 3, 0, 1],
        );
        assert!(frames.is_empty());

        let mut frames = VecDeque::from([vec![2], vec![3]]);
        let merged = coalesce_bedrock_frames(vec![1; BEDROCK_BATCH_TARGET_BYTES - 1], &mut frames)?;
        assert_eq!(merged.len(), BEDROCK_BATCH_TARGET_BYTES);
        assert_eq!(merged.last(), Some(&2));
        assert_eq!(frames, VecDeque::from([vec![3]]));

        let large = vec![7; BEDROCK_BATCH_TARGET_BYTES + 1];
        assert_eq!(coalesce_bedrock_frames(large.clone(), &mut frames)?, large);
        assert_eq!(frames, VecDeque::from([vec![3]]));
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn process_transfer_seals_writer_commands_in_fifo_order() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener
            .local_addr()
            .expect("test listener should expose its address");
        let client = tokio::spawn(TcpStream::connect(address));
        let (server, _) = listener
            .accept()
            .await
            .expect("test listener should accept the client");
        let mut client = client
            .await
            .expect("client connection task should join")
            .expect("client should connect");
        let mut transport = TransportSessionIo::tcp(server);
        for byte in 0_u8..64 {
            transport
                .enqueue_write(vec![byte], None)
                .expect("writer queue should admit the bounded fixture");
        }

        let frozen = transport
            .freeze_for_process_transfer()
            .await
            .expect("process transfer should seal the writer");
        let mut received = [0_u8; 64];
        tokio::time::timeout(Duration::from_secs(1), client.read_exact(&mut received))
            .await
            .expect("sealed writer commands should reach the client")
            .expect("client should read every queued command");
        assert_eq!(received, std::array::from_fn(|index| index as u8));
        assert!(matches!(frozen, FrozenProcessTransport::Tcp { .. }));
    }
}
