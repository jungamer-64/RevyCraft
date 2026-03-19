#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UdpDatagramAction {
    Ignore,
    UnsupportedBedrock,
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

use super::spawn_server;
use crate::RuntimeError;
use crate::config::{BEDROCK_OFFLINE_AUTH_PROFILE_ID, LevelType, ServerConfig};
use crate::host::{
    InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin, InProcessStoragePlugin,
    PluginAbiRange, PluginCatalog, PluginFailurePolicy, PluginHost, plugin_host_from_config,
};
use crate::registry::RuntimeRegistries;
use crate::transport::{MinecraftStreamCipher, build_listener_plans};
use bytes::BytesMut;
use mc_plugin_auth_offline::OFFLINE_AUTH_PROFILE_ID;
use mc_plugin_auth_online_stub::{
    ONLINE_STUB_AUTH_PLUGIN_ID, ONLINE_STUB_AUTH_PROFILE_ID,
    in_process_auth_entrypoints as online_stub_auth_entrypoints,
};
use mc_plugin_gameplay_canonical::in_process_gameplay_entrypoints as canonical_gameplay_entrypoints;
use mc_plugin_gameplay_readonly::in_process_gameplay_entrypoints as readonly_gameplay_entrypoints;
use mc_plugin_proto_be_26_3::in_process_protocol_entrypoints as be_26_3_entrypoints;
use mc_plugin_proto_be_placeholder::in_process_protocol_entrypoints as be_placeholder_entrypoints;
use mc_plugin_proto_je_1_7_10::in_process_protocol_entrypoints as je_1_7_10_entrypoints;
use mc_plugin_proto_je_1_8_x::in_process_protocol_entrypoints as je_1_8_x_entrypoints;
use mc_plugin_proto_je_1_12_2::in_process_protocol_entrypoints as je_1_12_2_entrypoints;
use mc_plugin_storage_je_anvil_1_7_10::in_process_storage_entrypoints as storage_entrypoints;
use mc_proto_be_26_3::BE_26_3_ADAPTER_ID;
use mc_proto_be_placeholder::BE_PLACEHOLDER_ADAPTER_ID;
use mc_proto_common::{
    Edition, MinecraftWireCodec, PacketReader, PacketWriter, ProtocolError, TransportKind,
    WireCodec,
};
use mc_proto_je_1_7_10::{JE_1_7_10_ADAPTER_ID, JE_1_7_10_STORAGE_PROFILE_ID};
use mc_proto_je_1_8_x::JE_1_8_X_ADAPTER_ID;
use mc_proto_je_1_12_2::JE_1_12_2_ADAPTER_ID;
use rand::RngCore;
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;

use crate::registry::ProtocolRegistry;

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

fn plugin_test_registries_with_allowlist(
    allowlist: &[&str],
) -> Result<RuntimeRegistries, RuntimeError> {
    let dist_dir = crate::packaged_plugin_test_harness_dist_dir()
        .map_err(RuntimeError::Config)?
        .clone();
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
) -> Result<RuntimeRegistries, RuntimeError> {
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
) -> Result<RuntimeRegistries, RuntimeError> {
    let config = ServerConfig {
        plugins_dir: dist_dir,
        plugin_allowlist: Some(plugin_allowlist_with_supporting_plugins(
            allowlist,
            supporting_plugin_ids,
        )),
        ..ServerConfig::default()
    };
    let plugin_host = plugin_host_from_config(&config)?.ok_or_else(|| {
        RuntimeError::Config("packaged protocol plugins should be discovered".to_string())
    })?;
    let mut registries = RuntimeRegistries::new();
    plugin_host.load_into_registries(&mut registries)?;
    Ok(registries)
}

fn plugin_test_registries_tcp_only() -> Result<RuntimeRegistries, RuntimeError> {
    plugin_test_registries_with_allowlist(TCP_ONLY_PROTOCOL_PLUGIN_IDS)
}

fn plugin_test_registries_all() -> Result<RuntimeRegistries, RuntimeError> {
    plugin_test_registries_with_allowlist(ALL_PROTOCOL_PLUGIN_IDS)
}

fn in_process_online_auth_registries(
    allowlist: &[&str],
) -> Result<RuntimeRegistries, RuntimeError> {
    let mut catalog = PluginCatalog::default();
    for adapter_id in allowlist {
        match *adapter_id {
            JE_1_7_10_ADAPTER_ID => {
                catalog.register_in_process_protocol_plugin(InProcessProtocolPlugin {
                    plugin_id: JE_1_7_10_ADAPTER_ID.to_string(),
                    manifest: je_1_7_10_entrypoints().manifest,
                    api: je_1_7_10_entrypoints().api,
                })
            }
            JE_1_8_X_ADAPTER_ID => {
                catalog.register_in_process_protocol_plugin(InProcessProtocolPlugin {
                    plugin_id: JE_1_8_X_ADAPTER_ID.to_string(),
                    manifest: je_1_8_x_entrypoints().manifest,
                    api: je_1_8_x_entrypoints().api,
                })
            }
            JE_1_12_2_ADAPTER_ID => {
                catalog.register_in_process_protocol_plugin(InProcessProtocolPlugin {
                    plugin_id: JE_1_12_2_ADAPTER_ID.to_string(),
                    manifest: je_1_12_2_entrypoints().manifest,
                    api: je_1_12_2_entrypoints().api,
                })
            }
            BE_26_3_ADAPTER_ID => {
                catalog.register_in_process_protocol_plugin(InProcessProtocolPlugin {
                    plugin_id: BE_26_3_ADAPTER_ID.to_string(),
                    manifest: be_26_3_entrypoints().manifest,
                    api: be_26_3_entrypoints().api,
                })
            }
            BE_PLACEHOLDER_ADAPTER_ID => {
                catalog.register_in_process_protocol_plugin(InProcessProtocolPlugin {
                    plugin_id: BE_PLACEHOLDER_ADAPTER_ID.to_string(),
                    manifest: be_placeholder_entrypoints().manifest,
                    api: be_placeholder_entrypoints().api,
                })
            }
            other => {
                return Err(RuntimeError::Config(format!(
                    "unknown in-process adapter `{other}`"
                )));
            }
        }
    }
    catalog.register_in_process_gameplay_plugin(InProcessGameplayPlugin {
        plugin_id: "gameplay-canonical".to_string(),
        manifest: canonical_gameplay_entrypoints().manifest,
        api: canonical_gameplay_entrypoints().api,
    });
    catalog.register_in_process_gameplay_plugin(InProcessGameplayPlugin {
        plugin_id: "gameplay-readonly".to_string(),
        manifest: readonly_gameplay_entrypoints().manifest,
        api: readonly_gameplay_entrypoints().api,
    });
    catalog.register_in_process_storage_plugin(InProcessStoragePlugin {
        plugin_id: "storage-je-anvil-1_7_10".to_string(),
        manifest: storage_entrypoints().manifest,
        api: storage_entrypoints().api,
    });
    catalog.register_in_process_auth_plugin(InProcessAuthPlugin {
        plugin_id: ONLINE_STUB_AUTH_PLUGIN_ID.to_string(),
        manifest: online_stub_auth_entrypoints().manifest,
        api: online_stub_auth_entrypoints().api,
    });

    let plugin_host = Arc::new(PluginHost::new(
        catalog,
        PluginAbiRange::default(),
        PluginFailurePolicy::Quarantine,
    ));
    let mut registries = RuntimeRegistries::new();
    plugin_host.load_into_registries(&mut registries)?;
    Ok(registries)
}

fn gameplay_profile_map(entries: &[(&str, &str)]) -> HashMap<String, String> {
    entries
        .iter()
        .map(|(adapter_id, profile_id)| ((*adapter_id).to_string(), (*profile_id).to_string()))
        .collect()
}

fn workspace_root() -> PathBuf {
    crate::packaged_plugin_test_workspace_root()
}

