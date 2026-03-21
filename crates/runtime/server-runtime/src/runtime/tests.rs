#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UdpDatagramAction {
    Ignore,
    UnsupportedBedrock,
}

mod failing_storage_plugin {
    use mc_core::{CapabilitySet, WorldSnapshot};
    use mc_plugin_api::codec::storage::StorageDescriptor;
    use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
    use mc_plugin_sdk_rust::storage::{RustStoragePlugin, export_storage_plugin};
    use mc_proto_common::StorageError;
    use std::path::Path;

    pub const PLUGIN_ID: &str = "storage-failing-runtime";
    pub const PROFILE_ID: &str = "failing-storage";

    #[derive(Default)]
    pub struct FailingStoragePlugin;

    impl RustStoragePlugin for FailingStoragePlugin {
        fn descriptor(&self) -> StorageDescriptor {
            StorageDescriptor {
                storage_profile: PROFILE_ID.to_string(),
            }
        }

        fn capability_set(&self) -> CapabilitySet {
            let mut capabilities = CapabilitySet::new();
            let _ = capabilities.insert("runtime.reload.storage");
            capabilities
        }

        fn load_snapshot(&self, _world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError> {
            Ok(None)
        }

        fn save_snapshot(
            &self,
            _world_dir: &Path,
            _snapshot: &WorldSnapshot,
        ) -> Result<(), StorageError> {
            Err(StorageError::Plugin("storage runtime failure".to_string()))
        }
    }

    const MANIFEST: StaticPluginManifest = StaticPluginManifest::storage(
        PLUGIN_ID,
        "Failing Storage Plugin",
        &["storage.profile:failing-storage", "runtime.reload.storage"],
    );

    export_storage_plugin!(FailingStoragePlugin, MANIFEST);
}

#[cfg(test)]
fn classify_udp_datagram(
    protocol_registry: &ProtocolRegistry,
    datagram: &[u8],
) -> Result<UdpDatagramAction, ProtocolError> {
    match protocol_registry.route_handshake(TransportKind::Udp, datagram)? {
        Some(intent) if intent.edition == Edition::Be => Ok(UdpDatagramAction::UnsupportedBedrock),
        Some(_) | None => Ok(UdpDatagramAction::Ignore),
    }
}

/// # Errors
///
/// Returns [`RuntimeError`] when the handshake payload cannot be encoded.
fn encode_handshake(protocol_version: i32, next_state: i32) -> Result<Vec<u8>, RuntimeError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x00);
    writer.write_varint(protocol_version);
    writer.write_string("localhost")?;
    writer.write_u16(25565);
    writer.write_varint(next_state);
    Ok(writer.into_inner())
}

use super::{
    ReloadableRunningServer, ServerBuilder, TopologyStatusState, format_runtime_status_summary,
};
use crate::RuntimeError;
use crate::config::{BEDROCK_OFFLINE_AUTH_PROFILE_ID, LevelType, ServerConfig, ServerConfigSource};
use crate::transport::{MinecraftStreamCipher, build_listener_plans, default_wire_codec};
use bytes::BytesMut;
use mc_plugin_auth_offline::{
    OFFLINE_AUTH_PROFILE_ID, in_process_auth_entrypoints as offline_auth_entrypoints,
};
use mc_plugin_auth_online_stub::{
    ONLINE_STUB_AUTH_PLUGIN_ID, ONLINE_STUB_AUTH_PROFILE_ID,
    in_process_auth_entrypoints as online_stub_auth_entrypoints,
};
use mc_plugin_gameplay_canonical::in_process_gameplay_entrypoints as canonical_gameplay_entrypoints;
use mc_plugin_gameplay_readonly::in_process_gameplay_entrypoints as readonly_gameplay_entrypoints;
use mc_plugin_host::registry::{LoadedPluginSet, ProtocolRegistry};
use mc_plugin_host_test_support::raw::{
    InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin, InProcessStoragePlugin,
};
use mc_plugin_host_test_support::{
    PluginAbiRange, PluginFailureAction, PluginFailureMatrix, TestPluginHost, TestPluginHostBuilder,
};
use mc_plugin_proto_be_26_3::in_process_protocol_entrypoints as be_26_3_entrypoints;
use mc_plugin_proto_be_placeholder::in_process_protocol_entrypoints as be_placeholder_entrypoints;
use mc_plugin_proto_je_1_7_10::in_process_protocol_entrypoints as je_1_7_10_entrypoints;
use mc_plugin_proto_je_1_8_x::in_process_protocol_entrypoints as je_1_8_x_entrypoints;
use mc_plugin_proto_je_1_12_2::in_process_protocol_entrypoints as je_1_12_2_entrypoints;
use mc_plugin_storage_je_anvil_1_7_10::in_process_storage_entrypoints as storage_entrypoints;
use mc_plugin_test_support::PackagedPluginHarness;
use mc_proto_be_26_3::BE_26_3_ADAPTER_ID;
use mc_proto_be_placeholder::BE_PLACEHOLDER_ADAPTER_ID;
use mc_proto_common::{
    Edition, MinecraftWireCodec, PacketReader, PacketWriter, ProtocolError, TransportKind,
    WireCodec, WireFormatKind,
};
use mc_proto_je_1_7_10::{JE_1_7_10_ADAPTER_ID, JE_1_7_10_STORAGE_PROFILE_ID};
use mc_proto_je_1_8_x::JE_1_8_X_ADAPTER_ID;
use mc_proto_je_1_12_2::JE_1_12_2_ADAPTER_ID;
use rand::RngCore;
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;

