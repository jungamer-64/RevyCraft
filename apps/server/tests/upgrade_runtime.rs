mod support;

use bytes::BytesMut;
use mc_proto_common::MinecraftWireCodec;
use mc_proto_test_support::{TestJavaPacket, TestJavaProtocol};
use revy_admin_grpc::admin as proto;
use std::fs;
#[cfg(unix)]
use std::fs::File;
use std::io::Write;
use std::net::SocketAddr;
use std::net::TcpStream;
use std::process::Stdio;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};
use support::*;
use tempfile::tempdir;
use tonic::metadata::MetadataValue;
use tonic::{Code, Request};

type AdminClient =
    proto::admin_control_plane_client::AdminControlPlaneClient<tonic::transport::Channel>;

fn upgrade_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn authorized_request<T>(message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert(
        "authorization",
        MetadataValue::try_from(format!("Bearer {OPS_TOKEN}"))
            .expect("bearer token metadata should be valid"),
    );
    request
}

async fn grpc_client(local_addr: SocketAddr) -> Result<AdminClient, tonic::transport::Error> {
    proto::admin_control_plane_client::AdminControlPlaneClient::connect(format!(
        "http://{local_addr}"
    ))
    .await
}

#[cfg(unix)]
fn wait_for_output_contains(
    path: &std::path::Path,
    needle: &str,
    timeout: Duration,
) -> Result<String, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + timeout;
    loop {
        let content = fs::read_to_string(path).unwrap_or_default();
        if content.contains(needle) {
            return Ok(content);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for `{needle}` in `{}`; current content: {content}",
                path.display()
            )
            .into());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_tcp_closed(
    addr: SocketAddr,
    timeout: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + timeout;
    loop {
        match TcpStream::connect_timeout(&addr, Duration::from_millis(100)) {
            Ok(stream) => {
                drop(stream);
                if Instant::now() >= deadline {
                    return Err(format!("timed out waiting for {addr} to close").into());
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return Ok(()),
        }
    }
}

fn upgrade_enabled_options<'a>(grpc_port: u16, motd: &'a str) -> ServerTomlOptions<'a> {
    let mut options = ServerTomlOptions::new(true, 0, grpc_port, motd);
    options.local_console_permissions = UPGRADE_LOCAL_CONSOLE_PERMISSIONS;
    options.remote_permissions = UPGRADE_REMOTE_PERMISSIONS;
    options
}

async fn wait_for_grpc_client(
    local_addr: SocketAddr,
    timeout: Duration,
) -> Result<AdminClient, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + timeout;
    loop {
        match grpc_client(local_addr).await {
            Ok(client) => return Ok(client),
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(error) => return Err(Box::new(error)),
        }
    }
}

fn runtime_tcp_listener_addr(
    status: &proto::AdminStatusView,
) -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let binding = status
        .listener_bindings
        .iter()
        .find(|binding| binding.transport == proto::TransportKind::Tcp as i32)
        .ok_or("status did not include a TCP game listener binding")?;
    Ok(binding.local_addr.parse()?)
}

async fn fetch_runtime_tcp_listener_addr(
    client: &mut AdminClient,
) -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let status = fetch_status(client).await?;
    runtime_tcp_listener_addr(&status)
}

async fn fetch_status(
    client: &mut AdminClient,
) -> Result<proto::AdminStatusView, Box<dyn std::error::Error>> {
    Ok(client
        .get_status(authorized_request(proto::GetStatusRequest {}))
        .await?
        .into_inner()
        .status
        .ok_or("status response was missing runtime status")?)
}

