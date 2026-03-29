mod support;

use mc_proto_test_support::{TestJavaPacket, TestJavaProtocol};
use std::fs;
use std::io::Write;
use std::process::Stdio;
use std::thread;
use std::time::Duration;
use support::*;

#[test]
fn stdin_null_with_grpc_admin_surface_keeps_server_running()
-> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let remote_admin_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    write_server_toml(
        temp_dir.path(),
        &repo_root,
        &world_dir,
        &ServerTomlOptions::new(true, 0, remote_admin_port, "stdin-null-grpc-admin-surface"),
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
fn stdin_null_with_console_only_keeps_server_running() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    write_server_toml(
        temp_dir.path(),
        &repo_root,
        &world_dir,
        &ServerTomlOptions::new(false, 0, 50051, "stdin-null-console-only"),
    )?;

    let mut child = spawn_server(
        temp_dir.path(),
        Stdio::null(),
        Stdio::null(),
        Stdio::piped(),
    )?;

    thread::sleep(Duration::from_millis(500));
    if let Some(status) = child.try_wait()? {
        let (_stdout, stderr) = read_child_output(&mut child)?;
        return Err(format!("server exited early with status {status}; stderr={stderr}").into());
    }

    child.kill()?;
    let _ = child.wait()?;
    Ok(())
}

#[test]
fn piped_status_command_keeps_server_running_after_eof() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let remote_admin_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    write_server_toml(
        temp_dir.path(),
        &repo_root,
        &world_dir,
        &ServerTomlOptions::new(true, 0, remote_admin_port, "stdin-pipe-grpc-admin-surface"),
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
    assert!(
        stdout.contains("runtime active-generation="),
        "expected console status output after EOF; stdout={stdout}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn runtime_failure_exits_even_when_grpc_admin_surface_is_available()
-> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let server_port = reserve_port()?;
    let remote_admin_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    fs::create_dir_all(&world_dir)?;
    set_world_read_only(&world_dir, true)?;
    write_server_toml(
        temp_dir.path(),
        &repo_root,
        &world_dir,
        &ServerTomlOptions::new(
            true,
            server_port,
            remote_admin_port,
            "runtime-failure-grpc-admin-surface",
        ),
    )?;

    let mut child = spawn_server(
        temp_dir.path(),
        Stdio::null(),
        Stdio::piped(),
        Stdio::piped(),
    )?;

    let server_addr = std::net::SocketAddr::from(([127, 0, 0, 1], server_port));
    wait_for_tcp_ready(server_addr, Duration::from_secs(3))?;

    let codec = mc_proto_common::MinecraftWireCodec;
    let protocol = TestJavaProtocol::Je5;
    let (mut stream, mut buffer) =
        connect_and_login_java_client(server_addr, &codec, protocol, "runtime-failure")?;
    let _ = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
        Duration::from_secs(5),
        24,
    )?;
    write_packet(&mut stream, &codec, &held_item_change(4))?;
    let held_item = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
        Duration::from_secs(5),
        24,
    )?;
    assert_eq!(held_item_from_packet(protocol, &held_item)?, 4);

    let Some(status) = wait_for_exit(&mut child, Duration::from_secs(8))? else {
        child.kill()?;
        let _ = child.wait()?;
        let (stdout, stderr) = read_child_output(&mut child)?;
        set_world_read_only(&world_dir, false)?;
        return Err(format!(
            "server did not exit after runtime loop failure; stdout={stdout}; stderr={stderr}"
        )
        .into());
    };
    let (_stdout, stderr) = read_child_output(&mut child)?;
    set_world_read_only(&world_dir, false)?;

    assert!(!status.success());
    assert!(stderr.contains("storage") || stderr.contains("runtime failure"));
    Ok(())
}

#[test]
fn revy_server_config_override_boots_from_custom_path() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempdir()?;
    let repo_root = repo_root()?;
    let remote_admin_port = reserve_port()?;
    let world_dir = temp_dir.path().join("world");
    let custom_config_path = temp_dir.path().join("config").join("server.toml");
    fs::create_dir_all(&world_dir)?;
    write_server_toml_at(
        temp_dir.path(),
        &custom_config_path,
        &repo_root,
        &world_dir,
        &ServerTomlOptions::new(true, 0, remote_admin_port, "env-override-remote-admin"),
    )?;

    let mut child = spawn_server_with_config_path(
        temp_dir.path(),
        Stdio::null(),
        Stdio::null(),
        Stdio::piped(),
        Some(&custom_config_path),
    )?;

    thread::sleep(Duration::from_millis(500));
    if let Some(status) = child.try_wait()? {
        let (_stdout, stderr) = read_child_output(&mut child)?;
        return Err(format!("server exited early with status {status}; stderr={stderr}").into());
    }

    child.kill()?;
    let _ = child.wait()?;
    Ok(())
}

#[test]
fn missing_default_server_config_fails_fast() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempdir()?;

    let mut child = spawn_server(
        temp_dir.path(),
        Stdio::null(),
        Stdio::null(),
        Stdio::piped(),
    )?;
    let Some(status) = wait_for_exit(&mut child, Duration::from_secs(5))? else {
        child.kill()?;
        let _ = child.wait()?;
        return Err("server did not exit after missing default config".into());
    };
    let (_stdout, stderr) = read_child_output(&mut child)?;

    assert!(!status.success());
    assert!(stderr.contains("server config path"));
    assert!(stderr.contains("runtime/server.toml"));
    assert!(stderr.contains("was not found"));
    assert!(!stderr.contains("booting with default config"));
    Ok(())
}

#[test]
fn missing_revy_server_config_fails_fast() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempdir()?;
    let missing_config_path = temp_dir.path().join("missing-server.toml");

    let mut child = spawn_server_with_config_path(
        temp_dir.path(),
        Stdio::null(),
        Stdio::null(),
        Stdio::piped(),
        Some(&missing_config_path),
    )?;
    let Some(status) = wait_for_exit(&mut child, Duration::from_secs(5))? else {
        child.kill()?;
        let _ = child.wait()?;
        return Err("server did not exit after missing REVY_SERVER_CONFIG".into());
    };
    let (_stdout, stderr) = read_child_output(&mut child)?;

    assert!(!status.success());
    assert!(stderr.contains("server config path"));
    assert!(stderr.contains("missing-server.toml"));
    assert!(stderr.contains("was not found"));
    assert!(!stderr.contains("booting with default config"));
    Ok(())
}