#[derive(Clone)]
struct LoadedPluginTestEnvironment {
    loaded_plugins: LoadedPluginSet,
    plugin_host: Option<TestPluginHost>,
}

impl LoadedPluginTestEnvironment {
    fn protocols(&self) -> &ProtocolRegistry {
        self.loaded_plugins.protocols()
    }
}

async fn build_test_server(
    config: ServerConfig,
    loaded_plugins: LoadedPluginTestEnvironment,
) -> Result<super::RunningServer, RuntimeError> {
    build_test_server_from_source(ServerConfigSource::Inline(config), loaded_plugins).await
}

async fn build_test_server_from_source(
    source: ServerConfigSource,
    loaded_plugins: LoadedPluginTestEnvironment,
) -> Result<super::RunningServer, RuntimeError> {
    match loaded_plugins.plugin_host {
        Some(plugin_host) => {
            let config = source.load()?;
            let loaded_plugins = plugin_host.load_plugin_set(&config.plugin_host_config())?;
            ServerBuilder::new(source, loaded_plugins)
                .with_reload_host(plugin_host.runtime_host())
                .build()
                .await
                .map(ReloadableRunningServer::into_running_server)
        }
        None => {
            ServerBuilder::new(source, loaded_plugins.loaded_plugins)
                .build()
                .await
        }
    }
}

async fn build_reloadable_test_server(
    config: ServerConfig,
    loaded_plugins: LoadedPluginTestEnvironment,
) -> Result<ReloadableRunningServer, RuntimeError> {
    build_reloadable_test_server_from_source(ServerConfigSource::Inline(config), loaded_plugins)
        .await
}

async fn build_reloadable_test_server_from_source(
    source: ServerConfigSource,
    loaded_plugins: LoadedPluginTestEnvironment,
) -> Result<ReloadableRunningServer, RuntimeError> {
    let LoadedPluginTestEnvironment {
        loaded_plugins: _loaded_plugins,
        plugin_host,
    } = loaded_plugins;
    let plugin_host = plugin_host.ok_or_else(|| {
        RuntimeError::Config("reloadable test server requires a reload host".to_string())
    })?;
    let config = source.load()?;
    let loaded_plugins = plugin_host.load_plugin_set(&config.plugin_host_config())?;
    ServerBuilder::new(source, loaded_plugins)
        .with_reload_host(plugin_host.runtime_host())
        .build()
        .await
}

fn active_protocol_registry(server: &super::RunningServer) -> ProtocolRegistry {
    server.runtime.active_topology().protocol_registry.clone()
}

const RAKNET_MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];

const ALL_PROTOCOL_PLUGIN_IDS: &[&str] = &[
    JE_1_7_10_ADAPTER_ID,
    JE_1_8_X_ADAPTER_ID,
    JE_1_12_2_ADAPTER_ID,
    BE_26_3_ADAPTER_ID,
    BE_PLACEHOLDER_ADAPTER_ID,
];
const TCP_ONLY_PROTOCOL_PLUGIN_IDS: &[&str] = &[JE_1_7_10_ADAPTER_ID];
const GAMEPLAY_PLUGIN_IDS: &[&str] = &["gameplay-canonical", "gameplay-readonly"];
const STORAGE_AND_AUTH_PLUGIN_IDS: &[&str] = &[
    "storage-je-anvil-1_7_10",
    "auth-offline",
    "auth-bedrock-offline",
    "auth-bedrock-xbl",
];
const PACKAGED_PLUGIN_TEST_HARNESS_TAG: &str = "runtime-test-harness";

fn plugin_test_registries_with_allowlist(
    allowlist: &[&str],
) -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    let dist_dir = PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .dist_dir()
        .to_path_buf();
    plugin_test_registries_from_dist(dist_dir, allowlist)
}

fn plugin_allowlist_with_supporting_plugins(
    allowlist: &[&str],
    supporting_plugin_ids: &[&str],
) -> Vec<String> {
    let mut plugin_allowlist = allowlist
        .iter()
        .map(|entry| (*entry).to_string())
        .collect::<Vec<_>>();
    plugin_allowlist.extend(
        GAMEPLAY_PLUGIN_IDS
            .iter()
            .map(|plugin_id| (*plugin_id).to_string()),
    );
    plugin_allowlist.extend(
        supporting_plugin_ids
            .iter()
            .map(|plugin_id| (*plugin_id).to_string()),
    );
    plugin_allowlist
}