async fn wait_for_upgrade_phase(
    client: &mut AdminClient,
    expected_role: proto::RuntimeUpgradeRole,
    expected_phase: proto::RuntimeUpgradePhase,
    timeout: Duration,
) -> Result<proto::AdminStatusView, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + timeout;
    loop {
        let status = fetch_status(client).await?;
        if let Some(upgrade) = status.upgrade.as_ref() {
            let role = proto::RuntimeUpgradeRole::try_from(upgrade.role)
                .map_err(|_| "status upgrade role was invalid")?;
            let phase = proto::RuntimeUpgradePhase::try_from(upgrade.phase)
                .map_err(|_| "status upgrade phase was invalid")?;
            if role == expected_role && phase == expected_phase {
                return Ok(status);
            }
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for upgrade phase role={expected_role:?} phase={expected_phase:?}"
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn upgrade_runtime_executable(
    client: &mut AdminClient,
    executable_path: &str,
) -> Result<proto::AdminUpgradeRuntimeView, Box<dyn std::error::Error>> {
    Ok(client
        .upgrade_runtime(authorized_request(proto::UpgradeRuntimeRequest {
            executable_path: executable_path.to_string(),
        }))
        .await?
        .into_inner()
        .result
        .ok_or("upgrade response was missing result")?)
}

async fn reload_runtime_full(client: &mut AdminClient) -> Result<(), Box<dyn std::error::Error>> {
    let response = client
        .reload_runtime(authorized_request(proto::ReloadRuntimeRequest {
            mode: proto::RuntimeReloadMode::Full as i32,
        }))
        .await?
        .into_inner();
    if response.result.is_none() {
        return Err("reload runtime response was missing result".into());
    }
    Ok(())
}

async fn shutdown_runtime_via_grpc(
    client: &mut AdminClient,
    grpc_addr: SocketAddr,
    parent: &mut std::process::Child,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Err(error) = client
        .shutdown(authorized_request(proto::ShutdownRequest {}))
        .await
    {
        if !matches!(
            error.code(),
            Code::Unknown | Code::Unavailable | Code::Internal
        ) {
            return Err(Box::new(error));
        }
    }
    wait_for_tcp_closed(grpc_addr, Duration::from_secs(10))?;
    let exit_status = wait_for_exit(parent, Duration::from_secs(10))?
        .ok_or("parent process should exit once the upgraded child shuts down")?;
    assert!(
        exit_status.success(),
        "parent process should exit cleanly; status={exit_status}"
    );
    Ok(())
}

fn spawn_logged_server(
    temp_dir: &std::path::Path,
    capture_name: &str,
) -> Result<(std::process::Child, PersistedServerLogCapture), Box<dyn std::error::Error>> {
    spawn_server_with_log_capture_and_envs(temp_dir, Stdio::null(), None, &[], capture_name)
}

fn spawn_logged_server_with_envs(
    temp_dir: &std::path::Path,
    capture_name: &str,
    extra_envs: &[(&str, &str)],
) -> Result<(std::process::Child, PersistedServerLogCapture), Box<dyn std::error::Error>> {
    spawn_server_with_log_capture_and_envs(temp_dir, Stdio::null(), None, extra_envs, capture_name)
}

#[test]
fn local_console_without_upgrade_permission_denies_upgrade_command()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = upgrade_test_lock().blocking_lock();
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    write_server_toml(
        temp_dir.path(),
        &repo_root,
        &world_dir,
        &ServerTomlOptions::new(true, 0, grpc_port, "upgrade-permission-denied"),
    )?;

    let mut child = spawn_server(
        temp_dir.path(),
        Stdio::piped(),
        Stdio::piped(),
        Stdio::piped(),
    )?;
    {
        let stdin = child.stdin.as_mut().ok_or("child stdin should be piped")?;
        writeln!(
            stdin,
            "upgrade runtime executable {}",
            env!("CARGO_BIN_EXE_server-bootstrap")
        )?;
    }
    drop(child.stdin.take());
    thread::sleep(Duration::from_millis(500));
    child.kill()?;
    let _ = child.wait()?;
    let (stdout, _stderr) = read_child_output(&mut child)?;
    assert!(stdout.contains("permission denied"));
    assert!(stdout.contains("upgrade-runtime"));
    Ok(())
}

#[tokio::test]
async fn grpc_upgrade_permission_denied_without_upgrade_runtime_grant()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = upgrade_test_lock().lock().await;
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    write_server_toml(
        temp_dir.path(),
        &repo_root,
        &world_dir,
        &ServerTomlOptions::new(true, 0, grpc_port, "grpc-upgrade-denied"),
    )?;

    let (mut child, _logs) = spawn_logged_server(temp_dir.path(), "grpc-upgrade-denied")?;
    let grpc_addr = SocketAddr::from(([127, 0, 0, 1], grpc_port));
    wait_for_tcp_ready(grpc_addr, Duration::from_secs(5))?;
    let mut client = grpc_client(grpc_addr).await?;

    let error = client
        .upgrade_runtime(authorized_request(proto::UpgradeRuntimeRequest {
            executable_path: env!("CARGO_BIN_EXE_server-bootstrap").to_string(),
        }))
        .await
        .expect_err("upgrade should be permission denied");
    assert_eq!(error.code(), Code::PermissionDenied);

    child.kill()?;
    let _ = child.wait()?;
    Ok(())
}

