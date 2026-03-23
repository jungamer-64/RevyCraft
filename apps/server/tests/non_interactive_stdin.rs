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
use tempfile::tempdir;

fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

fn reserve_port() -> Result<u16, Box<dyn std::error::Error>> {
    Ok(StdTcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

fn repo_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?)
}

fn write_server_toml(
    temp_dir: &Path,
    repo_root: &Path,
    world_dir: &Path,
    admin_grpc_enabled: bool,
    server_port: u16,
    admin_grpc_port: u16,
    motd: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime_dir = temp_dir.join("runtime");
    let plugins_dir = repo_root.join("runtime").join("plugins");
    fs::create_dir_all(&runtime_dir)?;
    let (principal_block, admin_bind_addr) = if admin_grpc_enabled {
        let token_path = temp_dir.join("admin").join("ops.token");
        fs::create_dir_all(token_path.parent().expect("token parent should exist"))?;
        fs::write(&token_path, "ops-token\n")?;
        (
            format!(
                "\n[static.admin.grpc.principals.ops]\ntoken_file = {}\npermissions = [\"status\", \"sessions\", \"reload-config\", \"reload-plugins\", \"reload-generation\", \"shutdown\"]\n",
                toml_string(&token_path.display().to_string())
            ),
            format!("127.0.0.1:{admin_grpc_port}"),
        )
    } else {
        (String::new(), "127.0.0.1:50051".to_string())
    };

    fs::write(
        runtime_dir.join("server.toml"),
        format!(
            "\
[static.bootstrap]
online_mode = false
level_name = \"world\"
level_type = \"flat\"
game_mode = 0
difficulty = 1
view_distance = 2
world_dir = {}
storage_profile = \"je-anvil-1_7_10\"

[static.plugins]
plugins_dir = {}
plugin_abi_min = \"3.0\"
plugin_abi_max = \"3.0\"

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
be_enabled = false
default_adapter = \"je-5\"
enabled_adapters = [\"je-5\"]
reload_watch = false
drain_grace_secs = 30

[live.plugins]
allowlist = [\"je-5\", \"gameplay-canonical\", \"storage-je-anvil-1_7_10\", \"auth-offline\"]
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
local_console_permissions = [\"status\", \"sessions\", \"reload-config\", \"reload-plugins\", \"reload-generation\", \"shutdown\"]
",
            toml_string(&world_dir.display().to_string()),
            toml_string(&plugins_dir.display().to_string()),
            admin_grpc_enabled,
            toml_string(&admin_bind_addr),
            principal_block,
            server_port,
            toml_string(motd),
        ),
    )?;
    Ok(())
}

fn spawn_server(
    temp_dir: &Path,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
) -> Result<Child, Box<dyn std::error::Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_server-bootstrap"))
        .current_dir(temp_dir)
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr)
        .spawn()?)
}

fn wait_for_exit(
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

fn read_child_output(child: &mut Child) -> Result<(String, String), Box<dyn std::error::Error>> {
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

fn wait_for_tcp_ready(
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

fn encode_handshake(
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

fn login_start(username: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x00);
    writer.write_string(username)?;
    Ok(writer.into_inner())
}

fn write_packet(
    stream: &mut TcpStream,
    codec: &MinecraftWireCodec,
    payload: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let frame = codec.encode_frame(payload)?;
    stream.write_all(&frame)?;
    Ok(())
}

#[cfg(unix)]
fn set_world_read_only(path: &Path, read_only: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(if read_only { 0o555 } else { 0o755 });
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[test]
fn stdin_null_with_admin_grpc_keeps_server_running() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    write_server_toml(
        temp_dir.path(),
        &repo_root,
        &world_dir,
        true,
        0,
        grpc_port,
        "stdin-null-admin",
    )?;

    let mut child = spawn_server(temp_dir.path(), Stdio::null(), Stdio::null(), Stdio::null())?;

    thread::sleep(Duration::from_millis(500));
    if let Some(status) = child.try_wait()? {
        return Err(format!("server exited early with status {status}").into());
    }

    child.kill()?;
    let _ = child.wait()?;
    Ok(())
}

#[test]
fn stdin_null_without_other_admin_surface_warns_and_exits() -> Result<(), Box<dyn std::error::Error>>
{
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    write_server_toml(
        temp_dir.path(),
        &repo_root,
        &world_dir,
        false,
        0,
        50051,
        "stdin-null-no-admin",
    )?;

    let mut child = spawn_server(
        temp_dir.path(),
        Stdio::null(),
        Stdio::null(),
        Stdio::piped(),
    )?;

    let Some(_status) = wait_for_exit(&mut child, Duration::from_secs(2))? else {
        child.kill()?;
        let _ = child.wait()?;
        return Err("server did not exit after stdin EOF without another admin surface".into());
    };
    let (_stdout, stderr) = read_child_output(&mut child)?;
    assert!(stderr.contains("stdin reached EOF and no other admin surface is available"));
    Ok(())
}

#[test]
fn piped_status_command_detaches_after_eof_when_admin_grpc_is_available()
-> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    write_server_toml(
        temp_dir.path(),
        &repo_root,
        &world_dir,
        true,
        0,
        grpc_port,
        "stdin-pipe-admin",
    )?;

    let mut child = spawn_server(
        temp_dir.path(),
        Stdio::piped(),
        Stdio::piped(),
        Stdio::piped(),
    )?;
    {
        let stdin = child.stdin.as_mut().ok_or("child stdin should be piped")?;
        stdin.write_all(b"status\n")?;
    }
    drop(child.stdin.take());

    thread::sleep(Duration::from_millis(500));
    if let Some(status) = child.try_wait()? {
        return Err(format!("server exited early with status {status}").into());
    }

    child.kill()?;
    let _ = child.wait()?;
    let (stdout, _stderr) = read_child_output(&mut child)?;
    assert!(stdout.contains("status: generation="));
    Ok(())
}

#[cfg(unix)]
#[test]
fn runtime_failure_exits_even_when_admin_grpc_is_available()
-> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let server_port = reserve_port()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    set_world_read_only(&world_dir, true)?;
    write_server_toml(
        temp_dir.path(),
        &repo_root,
        &world_dir,
        true,
        server_port,
        grpc_port,
        "runtime-failure-admin",
    )?;

    let mut child = spawn_server(
        temp_dir.path(),
        Stdio::null(),
        Stdio::piped(),
        Stdio::piped(),
    )?;

    let server_addr = SocketAddr::from(([127, 0, 0, 1], server_port));
    wait_for_tcp_ready(server_addr, Duration::from_secs(3))?;

    let codec = MinecraftWireCodec;
    let mut stream = TcpStream::connect(server_addr)?;
    write_packet(&mut stream, &codec, &encode_handshake(5, 2)?)?;
    write_packet(&mut stream, &codec, &login_start("runtime-failure")?)?;

    let Some(status) = wait_for_exit(&mut child, Duration::from_secs(5))? else {
        child.kill()?;
        let _ = child.wait()?;
        set_world_read_only(&world_dir, false)?;
        return Err("server did not exit after runtime loop failure".into());
    };
    let (_stdout, stderr) = read_child_output(&mut child)?;
    set_world_read_only(&world_dir, false)?;

    assert!(!status.success());
    assert!(stderr.contains("storage") || stderr.contains("runtime failure"));
    Ok(())
}