fn plugin_test_registries_from_dist(
    dist_dir: PathBuf,
    allowlist: &[&str],
) -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    plugin_test_registries_from_dist_with_supporting_plugins(
        dist_dir,
        allowlist,
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )
}

fn plugin_test_registries_from_dist_with_supporting_plugins(
    dist_dir: PathBuf,
    allowlist: &[&str],
    supporting_plugin_ids: &[&str],
) -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    let mut config = ServerConfig {
        plugins_dir: dist_dir,
        plugin_allowlist: Some(plugin_allowlist_with_supporting_plugins(
            allowlist,
            supporting_plugin_ids,
        )),
        ..ServerConfig::default()
    };
    if supporting_plugin_ids.contains(&ONLINE_STUB_AUTH_PLUGIN_ID) {
        config.auth_profile = ONLINE_STUB_AUTH_PROFILE_ID.to_string();
    }
    let plugin_host = TestPluginHost::discover(&config.plugin_host_config())?.ok_or_else(|| {
        RuntimeError::Config("packaged protocol plugins should be discovered".to_string())
    })?;
    Ok(LoadedPluginTestEnvironment {
        loaded_plugins: plugin_host.load_plugin_set(&config.plugin_host_config())?,
        plugin_host: Some(plugin_host),
    })
}

fn plugin_test_registries_from_config(
    config: &ServerConfig,
) -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    let plugin_host = TestPluginHost::discover(&config.plugin_host_config())?.ok_or_else(|| {
        RuntimeError::Config("packaged protocol plugins should be discovered".to_string())
    })?;
    Ok(LoadedPluginTestEnvironment {
        loaded_plugins: plugin_host.load_plugin_set(&config.plugin_host_config())?,
        plugin_host: Some(plugin_host),
    })
}

fn seed_runtime_plugins(
    dist_dir: &Path,
    allowlist: &[&str],
    supporting_plugin_ids: &[&str],
) -> Result<(), RuntimeError> {
    let mut plugin_ids = Vec::new();
    plugin_ids.extend_from_slice(allowlist);
    plugin_ids.extend_from_slice(GAMEPLAY_PLUGIN_IDS);
    plugin_ids.extend_from_slice(supporting_plugin_ids);
    PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .seed_subset(dist_dir, &plugin_ids)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

fn plugin_test_registries_tcp_only() -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    plugin_test_registries_with_allowlist(TCP_ONLY_PROTOCOL_PLUGIN_IDS)
}

fn plugin_test_registries_all() -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    plugin_test_registries_with_allowlist(ALL_PROTOCOL_PLUGIN_IDS)
}

fn register_in_process_protocol_adapter(
    builder: TestPluginHostBuilder,
    adapter_id: &str,
) -> Result<TestPluginHostBuilder, RuntimeError> {
    let plugin = match adapter_id {
        JE_1_7_10_ADAPTER_ID => InProcessProtocolPlugin {
            plugin_id: JE_1_7_10_ADAPTER_ID.to_string(),
            manifest: je_1_7_10_entrypoints().manifest,
            api: je_1_7_10_entrypoints().api,
        },
        JE_1_8_X_ADAPTER_ID => InProcessProtocolPlugin {
            plugin_id: JE_1_8_X_ADAPTER_ID.to_string(),
            manifest: je_1_8_x_entrypoints().manifest,
            api: je_1_8_x_entrypoints().api,
        },
        JE_1_12_2_ADAPTER_ID => InProcessProtocolPlugin {
            plugin_id: JE_1_12_2_ADAPTER_ID.to_string(),
            manifest: je_1_12_2_entrypoints().manifest,
            api: je_1_12_2_entrypoints().api,
        },
        BE_26_3_ADAPTER_ID => InProcessProtocolPlugin {
            plugin_id: BE_26_3_ADAPTER_ID.to_string(),
            manifest: be_26_3_entrypoints().manifest,
            api: be_26_3_entrypoints().api,
        },
        BE_PLACEHOLDER_ADAPTER_ID => InProcessProtocolPlugin {
            plugin_id: BE_PLACEHOLDER_ADAPTER_ID.to_string(),
            manifest: be_placeholder_entrypoints().manifest,
            api: be_placeholder_entrypoints().api,
        },
        other => {
            return Err(RuntimeError::Config(format!(
                "unknown in-process adapter `{other}`"
            )));
        }
    };
    Ok(builder.protocol_raw(plugin))
}