#[tokio::test]
async fn grpc_upgrade_invalid_executable_path_rolls_back_and_keeps_grpc_available()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = upgrade_test_lock().lock().await;
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    let options = upgrade_enabled_options(grpc_port, "grpc-upgrade-invalid-executable");
    write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;

    let (mut child, _logs) =
        spawn_logged_server(temp_dir.path(), "grpc-upgrade-invalid-executable")?;
    let grpc_addr = SocketAddr::from(([127, 0, 0, 1], grpc_port));
    wait_for_tcp_ready(grpc_addr, Duration::from_secs(5))?;
    let mut client = grpc_client(grpc_addr).await?;

    let error = client
        .upgrade_runtime(authorized_request(proto::UpgradeRuntimeRequest {
            executable_path: temp_dir
                .path()
                .join("missing-server-bootstrap")
                .display()
                .to_string(),
        }))
        .await
        .expect_err("upgrade should fail for a missing executable");
    assert_eq!(error.code(), Code::FailedPrecondition);

    let status = client
        .get_status(authorized_request(proto::GetStatusRequest {}))
        .await?
        .into_inner();
    assert!(status.status.is_some());

    child.kill()?;
    let _ = child.wait()?;
    Ok(())
}

#[tokio::test]
async fn grpc_upgrade_play_session_survives_cutover_and_accepts_new_connections()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = upgrade_test_lock().lock().await;
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    let options = upgrade_enabled_options(grpc_port, "grpc-upgrade-play-continuity");
    write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;

    let (mut child, _logs) = spawn_logged_server(temp_dir.path(), "grpc-upgrade-play-continuity")?;
    let grpc_addr = SocketAddr::from(([127, 0, 0, 1], grpc_port));
    let mut client = wait_for_grpc_client(grpc_addr, Duration::from_secs(5)).await?;
    let game_addr = fetch_runtime_tcp_listener_addr(&mut client).await?;
    let codec = MinecraftWireCodec;
    let protocol = TestJavaProtocol::Je5;
    let (mut stream, mut buffer) =
        connect_and_login_java_client(game_addr, &codec, protocol, "up-play-a")
            .map_err(|error| format!("initial play login failed: {error}"))?;
    let _ = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
        Duration::from_secs(5),
        24,
    )
    .map_err(|error| format!("initial held-item bootstrap failed: {error}"))?;
    write_packet(&mut stream, &codec, &held_item_change(3))
        .map_err(|error| format!("pre-upgrade held-item write failed: {error}"))?;
    let pre_upgrade_slot = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
        Duration::from_secs(5),
        24,
    )
    .map_err(|error| format!("pre-upgrade held-item echo failed: {error}"))?;
    assert_eq!(held_item_from_packet(protocol, &pre_upgrade_slot)?, 3);

    let upgrade =
        upgrade_runtime_executable(&mut client, env!("CARGO_BIN_EXE_server-bootstrap")).await?;
    assert_eq!(
        upgrade.executable_path,
        env!("CARGO_BIN_EXE_server-bootstrap")
    );

    let mut child_client = wait_for_grpc_client(grpc_addr, Duration::from_secs(5)).await?;
    let upgraded_game_addr = fetch_runtime_tcp_listener_addr(&mut child_client).await?;
    assert_eq!(upgraded_game_addr, game_addr);

    assert_no_packet_id(
        &mut stream,
        &codec,
        &mut buffer,
        protocol
            .clientbound_packet_id(TestJavaPacket::LoginSuccess)
            .ok_or("login success packet should be supported")?,
        Duration::from_millis(400),
    )?;
    write_packet(&mut stream, &codec, &held_item_change(4))?;
    let changed_slot = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
        Duration::from_secs(5),
        24,
    )
    .map_err(|error| format!("post-upgrade held-item echo failed: {error}"))?;
    assert_eq!(held_item_from_packet(protocol, &changed_slot)?, 4);

    let (_fresh_stream, _fresh_buffer) =
        connect_and_login_java_client(upgraded_game_addr, &codec, protocol, "up-play-b")
            .map_err(|error| format!("fresh post-upgrade login failed: {error}"))?;
    shutdown_runtime_via_grpc(&mut child_client, grpc_addr, &mut child).await
}