#[cfg(target_os = "linux")]
fn package_single_plugin(
    cargo_package: &str,
    plugin_id: &str,
    plugin_kind: &str,
    dist_dir: &Path,
    target_dir: &Path,
    build_tag: &str,
) -> Result<(), RuntimeError> {
    let _guard = crate::packaged_plugin_test_build_lock()
        .lock()
        .expect("packaged plugin build lock should not be poisoned");
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let status = Command::new(cargo)
        .current_dir(workspace_root())
        .env("CARGO_TARGET_DIR", target_dir)
        .env("REVY_PLUGIN_BUILD_TAG", build_tag)
        .arg("build")
        .arg("-p")
        .arg(cargo_package)
        .status()
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    if !status.success() {
        return Err(RuntimeError::Config(format!(
            "cargo build failed for `{cargo_package}`"
        )));
    }

    let artifact_name = dynamic_library_filename(cargo_package);
    let source = target_dir.join("debug").join(&artifact_name);
    let plugin_dir = dist_dir.join(plugin_id);
    fs::create_dir_all(&plugin_dir)?;
    let packaged_artifact = packaged_artifact_name(&artifact_name, build_tag);
    let destination = plugin_dir.join(&packaged_artifact);
    let staging = plugin_dir.join(format!(".{packaged_artifact}.tmp"));
    fs::copy(&source, &staging)?;
    if destination.exists() {
        fs::remove_file(&destination)?;
    }
    fs::rename(&staging, &destination)?;
    let manifest = format!(
        "[plugin]\nid = \"{plugin_id}\"\nkind = \"{plugin_kind}\"\n\n[artifacts]\n\"{}-{}\" = \"{packaged_artifact}\"\n",
        env::consts::OS,
        env::consts::ARCH
    );
    fs::write(plugin_dir.join("plugin.toml"), manifest)?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn package_single_gameplay_plugin(
    cargo_package: &str,
    plugin_id: &str,
    dist_dir: &Path,
    target_dir: &Path,
    build_tag: &str,
) -> Result<(), RuntimeError> {
    package_single_plugin(
        cargo_package,
        plugin_id,
        "gameplay",
        dist_dir,
        target_dir,
        build_tag,
    )
}

#[cfg(target_os = "linux")]
fn dynamic_library_filename(package: &str) -> String {
    let crate_name = package.replace('-', "_");
    match env::consts::OS {
        "windows" => format!("{crate_name}.dll"),
        "macos" => format!("lib{crate_name}.dylib"),
        _ => format!("lib{crate_name}.so"),
    }
}

#[cfg(target_os = "linux")]
fn packaged_artifact_name(base_name: &str, build_tag: &str) -> String {
    if let Some((stem, extension)) = base_name.rsplit_once('.') {
        format!("{stem}-{build_tag}.{extension}")
    } else {
        format!("{base_name}-{build_tag}")
    }
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
        Err(_) => Ok(()),
        Ok(Err(RuntimeError::Config(_))) => Ok(()),
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
fn plugin_test_harness_is_packaged_once_per_process() -> Result<(), RuntimeError> {
    let _ = plugin_test_registries_tcp_only()?;
    let _ = plugin_test_registries_all()?;
    assert_eq!(crate::packaged_plugin_test_harness_build_count(), 1);
    Ok(())
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
    Ok(())
}

#[tokio::test]
async fn running_server_exposes_listener_bindings() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await?;

    let binding = server
        .listener_bindings()
        .first()
        .expect("tcp listener binding should exist")
        .clone();
    assert_eq!(binding.transport, TransportKind::Tcp);
    assert!(binding.local_addr.port() > 0);
    assert!(
        binding
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == JE_1_7_10_ADAPTER_ID)
    );

    server.shutdown().await
}

#[tokio::test]
async fn running_server_exposes_udp_listener_binding_when_enabled() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            be_enabled: true,
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_all()?,
    )
    .await?;

    assert_eq!(server.listener_bindings().len(), 2);
    let binding = server
        .listener_bindings()
        .iter()
        .find(|binding| binding.transport == TransportKind::Udp)
        .expect("udp listener binding should exist");
    assert!(binding.local_addr.port() > 0);
    assert!(
        binding
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == BE_26_3_ADAPTER_ID)
    );

    server.shutdown().await
}

#[test]
fn server_properties_accept_flat_level_type() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(
        &path,
        "level-name=flatland\nlevel-type=FLAT\nbe-enabled=true\nonline-mode=false\ndefault-adapter=je-1_7_10\nstorage-profile=je-anvil-1_7_10\nauth-profile=offline-v1\n",
    )?;

    let config = ServerConfig::from_properties(&path)?;

    assert_eq!(config.level_name, "flatland");
    assert_eq!(config.level_type, LevelType::Flat);
    assert!(config.be_enabled);
    assert_eq!(config.default_adapter, JE_1_7_10_ADAPTER_ID);
    assert_eq!(config.default_bedrock_adapter, BE_26_3_ADAPTER_ID);
    assert_eq!(config.storage_profile, JE_1_7_10_STORAGE_PROFILE_ID);
    assert_eq!(config.auth_profile, OFFLINE_AUTH_PROFILE_ID);
    assert_eq!(config.bedrock_auth_profile, BEDROCK_OFFLINE_AUTH_PROFILE_ID);
    assert_eq!(config.world_dir, temp_dir.path().join("flatland"));
    Ok(())
}

#[test]
fn server_properties_use_default_adapter_and_storage_profile() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(&path, "level-name=flatland\nlevel-type=FLAT\n")?;

    let config = ServerConfig::from_properties(&path)?;

    assert!(!config.be_enabled);
    assert_eq!(config.default_adapter, JE_1_7_10_ADAPTER_ID);
    assert_eq!(config.default_bedrock_adapter, BE_26_3_ADAPTER_ID);
    assert_eq!(config.storage_profile, JE_1_7_10_STORAGE_PROFILE_ID);
    assert_eq!(config.auth_profile, OFFLINE_AUTH_PROFILE_ID);
    assert_eq!(config.bedrock_auth_profile, BEDROCK_OFFLINE_AUTH_PROFILE_ID);
    Ok(())
}

#[test]
fn server_properties_parse_enabled_adapters() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(&path, "enabled-adapters=je-1_7_10, je-1_8_x,je-1_12_2\n")?;

    let config = ServerConfig::from_properties(&path)?;
    assert_eq!(
        config.enabled_adapters,
        Some(vec![
            JE_1_7_10_ADAPTER_ID.to_string(),
            JE_1_8_X_ADAPTER_ID.to_string(),
            JE_1_12_2_ADAPTER_ID.to_string(),
        ])
    );
    Ok(())
}

#[test]
fn server_properties_parse_bedrock_adapter_and_auth_profile() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(
        &path,
        "be-enabled=true\ndefault-bedrock-adapter=be-26_3\nenabled-bedrock-adapters=be-26_3,be-placeholder\nbedrock-auth-profile=bedrock-xbl-v1\n",
    )?;

    let config = ServerConfig::from_properties(&path)?;
    assert!(config.be_enabled);
    assert_eq!(config.default_bedrock_adapter, BE_26_3_ADAPTER_ID);
    assert_eq!(
        config.enabled_bedrock_adapters,
        Some(vec![
            BE_26_3_ADAPTER_ID.to_string(),
            BE_PLACEHOLDER_ADAPTER_ID.to_string(),
        ])
    );
    assert_eq!(config.bedrock_auth_profile, "bedrock-xbl-v1");
    Ok(())
}

#[test]
fn server_properties_parse_gameplay_profile_configuration() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(
        &path,
        "default-gameplay-profile=canonical\ngameplay-profile-map=je-1_7_10:readonly,je-1_12_2:canonical\n",
    )?;

    let config = ServerConfig::from_properties(&path)?;
    assert_eq!(config.default_gameplay_profile, "canonical");
    assert_eq!(
        config.gameplay_profile_map,
        gameplay_profile_map(&[
            (JE_1_7_10_ADAPTER_ID, "readonly"),
            (JE_1_12_2_ADAPTER_ID, "canonical"),
        ])
    );
    Ok(())
}

#[test]
fn server_properties_parse_auth_profile() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(&path, "auth-profile=offline-v1\n")?;

    let config = ServerConfig::from_properties(&path)?;
    assert_eq!(config.auth_profile, OFFLINE_AUTH_PROFILE_ID);
    Ok(())
}

#[test]
fn server_properties_reject_non_flat_level_type() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(&path, "level-type=DEFAULT\n")?;

    let error = ServerConfig::from_properties(&path).expect_err("DEFAULT should be rejected");
    assert!(matches!(error, RuntimeError::Unsupported(message) if message.contains("only FLAT")));
    Ok(())
}

#[test]
fn be_enabled_requires_udp_adapter() {
    let registry =
        plugin_test_registries_tcp_only().expect("tcp-only plugin registry should be available");
    let error = build_listener_plans(
        &ServerConfig {
            be_enabled: true,
            ..ServerConfig::default()
        },
        registry.protocols(),
    )
    .expect_err("be-enabled should require udp adapter");
    assert!(matches!(
        error,
        RuntimeError::Config(message) if message.contains("be-enabled=true")
    ));
}

