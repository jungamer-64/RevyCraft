#![allow(dead_code)]

use aes::Aes128;
use aes::cipher::{BlockEncrypt, KeyInit};
use bytes::BytesMut;
use mc_plugin_test_support::PackagedPluginHarness;
use mc_proto_common::{MinecraftWireCodec, PacketReader, PacketWriter, WireCodec};
use mc_proto_test_support::{TestJavaPacket, TestJavaProtocol};
use rsa::pkcs8::DecodePublicKey;
use rsa::rand_core::{OsRng, RngCore};
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use std::collections::BTreeSet;
use std::fs;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener as StdTcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

pub type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

static LOG_CAPTURE_COUNTER: AtomicU64 = AtomicU64::new(1);

pub const DEFAULT_LOCAL_CONSOLE_PERMISSIONS: &[&str] =
    &["status", "sessions", "reload-runtime", "shutdown"];
pub const DEFAULT_REMOTE_PERMISSIONS: &[&str] =
    &["status", "sessions", "reload-runtime", "shutdown"];
pub const UPGRADE_LOCAL_CONSOLE_PERMISSIONS: &[&str] = &[
    "status",
    "sessions",
    "reload-runtime",
    "upgrade-runtime",
    "shutdown",
];
pub const UPGRADE_REMOTE_PERMISSIONS: &[&str] = &[
    "status",
    "sessions",
    "reload-runtime",
    "upgrade-runtime",
    "shutdown",
];
pub const OPS_TOKEN: &str = "ops-token";

pub struct PersistedServerLogCapture {
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
}

impl PersistedServerLogCapture {
    pub fn read(&self) -> TestResult<(String, String)> {
        Ok((
            fs::read_to_string(&self.stdout_path).unwrap_or_default(),
            fs::read_to_string(&self.stderr_path).unwrap_or_default(),
        ))
    }
}

pub struct ServerTomlOptions<'a> {
    pub remote_admin_enabled: bool,
    pub server_port: u16,
    pub remote_admin_port: u16,
    pub motd: &'a str,
    pub online_mode: bool,
    pub bedrock_enabled: bool,
    pub auth_profile: &'a str,
    pub extra_plugin_allowlist: &'a [&'a str],
    pub plugins_dir_override: Option<PathBuf>,
    pub local_console_permissions: &'a [&'a str],
    pub remote_permissions: &'a [&'a str],
}

impl<'a> ServerTomlOptions<'a> {
    #[must_use]
    pub fn new(
        remote_admin_enabled: bool,
        server_port: u16,
        remote_admin_port: u16,
        motd: &'a str,
    ) -> Self {
        Self {
            remote_admin_enabled,
            server_port,
            remote_admin_port,
            motd,
            online_mode: false,
            bedrock_enabled: false,
            auth_profile: "offline-v1",
            extra_plugin_allowlist: &[],
            plugins_dir_override: None,
            local_console_permissions: DEFAULT_LOCAL_CONSOLE_PERMISSIONS,
            remote_permissions: DEFAULT_REMOTE_PERMISSIONS,
        }
    }
}

pub struct ProcessTestClientEncryptionState {
    pub encrypt: ProcessTestStreamCipher,
    pub decrypt: ProcessTestStreamCipher,
}

pub struct ProcessTestStreamCipher {
    cipher: Aes128,
    shift_register: [u8; 16],
}

impl ProcessTestClientEncryptionState {
    #[must_use]
    pub fn new(shared_secret: [u8; 16]) -> Self {
        Self {
            encrypt: ProcessTestStreamCipher::new(shared_secret),
            decrypt: ProcessTestStreamCipher::new(shared_secret),
        }
    }
}

impl ProcessTestStreamCipher {
    #[must_use]
    pub fn new(shared_secret: [u8; 16]) -> Self {
        Self::from_parts(shared_secret, shared_secret)
    }