#[tokio::test]
async fn grpc_upgrade_freeze_blocks_mutating_admin_requests_and_preserves_buffered_bytes()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = upgrade_test_lock().lock().await;
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    let options = upgrade_enabled_options(grpc_port, "grpc-upgrade-freeze-phase");
    write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;

    let (mut child, _logs) = spawn_logged_server_with_envs(
        temp_dir.path(),
        "grpc-upgrade-freeze-phase",
        &[("REVY_UPGRADE_TEST_HOLD_AFTER_SESSION_FREEZE_MS", "600")],
    )?;
    let grpc_addr = SocketAddr::from(([127, 0, 0, 1], grpc_port));
    let mut upgrade_client = wait_for_grpc_client(grpc_addr, Duration::from_secs(5)).await?;
    let mut status_client = grpc_client(grpc_addr).await?;
    let mut reload_client = grpc_client(grpc_addr).await?;
    let mut shutdown_client = grpc_client(grpc_addr).await?;
    let mut second_upgrade_client = grpc_client(grpc_addr).await?;
    let game_addr = fetch_runtime_tcp_listener_addr(&mut upgrade_client).await?;
    let codec = MinecraftWireCodec;
    let protocol = TestJavaProtocol::Je5;
    let (mut stream, mut buffer) =
        connect_and_login_java_client(game_addr, &codec, protocol, "freeze-play-a")
            .map_err(|error| format!("freeze test login failed: {error}"))?;
    let _ = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
        Duration::from_secs(5),
        24,
    )
    .map_err(|error| format!("freeze bootstrap held-item read failed: {error}"))?;

    let upgrade_task = tokio::spawn(async move {
        upgrade_client
            .upgrade_runtime(authorized_request(proto::UpgradeRuntimeRequest {
                executable_path: env!("CARGO_BIN_EXE_server-bootstrap").to_string(),
            }))
            .await
    });

    let freeze_status = wait_for_upgrade_phase(
        &mut status_client,
        proto::RuntimeUpgradeRole::Parent,
        proto::RuntimeUpgradePhase::ParentFreezing,
        Duration::from_secs(5),
    )
    .await?;
    assert!(freeze_status.upgrade.is_some());

    let reload_error = reload_client
        .reload_runtime(authorized_request(proto::ReloadRuntimeRequest {
            mode: proto::RuntimeReloadMode::Full as i32,
        }))
        .await
        .expect_err("reload should be rejected while upgrade freeze is active");
    assert_eq!(reload_error.code(), Code::FailedPrecondition);

    let shutdown_error = shutdown_client
        .shutdown(authorized_request(proto::ShutdownRequest {}))
        .await
        .expect_err("shutdown should be rejected while upgrade freeze is active");
    assert_eq!(shutdown_error.code(), Code::FailedPrecondition);

    let second_upgrade_error = second_upgrade_client
        .upgrade_runtime(authorized_request(proto::UpgradeRuntimeRequest {
            executable_path: env!("CARGO_BIN_EXE_server-bootstrap").to_string(),
        }))
        .await
        .expect_err("second upgrade should be rejected while upgrade freeze is active");
    assert_eq!(second_upgrade_error.code(), Code::FailedPrecondition);

    write_packet(&mut stream, &codec, &held_item_change(8))?;
    assert_no_packet_id(
        &mut stream,
        &codec,
        &mut buffer,
        protocol
            .clientbound_packet_id(TestJavaPacket::HeldItemChange)
            .ok_or("held item change packet should be supported")?,
        Duration::from_millis(200),
    )?;

    let upgrade = upgrade_task.await??.into_inner();
    let upgrade = upgrade
        .result
        .ok_or("upgrade response was missing result during freeze test")?;
    assert_eq!(
        upgrade.executable_path,
        env!("CARGO_BIN_EXE_server-bootstrap")
    );

    let mut child_client = wait_for_grpc_client(grpc_addr, Duration::from_secs(5)).await?;
    let held_item = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
        Duration::from_secs(5),
        24,
    )
    .map_err(|error| {
        format!("buffered held-item change was not delivered after cutover: {error}")
    })?;
    assert_eq!(held_item_from_packet(protocol, &held_item)?, 8);

    shutdown_runtime_via_grpc(&mut child_client, grpc_addr, &mut child).await
}

#[tokio::test]
async fn grpc_upgrade_status_session_survives_cutover() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = upgrade_test_lock().lock().await;
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    let options = upgrade_enabled_options(grpc_port, "grpc-upgrade-status-continuity");
    write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;

    let (mut child, _logs) =
        spawn_logged_server(temp_dir.path(), "grpc-upgrade-status-continuity")?;
    let grpc_addr = SocketAddr::from(([127, 0, 0, 1], grpc_port));
    let mut client = wait_for_grpc_client(grpc_addr, Duration::from_secs(5)).await?;
    let game_addr = fetch_runtime_tcp_listener_addr(&mut client).await?;
    let codec = MinecraftWireCodec;
    let mut stream = connect_tcp(game_addr)?;
    write_packet(
        &mut stream,
        &codec,
        &encode_handshake(TestJavaProtocol::Je5.protocol_version(), 1)?,
    )?;
    write_packet(&mut stream, &codec, &status_request())?;
    let mut buffer = BytesMut::new();
    let response = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::StatusResponse,
        Duration::from_secs(5),
        8,
    )?;
    assert!(parse_status_response(&response)?.contains("grpc-upgrade-status-continuity"));

    let _ = upgrade_runtime_executable(&mut client, env!("CARGO_BIN_EXE_server-bootstrap")).await?;
    let mut child_client = wait_for_grpc_client(grpc_addr, Duration::from_secs(5)).await?;
    assert_eq!(
        fetch_runtime_tcp_listener_addr(&mut child_client).await?,
        game_addr
    );

    write_packet(&mut stream, &codec, &status_ping(12_345))?;
    let pong = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::StatusPong,
        Duration::from_secs(5),
        8,
    )?;
    assert_eq!(parse_status_pong(&pong)?, 12_345);

    drop(stream);
    shutdown_runtime_via_grpc(&mut child_client, grpc_addr, &mut child).await
}

