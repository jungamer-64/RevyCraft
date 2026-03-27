mod support;

use revy_admin_grpc::admin as proto;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::net::TcpStream;
use std::net::SocketAddr;
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};
use support::*;
use tempfile::tempdir;
use tonic::metadata::MetadataValue;
use tonic::{Code, Request};

fn authorized_request<T>(message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert(
        "authorization",
        MetadataValue::try_from(format!("Bearer {OPS_TOKEN}"))
            .expect("bearer token metadata should be valid"),
    );
    request
}

async fn grpc_client(
    local_addr: SocketAddr,
) -> Result<
    proto::admin_control_plane_client::AdminControlPlaneClient<tonic::transport::Channel>,
    tonic::transport::Error,
> {
    proto::admin_control_plane_client::AdminControlPlaneClient::connect(format!(
        "http://{local_addr}"
    ))
    .await
}

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

#[test]
fn local_console_without_upgrade_permission_denies_upgrade_command()
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

    let mut child = spawn_server(temp_dir.path(), Stdio::null(), Stdio::null(), Stdio::null())?;
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
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    let options = upgrade_enabled_options(grpc_port, "grpc-upgrade-invalid-executable");
    write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;

    let mut child = spawn_server(temp_dir.path(), Stdio::null(), Stdio::null(), Stdio::null())?;
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
async fn grpc_upgrade_session_transfer_failure_rolls_back_and_keeps_grpc_available()
-> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    let options = upgrade_enabled_options(grpc_port, "grpc-upgrade-session-transfer");
    write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;

    let mut child = spawn_server_with_config_path_and_envs(
        temp_dir.path(),
        Stdio::piped(),
        Stdio::piped(),
        Stdio::piped(),
        None,
        &[("REVY_UPGRADE_TEST_FAULT", "session-transfer-failure")],
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
        let stdin = child.stdin.as_mut().ok_or("child stdin should still be piped")?;
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
    let stdout = wait_for_output_contains(
        &stdout_path,
        "shutdown: scheduled",
        Duration::from_secs(5),
    )?;
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
        return Err(format!("server exited unexpectedly after rollback with status {status}").into());
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
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    let options = upgrade_enabled_options(grpc_port, "grpc-upgrade-ready-timeout");
    write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;

    let mut child = spawn_server_with_config_path_and_envs(
        temp_dir.path(),
        Stdio::null(),
        Stdio::null(),
        Stdio::null(),
        None,
        &[
            ("REVY_UPGRADE_TEST_FAULT", "child-ready-timeout"),
            ("REVY_UPGRADE_TEST_READY_TIMEOUT_MS", "200"),
        ],
    )?;
    let grpc_addr = SocketAddr::from(([127, 0, 0, 1], grpc_port));
    wait_for_tcp_ready(grpc_addr, Duration::from_secs(5))?;
    let mut client = grpc_client(grpc_addr).await?;

    let error = client
        .upgrade_runtime(authorized_request(proto::UpgradeRuntimeRequest {
            executable_path: env!("CARGO_BIN_EXE_server-bootstrap").to_string(),
        }))
        .await
        .expect_err("upgrade should fail with injected child ready timeout");
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
#[tokio::test]
async fn grpc_upgrade_grpc_takeover_failure_rolls_back_and_keeps_grpc_available()
-> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    let options = upgrade_enabled_options(grpc_port, "grpc-upgrade-grpc-takeover");
    write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;

    let mut child = spawn_server_with_config_path_and_envs(
        temp_dir.path(),
        Stdio::null(),
        Stdio::null(),
        Stdio::null(),
        None,
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
async fn grpc_upgrade_rejects_bedrock_enabled_runtime()
-> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let grpc_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    let mut options = upgrade_enabled_options(grpc_port, "grpc-upgrade-bedrock-reject");
    options.bedrock_enabled = true;
    write_server_toml(temp_dir.path(), &repo_root, &world_dir, &options)?;

    let mut child = spawn_server(temp_dir.path(), Stdio::null(), Stdio::null(), Stdio::null())?;
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