    #[must_use]
    pub fn from_parts(shared_secret: [u8; 16], shift_register: [u8; 16]) -> Self {
        Self {
            cipher: Aes128::new_from_slice(&shared_secret)
                .expect("AES-128 key length should be exactly 16 bytes"),
            shift_register,
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

pub fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

pub fn reserve_port() -> Result<u16, Box<dyn std::error::Error>> {
    Ok(StdTcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

pub fn repo_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?)
}

fn runtime_plugin_allowlist<'a>(options: &'a ServerTomlOptions<'a>) -> BTreeSet<&'a str> {
    let mut plugin_allowlist = BTreeSet::from([
        "auth-offline",
        "gameplay-canonical",
        "je-5",
        "storage-je-anvil-1_7_10",
    ]);
    if options.remote_admin_enabled {
        plugin_allowlist.insert("admin-transport-grpc");
    }
    if options.bedrock_enabled {
        plugin_allowlist.insert("auth-bedrock-offline");
        plugin_allowlist.insert("be-924");
    }
    for plugin_id in options.extra_plugin_allowlist {
        plugin_allowlist.insert(plugin_id);
    }
    plugin_allowlist
}

fn seed_runtime_plugins(temp_root: &Path, plugin_ids: &[&str]) -> TestResult<PathBuf> {
    let dist_dir = temp_root.join("runtime").join("plugins");
    PackagedPluginHarness::shared()?.seed_subset(&dist_dir, plugin_ids)?;
    Ok(dist_dir)
}

pub fn write_server_toml(
    temp_dir: &Path,
    repo_root: &Path,
    world_dir: &Path,
    options: &ServerTomlOptions<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    write_server_toml_at(
        temp_dir,
        &temp_dir.join("runtime").join("server.toml"),
        repo_root,
        world_dir,
        options,
    )
}

pub fn write_server_toml_at(
    temp_root: &Path,
    config_path: &Path,
    _repo_root: &Path,
    world_dir: &Path,
    options: &ServerTomlOptions<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime_dir = config_path
        .parent()
        .ok_or("config path should have a parent directory")?;
    let plugin_allowlist = runtime_plugin_allowlist(options);
    let seeded_plugin_ids = plugin_allowlist.iter().copied().collect::<Vec<_>>();
    let plugins_dir = match &options.plugins_dir_override {
        Some(plugins_dir) => plugins_dir.clone(),
        None => seed_runtime_plugins(temp_root, &seeded_plugin_ids)?,
    };
    fs::create_dir_all(runtime_dir)?;
    let remote_permissions = format!(
        "[{}]",
        options
            .remote_permissions
            .iter()
            .map(|permission| toml_string(permission))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let remote_admin_block = if options.remote_admin_enabled {
        let token_path = temp_root.join("admin").join("ops.token");
        fs::create_dir_all(token_path.parent().expect("token parent should exist"))?;
        fs::write(&token_path, format!("{OPS_TOKEN}\n"))?;
        let transport_config_path = runtime_dir.join("admin-transport-grpc.toml");
        fs::write(
            &transport_config_path,
            format!(
                concat!(
                    "bind_addr = {}\n",
                    "allow_non_loopback = false\n\n",
                    "[principals.ops]\n",
                    "token_file = {}\n",
                ),
                toml_string(&format!("127.0.0.1:{}", options.remote_admin_port)),
                toml_string(&token_path.display().to_string()),
            ),
        )?;
        format!(
            concat!(
                "[static.admin.remote]\n",
                "transport_profile = \"grpc-v1\"\n",
                "transport_config = \"admin-transport-grpc.toml\"\n\n",
                "[static.admin.principals.ops]\n",
                "permissions = {}\n\n",
            ),
            remote_permissions,
        )
    } else {
        String::new()
    };
    let local_console_permissions = format!(
        "[{}]",
        options
            .local_console_permissions
            .iter()
            .map(|permission| toml_string(permission))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let plugin_allowlist = format!(
        "[{}]",
        plugin_allowlist
            .iter()
            .map(|plugin_id| toml_string(plugin_id))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let bedrock_adapter_block = if options.bedrock_enabled {
        "\ndefault_bedrock_adapter = \"be-924\"\nenabled_bedrock_adapters = [\"be-924\"]"
    } else {
        ""
    };

    fs::write(
        config_path,
        format!(
            "\
[static.bootstrap]
online_mode = {}
level_name = \"world\"
level_type = \"flat\"
game_mode = 0
difficulty = 1
view_distance = 2
world_dir = {}
storage_profile = \"je-anvil-1_7_10\"

[static.plugins]
plugins_dir = {}
plugin_abi_min = \"4.0\"
plugin_abi_max = \"4.0\"

{}\
[live.network]
server_ip = \"127.0.0.1\"
server_port = {}
motd = {}
max_players = 20

[live.topology]
be_enabled = {}
{}
default_adapter = \"je-5\"
enabled_adapters = [\"je-5\"]
reload_watch = false
drain_grace_secs = 30

[live.plugins]
allowlist = {}
reload_watch = false

[live.plugins.failure_policy]
protocol = \"quarantine\"
gameplay = \"quarantine\"
storage = \"fail-fast\"
auth = \"skip\"
admin_transport = \"skip\"
admin_ui = \"skip\"

[live.profiles]
auth = {}
bedrock_auth = \"bedrock-offline-v1\"
default_gameplay = \"canonical\"

[live.admin]
ui_profile = \"console-v1\"
local_console_permissions = {}
",
            options.online_mode,
            toml_string(&world_dir.display().to_string()),
            toml_string(&plugins_dir.display().to_string()),
            remote_admin_block,
            options.server_port,
            toml_string(options.motd),
            options.bedrock_enabled,
            bedrock_adapter_block,
            plugin_allowlist,
            toml_string(options.auth_profile),
            local_console_permissions,
        ),
    )?;
    Ok(())
}

pub fn prepare_online_auth_runtime_plugins(
    temp_root: &Path,
    scenario: &str,
) -> TestResult<PathBuf> {
    let dist_dir = seed_runtime_plugins(
        temp_root,
        &[
            "admin-transport-grpc",
            "je-5",
            "gameplay-canonical",
            "storage-je-anvil-1_7_10",
            "auth-offline",
        ],
    )?;
    let harness = PackagedPluginHarness::shared()?;
    harness.install_auth_plugin(
        "mc-plugin-auth-online-stub",
        "auth-online-stub",
        &dist_dir,
        &harness.scoped_target_dir(scenario),
        "process-online-auth-v1",
    )?;
    Ok(dist_dir)
}

fn sanitize_log_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

pub fn create_persisted_server_log_capture(name: &str) -> TestResult<PersistedServerLogCapture> {
    let capture_id = LOG_CAPTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let capture_dir = repo_root()?
        .join("target")
        .join("upgrade-runtime-logs")
        .join(format!(
            "{}-{}-{}",
            sanitize_log_name(name),
            std::process::id(),
            capture_id
        ));
    fs::create_dir_all(&capture_dir)?;
    Ok(PersistedServerLogCapture {
        stdout_path: capture_dir.join("stdout.log"),
        stderr_path: capture_dir.join("stderr.log"),
    })
}

pub fn spawn_server_with_log_capture_and_envs(
    temp_dir: &Path,
    stdin: Stdio,
    config_path: Option<&Path>,
    extra_envs: &[(&str, &str)],
    capture_name: &str,
) -> TestResult<(Child, PersistedServerLogCapture)> {
    let logs = create_persisted_server_log_capture(capture_name)?;
    let stdout_file = File::create(&logs.stdout_path)?;
    let stderr_file = File::create(&logs.stderr_path)?;
    let child = spawn_server_with_config_path_and_envs(
        temp_dir,
        stdin,
        Stdio::from(stdout_file),
        Stdio::from(stderr_file),
        config_path,
        extra_envs,
    )?;
    Ok((child, logs))
}

pub fn spawn_server(
    temp_dir: &Path,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
) -> Result<Child, Box<dyn std::error::Error>> {
    spawn_server_with_config_path_and_envs(temp_dir, stdin, stdout, stderr, None, &[])
}

pub fn spawn_server_with_config_path(
    temp_dir: &Path,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
    config_path: Option<&Path>,
) -> Result<Child, Box<dyn std::error::Error>> {
    spawn_server_with_config_path_and_envs(temp_dir, stdin, stdout, stderr, config_path, &[])
}

pub fn spawn_server_with_config_path_and_envs(
    temp_dir: &Path,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
    config_path: Option<&Path>,
    extra_envs: &[(&str, &str)],
) -> Result<Child, Box<dyn std::error::Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_server-bootstrap"));
    command
        .current_dir(temp_dir)
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr);
    if let Some(config_path) = config_path {
        command.env("REVY_SERVER_CONFIG", config_path);
    }
    for (key, value) in extra_envs {
        command.env(key, value);
    }
    Ok(command.spawn()?)
}

pub fn wait_for_exit(
    child: &mut Child,
    timeout: Duration,
) -> Result<Option<ExitStatus>, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(25));
    }
}

pub fn read_child_output(
    child: &mut Child,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_string(&mut stdout)?;
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_string(&mut stderr)?;
    }
    Ok((stdout, stderr))
}

pub fn wait_for_tcp_ready(
    addr: SocketAddr,
    timeout: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + timeout;
    loop {
        match TcpStream::connect_timeout(&addr, Duration::from_millis(100)) {
            Ok(stream) => {
                drop(stream);
                return Ok(());
            }
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(Box::new(error)),
        }
    }
}

pub fn encode_handshake(
    protocol_version: i32,
    next_state: i32,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x00);
    writer.write_varint(protocol_version);
    writer.write_string("localhost")?;
    writer.write_u16(25565);
    writer.write_varint(next_state);
    Ok(writer.into_inner())
}