#[tokio::test]
async fn grpc_upgrade_online_login_session_survives_cutover()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = upgrade_test_lock().lock().await;
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    let runtime_plugins_dir =
        prepare_online_auth_runtime_plugins(temp_dir.path(), "grpc-upgrade-online-login")?;
    let mut options = upgrade_enabled_options(grpc_port, "grpc-upgrade-online-login");
    options.online_mode = true;
    options.auth_profile = "mojang-online-v1";
    options.extra_plugin_allowlist = &["auth-online-stub"];
    options.plugins_dir_override = Some(runtime_plugins_dir);
    write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;

    let (mut child, _logs) = spawn_logged_server(temp_dir.path(), "grpc-upgrade-online-login")?;
    let grpc_addr = SocketAddr::from(([127, 0, 0, 1], grpc_port));
    let mut client = wait_for_grpc_client(grpc_addr, Duration::from_secs(10)).await?;
    let game_addr = fetch_runtime_tcp_listener_addr(&mut client).await?;
    let codec = MinecraftWireCodec;
    let protocol = TestJavaProtocol::Je5;
    let mut stream = connect_tcp(game_addr)?;
    let (mut buffer, public_key, verify_token) =
        begin_online_login(&mut stream, &codec, protocol, "up-online-a")
            .map_err(|error| format!("initial online login handshake failed: {error}"))?;

    let _ = upgrade_runtime_executable(&mut client, env!("CARGO_BIN_EXE_server-bootstrap")).await?;
    let mut child_client = wait_for_grpc_client(grpc_addr, Duration::from_secs(10)).await?;
    assert_eq!(
        fetch_runtime_tcp_listener_addr(&mut child_client).await?,
        game_addr
    );

    let mut encryption = complete_online_login(&mut stream, &codec, &public_key, &verify_token)
        .map_err(|error| format!("post-upgrade online login completion failed: {error}"))?;
    let login_success = read_until_java_packet_encrypted(
        &mut stream,
        &codec,
        &mut buffer,
        protocol,
        TestJavaPacket::LoginSuccess,
        Duration::from_secs(5),
        12,
        &mut encryption,
    )
    .map_err(|error| format!("encrypted login success after cutover failed: {error}"))?;
    assert_eq!(packet_id(&login_success)?, 0x02);
    let _ = read_until_java_packet_encrypted(
        &mut stream,
        &codec,
        &mut buffer,
        protocol,
        TestJavaPacket::WindowItems,
        Duration::from_secs(5),
        24,
        &mut encryption,
    )
    .map_err(|error| format!("encrypted window-items bootstrap after cutover failed: {error}"))?;
    let _ = read_until_java_packet_encrypted(
        &mut stream,
        &codec,
        &mut buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
        Duration::from_secs(5),
        24,
        &mut encryption,
    )
    .map_err(|error| format!("encrypted held-item bootstrap after cutover failed: {error}"))?;
    write_packet_encrypted(&mut stream, &codec, &held_item_change(5), &mut encryption)?;
    let changed_slot = read_until_java_packet_encrypted(
        &mut stream,
        &codec,
        &mut buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
        Duration::from_secs(5),
        12,
        &mut encryption,
    )
    .map_err(|error| format!("encrypted held-item echo after cutover failed: {error}"))?;
    assert_eq!(held_item_from_packet(protocol, &changed_slot)?, 5);

    shutdown_runtime_via_grpc(&mut child_client, grpc_addr, &mut child).await
}