fn register_in_process_supporting_plugins(builder: TestPluginHostBuilder) -> TestPluginHostBuilder {
    builder
        .gameplay_raw(InProcessGameplayPlugin {
            plugin_id: "gameplay-canonical".to_string(),
            manifest: canonical_gameplay_entrypoints().manifest,
            api: canonical_gameplay_entrypoints().api,
        })
        .gameplay_raw(InProcessGameplayPlugin {
            plugin_id: "gameplay-readonly".to_string(),
            manifest: readonly_gameplay_entrypoints().manifest,
            api: readonly_gameplay_entrypoints().api,
        })
        .storage_raw(InProcessStoragePlugin {
            plugin_id: "storage-je-anvil-1_7_10".to_string(),
            manifest: storage_entrypoints().manifest,
            api: storage_entrypoints().api,
        })
        .auth_raw(InProcessAuthPlugin {
            plugin_id: ONLINE_STUB_AUTH_PLUGIN_ID.to_string(),
            manifest: online_stub_auth_entrypoints().manifest,
            api: online_stub_auth_entrypoints().api,
        })
}

fn in_process_online_auth_registries(
    allowlist: &[&str],
) -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    let mut builder = TestPluginHostBuilder::new();
    for adapter_id in allowlist {
        builder = register_in_process_protocol_adapter(builder, adapter_id)?;
    }
    let plugin_host = register_in_process_supporting_plugins(builder)
        .abi_range(PluginAbiRange::default())
        .failure_matrix(PluginFailureMatrix::default())
        .build();
    let config = ServerConfig {
        auth_profile: ONLINE_STUB_AUTH_PROFILE_ID.to_string(),
        ..ServerConfig::default()
    };
    Ok(LoadedPluginTestEnvironment {
        loaded_plugins: plugin_host.load_plugin_set(&config.plugin_host_config())?,
        plugin_host: Some(plugin_host),
    })
}

fn in_process_failing_storage_registries(
    failure_action: PluginFailureAction,
) -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    let builder =
        register_in_process_protocol_adapter(TestPluginHostBuilder::new(), JE_1_7_10_ADAPTER_ID)?;
    let plugin_host = builder
        .gameplay_raw(InProcessGameplayPlugin {
            plugin_id: "gameplay-canonical".to_string(),
            manifest: canonical_gameplay_entrypoints().manifest,
            api: canonical_gameplay_entrypoints().api,
        })
        .gameplay_raw(InProcessGameplayPlugin {
            plugin_id: "gameplay-readonly".to_string(),
            manifest: readonly_gameplay_entrypoints().manifest,
            api: readonly_gameplay_entrypoints().api,
        })
        .storage_raw(InProcessStoragePlugin {
            plugin_id: failing_storage_plugin::PLUGIN_ID.to_string(),
            manifest: failing_storage_plugin::in_process_storage_entrypoints().manifest,
            api: failing_storage_plugin::in_process_storage_entrypoints().api,
        })
        .auth_raw(InProcessAuthPlugin {
            plugin_id: "auth-offline".to_string(),
            manifest: offline_auth_entrypoints().manifest,
            api: offline_auth_entrypoints().api,
        })
        .abi_range(PluginAbiRange::default())
        .failure_matrix(PluginFailureMatrix {
            storage: failure_action,
            ..PluginFailureMatrix::default()
        })
        .build();
    let config = ServerConfig {
        storage_profile: failing_storage_plugin::PROFILE_ID.to_string(),
        ..ServerConfig::default()
    };
    Ok(LoadedPluginTestEnvironment {
        loaded_plugins: plugin_host.load_plugin_set(&config.plugin_host_config())?,
        plugin_host: Some(plugin_host),
    })
}

fn gameplay_profile_map(entries: &[(&str, &str)]) -> HashMap<String, String> {
    entries
        .iter()
        .map(|(adapter_id, profile_id)| ((*adapter_id).to_string(), (*profile_id).to_string()))
        .collect()
}

async fn write_packet(
    stream: &mut tokio::net::TcpStream,
    codec: &MinecraftWireCodec,
    payload: &[u8],
) -> Result<(), RuntimeError> {
    let frame = codec.encode_frame(payload)?;
    stream.write_all(&frame).await?;
    Ok(())
}

async fn connect_tcp(addr: SocketAddr) -> Result<tokio::net::TcpStream, RuntimeError> {
    Ok(tokio::net::TcpStream::connect(addr).await?)
}

async fn connect_and_login_java_client(
    addr: SocketAddr,
    codec: &MinecraftWireCodec,
    protocol_version: i32,
    username: &str,
    ready_packet_id: i32,
    max_reads: usize,
) -> Result<(tokio::net::TcpStream, BytesMut), RuntimeError> {
    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, codec, &encode_handshake(protocol_version, 2)?).await?;
    write_packet(&mut stream, codec, &login_start(username)).await?;
    let mut buffer = BytesMut::new();
    let _ =
        read_until_packet_id(&mut stream, codec, &mut buffer, ready_packet_id, max_reads).await?;
    Ok((stream, buffer))
}