pub fn login_start(username: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x00);
    writer.write_string(username)?;
    Ok(writer.into_inner())
}

pub fn write_packet(
    stream: &mut TcpStream,
    codec: &MinecraftWireCodec,
    payload: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let frame = codec.encode_frame(payload)?;
    stream.write_all(&frame)?;
    Ok(())
}

pub fn connect_tcp(addr: SocketAddr) -> TestResult<TcpStream> {
    let stream = TcpStream::connect(addr)?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

pub fn status_request() -> Vec<u8> {
    vec![0x00]
}

pub fn status_ping(value: i64) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x01);
    writer.write_i64(value);
    writer.into_inner()
}

pub fn held_item_change(slot: i16) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x09);
    writer.write_i16(slot);
    writer.into_inner()
}

pub fn packet_id(frame: &[u8]) -> TestResult<i32> {
    let mut reader = PacketReader::new(frame);
    Ok(reader.read_varint()?)
}

pub fn parse_status_response(packet: &[u8]) -> TestResult<String> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint()? != 0x00 {
        return Err("expected status response packet".into());
    }
    Ok(reader.read_string(32 * 1024)?)
}

pub fn parse_status_pong(packet: &[u8]) -> TestResult<i64> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint()? != 0x01 {
        return Err("expected status pong packet".into());
    }
    Ok(reader.read_i64()?)
}