#[tokio::test]
async fn grpc_upgrade_after_full_reload_preserves_play_session()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = upgrade_test_lock().lock().await;
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    let options = upgrade_enabled_options(grpc_port, "grpc-upgrade-after-full-reload");
    write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;

    let (mut child, _logs) =
        spawn_logged_server(temp_dir.path(), "grpc-upgrade-after-full-reload")?;
    let grpc_addr = SocketAddr::from(([127, 0, 0, 1], grpc_port));
    let mut client = wait_for_grpc_client(grpc_addr, Duration::from_secs(5)).await?;
    reload_runtime_full(&mut client).await?;
    let game_addr = fetch_runtime_tcp_listener_addr(&mut client).await?;
    let codec = MinecraftWireCodec;
    let protocol = TestJavaProtocol::Je5;
    let (mut stream, mut buffer) =
        connect_and_login_java_client(game_addr, &codec, protocol, "up-reload-a")
            .map_err(|error| format!("post-reload play login failed: {error}"))?;
    let _ = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
        Duration::from_secs(5),
        24,
    )
    .map_err(|error| format!("post-reload held-item bootstrap failed: {error}"))?;
    write_packet(&mut stream, &codec, &held_item_change(2))
        .map_err(|error| format!("post-reload held-item write failed: {error}"))?;
    let pre_upgrade_slot = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
        Duration::from_secs(5),
        24,
    )
    .map_err(|error| format!("post-reload held-item echo failed: {error}"))?;
    assert_eq!(held_item_from_packet(protocol, &pre_upgrade_slot)?, 2);

    let _ = upgrade_runtime_executable(&mut client, env!("CARGO_BIN_EXE_server-bootstrap")).await?;
    let mut child_client = wait_for_grpc_client(grpc_addr, Duration::from_secs(5)).await?;
    assert_eq!(
        fetch_runtime_tcp_listener_addr(&mut child_client).await?,
        game_addr
    );

    assert_no_packet_id(
        &mut stream,
        &codec,
        &mut buffer,
        protocol
            .clientbound_packet_id(TestJavaPacket::LoginSuccess)
            .ok_or("login success packet should be supported")?,
        Duration::from_millis(400),
    )?;
    write_packet(&mut stream, &codec, &held_item_change(6))?;
    let changed_slot = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
        Duration::from_secs(5),
        24,
    )
    .map_err(|error| format!("post-reload post-upgrade held-item echo failed: {error}"))?;
    assert_eq!(held_item_from_packet(protocol, &changed_slot)?, 6);

    shutdown_runtime_via_grpc(&mut child_client, grpc_addr, &mut child).await
}