fn listener_addr(server: &super::RunningServer) -> SocketAddr {
    server
        .listener_bindings()
        .iter()
        .find(|binding| binding.transport == TransportKind::Tcp)
        .expect("tcp listener binding should exist")
        .local_addr
}

fn udp_listener_addr(server: &super::RunningServer) -> SocketAddr {
    server
        .listener_bindings()
        .iter()
        .find(|binding| binding.transport == TransportKind::Udp)
        .expect("udp listener binding should exist")
        .local_addr
}

async fn read_packet(
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

async fn read_until_packet_id(
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

async fn assert_no_packet_id(
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

fn packet_id(frame: &[u8]) -> i32 {
    let mut reader = PacketReader::new(frame);
    reader.read_varint().expect("packet id should decode")
}

struct TestClientEncryptionState {
    encrypt: MinecraftStreamCipher,
    decrypt: MinecraftStreamCipher,
}

impl TestClientEncryptionState {
    fn new(shared_secret: [u8; 16]) -> Self {
        Self {
            encrypt: MinecraftStreamCipher::new(shared_secret),
            decrypt: MinecraftStreamCipher::new(shared_secret),
        }
    }
}

fn login_encryption_response(
    shared_secret_encrypted: &[u8],
    verify_token_encrypted: &[u8],
) -> Result<Vec<u8>, RuntimeError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x01);
    writer.write_varint(
        i32::try_from(shared_secret_encrypted.len())
            .map_err(|_| RuntimeError::Config("encrypted shared secret too large".to_string()))?,
    );
    writer.write_bytes(shared_secret_encrypted);
    writer.write_varint(
        i32::try_from(verify_token_encrypted.len())
            .map_err(|_| RuntimeError::Config("encrypted verify token too large".to_string()))?,
    );
    writer.write_bytes(verify_token_encrypted);
    Ok(writer.into_inner())
}

fn parse_encryption_request(packet: &[u8]) -> Result<(String, Vec<u8>, Vec<u8>), RuntimeError> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint()? != 0x01 {
        return Err(RuntimeError::Config(
            "expected login encryption request packet".to_string(),
        ));
    }
    let server_id = reader.read_string(20)?;
    let public_key_len = usize::try_from(reader.read_varint()?)
        .map_err(|_| RuntimeError::Config("negative public key length".to_string()))?;
    let public_key_der = reader.read_bytes(public_key_len)?.to_vec();
    let verify_token_len = usize::try_from(reader.read_varint()?)
        .map_err(|_| RuntimeError::Config("negative verify token length".to_string()))?;
    let verify_token = reader.read_bytes(verify_token_len)?.to_vec();
    Ok((server_id, public_key_der, verify_token))
}

async fn write_packet_encrypted(
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

async fn read_packet_encrypted(
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

async fn read_until_packet_id_encrypted(
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

async fn perform_online_login(
    stream: &mut tokio::net::TcpStream,
    codec: &MinecraftWireCodec,
    protocol_version: i32,
    username: &str,
) -> Result<(TestClientEncryptionState, BytesMut), RuntimeError> {
    let mut buffer = BytesMut::new();
    write_packet(stream, codec, &encode_handshake(protocol_version, 2)?).await?;
    write_packet(stream, codec, &login_start(username)).await?;
    let request = read_packet(stream, codec, &mut buffer).await?;
    let (server_id, public_key_der, verify_token) = parse_encryption_request(&request)?;
    assert_eq!(server_id, super::LOGIN_SERVER_ID);
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

#[test]
fn protocol_registry_resolves_registered_adapter() -> Result<(), RuntimeError> {
    let registry = plugin_test_registries_tcp_only()?;
    let by_id = registry
        .protocols()
        .resolve_adapter(JE_1_7_10_ADAPTER_ID)
        .expect("registered adapter should resolve by id");
    let by_route = registry
        .protocols()
        .resolve_route(TransportKind::Tcp, Edition::Je, 5)
        .expect("registered adapter should resolve by route");

    assert_eq!(by_id.descriptor().adapter_id, JE_1_7_10_ADAPTER_ID);
    assert_eq!(by_id.descriptor().transport, TransportKind::Tcp);
    assert_eq!(by_route.descriptor().version_name, "1.7.10");
    Ok(())
}

#[test]
fn handshake_probe_transport_kind_filters_routing() -> Result<(), RuntimeError> {
    let registry = plugin_test_registries_all()?;
    let tcp_route = registry
        .protocols()
        .route_handshake(TransportKind::Tcp, &raknet_unconnected_ping())
        .expect("tcp routing should not fail");
    let udp_route = registry
        .protocols()
        .route_handshake(TransportKind::Udp, &raknet_unconnected_ping())
        .expect("udp routing should not fail");

    assert!(tcp_route.is_none());
    assert!(udp_route.is_some());
    Ok(())
}

#[test]
fn listener_plan_includes_tcp_binding_and_registered_adapter() -> Result<(), RuntimeError> {
    let registries = plugin_test_registries_tcp_only()?;
    let plans = build_listener_plans(&ServerConfig::default(), registries.protocols())?;

    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].transport, TransportKind::Tcp);
    assert!(
        plans[0]
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == JE_1_7_10_ADAPTER_ID)
    );
    Ok(())
}

#[test]
fn listener_plan_includes_udp_binding_when_bedrock_is_enabled() -> Result<(), RuntimeError> {
    let registries = plugin_test_registries_all()?;
    let plans = build_listener_plans(
        &ServerConfig {
            be_enabled: true,
            ..ServerConfig::default()
        },
        registries.protocols(),
    )?;

    assert_eq!(plans.len(), 2);
    assert_eq!(plans[1].transport, TransportKind::Udp);
    assert!(
        plans[1]
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == BE_26_3_ADAPTER_ID)
    );
    let default_bedrock_adapter = registries
        .protocols()
        .resolve_adapter(BE_26_3_ADAPTER_ID)
        .expect("default bedrock adapter should resolve");
    let expected_bedrock_listener = default_bedrock_adapter
        .bedrock_listener_descriptor()
        .expect("bedrock adapter should expose listener metadata");
    let expected_descriptor = default_bedrock_adapter.descriptor();
    let bedrock_metadata = plans[1]
        .bedrock_bind_metadata
        .as_ref()
        .expect("udp listener plan should keep bedrock metadata");
    assert_eq!(
        bedrock_metadata.protocol_number,
        expected_descriptor.protocol_number
    );
    assert_eq!(
        bedrock_metadata.raknet_version,
        expected_bedrock_listener.raknet_version
    );
    assert_eq!(
        bedrock_metadata.game_version,
        expected_bedrock_listener.game_version
    );
    Ok(())
}