pub fn parse_encryption_request(packet: &[u8]) -> TestResult<(String, Vec<u8>, Vec<u8>)> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint()? != 0x01 {
        return Err("expected login encryption request packet".into());
    }
    let server_id = reader.read_string(20)?;
    let public_key_len =
        usize::try_from(reader.read_varint()?).map_err(|_| "negative public key length")?;
    let public_key_der = reader.read_bytes(public_key_len)?.to_vec();
    let verify_token_len =
        usize::try_from(reader.read_varint()?).map_err(|_| "negative verify token length")?;
    let verify_token = reader.read_bytes(verify_token_len)?.to_vec();
    Ok((server_id, public_key_der, verify_token))
}

pub fn login_encryption_response(
    shared_secret_encrypted: &[u8],
    verify_token_encrypted: &[u8],
) -> TestResult<Vec<u8>> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x01);
    writer.write_varint(
        i32::try_from(shared_secret_encrypted.len())
            .map_err(|_| "encrypted shared secret too large")?,
    );
    writer.write_bytes(shared_secret_encrypted);
    writer.write_varint(
        i32::try_from(verify_token_encrypted.len())
            .map_err(|_| "encrypted verify token too large")?,
    );
    writer.write_bytes(verify_token_encrypted);
    Ok(writer.into_inner())
}

pub fn held_item_from_packet(protocol: TestJavaProtocol, packet: &[u8]) -> TestResult<i8> {
    let mut reader = PacketReader::new(packet);
    let expected_packet_id = protocol
        .clientbound_packet_id(TestJavaPacket::HeldItemChange)
        .ok_or("held item change packet is unsupported")?;
    if reader.read_varint()? != expected_packet_id {
        return Err("expected held item change packet".into());
    }
    Ok(reader.read_i8()?)
}