#[tokio::test]
async fn enabled_adapters_must_include_default_adapter() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let result = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            enabled_adapters: Some(vec![JE_1_8_X_ADAPTER_ID.to_string()]),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_all()?,
    )
    .await;

    let Err(error) = result else {
        panic!("default adapter missing from enabled list should fail");
    };
    assert!(matches!(
        error,
        RuntimeError::Config(message) if message.contains("default-adapter")
    ));
    Ok(())
}

#[tokio::test]
async fn duplicate_enabled_adapters_fail_fast() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let result = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            enabled_adapters: Some(vec![
                JE_1_7_10_ADAPTER_ID.to_string(),
                JE_1_7_10_ADAPTER_ID.to_string(),
            ]),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_all()?,
    )
    .await;

    let Err(error) = result else {
        panic!("duplicate enabled adapters should fail");
    };
    assert!(matches!(
        error,
        RuntimeError::Config(message) if message.contains("duplicate adapter")
    ));
    Ok(())
}

#[tokio::test]
async fn tcp_listener_binding_reports_enabled_java_versions() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            enabled_adapters: Some(vec![
                JE_1_7_10_ADAPTER_ID.to_string(),
                JE_1_8_X_ADAPTER_ID.to_string(),
                JE_1_12_2_ADAPTER_ID.to_string(),
            ]),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_all()?,
    )
    .await?;

    let binding = server
        .listener_bindings()
        .iter()
        .find(|binding| binding.transport == TransportKind::Tcp)
        .expect("tcp listener binding should exist");
    assert_eq!(binding.adapter_ids.len(), 3);
    assert!(
        binding
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == JE_1_7_10_ADAPTER_ID)
    );
    assert!(
        binding
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == JE_1_8_X_ADAPTER_ID)
    );
    assert!(
        binding
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == JE_1_12_2_ADAPTER_ID)
    );

    server.shutdown().await
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
    writer.write_i64(mc_proto_je_common::pack_block_position(
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
    let position = mc_proto_je_common::unpack_block_position(reader.read_i64()?);
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
async fn status_ping_login_and_initial_world_work() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut status_stream = connect_tcp(addr).await?;
    write_packet(&mut status_stream, &codec, &encode_handshake(5, 1)?).await?;
    write_packet(&mut status_stream, &codec, &status_request()).await?;
    let mut buffer = BytesMut::new();
    let status_response = read_packet(&mut status_stream, &codec, &mut buffer).await?;
    assert_eq!(packet_id(&status_response), 0x00);
    write_packet(&mut status_stream, &codec, &status_ping(42)).await?;
    let pong = read_packet(&mut status_stream, &codec, &mut buffer).await?;
    assert_eq!(packet_id(&pong), 0x01);

    let mut login_stream = connect_tcp(addr).await?;
    write_packet(&mut login_stream, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut login_stream, &codec, &login_start("alpha")).await?;
    let mut login_buffer = BytesMut::new();
    let login_success = read_packet(&mut login_stream, &codec, &mut login_buffer).await?;
    assert_eq!(packet_id(&login_success), 0x02);
    let join_game = read_packet(&mut login_stream, &codec, &mut login_buffer).await?;
    assert_eq!(packet_id(&join_game), 0x01);
    let chunk_bulk =
        read_until_packet_id(&mut login_stream, &codec, &mut login_buffer, 0x26, 8).await?;
    assert_eq!(packet_id(&chunk_bulk), 0x26);

    server.shutdown().await
}

#[tokio::test]
async fn creative_join_sends_inventory_selected_slot_and_abilities() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut stream, &codec, &login_start("creative")).await?;
    let mut buffer = BytesMut::new();
    let mut window_items = None;
    let mut held_item = None;
    let mut abilities = None;
    for _ in 0..12 {
        let packet = read_packet(&mut stream, &codec, &mut buffer).await?;
        match packet_id(&packet) {
            0x30 if window_items.is_none() => window_items = Some(packet),
            0x09 if held_item.is_none() => held_item = Some(packet),
            0x39 if abilities.is_none() => abilities = Some(packet),
            _ => {}
        }
        if window_items.is_some() && held_item.is_some() && abilities.is_some() {
            break;
        }
    }
    let window_items = window_items
        .ok_or_else(|| RuntimeError::Config("window items not received".to_string()))?;
    let held_item = held_item
        .ok_or_else(|| RuntimeError::Config("held item change not received".to_string()))?;
    let abilities = abilities
        .ok_or_else(|| RuntimeError::Config("player abilities not received".to_string()))?;

    assert_eq!(window_items_slot(&window_items, 36)?, Some((1, 64, 0)));
    assert_eq!(window_items_slot(&window_items, 44)?, Some((45, 64, 0)));
    assert_eq!(held_item_from_packet(&held_item)?, 0);
    assert_eq!(player_abilities_flags(&abilities)? & 0x0d, 0x0d);

    server.shutdown().await
}

#[tokio::test]
async fn unsupported_status_protocol_receives_server_list_response() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(47, 1)?).await?;
    write_packet(&mut stream, &codec, &status_request()).await?;
    let mut buffer = BytesMut::new();
    let status_response = read_packet(&mut stream, &codec, &mut buffer).await?;
    assert_eq!(packet_id(&status_response), 0x00);
    let mut reader = PacketReader::new(&status_response);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x00);
    let payload = reader
        .read_string(32767)
        .expect("status json should decode");
    assert!(payload.contains("\"protocol\":5"));
    assert!(payload.contains("\"name\":\"1.7.10\""));

    write_packet(&mut stream, &codec, &status_ping(99)).await?;
    let pong = read_packet(&mut stream, &codec, &mut buffer).await?;
    assert_eq!(packet_id(&pong), 0x01);

    server.shutdown().await
}

#[test]
fn udp_bedrock_probe_classifies_placeholder_datagram() -> Result<(), RuntimeError> {
    let registry = plugin_test_registries_all()?;
    let action = classify_udp_datagram(registry.protocols(), &raknet_unconnected_ping())?;
    assert_eq!(action, UdpDatagramAction::UnsupportedBedrock);
    Ok(())
}

#[test]
fn udp_unknown_datagram_is_ignored() -> Result<(), RuntimeError> {
    let registry = plugin_test_registries_all()?;
    let action = classify_udp_datagram(registry.protocols(), &[0xde, 0xad, 0xbe, 0xef])?;
    assert_eq!(action, UdpDatagramAction::Ignore);
    Ok(())
}

#[tokio::test]
async fn udp_bedrock_probe_does_not_block_je_status() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            be_enabled: true,
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_all()?,
    )
    .await?;

    let udp_addr = udp_listener_addr(&server);
    let udp_client = UdpSocket::bind("127.0.0.1:0").await?;
    udp_client
        .send_to(&raknet_unconnected_ping(), udp_addr)
        .await?;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(5, 1)?).await?;
    write_packet(&mut stream, &codec, &status_request()).await?;
    let mut buffer = BytesMut::new();
    let status_response = read_packet(&mut stream, &codec, &mut buffer).await?;
    let mut reader = PacketReader::new(&status_response);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x00);
    let payload = reader
        .read_string(32767)
        .expect("status json should decode");
    assert!(payload.contains("\"online\":0"));

    server.shutdown().await
}