#[test]
fn plugin_host_preserves_wire_format_per_adapter() -> Result<(), RuntimeError> {
    let registries = plugin_test_registries_all()?;
    let je_adapter = registries
        .protocols()
        .resolve_adapter(JE_1_7_10_ADAPTER_ID)
        .expect("je adapter should resolve");
    let bedrock_adapter = registries
        .protocols()
        .resolve_adapter(BE_26_3_ADAPTER_ID)
        .expect("bedrock adapter should resolve");

    assert_eq!(
        je_adapter.descriptor().wire_format,
        WireFormatKind::MinecraftFramed
    );
    assert_eq!(
        bedrock_adapter.descriptor().wire_format,
        WireFormatKind::RawPacketStream
    );
    assert_eq!(
        je_adapter.wire_codec().encode_frame(&[1, 2, 3])?,
        vec![3, 1, 2, 3]
    );
    assert_eq!(
        bedrock_adapter.wire_codec().encode_frame(&[1, 2, 3])?,
        vec![1, 2, 3]
    );
    Ok(())
}

#[test]
fn default_wire_codec_requires_adapter_for_udp_sessions() {
    assert!(matches!(
        default_wire_codec(TransportKind::Udp),
        Err(RuntimeError::Config(message))
            if message.contains("udp sessions require an active protocol adapter")
    ));
}

fn login_start(username: &str) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x00);
    let _ = writer.write_string(username);
    writer.into_inner()
}

fn status_request() -> Vec<u8> {
    vec![0x00]
}

fn status_ping(value: i64) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x01);
    writer.write_i64(value);
    writer.into_inner()
}

fn raknet_unconnected_ping() -> Vec<u8> {
    let mut frame = Vec::with_capacity(33);
    frame.push(0x01);
    frame.extend_from_slice(&123_i64.to_be_bytes());
    frame.extend_from_slice(&RAKNET_MAGIC);
    frame.extend_from_slice(&456_i64.to_be_bytes());
    frame
}

fn player_position_look(x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x06);
    writer.write_f64(x);
    writer.write_f64(y + 1.62);
    writer.write_f64(y);
    writer.write_f64(z);
    writer.write_f32(yaw);
    writer.write_f32(pitch);
    writer.write_bool(true);
    writer.into_inner()
}

fn held_item_change(slot: i16) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x09);
    writer.write_i16(slot);
    writer.into_inner()
}

fn creative_inventory_action(slot: i16, item_id: i16, count: u8, damage: i16) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x10);
    writer.write_i16(slot);
    writer.write_i16(item_id);
    writer.write_u8(count);
    writer.write_i16(damage);
    writer.write_i16(-1);
    writer.into_inner()
}