fn read_packet_with_timeout_impl(
    stream: &mut TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
    deadline: Instant,
) -> TestResult<Vec<u8>> {
    loop {
        if let Some(frame) = codec.try_decode_frame(buffer)? {
            return Ok(frame);
        }
        if Instant::now() >= deadline {
            return Err("timed out waiting for packet".into());
        }
        let read_timeout = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(250));
        stream.set_read_timeout(Some(read_timeout))?;
        let mut chunk = [0_u8; 8192];
        match stream.read(&mut chunk) {
            Ok(0) => return Err("connection closed".into()),
            Ok(bytes_read) => buffer.extend_from_slice(&chunk[..bytes_read]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(Box::new(error)),
        }
    }
}

pub fn read_packet(
    stream: &mut TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
    timeout: Duration,
) -> TestResult<Vec<u8>> {
    read_packet_with_timeout_impl(stream, codec, buffer, Instant::now() + timeout)
}

pub fn read_until_packet_id(
    stream: &mut TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
    wanted_packet_id: i32,
    timeout: Duration,
    max_attempts: usize,
) -> TestResult<Vec<u8>> {
    let attempts = max_attempts.max(1);
    let deadline = Instant::now() + timeout;
    for _ in 0..attempts {
        let packet = read_packet_with_timeout_impl(stream, codec, buffer, deadline)?;
        if packet_id(&packet)? == wanted_packet_id {
            return Ok(packet);
        }
    }
    Err(format!("did not receive packet id 0x{wanted_packet_id:02x}").into())
}

pub fn read_until_java_packet(
    stream: &mut TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
    protocol: TestJavaProtocol,
    wanted_packet: TestJavaPacket,
    timeout: Duration,
    max_attempts: usize,
) -> TestResult<Vec<u8>> {
    let wanted_packet_id = protocol
        .clientbound_packet_id(wanted_packet)
        .ok_or("wanted packet is unsupported")?;
    read_until_packet_id(
        stream,
        codec,
        buffer,
        wanted_packet_id,
        timeout,
        max_attempts,
    )
}

pub fn assert_no_packet_id(
    stream: &mut TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
    unwanted_packet_id: i32,
    timeout: Duration,
) -> TestResult<()> {
    let result = read_until_packet_id(stream, codec, buffer, unwanted_packet_id, timeout, 2);
    match result {
        Err(error) if error.to_string().contains("timed out waiting for packet") => Ok(()),
        Err(error) if error.to_string().contains("did not receive packet id") => Ok(()),
        Ok(packet) => Err(format!(
            "unexpected packet id 0x{unwanted_packet_id:02x}: got 0x{:02x}",
            packet_id(&packet)?
        )
        .into()),
        Err(error) => Err(error),
    }
}

pub fn connect_and_login_java_client_until(
    addr: SocketAddr,
    codec: &MinecraftWireCodec,
    protocol: TestJavaProtocol,
    username: &str,
    wanted_packet: TestJavaPacket,
) -> TestResult<(TcpStream, BytesMut, Vec<u8>)> {
    let mut stream = connect_tcp(addr)?;
    write_packet(
        &mut stream,
        codec,
        &encode_handshake(protocol.protocol_version(), 2)?,
    )?;
    write_packet(&mut stream, codec, &login_start(username)?)?;
    let mut buffer = BytesMut::new();
    let packet = read_until_java_packet(
        &mut stream,
        codec,
        &mut buffer,
        protocol,
        wanted_packet,
        Duration::from_secs(5),
        24,
    )?;
    Ok((stream, buffer, packet))
}

pub fn connect_and_login_java_client(
    addr: SocketAddr,
    codec: &MinecraftWireCodec,
    protocol: TestJavaProtocol,
    username: &str,
) -> TestResult<(TcpStream, BytesMut)> {
    let (stream, buffer, _) = connect_and_login_java_client_until(
        addr,
        codec,
        protocol,
        username,
        TestJavaPacket::WindowItems,
    )?;
    Ok((stream, buffer))
}