#[tokio::test]
async fn grpc_upgrade_session_transfer_failure_rolls_back_and_keeps_grpc_available()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = upgrade_test_lock().lock().await;
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    let options = upgrade_enabled_options(grpc_port, "grpc-upgrade-session-transfer");
    write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;

    let (mut child, _logs) = spawn_server_with_log_capture_and_envs(
        temp_dir.path(),
        Stdio::piped(),
        None,
        &[("REVY_UPGRADE_TEST_FAULT", "session-transfer-failure")],
        "grpc-upgrade-session-transfer",
    )?;
    let grpc_addr = SocketAddr::from(([127, 0, 0, 1], grpc_port));
    wait_for_tcp_ready(grpc_addr, Duration::from_secs(5))?;
    let mut client = grpc_client(grpc_addr).await?;

    let error = client
        .upgrade_runtime(authorized_request(proto::UpgradeRuntimeRequest {
            executable_path: env!("CARGO_BIN_EXE_server-bootstrap").to_string(),
        }))
        .await
        .expect_err("upgrade should fail with injected session transfer error");
    assert_eq!(error.code(), Code::FailedPrecondition);

    let status = client
        .get_status(authorized_request(proto::GetStatusRequest {}))
        .await?
        .into_inner();
    assert!(status.status.is_some());

    child.kill()?;
    let _ = child.wait()?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn console_upgrade_reaches_child_and_child_console_keeps_working()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = upgrade_test_lock().blocking_lock();
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    let mut options = ServerTomlOptions::new(true, 0, grpc_port, "console-upgrade");
    options.local_console_permissions = UPGRADE_LOCAL_CONSOLE_PERMISSIONS;
    options.remote_permissions = UPGRADE_REMOTE_PERMISSIONS;
    write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;

    let stdout_path = temp_dir.path().join("server.stdout.log");
    let stderr_path = temp_dir.path().join("server.stderr.log");
    let stdout_file = File::create(&stdout_path)?;
    let stderr_file = File::create(&stderr_path)?;
    let mut child = spawn_server(
        temp_dir.path(),
        Stdio::piped(),
        Stdio::from(stdout_file),
        Stdio::from(stderr_file),
    )?;
    let grpc_addr = SocketAddr::from(([127, 0, 0, 1], grpc_port));
    wait_for_tcp_ready(grpc_addr, Duration::from_secs(5))?;

    {
        let stdin = child.stdin.as_mut().ok_or("child stdin should be piped")?;
        writeln!(stdin, "status")?;
        writeln!(
            stdin,
            "upgrade runtime executable {}",
            env!("CARGO_BIN_EXE_server-bootstrap")
        )?;
    }
    wait_for_output_contains(
        &stdout_path,
        "upgrade runtime: executable=",
        Duration::from_secs(5),
    )?;
    wait_for_tcp_ready(grpc_addr, Duration::from_secs(5))?;
    let runtime = tokio::runtime::Runtime::new()?;
    let mut client = runtime.block_on(async { grpc_client(grpc_addr).await })?;
    let status = runtime.block_on(async {
        client
            .get_status(authorized_request(proto::GetStatusRequest {}))
            .await
    })?;
    assert!(status.into_inner().status.is_some());
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or("child stdin should still be piped")?;
        writeln!(stdin, "status")?;
        writeln!(stdin, "shutdown")?;
    }
    wait_for_tcp_closed(grpc_addr, Duration::from_secs(5))?;
    let exit_status = wait_for_exit(&mut child, Duration::from_secs(5))?
        .ok_or("original parent process should exit after successful cutover")?;
    assert!(
        exit_status.success(),
        "original parent should exit cleanly after cutover; status={exit_status}"
    );
    let stdout =
        wait_for_output_contains(&stdout_path, "shutdown: scheduled", Duration::from_secs(5))?;
    let stderr = fs::read_to_string(&stderr_path).unwrap_or_default();
    assert!(stderr.is_empty() || !stderr.contains("error:"));
    assert!(stdout.contains("upgrade runtime: executable="));
    assert!(
        stdout.matches("runtime active-generation=").count() >= 2,
        "expected status output from both pre-upgrade and post-upgrade consoles; stdout={stdout}"
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn grpc_upgrade_child_import_failure_rolls_back_and_keeps_grpc_available()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = upgrade_test_lock().lock().await;
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    let mut options = ServerTomlOptions::new(true, 0, grpc_port, "grpc-upgrade-rollback");
    options.local_console_permissions = UPGRADE_LOCAL_CONSOLE_PERMISSIONS;
    options.remote_permissions = UPGRADE_REMOTE_PERMISSIONS;
    write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;

    let mut child = spawn_server_with_config_path_and_envs(
        temp_dir.path(),
        Stdio::piped(),
        Stdio::piped(),
        Stdio::piped(),
        None,
        &[("REVY_UPGRADE_TEST_FAULT", "child-import-failure")],
    )?;
    let grpc_addr = SocketAddr::from(([127, 0, 0, 1], grpc_port));
    wait_for_tcp_ready(grpc_addr, Duration::from_secs(5))?;
    let mut client = grpc_client(grpc_addr).await?;

    let error = client
        .upgrade_runtime(authorized_request(proto::UpgradeRuntimeRequest {
            executable_path: env!("CARGO_BIN_EXE_server-bootstrap").to_string(),
        }))
        .await
        .expect_err("upgrade should fail with injected child import error");
    assert_eq!(error.code(), Code::FailedPrecondition);

    let status = client
        .get_status(authorized_request(proto::GetStatusRequest {}))
        .await?
        .into_inner();
    assert!(status.status.is_some());

    {
        let stdin = child.stdin.as_mut().ok_or("child stdin should be piped")?;
        writeln!(stdin, "status")?;
    }
    thread::sleep(Duration::from_millis(500));
    if let Some(status) = child.try_wait()? {
        return Err(
            format!("server exited unexpectedly after rollback with status {status}").into(),
        );
    }

    child.kill()?;
    let _ = child.wait()?;
    let (stdout, _stderr) = read_child_output(&mut child)?;
    assert!(stdout.contains("runtime active-generation="));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn grpc_upgrade_ready_timeout_rolls_back_and_keeps_grpc_available()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = upgrade_test_lock().lock().await;
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    let options = upgrade_enabled_options(grpc_port, "grpc-upgrade-ready-timeout");
    write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;

    let (mut child, _logs) = spawn_logged_server_with_envs(
        temp_dir.path(),
        "grpc-upgrade-ready-timeout",
        &[
            ("REVY_UPGRADE_TEST_FAULT", "child-ready-timeout"),
            ("REVY_UPGRADE_TEST_READY_TIMEOUT_MS", "500"),
        ],
    )?;
    let grpc_addr = SocketAddr::from(([127, 0, 0, 1], grpc_port));
    let mut upgrade_client = wait_for_grpc_client(grpc_addr, Duration::from_secs(5)).await?;
    let mut status_client = grpc_client(grpc_addr).await?;
    let game_addr = fetch_runtime_tcp_listener_addr(&mut upgrade_client).await?;
    let codec = MinecraftWireCodec;
    let protocol = TestJavaProtocol::Je5;
    let (mut stream, mut buffer) =
        connect_and_login_java_client(game_addr, &codec, protocol, "timeout-play-a")
            .map_err(|error| format!("ready-timeout login failed: {error}"))?;
    let _ = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
        Duration::from_secs(5),
        24,
    )
    .map_err(|error| format!("ready-timeout bootstrap held-item read failed: {error}"))?;

    let upgrade_task = tokio::spawn(async move {
        upgrade_client
            .upgrade_runtime(authorized_request(proto::UpgradeRuntimeRequest {
                executable_path: env!("CARGO_BIN_EXE_server-bootstrap").to_string(),
            }))
            .await
    });

    let waiting_status = wait_for_upgrade_phase(
        &mut status_client,
        proto::RuntimeUpgradeRole::Parent,
        proto::RuntimeUpgradePhase::ParentWaitingChildReady,
        Duration::from_secs(5),
    )
    .await?;
    assert!(waiting_status.upgrade.is_some());

    write_packet(&mut stream, &codec, &held_item_change(9))?;
    assert_no_packet_id(
        &mut stream,
        &codec,
        &mut buffer,
        protocol
            .clientbound_packet_id(TestJavaPacket::HeldItemChange)
            .ok_or("held item change packet should be supported")?,
        Duration::from_millis(200),
    )?;

    let error = upgrade_task
        .await?
        .expect_err("upgrade should fail with injected child ready timeout");
    assert_eq!(error.code(), Code::FailedPrecondition);

    let status = fetch_status(&mut status_client).await?;
    assert!(status.upgrade.is_none());
    assert_eq!(
        status
            .session_summary
            .as_ref()
            .ok_or("status session summary was missing after rollback")?
            .total,
        1,
        "rollback should restore the existing play session"
    );

    child.kill()?;
    let _ = child.wait()?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn grpc_upgrade_grpc_takeover_failure_rolls_back_and_keeps_grpc_available()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = upgrade_test_lock().lock().await;
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    let options = upgrade_enabled_options(grpc_port, "grpc-upgrade-grpc-takeover");
    write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;

    let (mut child, _logs) = spawn_logged_server_with_envs(
        temp_dir.path(),
        "grpc-upgrade-grpc-takeover",
        &[("REVY_UPGRADE_TEST_FAULT", "grpc-takeover-failure")],
    )?;
    let grpc_addr = SocketAddr::from(([127, 0, 0, 1], grpc_port));
    wait_for_tcp_ready(grpc_addr, Duration::from_secs(5))?;
    let mut client = grpc_client(grpc_addr).await?;

    let error = client
        .upgrade_runtime(authorized_request(proto::UpgradeRuntimeRequest {
            executable_path: env!("CARGO_BIN_EXE_server-bootstrap").to_string(),
        }))
        .await
        .expect_err("upgrade should fail with injected child gRPC takeover error");
    assert_eq!(error.code(), Code::FailedPrecondition);

    let status = client
        .get_status(authorized_request(proto::GetStatusRequest {}))
        .await?
        .into_inner();
    assert!(status.status.is_some());

    child.kill()?;
    let _ = child.wait()?;
    Ok(())
}

#[tokio::test]
async fn grpc_upgrade_rejects_bedrock_enabled_runtime() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = upgrade_test_lock().lock().await;
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    let mut options = upgrade_enabled_options(grpc_port, "grpc-upgrade-bedrock-reject");
    options.bedrock_enabled = true;
    write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;

    let (mut child, _logs) = spawn_logged_server(temp_dir.path(), "grpc-upgrade-bedrock-reject")?;
    let grpc_addr = SocketAddr::from(([127, 0, 0, 1], grpc_port));
    wait_for_tcp_ready(grpc_addr, Duration::from_secs(5))?;
    let mut client = grpc_client(grpc_addr).await?;

    let error = client
        .upgrade_runtime(authorized_request(proto::UpgradeRuntimeRequest {
            executable_path: env!("CARGO_BIN_EXE_server-bootstrap").to_string(),
        }))
        .await
        .expect_err("upgrade should reject bedrock-enabled topology");
    assert_eq!(error.code(), Code::FailedPrecondition);

    let status = client
        .get_status(authorized_request(proto::GetStatusRequest {}))
        .await?
        .into_inner();
    assert!(status.status.is_some());

    child.kill()?;
    let _ = child.wait()?;
    Ok(())
}
