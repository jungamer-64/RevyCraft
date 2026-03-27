#![allow(dead_code)]

use mc_proto_common::{MinecraftWireCodec, PacketWriter, WireCodec};
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener as StdTcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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

pub struct ServerTomlOptions<'a> {
    pub admin_grpc_enabled: bool,
    pub server_port: u16,
    pub admin_grpc_port: u16,
    pub motd: &'a str,
    pub online_mode: bool,
    pub bedrock_enabled: bool,
    pub local_console_permissions: &'a [&'a str],
    pub remote_permissions: &'a [&'a str],
}

impl<'a> ServerTomlOptions<'a> {
    #[must_use]
    pub fn new(
        admin_grpc_enabled: bool,
        server_port: u16,
        admin_grpc_port: u16,
        motd: &'a str,
    ) -> Self {
        Self {
            admin_grpc_enabled,
            server_port,
            admin_grpc_port,
            motd,
            online_mode: false,
            bedrock_enabled: false,
            local_console_permissions: DEFAULT_LOCAL_CONSOLE_PERMISSIONS,
            remote_permissions: DEFAULT_REMOTE_PERMISSIONS,
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
    repo_root: &Path,
    world_dir: &Path,
    options: &ServerTomlOptions<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime_dir = config_path
        .parent()
        .ok_or("config path should have a parent directory")?;
    let plugins_dir = repo_root.join("runtime").join("plugins");
    fs::create_dir_all(runtime_dir)?;
    let (principal_block, admin_bind_addr) = if options.admin_grpc_enabled {
        let token_path = temp_root.join("admin").join("ops.token");
        fs::create_dir_all(token_path.parent().expect("token parent should exist"))?;
        fs::write(&token_path, format!("{OPS_TOKEN}\n"))?;
        (
            format!(
                "\n[static.admin.grpc.principals.ops]\ntoken_file = {}\npermissions = {}\n",
                toml_string(&token_path.display().to_string()),
                format!(
                    "[{}]",
                    options
                        .remote_permissions
                        .iter()
                        .map(|permission| toml_string(permission))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            ),
            format!("127.0.0.1:{}", options.admin_grpc_port),
        )
    } else {
        (String::new(), "127.0.0.1:50051".to_string())
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
    let plugin_allowlist = if options.bedrock_enabled {
        "[\"je-5\", \"be-924\", \"gameplay-canonical\", \"storage-je-anvil-1_7_10\", \"auth-offline\", \"auth-bedrock-offline\"]"
    } else {
        "[\"je-5\", \"gameplay-canonical\", \"storage-je-anvil-1_7_10\", \"auth-offline\"]"
    };
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

[static.admin.grpc]
enabled = {}
bind_addr = {}
allow_non_loopback = false
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
admin_ui = \"skip\"

[live.profiles]
auth = \"offline-v1\"
bedrock_auth = \"bedrock-offline-v1\"
default_gameplay = \"canonical\"

[live.admin]
ui_profile = \"console-v1\"
local_console_permissions = {}
",
            options.online_mode,
            toml_string(&world_dir.display().to_string()),
            toml_string(&plugins_dir.display().to_string()),
            options.admin_grpc_enabled,
            toml_string(&admin_bind_addr),
            principal_block,
            options.server_port,
            toml_string(options.motd),
            options.bedrock_enabled,
            bedrock_adapter_block,
            plugin_allowlist,
            local_console_permissions,
        ),
    )?;
    Ok(())
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

#[cfg(unix)]
pub fn set_world_read_only(
    path: &Path,
    read_only: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(if read_only { 0o555 } else { 0o755 });
    fs::set_permissions(path, permissions)?;
    Ok(())
}