fn player_block_placement(
    x: i32,
    y: u8,
    z: i32,
    face: u8,
    held_item: Option<(i16, u8, i16)>,
) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x08);
    writer.write_i32(x);
    writer.write_u8(y);
    writer.write_i32(z);
    writer.write_u8(face);
    if let Some((item_id, count, damage)) = held_item {
        writer.write_i16(item_id);
        writer.write_u8(count);
        writer.write_i16(damage);
    }
    writer.write_i16(-1);
    writer.write_u8(8);
    writer.write_u8(8);
    writer.write_u8(8);
    writer.into_inner()
}

fn player_digging(status: u8, x: i32, y: u8, z: i32, face: u8) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x07);
    writer.write_u8(status);
    writer.write_i32(x);
    writer.write_u8(y);
    writer.write_i32(z);
    writer.write_u8(face);
    writer.into_inner()
}

fn player_position_look_1_8(x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x06);
    writer.write_f64(x);
    writer.write_f64(y);
    writer.write_f64(z);
    writer.write_f32(yaw);
    writer.write_f32(pitch);
    writer.write_bool(true);
    writer.into_inner()
}

fn player_position_look_1_12(x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x0e);
    writer.write_f64(x);
    writer.write_f64(y);
    writer.write_f64(z);
    writer.write_f32(yaw);
    writer.write_f32(pitch);
    writer.write_bool(true);
    writer.into_inner()
}

fn creative_inventory_action_1_12(slot: i16, item_id: i16, count: u8, damage: i16) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x1b);
    writer.write_i16(slot);
    writer.write_i16(item_id);
    writer.write_u8(count);
    writer.write_i16(damage);
    writer.write_i16(-1);
    writer.into_inner()
}

fn player_block_placement_1_12(x: i32, y: i32, z: i32, face: i32, hand: i32) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x1f);
    writer.write_i64(mc_proto_je_common::internal::pack_block_position(
        mc_core::BlockPos::new(x, y, z),
    ));
    writer.write_varint(face);
    writer.write_varint(hand);
    writer.write_f32(0.5);
    writer.write_f32(0.5);
    writer.write_f32(0.5);
    writer.into_inner()
}

fn read_slot(reader: &mut PacketReader<'_>) -> Result<Option<(i16, u8, i16)>, RuntimeError> {
    let item_id = reader.read_i16()?;
    if item_id < 0 {
        return Ok(None);
    }
    let count = reader.read_u8()?;
    let damage = reader.read_i16()?;
    let nbt_length = reader.read_i16()?;
    if nbt_length != -1 {
        return Err(RuntimeError::Config(
            "test helper only supports empty slot nbt".to_string(),
        ));
    }
    Ok(Some((item_id, count, damage)))
}

fn window_items_slot(
    packet: &[u8],
    wanted_slot: usize,
) -> Result<Option<(i16, u8, i16)>, RuntimeError> {
    window_items_slot_with_packet_id(packet, 0x30, wanted_slot)
}

fn window_items_slot_with_packet_id(
    packet: &[u8],
    expected_packet_id: i32,
    wanted_slot: usize,
) -> Result<Option<(i16, u8, i16)>, RuntimeError> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint()? != expected_packet_id {
        return Err(RuntimeError::Config(
            "expected window items packet".to_string(),
        ));
    }
    let _window_id = reader.read_u8()?;
    let count = usize::try_from(reader.read_i16()?)
        .map_err(|_| RuntimeError::Config("negative window item count".to_string()))?;
    if wanted_slot >= count {
        return Err(RuntimeError::Config(
            "wanted slot out of bounds".to_string(),
        ));
    }
    for slot in 0..count {
        let item = read_slot(&mut reader)?;
        if slot == wanted_slot {
            return Ok(item);
        }
    }
    Err(RuntimeError::Config("wanted slot missing".to_string()))
}

fn set_slot_slot(packet: &[u8], expected_packet_id: i32) -> Result<i16, RuntimeError> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint()? != expected_packet_id {
        return Err(RuntimeError::Config("expected set slot packet".to_string()));
    }
    let _window_id = reader.read_i8()?;
    reader.read_i16().map_err(RuntimeError::from)
}

fn held_item_from_packet(packet: &[u8]) -> Result<i8, RuntimeError> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint()? != 0x09 {
        return Err(RuntimeError::Config(
            "expected held item change packet".to_string(),
        ));
    }
    reader.read_i8().map_err(RuntimeError::from)
}

fn block_change_from_packet(packet: &[u8]) -> Result<(i32, u8, i32, i32, u8), RuntimeError> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint()? != 0x23 {
        return Err(RuntimeError::Config(
            "expected block change packet".to_string(),
        ));
    }
    let x = reader.read_i32()?;
    let y = reader.read_u8()?;
    let z = reader.read_i32()?;
    let block_id = reader.read_varint()?;
    let metadata = reader.read_u8()?;
    Ok((x, y, z, block_id, metadata))
}