pub fn begin_online_login(
    stream: &mut TcpStream,
    codec: &MinecraftWireCodec,
    protocol: TestJavaProtocol,
    username: &str,
) -> TestResult<(BytesMut, RsaPublicKey, Vec<u8>)> {
    write_packet(
        stream,
        codec,
        &encode_handshake(protocol.protocol_version(), 2)?,
    )?;
    write_packet(stream, codec, &login_start(username)?)?;
    let mut buffer = BytesMut::new();
    let request = read_packet(stream, codec, &mut buffer, Duration::from_secs(5))?;
    let (_server_id, public_key_der, verify_token) = parse_encryption_request(&request)?;
    let public_key = RsaPublicKey::from_public_key_der(&public_key_der)?;
    Ok((buffer, public_key, verify_token))
}

pub fn complete_online_login(
    stream: &mut TcpStream,
    codec: &MinecraftWireCodec,
    public_key: &RsaPublicKey,
    verify_token: &[u8],
) -> TestResult<ProcessTestClientEncryptionState> {
    let mut shared_secret = [0_u8; 16];
    OsRng.fill_bytes(&mut shared_secret);
    let shared_secret_encrypted =
        public_key.encrypt(&mut OsRng, Pkcs1v15Encrypt, &shared_secret)?;
    let verify_token_encrypted = public_key.encrypt(&mut OsRng, Pkcs1v15Encrypt, verify_token)?;
    let response = login_encryption_response(&shared_secret_encrypted, &verify_token_encrypted)?;
    write_packet(stream, codec, &response)?;
    Ok(ProcessTestClientEncryptionState::new(shared_secret))
}

pub fn perform_online_login(
    stream: &mut TcpStream,
    codec: &MinecraftWireCodec,
    protocol: TestJavaProtocol,
    username: &str,
) -> TestResult<(ProcessTestClientEncryptionState, BytesMut)> {
    let (mut buffer, public_key, verify_token) =
        begin_online_login(stream, codec, protocol, username)?;
    let encryption = complete_online_login(stream, codec, &public_key, &verify_token)?;
    Ok((encryption, std::mem::take(&mut buffer)))
}

pub fn write_packet_encrypted(
    stream: &mut TcpStream,
    codec: &MinecraftWireCodec,
    payload: &[u8],
    encryption: &mut ProcessTestClientEncryptionState,
) -> TestResult<()> {
    let mut frame = codec.encode_frame(payload)?;
    encryption.encrypt.apply_encrypt(&mut frame);
    stream.write_all(&frame)?;
    Ok(())
}

fn read_packet_encrypted_with_timeout_impl(
    stream: &mut TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
    encryption: &mut ProcessTestClientEncryptionState,
    deadline: Instant,
) -> TestResult<Vec<u8>> {
    loop {
        if let Some(frame) = codec.try_decode_frame(buffer)? {
            return Ok(frame);
        }
        if Instant::now() >= deadline {
            return Err("timed out waiting for encrypted packet".into());
        }
        let read_timeout = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(250));
        stream.set_read_timeout(Some(read_timeout))?;
        let mut chunk = [0_u8; 8192];
        match stream.read(&mut chunk) {
            Ok(0) => return Err("connection closed".into()),
            Ok(bytes_read) => {
                let bytes = &mut chunk[..bytes_read];
                encryption.decrypt.apply_decrypt(bytes);
                buffer.extend_from_slice(bytes);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(Box::new(error)),
        }
    }
}

pub fn read_until_java_packet_encrypted(
    stream: &mut TcpStream,
    codec: &MinecraftWireCodec,
    buffer: &mut BytesMut,
    protocol: TestJavaProtocol,
    wanted_packet: TestJavaPacket,
    timeout: Duration,
    max_attempts: usize,
    encryption: &mut ProcessTestClientEncryptionState,
) -> TestResult<Vec<u8>> {
    let attempts = max_attempts.max(1);
    let deadline = Instant::now() + timeout;
    let wanted_packet_id = protocol
        .clientbound_packet_id(wanted_packet)
        .ok_or("wanted encrypted packet is unsupported")?;
    for _ in 0..attempts {
        let packet =
            read_packet_encrypted_with_timeout_impl(stream, codec, buffer, encryption, deadline)?;
        if packet_id(&packet)? == wanted_packet_id {
            return Ok(packet);
        }
    }
    Err(format!("did not receive encrypted packet id 0x{wanted_packet_id:02x}").into())
}

#[cfg(unix)]
pub fn set_world_read_only(path: &Path, read_only: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(if read_only { 0o555 } else { 0o755 });
    fs::set_permissions(path, permissions)?;
    Ok(())
}