#[tokio::test]
async fn online_mode_requires_online_auth_profile() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let result = spawn_server(
        ServerConfig {
            online_mode: true,
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await;
    let Err(error) = result else {
        panic!("online-mode should require an online auth profile");
    };
    assert!(
        matches!(error, RuntimeError::Config(message) if message.contains("requires an online auth profile"))
    );
    Ok(())
}

#[tokio::test]
async fn offline_mode_rejects_online_auth_profile() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let result = spawn_server(
        ServerConfig {
            auth_profile: ONLINE_STUB_AUTH_PROFILE_ID.to_string(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        in_process_online_auth_registries(&[JE_1_7_10_ADAPTER_ID])?,
    )
    .await;
    let Err(error) = result else {
        panic!("offline mode should reject online auth profile");
    };
    assert!(
        matches!(error, RuntimeError::Config(message) if message.contains("requires an offline auth profile"))
    );
    Ok(())
}

#[tokio::test]
async fn online_auth_supports_encrypted_login_across_java_versions() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            online_mode: true,
            auth_profile: ONLINE_STUB_AUTH_PROFILE_ID.to_string(),
            enabled_adapters: Some(vec![
                JE_1_7_10_ADAPTER_ID.to_string(),
                JE_1_8_X_ADAPTER_ID.to_string(),
                JE_1_12_2_ADAPTER_ID.to_string(),
            ]),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        in_process_online_auth_registries(&[
            JE_1_7_10_ADAPTER_ID,
            JE_1_8_X_ADAPTER_ID,
            JE_1_12_2_ADAPTER_ID,
        ])?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    for (protocol_version, username, expected_packet_id) in [
        (5, "legacy-online", 0x30),
        (47, "middle-online", 0x30),
        (340, "latest-online", 0x14),
    ] {
        let mut stream = connect_tcp(addr).await?;
        let (mut encryption, mut buffer) =
            perform_online_login(&mut stream, &codec, protocol_version, username).await?;
        let login_success = read_until_packet_id_encrypted(
            &mut stream,
            &codec,
            &mut buffer,
            0x02,
            8,
            &mut encryption,
        )
        .await?;
        assert_eq!(packet_id(&login_success), 0x02);

        let bootstrap = read_until_packet_id_encrypted(
            &mut stream,
            &codec,
            &mut buffer,
            expected_packet_id,
            24,
            &mut encryption,
        )
        .await?;
        assert_eq!(packet_id(&bootstrap), expected_packet_id);
    }

    server.shutdown().await
}

#[tokio::test]
async fn encrypted_play_packets_are_processed_after_online_login() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            online_mode: true,
            auth_profile: ONLINE_STUB_AUTH_PROFILE_ID.to_string(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        in_process_online_auth_registries(&[JE_1_7_10_ADAPTER_ID])?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let mut stream = connect_tcp(addr).await?;
    let (mut encryption, mut buffer) =
        perform_online_login(&mut stream, &codec, 5, "encrypted-alpha").await?;
    let _ =
        read_until_packet_id_encrypted(&mut stream, &codec, &mut buffer, 0x30, 16, &mut encryption)
            .await?;
    let _ =
        read_until_packet_id_encrypted(&mut stream, &codec, &mut buffer, 0x09, 16, &mut encryption)
            .await?;

    write_packet_encrypted(&mut stream, &codec, &held_item_change(4), &mut encryption).await?;
    let held_item =
        read_until_packet_id_encrypted(&mut stream, &codec, &mut buffer, 0x09, 8, &mut encryption)
            .await?;
    assert_eq!(held_item_from_packet(&held_item)?, 4);

    server.shutdown().await
}

#[tokio::test]
async fn verify_token_mismatch_disconnects_in_online_mode() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            online_mode: true,
            auth_profile: ONLINE_STUB_AUTH_PROFILE_ID.to_string(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        in_process_online_auth_registries(&[JE_1_7_10_ADAPTER_ID])?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut stream, &codec, &login_start("mismatch")).await?;
    let mut buffer = BytesMut::new();
    let request = read_packet(&mut stream, &codec, &mut buffer).await?;
    let (_server_id, public_key_der, _verify_token) = parse_encryption_request(&request)?;
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
        .encrypt(&mut rand::rngs::OsRng, Pkcs1v15Encrypt, &[9, 9, 9, 9])
        .map_err(|error| {
            RuntimeError::Config(format!("failed to encrypt verify token: {error}"))
        })?;
    let response = login_encryption_response(&shared_secret_encrypted, &verify_token_encrypted)?;
    write_packet(&mut stream, &codec, &response).await?;

    let mut encryption = TestClientEncryptionState::new(shared_secret);
    let disconnect =
        read_until_packet_id_encrypted(&mut stream, &codec, &mut buffer, 0x00, 4, &mut encryption)
            .await?;
    assert_eq!(packet_id(&disconnect), 0x00);

    server.shutdown().await
}

#[tokio::test]
async fn unknown_default_adapter_fails_fast() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let result = spawn_server(
        ServerConfig {
            default_adapter: "missing".to_string(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await;
    let Err(error) = result else {
        panic!("unknown default adapter should fail fast");
    };
    assert!(
        matches!(error, RuntimeError::Config(message) if message.contains("unknown default-adapter"))
    );
    Ok(())
}

#[tokio::test]
async fn unknown_gameplay_profile_fails_fast() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let result = spawn_server(
        ServerConfig {
            default_gameplay_profile: "missing".to_string(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await;
    let Err(error) = result else {
        panic!("unknown gameplay profile should fail fast");
    };
    assert!(
        matches!(error, RuntimeError::Config(message) if message.contains("unknown gameplay profile"))
    );
    Ok(())
}

#[tokio::test]
async fn unknown_storage_profile_fails_fast() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let result = spawn_server(
        ServerConfig {
            storage_profile: "missing".to_string(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await;
    let Err(error) = result else {
        panic!("unknown storage profile should fail fast");
    };
    assert!(
        matches!(error, RuntimeError::Config(message) if message.contains("unknown storage profile"))
    );
    Ok(())
}

#[tokio::test]
async fn unknown_auth_profile_fails_fast() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let result = spawn_server(
        ServerConfig {
            auth_profile: "missing".to_string(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await;
    let Err(error) = result else {
        panic!("unknown auth profile should fail fast");
    };
    assert!(
        matches!(error, RuntimeError::Config(message) if message.contains("unknown auth profile"))
    );
    Ok(())
}

#[tokio::test]
async fn unmatched_probe_closes_without_response() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &[0x01]).await?;

    let mut bytes = [0_u8; 1];
    let read = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut bytes))
        .await
        .map_err(|_| RuntimeError::Config("probe mismatch did not close".to_string()))??;
    assert_eq!(read, 0);

    server.shutdown().await
}

#[tokio::test]
async fn unsupported_login_protocol_receives_disconnect() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(47, 2)?).await?;
    let mut buffer = BytesMut::new();
    let disconnect = read_packet(&mut stream, &codec, &mut buffer).await?;
    let mut reader = PacketReader::new(&disconnect);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x00);
    let reason = reader
        .read_string(32767)
        .expect("disconnect reason should decode");
    assert!(reason.contains("Unsupported protocol 47"));
    assert!(reason.contains("1.7.10"));

    server.shutdown().await
}

#[tokio::test]
async fn mixed_java_versions_share_login_movement_and_block_sync() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            enabled_adapters: Some(vec![
                JE_1_7_10_ADAPTER_ID.to_string(),
                JE_1_8_X_ADAPTER_ID.to_string(),
                JE_1_12_2_ADAPTER_ID.to_string(),
            ]),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_all()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut legacy = connect_tcp(addr).await?;
    write_packet(&mut legacy, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut legacy, &codec, &login_start("legacy")).await?;
    let mut legacy_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x30, 12).await?;

    let mut modern_18 = connect_tcp(addr).await?;
    write_packet(&mut modern_18, &codec, &encode_handshake(47, 2)?).await?;
    write_packet(&mut modern_18, &codec, &login_start("middle")).await?;
    let mut modern_18_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut modern_18, &codec, &mut modern_18_buffer, 0x30, 24).await?;
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;

    let mut modern_112 = connect_tcp(addr).await?;
    write_packet(&mut modern_112, &codec, &encode_handshake(340, 2)?).await?;
    write_packet(&mut modern_112, &codec, &login_start("latest")).await?;
    let mut modern_112_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut modern_112, &codec, &mut modern_112_buffer, 0x14, 24).await?;
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;
    let _ = read_until_packet_id(&mut modern_18, &codec, &mut modern_18_buffer, 0x0c, 12).await?;

    write_packet(
        &mut modern_18,
        &codec,
        &player_position_look_1_8(32.5, 4.0, 0.5, 90.0, 0.0),
    )
    .await?;
    let legacy_teleport =
        read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x18, 16).await?;
    let modern_112_teleport =
        read_until_packet_id(&mut modern_112, &codec, &mut modern_112_buffer, 0x4c, 16).await?;
    assert_eq!(packet_id(&legacy_teleport), 0x18);
    assert_eq!(packet_id(&modern_112_teleport), 0x4c);

    write_packet(
        &mut modern_112,
        &codec,
        &player_block_placement_1_12(2, 3, 0, 1, 0),
    )
    .await?;
    let legacy_block_change =
        read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x23, 16).await?;
    let modern_18_block_change =
        read_until_packet_id(&mut modern_18, &codec, &mut modern_18_buffer, 0x23, 16).await?;
    assert_eq!(
        block_change_from_packet(&legacy_block_change)?,
        (2, 4, 0, 1, 0)
    );
    assert_eq!(
        block_change_from_packet_1_8(&modern_18_block_change)?,
        (2, 4, 0, 16)
    );

    server.shutdown().await
}