fn block_change_from_packet_1_8(packet: &[u8]) -> Result<(i32, i32, i32, i32), RuntimeError> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint()? != 0x23 {
        return Err(RuntimeError::Config(
            "expected 1.8 block change packet".to_string(),
        ));
    }
    let position = mc_proto_je_common::internal::unpack_block_position(reader.read_i64()?);
    let block_state = reader.read_varint()?;
    Ok((position.x, position.y, position.z, block_state))
}

fn player_abilities_flags(packet: &[u8]) -> Result<u8, RuntimeError> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint()? != 0x39 {
        return Err(RuntimeError::Config(
            "expected player abilities packet".to_string(),
        ));
    }
    reader.read_u8().map_err(RuntimeError::from)
}

#[tokio::test]
async fn storage_skip_keeps_dirty_state_after_runtime_save_failure() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            storage_profile: failing_storage_plugin::PROFILE_ID.to_string(),
            plugin_failure_policy_storage: PluginFailureAction::Skip,
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        in_process_failing_storage_registries(PluginFailureAction::Skip)?,
    )
    .await?;

    {
        let mut state = server.runtime.state.lock().await;
        state.dirty = true;
    }
    server.runtime.maybe_save().await?;
    assert!(server.runtime.state.lock().await.dirty);

    server.shutdown().await
}

#[tokio::test]
async fn plain_server_builder_rejects_reload_watch_without_reload_host() -> Result<(), RuntimeError>
{
    let temp_dir = tempdir()?;
    let config = ServerConfig {
        server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
        server_port: 0,
        plugin_reload_watch: true,
        world_dir: temp_dir.path().join("world"),
        ..ServerConfig::default()
    };
    let LoadedPluginTestEnvironment { loaded_plugins, .. } =
        in_process_failing_storage_registries(PluginFailureAction::Skip)?;
    let error = match ServerBuilder::new(ServerConfigSource::Inline(config), loaded_plugins)
        .build()
        .await
    {
        Ok(_) => panic!("plain server builder should reject reload watch settings"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        RuntimeError::Config(message)
            if message.contains("plugin-reload-watch requires ServerBuilder::with_reload_host")
    ));
    Ok(())
}

#[tokio::test]
async fn reloadable_server_builder_applies_reload_host_failure_policy() -> Result<(), RuntimeError>
{
    let temp_dir = tempdir()?;
    let config = ServerConfig {
        server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
        server_port: 0,
        storage_profile: failing_storage_plugin::PROFILE_ID.to_string(),
        plugin_failure_policy_storage: PluginFailureAction::Skip,
        world_dir: temp_dir.path().join("world"),
        ..ServerConfig::default()
    };
    let LoadedPluginTestEnvironment { loaded_plugins, .. } =
        in_process_failing_storage_registries(PluginFailureAction::Skip)?;
    let LoadedPluginTestEnvironment {
        plugin_host: Some(reload_host),
        ..
    } = in_process_failing_storage_registries(PluginFailureAction::FailFast)?
    else {
        panic!("failing storage registries should include a plugin host");
    };
    let server = ServerBuilder::new(ServerConfigSource::Inline(config), loaded_plugins)
        .with_reload_host(reload_host.runtime_host())
        .build()
        .await?;

    {
        let mut state = server.runtime.state.lock().await;
        state.dirty = true;
    }
    let error = server
        .runtime
        .maybe_save()
        .await
        .expect_err("reload host should apply fail-fast failure policy");
    assert!(
        matches!(error, RuntimeError::PluginFatal(message) if message.contains("failed during runtime"))
    );

    let shutdown_error = server
        .shutdown()
        .await
        .expect_err("fail-fast runtime should surface plugin fatal during shutdown");
    assert!(
        matches!(shutdown_error, RuntimeError::PluginFatal(message) if message.contains("failed during runtime"))
    );
    Ok(())
}

#[tokio::test]
async fn storage_fail_fast_returns_plugin_fatal_on_runtime_save_failure() -> Result<(), RuntimeError>
{
    let temp_dir = tempdir()?;
    let server = build_test_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            storage_profile: failing_storage_plugin::PROFILE_ID.to_string(),
            plugin_failure_policy_storage: PluginFailureAction::FailFast,
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        in_process_failing_storage_registries(PluginFailureAction::FailFast)?,
    )
    .await?;

    {
        let mut state = server.runtime.state.lock().await;
        state.dirty = true;
    }
    let error = server
        .runtime
        .maybe_save()
        .await
        .expect_err("fail-fast storage policy should return a fatal runtime error");
    assert!(matches!(
        error,
        RuntimeError::PluginFatal(message) if message.contains("storage plugin")
    ));
    {
        let mut state = server.runtime.state.lock().await;
        state.dirty = false;
    }

    server.shutdown().await
}

mod auth;
mod config_props;
mod connectivity;
mod gameplay;
mod guardrails;
mod multiversion;

#[cfg(target_os = "linux")]
mod reload;