#[tokio::test]
async fn adapter_mapped_gameplay_profiles_can_run_concurrently() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            enabled_adapters: Some(vec![
                JE_1_7_10_ADAPTER_ID.to_string(),
                JE_1_12_2_ADAPTER_ID.to_string(),
            ]),
            default_gameplay_profile: "canonical".to_string(),
            gameplay_profile_map: gameplay_profile_map(&[
                (JE_1_7_10_ADAPTER_ID, "readonly"),
                (JE_1_12_2_ADAPTER_ID, "canonical"),
            ]),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_all()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut legacy = connect_tcp(addr).await?;
    write_packet(&mut legacy, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut legacy, &codec, &login_start("legacy-readonly")).await?;
    let mut legacy_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x30, 12).await?;

    let mut modern = connect_tcp(addr).await?;
    write_packet(&mut modern, &codec, &encode_handshake(340, 2)?).await?;
    write_packet(&mut modern, &codec, &login_start("modern-canonical")).await?;
    let mut modern_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut modern, &codec, &mut modern_buffer, 0x14, 24).await?;
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;

    write_packet(
        &mut modern,
        &codec,
        &player_block_placement_1_12(2, 3, 0, 1, 0),
    )
    .await?;
    let _ = read_until_packet_id(&mut modern, &codec, &mut modern_buffer, 0x0b, 16).await?;
    let legacy_block_change =
        read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x23, 16).await?;
    assert_eq!(
        block_change_from_packet(&legacy_block_change)?,
        (2, 4, 0, 1, 0)
    );

    write_packet(
        &mut legacy,
        &codec,
        &player_block_placement(3, 3, 0, 1, Some((1, 64, 0))),
    )
    .await?;
    assert_no_packet_id(&mut modern, &codec, &mut modern_buffer, 0x0b).await?;

    write_packet(
        &mut legacy,
        &codec,
        &player_position_look(12.5, 4.0, 0.5, 0.0, 0.0),
    )
    .await?;
    let modern_teleport =
        read_until_packet_id(&mut modern, &codec, &mut modern_buffer, 0x4c, 16).await?;
    assert_eq!(packet_id(&modern_teleport), 0x4c);

    server.shutdown().await
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn packaged_plugins_support_mixed_versions_and_bedrock_probe() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let registries = plugin_test_registries_all()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            be_enabled: true,
            game_mode: 1,
            enabled_adapters: Some(vec![
                JE_1_7_10_ADAPTER_ID.to_string(),
                JE_1_8_X_ADAPTER_ID.to_string(),
                JE_1_12_2_ADAPTER_ID.to_string(),
                BE_PLACEHOLDER_ADAPTER_ID.to_string(),
            ]),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        registries,
    )
    .await?;

    let udp_addr = udp_listener_addr(&server);
    let udp_client = UdpSocket::bind("127.0.0.1:0").await?;
    udp_client
        .send_to(&raknet_unconnected_ping(), udp_addr)
        .await?;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut status_stream = connect_tcp(addr).await?;
    write_packet(&mut status_stream, &codec, &encode_handshake(5, 1)?).await?;
    write_packet(&mut status_stream, &codec, &[0x00]).await?;
    let mut status_buffer = BytesMut::new();
    let status = read_packet(&mut status_stream, &codec, &mut status_buffer).await?;
    assert_eq!(packet_id(&status), 0x00);

    let mut legacy = connect_tcp(addr).await?;
    write_packet(&mut legacy, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut legacy, &codec, &login_start("legacy")).await?;
    let mut legacy_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x30, 12).await?;

    let mut modern_18 = connect_tcp(addr).await?;
    write_packet(&mut modern_18, &codec, &encode_handshake(47, 2)?).await?;
    write_packet(&mut modern_18, &codec, &login_start("middle")).await?;
    let mut modern_18_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut modern_18, &codec, &mut modern_18_buffer, 0x30, 24).await?;
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;

    let mut modern_112 = connect_tcp(addr).await?;
    write_packet(&mut modern_112, &codec, &encode_handshake(340, 2)?).await?;
    write_packet(&mut modern_112, &codec, &login_start("latest")).await?;
    let mut modern_112_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut modern_112, &codec, &mut modern_112_buffer, 0x14, 24).await?;
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;
    let _ = read_until_packet_id(&mut modern_18, &codec, &mut modern_18_buffer, 0x0c, 12).await?;

    write_packet(
        &mut modern_18,
        &codec,
        &player_position_look_1_8(32.5, 4.0, 0.5, 90.0, 0.0),
    )
    .await?;
    let legacy_teleport =
        read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x18, 16).await?;
    let modern_112_teleport =
        read_until_packet_id(&mut modern_112, &codec, &mut modern_112_buffer, 0x4c, 16).await?;
    assert_eq!(packet_id(&legacy_teleport), 0x18);
    assert_eq!(packet_id(&modern_112_teleport), 0x4c);

    write_packet(
        &mut modern_112,
        &codec,
        &player_block_placement_1_12(2, 3, 0, 1, 0),
    )
    .await?;
    let legacy_block_change =
        read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x23, 16).await?;
    let modern_18_block_change =
        read_until_packet_id(&mut modern_18, &codec, &mut modern_18_buffer, 0x23, 16).await?;
    assert_eq!(
        block_change_from_packet(&legacy_block_change)?,
        (2, 4, 0, 1, 0)
    );
    assert_eq!(
        block_change_from_packet_1_8(&modern_18_block_change)?,
        (2, 4, 0, 16)
    );

    server.shutdown().await
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn gameplay_reload_updates_target_profile_generation_only() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("gameplay-reload-success");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;
    let registries = plugin_test_registries_from_dist(
        dist_dir.clone(),
        &[JE_1_7_10_ADAPTER_ID, JE_1_12_2_ADAPTER_ID],
    )?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            enabled_adapters: Some(vec![
                JE_1_7_10_ADAPTER_ID.to_string(),
                JE_1_12_2_ADAPTER_ID.to_string(),
            ]),
            default_gameplay_profile: "canonical".to_string(),
            gameplay_profile_map: gameplay_profile_map(&[
                (JE_1_7_10_ADAPTER_ID, "readonly"),
                (JE_1_12_2_ADAPTER_ID, "canonical"),
            ]),
            plugins_dir: dist_dir.clone(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        registries,
    )
    .await?;
    let plugin_host = server
        .plugin_host
        .as_ref()
        .expect("runtime should keep plugin host");
    let canonical_before = plugin_host
        .resolve_gameplay_profile("canonical")
        .expect("canonical gameplay profile should resolve");
    let readonly_before = plugin_host
        .resolve_gameplay_profile("readonly")
        .expect("readonly gameplay profile should resolve");
    let canonical_generation = canonical_before
        .plugin_generation_id()
        .expect("canonical profile should report generation");
    let readonly_generation = readonly_before
        .plugin_generation_id()
        .expect("readonly profile should report generation");
    assert!(canonical_before.capability_set().contains(&format!(
        "build-tag:{}",
        crate::PACKAGED_PLUGIN_TEST_HARNESS_TAG
    )));

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut legacy = connect_tcp(addr).await?;
    write_packet(&mut legacy, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut legacy, &codec, &login_start("legacy-observer")).await?;
    let mut legacy_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x30, 12).await?;

    let mut modern = connect_tcp(addr).await?;
    write_packet(&mut modern, &codec, &encode_handshake(340, 2)?).await?;
    write_packet(&mut modern, &codec, &login_start("modern-reload")).await?;
    let mut modern_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut modern, &codec, &mut modern_buffer, 0x14, 24).await?;
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;

    std::thread::sleep(Duration::from_secs(1));
    package_single_gameplay_plugin(
        "mc-plugin-gameplay-canonical",
        "gameplay-canonical",
        &dist_dir,
        &target_dir,
        "gameplay-reload-v2",
    )?;

    let reloaded = server.reload_plugins().await?;
    assert!(
        reloaded
            .iter()
            .any(|plugin_id| plugin_id == "gameplay-canonical"),
        "gameplay reload should report canonical plugin reload"
    );

    let canonical_after = plugin_host
        .resolve_gameplay_profile("canonical")
        .expect("canonical gameplay profile should still resolve");
    let readonly_after = plugin_host
        .resolve_gameplay_profile("readonly")
        .expect("readonly gameplay profile should still resolve");
    assert_ne!(
        canonical_after.plugin_generation_id(),
        Some(canonical_generation)
    );
    assert_eq!(
        readonly_after.plugin_generation_id(),
        Some(readonly_generation)
    );
    assert!(
        canonical_after
            .capability_set()
            .contains("build-tag:gameplay-reload-v2")
    );

    write_packet(
        &mut modern,
        &codec,
        &player_position_look_1_12(18.5, 4.0, 0.5, 30.0, 0.0),
    )
    .await?;
    let legacy_teleport =
        read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x18, 16).await?;
    assert_eq!(packet_id(&legacy_teleport), 0x18);

    server.shutdown().await
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn gameplay_reload_failure_keeps_existing_generation() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("gameplay-reload-failure");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;
    let registries = plugin_test_registries_from_dist(
        dist_dir.clone(),
        &[JE_1_7_10_ADAPTER_ID, JE_1_12_2_ADAPTER_ID],
    )?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            enabled_adapters: Some(vec![
                JE_1_7_10_ADAPTER_ID.to_string(),
                JE_1_12_2_ADAPTER_ID.to_string(),
            ]),
            default_gameplay_profile: "canonical".to_string(),
            gameplay_profile_map: gameplay_profile_map(&[
                (JE_1_7_10_ADAPTER_ID, "readonly"),
                (JE_1_12_2_ADAPTER_ID, "canonical"),
            ]),
            plugins_dir: dist_dir.clone(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        registries,
    )
    .await?;
    let plugin_host = server
        .plugin_host
        .as_ref()
        .expect("runtime should keep plugin host");
    let canonical_before = plugin_host
        .resolve_gameplay_profile("canonical")
        .expect("canonical gameplay profile should resolve");
    let before_generation = canonical_before
        .plugin_generation_id()
        .expect("canonical profile should report generation");
    assert!(canonical_before.capability_set().contains(&format!(
        "build-tag:{}",
        crate::PACKAGED_PLUGIN_TEST_HARNESS_TAG
    )));

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut legacy = connect_tcp(addr).await?;
    write_packet(&mut legacy, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut legacy, &codec, &login_start("legacy-failure")).await?;
    let mut legacy_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x30, 12).await?;

    let mut modern = connect_tcp(addr).await?;
    write_packet(&mut modern, &codec, &encode_handshake(340, 2)?).await?;
    write_packet(&mut modern, &codec, &login_start("modern-failure")).await?;
    let mut modern_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut modern, &codec, &mut modern_buffer, 0x14, 24).await?;
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;

    std::thread::sleep(Duration::from_secs(1));
    package_single_gameplay_plugin(
        "mc-plugin-gameplay-canonical",
        "gameplay-canonical",
        &dist_dir,
        &target_dir,
        "gameplay-reload-fail",
    )?;

    let reloaded = server.reload_plugins().await?;
    assert!(
        !reloaded
            .iter()
            .any(|plugin_id| plugin_id == "gameplay-canonical"),
        "failed gameplay migration should not swap the canonical generation"
    );

    let canonical_after = plugin_host
        .resolve_gameplay_profile("canonical")
        .expect("canonical gameplay profile should still resolve");
    assert_eq!(
        canonical_after.plugin_generation_id(),
        Some(before_generation)
    );
    assert!(canonical_after.capability_set().contains(&format!(
        "build-tag:{}",
        crate::PACKAGED_PLUGIN_TEST_HARNESS_TAG
    )));

    write_packet(
        &mut modern,
        &codec,
        &player_position_look_1_12(22.5, 4.0, 0.5, 45.0, 0.0),
    )
    .await?;
    let legacy_teleport =
        read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x18, 16).await?;
    assert_eq!(packet_id(&legacy_teleport), 0x18);

    server.shutdown().await
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn storage_reload_updates_generation_and_preserves_persistence() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("storage-reload-success");
    let world_dir = temp_dir.path().join("world");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;
    let registries = plugin_test_registries_from_dist(dist_dir.clone(), &[JE_1_7_10_ADAPTER_ID])?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            plugins_dir: dist_dir.clone(),
            world_dir: world_dir.clone(),
            ..ServerConfig::default()
        },
        registries,
    )
    .await?;
    let plugin_host = server
        .plugin_host
        .as_ref()
        .expect("runtime should keep plugin host");
    let storage_before = plugin_host
        .resolve_storage_profile(JE_1_7_10_STORAGE_PROFILE_ID)
        .expect("storage profile should resolve");
    let before_generation = storage_before
        .plugin_generation_id()
        .expect("storage profile should report generation");
    assert!(storage_before.capability_set().contains(&format!(
        "build-tag:{}",
        crate::PACKAGED_PLUGIN_TEST_HARNESS_TAG
    )));

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut stream, &codec, &login_start("alpha")).await?;
    let mut buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x30, 12).await?;
    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(36, 20, 64, 0),
    )
    .await?;
    let _ = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x2f, 8).await?;

    std::thread::sleep(Duration::from_secs(1));
    package_single_plugin(
        "mc-plugin-storage-je-anvil-1_7_10",
        "storage-je-anvil-1_7_10",
        "storage",
        &dist_dir,
        &target_dir,
        "storage-reload-v2",
    )?;

    let reloaded = server.reload_plugins().await?;
    assert!(
        reloaded
            .iter()
            .any(|plugin_id| plugin_id == "storage-je-anvil-1_7_10"),
        "storage reload should report generation swap"
    );

    let storage_after = plugin_host
        .resolve_storage_profile(JE_1_7_10_STORAGE_PROFILE_ID)
        .expect("storage profile should still resolve");
    assert_ne!(
        storage_after.plugin_generation_id(),
        Some(before_generation)
    );
    assert!(
        storage_after
            .capability_set()
            .contains("build-tag:storage-reload-v2")
    );

    server.shutdown().await?;

    let restarted = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            plugins_dir: dist_dir.clone(),
            world_dir: world_dir.clone(),
            ..ServerConfig::default()
        },
        plugin_test_registries_from_dist(dist_dir, &[JE_1_7_10_ADAPTER_ID])?,
    )
    .await?;
    let addr = listener_addr(&restarted);
    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut stream, &codec, &login_start("alpha")).await?;
    let mut buffer = BytesMut::new();
    let window_items = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x30, 12).await?;
    assert_eq!(window_items_slot(&window_items, 36)?, Some((20, 64, 0)));

    restarted.shutdown().await
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn storage_reload_failure_keeps_existing_generation() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("storage-reload-failure");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            plugins_dir: dist_dir.clone(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_1_7_10_ADAPTER_ID])?,
    )
    .await?;
    let plugin_host = server
        .plugin_host
        .as_ref()
        .expect("runtime should keep plugin host");
    let storage_before = plugin_host
        .resolve_storage_profile(JE_1_7_10_STORAGE_PROFILE_ID)
        .expect("storage profile should resolve");
    let before_generation = storage_before
        .plugin_generation_id()
        .expect("storage profile should report generation");

    std::thread::sleep(Duration::from_secs(1));
    package_single_plugin(
        "mc-plugin-storage-je-anvil-1_7_10",
        "storage-je-anvil-1_7_10",
        "storage",
        &dist_dir,
        &target_dir,
        "storage-reload-fail",
    )?;

    let reloaded = server.reload_plugins().await?;
    assert!(
        !reloaded
            .iter()
            .any(|plugin_id| plugin_id == "storage-je-anvil-1_7_10"),
        "failed storage migration should not swap the storage generation"
    );

    let storage_after = plugin_host
        .resolve_storage_profile(JE_1_7_10_STORAGE_PROFILE_ID)
        .expect("storage profile should still resolve");
    assert_eq!(
        storage_after.plugin_generation_id(),
        Some(before_generation)
    );
    assert!(storage_after.capability_set().contains(&format!(
        "build-tag:{}",
        crate::PACKAGED_PLUGIN_TEST_HARNESS_TAG
    )));

    server.shutdown().await
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn auth_reload_updates_generation_for_new_logins_only() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("auth-reload-offline");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            plugins_dir: dist_dir.clone(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_1_7_10_ADAPTER_ID])?,
    )
    .await?;
    let plugin_host = server
        .plugin_host
        .as_ref()
        .expect("runtime should keep plugin host");
    let auth_before = plugin_host
        .resolve_auth_profile(OFFLINE_AUTH_PROFILE_ID)
        .expect("auth profile should resolve");
    let before_generation = auth_before
        .plugin_generation_id()
        .expect("auth profile should report generation");
    assert!(auth_before.capability_set().contains(&format!(
        "build-tag:{}",
        crate::PACKAGED_PLUGIN_TEST_HARNESS_TAG
    )));

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let mut alpha = connect_tcp(addr).await?;
    write_packet(&mut alpha, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut alpha, &codec, &login_start("alpha")).await?;
    let mut alpha_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut alpha, &codec, &mut alpha_buffer, 0x30, 12).await?;
    let _ = read_until_packet_id(&mut alpha, &codec, &mut alpha_buffer, 0x09, 12).await?;

    std::thread::sleep(Duration::from_secs(1));
    package_single_plugin(
        "mc-plugin-auth-offline",
        "auth-offline",
        "auth",
        &dist_dir,
        &target_dir,
        "auth-reload-v2",
    )?;

    let reloaded = server.reload_plugins().await?;
    assert!(
        reloaded.iter().any(|plugin_id| plugin_id == "auth-offline"),
        "auth reload should report generation swap"
    );

    let auth_after = plugin_host
        .resolve_auth_profile(OFFLINE_AUTH_PROFILE_ID)
        .expect("auth profile should still resolve");
    assert_ne!(auth_after.plugin_generation_id(), Some(before_generation));
    assert!(
        auth_after
            .capability_set()
            .contains("build-tag:auth-reload-v2")
    );

    write_packet(&mut alpha, &codec, &held_item_change(4)).await?;
    let held_item = read_until_packet_id(&mut alpha, &codec, &mut alpha_buffer, 0x09, 8).await?;
    assert_eq!(held_item_from_packet(&held_item)?, 4);

    let mut beta = connect_tcp(addr).await?;
    write_packet(&mut beta, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut beta, &codec, &login_start("beta")).await?;
    let mut beta_buffer = BytesMut::new();
    let login_success = read_until_packet_id(&mut beta, &codec, &mut beta_buffer, 0x02, 8).await?;
    assert_eq!(packet_id(&login_success), 0x02);

    server.shutdown().await
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn packaged_online_auth_stub_boot_supports_mixed_versions() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("auth-online-packaged");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;
    package_single_plugin(
        "mc-plugin-auth-online-stub",
        ONLINE_STUB_AUTH_PLUGIN_ID,
        "auth",
        &dist_dir,
        &target_dir,
        "online-stub-v1",
    )?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            online_mode: true,
            auth_profile: ONLINE_STUB_AUTH_PROFILE_ID.to_string(),
            enabled_adapters: Some(vec![
                JE_1_7_10_ADAPTER_ID.to_string(),
                JE_1_8_X_ADAPTER_ID.to_string(),
                JE_1_12_2_ADAPTER_ID.to_string(),
            ]),
            plugins_dir: dist_dir.clone(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_from_dist_with_supporting_plugins(
            dist_dir,
            &[
                JE_1_7_10_ADAPTER_ID,
                JE_1_8_X_ADAPTER_ID,
                JE_1_12_2_ADAPTER_ID,
            ],
            &["storage-je-anvil-1_7_10", ONLINE_STUB_AUTH_PLUGIN_ID],
        )?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    for (protocol_version, username, expected_packet_id) in [
        (5, "packaged-legacy", 0x30),
        (47, "packaged-middle", 0x30),
        (340, "packaged-latest", 0x14),
    ] {
        let mut stream = connect_tcp(addr).await?;
        let (mut encryption, mut buffer) =
            perform_online_login(&mut stream, &codec, protocol_version, username).await?;
        let login_success = read_until_packet_id_encrypted(
            &mut stream,
            &codec,
            &mut buffer,
            0x02,
            8,
            &mut encryption,
        )
        .await?;
        assert_eq!(packet_id(&login_success), 0x02);

        let bootstrap = read_until_packet_id_encrypted(
            &mut stream,
            &codec,
            &mut buffer,
            expected_packet_id,
            24,
            &mut encryption,
        )
        .await?;
        assert_eq!(packet_id(&bootstrap), expected_packet_id);
    }

    server.shutdown().await
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn online_auth_reload_keeps_existing_challenge_generation() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("auth-online-reload");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;
    package_single_plugin(
        "mc-plugin-auth-online-stub",
        ONLINE_STUB_AUTH_PLUGIN_ID,
        "auth",
        &dist_dir,
        &target_dir,
        "online-auth-v1",
    )?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            online_mode: true,
            auth_profile: ONLINE_STUB_AUTH_PROFILE_ID.to_string(),
            plugins_dir: dist_dir.clone(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_from_dist_with_supporting_plugins(
            dist_dir.clone(),
            &[JE_1_7_10_ADAPTER_ID],
            &["storage-je-anvil-1_7_10", ONLINE_STUB_AUTH_PLUGIN_ID],
        )?,
    )
    .await?;
    let plugin_host = server
        .plugin_host
        .as_ref()
        .expect("runtime should keep plugin host");
    let auth_before = plugin_host
        .resolve_auth_profile(ONLINE_STUB_AUTH_PROFILE_ID)
        .expect("online auth profile should resolve");
    let before_generation = auth_before
        .plugin_generation_id()
        .expect("online auth profile should report generation");
    assert!(
        auth_before
            .capability_set()
            .contains("build-tag:online-auth-v1")
    );

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let mut alpha = connect_tcp(addr).await?;
    write_packet(&mut alpha, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut alpha, &codec, &login_start("alpha-online")).await?;
    let mut alpha_buffer = BytesMut::new();
    let request = read_packet(&mut alpha, &codec, &mut alpha_buffer).await?;
    let (_server_id, public_key_der, verify_token) = parse_encryption_request(&request)?;
    let public_key = RsaPublicKey::from_public_key_der(&public_key_der)
        .map_err(|error| RuntimeError::Config(format!("invalid test public key: {error}")))?;

    std::thread::sleep(Duration::from_secs(1));
    package_single_plugin(
        "mc-plugin-auth-online-stub",
        ONLINE_STUB_AUTH_PLUGIN_ID,
        "auth",
        &dist_dir,
        &target_dir,
        "online-auth-v2",
    )?;
    let reloaded = server.reload_plugins().await?;
    assert!(
        reloaded
            .iter()
            .any(|plugin_id| plugin_id == ONLINE_STUB_AUTH_PLUGIN_ID)
    );

    let auth_after = plugin_host
        .resolve_auth_profile(ONLINE_STUB_AUTH_PROFILE_ID)
        .expect("online auth profile should still resolve");
    assert_ne!(auth_after.plugin_generation_id(), Some(before_generation));
    assert!(
        auth_after
            .capability_set()
            .contains("build-tag:online-auth-v2")
    );

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
    write_packet(&mut alpha, &codec, &response).await?;

    let mut alpha_encryption = TestClientEncryptionState::new(shared_secret);
    let login_success = read_until_packet_id_encrypted(
        &mut alpha,
        &codec,
        &mut alpha_buffer,
        0x02,
        8,
        &mut alpha_encryption,
    )
    .await?;
    assert_eq!(packet_id(&login_success), 0x02);

    let mut beta = connect_tcp(addr).await?;
    let (mut beta_encryption, mut beta_buffer) =
        perform_online_login(&mut beta, &codec, 5, "beta-online").await?;
    let beta_login_success = read_until_packet_id_encrypted(
        &mut beta,
        &codec,
        &mut beta_buffer,
        0x02,
        8,
        &mut beta_encryption,
    )
    .await?;
    assert_eq!(packet_id(&beta_login_success), 0x02);

    server.shutdown().await
}

#[tokio::test]
async fn modern_offhand_persists_without_leaking_legacy_slots() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    let codec = MinecraftWireCodec;

    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            enabled_adapters: Some(vec![
                JE_1_7_10_ADAPTER_ID.to_string(),
                JE_1_8_X_ADAPTER_ID.to_string(),
                JE_1_12_2_ADAPTER_ID.to_string(),
            ]),
            world_dir: world_dir.clone(),
            ..ServerConfig::default()
        },
        plugin_test_registries_all()?,
    )
    .await?;
    let addr = listener_addr(&server);

    let mut modern = connect_tcp(addr).await?;
    write_packet(&mut modern, &codec, &encode_handshake(340, 2)?).await?;
    write_packet(&mut modern, &codec, &login_start("alpha")).await?;
    let mut modern_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut modern, &codec, &mut modern_buffer, 0x14, 24).await?;

    write_packet(
        &mut modern,
        &codec,
        &creative_inventory_action_1_12(45, 20, 64, 0),
    )
    .await?;
    let set_slot = read_until_packet_id(&mut modern, &codec, &mut modern_buffer, 0x16, 8).await?;
    assert_eq!(set_slot_slot(&set_slot, 0x16)?, 45);

    server.shutdown().await?;

    let restarted = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            enabled_adapters: Some(vec![
                JE_1_7_10_ADAPTER_ID.to_string(),
                JE_1_8_X_ADAPTER_ID.to_string(),
                JE_1_12_2_ADAPTER_ID.to_string(),
            ]),
            world_dir,
            ..ServerConfig::default()
        },
        plugin_test_registries_all()?,
    )
    .await?;
    let addr = listener_addr(&restarted);

    let mut modern = connect_tcp(addr).await?;
    write_packet(&mut modern, &codec, &encode_handshake(340, 2)?).await?;
    write_packet(&mut modern, &codec, &login_start("alpha")).await?;
    let mut modern_buffer = BytesMut::new();
    let window_items =
        read_until_packet_id(&mut modern, &codec, &mut modern_buffer, 0x14, 24).await?;
    assert_eq!(
        window_items_slot_with_packet_id(&window_items, 0x14, 45)?,
        Some((20, 64, 0))
    );

    let mut legacy = connect_tcp(addr).await?;
    write_packet(&mut legacy, &codec, &encode_handshake(47, 2)?).await?;
    write_packet(&mut legacy, &codec, &login_start("beta")).await?;
    let mut legacy_buffer = BytesMut::new();
    let legacy_window_items =
        read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x30, 24).await?;
    assert!(window_items_slot(&legacy_window_items, 45).is_err());

    restarted.shutdown().await
}

#[tokio::test]
async fn creative_place_and_break_broadcast_block_changes() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut first = connect_tcp(addr).await?;
    write_packet(&mut first, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut first, &codec, &login_start("alpha")).await?;
    let mut first_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut first, &codec, &mut first_buffer, 0x30, 12).await?;

    let mut second = connect_tcp(addr).await?;
    write_packet(&mut second, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut second, &codec, &login_start("beta")).await?;
    let mut second_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut second, &codec, &mut second_buffer, 0x30, 12).await?;
    let _ = read_until_packet_id(&mut first, &codec, &mut first_buffer, 0x0c, 12).await?;

    write_packet(
        &mut first,
        &codec,
        &player_block_placement(2, 3, 0, 1, Some((1, 64, 0))),
    )
    .await?;
    let place_change =
        read_until_packet_id(&mut second, &codec, &mut second_buffer, 0x23, 8).await?;
    assert_eq!(block_change_from_packet(&place_change)?, (2, 4, 0, 1, 0));

    write_packet(&mut first, &codec, &player_digging(0, 2, 4, 0, 1)).await?;
    let break_change =
        read_until_packet_id(&mut second, &codec, &mut second_buffer, 0x23, 8).await?;
    assert_eq!(block_change_from_packet(&break_change)?, (2, 4, 0, 0, 0));

    server.shutdown().await
}

#[tokio::test]
async fn creative_inventory_and_selected_slot_persist_across_restart() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    let codec = MinecraftWireCodec;

    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            world_dir: world_dir.clone(),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);

    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut stream, &codec, &login_start("alpha")).await?;
    let mut buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x30, 12).await?;
    let _ = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x09, 12).await?;

    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(36, 20, 64, 0),
    )
    .await?;
    let set_slot = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x2f, 8).await?;
    let mut set_slot_reader = PacketReader::new(&set_slot);
    assert_eq!(set_slot_reader.read_varint()?, 0x2f);
    assert_eq!(set_slot_reader.read_i8()?, 0);
    assert_eq!(set_slot_reader.read_i16()?, 36);
    assert_eq!(read_slot(&mut set_slot_reader)?, Some((20, 64, 0)));

    write_packet(&mut stream, &codec, &held_item_change(4)).await?;
    let held_slot_packet = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x09, 8).await?;
    assert_eq!(held_item_from_packet(&held_slot_packet)?, 4);

    server.shutdown().await?;

    let restarted = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            world_dir,
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&restarted);
    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut stream, &codec, &login_start("alpha")).await?;
    let mut buffer = BytesMut::new();
    let window_items = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x30, 12).await?;
    let held_item = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x09, 12).await?;

    assert_eq!(window_items_slot(&window_items, 36)?, Some((20, 64, 0)));
    assert_eq!(held_item_from_packet(&held_item)?, 4);

    restarted.shutdown().await
}

#[tokio::test]
async fn plugin_backed_storage_and_auth_profiles_boot_and_persist() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let codec = MinecraftWireCodec;
    let world_dir = temp_dir.path().join("world");

    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            storage_profile: JE_1_7_10_STORAGE_PROFILE_ID.to_string(),
            auth_profile: OFFLINE_AUTH_PROFILE_ID.to_string(),
            world_dir: world_dir.clone(),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await?;

    let addr = listener_addr(&server);
    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut stream, &codec, &login_start("alpha")).await?;
    let mut buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x02, 8).await?;

    server.shutdown().await?;

    assert!(world_dir.join("level.dat").exists());
    assert!(fs::read_dir(world_dir.join("playerdata"))?.next().is_some());
    Ok(())
}

#[tokio::test]
async fn unsupported_creative_inventory_action_is_corrected() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut stream, &codec, &login_start("alpha")).await?;
    let mut buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x30, 12).await?;

    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(36, 999, 64, 0),
    )
    .await?;
    let set_slot = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x2f, 8).await?;
    let mut reader = PacketReader::new(&set_slot);
    assert_eq!(reader.read_varint()?, 0x2f);
    assert_eq!(reader.read_i8()?, 0);
    assert_eq!(reader.read_i16()?, 36);
    assert_eq!(read_slot(&mut reader)?, Some((1, 64, 0)));

    server.shutdown().await
}

#[tokio::test]
async fn survival_place_is_rejected_with_block_and_inventory_correction() -> Result<(), RuntimeError>
{
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut stream, &codec, &login_start("alpha")).await?;
    let mut buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x30, 12).await?;

    write_packet(
        &mut stream,
        &codec,
        &player_block_placement(2, 3, 0, 1, Some((1, 64, 0))),
    )
    .await?;
    let block_change = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x23, 8).await?;
    let set_slot = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x2f, 8).await?;

    assert_eq!(block_change_from_packet(&block_change)?, (2, 4, 0, 0, 0));
    let mut reader = PacketReader::new(&set_slot);
    assert_eq!(reader.read_varint()?, 0x2f);
    assert_eq!(reader.read_i8()?, 0);
    assert_eq!(reader.read_i16()?, 36);
    assert_eq!(read_slot(&mut reader)?, Some((1, 64, 0)));

    server.shutdown().await
}

#[tokio::test]
async fn two_players_can_see_movement_and_restart_persists_position() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    let codec = MinecraftWireCodec;

    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            world_dir: world_dir.clone(),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);

    let mut first = connect_tcp(addr).await?;
    write_packet(&mut first, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut first, &codec, &login_start("alpha")).await?;
    let mut first_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut first, &codec, &mut first_buffer, 0x08, 8).await?;

    let mut second = connect_tcp(addr).await?;
    write_packet(&mut second, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut second, &codec, &login_start("beta")).await?;
    let mut second_buffer = BytesMut::new();
    let _ = read_until_packet_id(&mut second, &codec, &mut second_buffer, 0x08, 8).await?;
    let spawn_packet = read_until_packet_id(&mut first, &codec, &mut first_buffer, 0x0c, 8).await?;
    assert_eq!(packet_id(&spawn_packet), 0x0c);

    write_packet(
        &mut second,
        &codec,
        &player_position_look(32.5, 4.0, 0.5, 90.0, 0.0),
    )
    .await?;
    let mut saw_teleport = false;
    for _ in 0..4 {
        let packet = read_packet(&mut first, &codec, &mut first_buffer).await?;
        if packet_id(&packet) == 0x18 {
            saw_teleport = true;
            break;
        }
    }
    assert!(saw_teleport);
    second.shutdown().await.ok();
    first.shutdown().await.ok();
    server.shutdown().await?;

    let restarted = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            world_dir,
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&restarted);
    let mut alpha = connect_tcp(addr).await?;
    write_packet(&mut alpha, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut alpha, &codec, &login_start("beta")).await?;
    let mut alpha_buffer = BytesMut::new();
    let position_packet =
        read_until_packet_id(&mut alpha, &codec, &mut alpha_buffer, 0x08, 8).await?;
    assert_eq!(packet_id(&position_packet), 0x08);
    let mut reader = PacketReader::new(&position_packet);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x08);
    let x = reader.read_f64().expect("x should decode");
    let _y = reader.read_f64().expect("y should decode");
    let _z = reader.read_f64().expect("z should decode");
    assert!(x >= 32.0);

    restarted.shutdown().await
}
