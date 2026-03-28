use crate::common::{
    PreparedServer, SERVER_BOOTSTRAP_BIN, expect_upgrade_error, kill_server, upgrade_test_lock,
    write_stdin_lines,
};
use crate::support::{ServerTomlOptions, TestResult, read_child_output};
use std::thread;
use std::time::Duration;
use tonic::Code;

#[test]
fn local_console_without_upgrade_permission_denies_upgrade_command() -> TestResult<()> {
    let _guard = upgrade_test_lock().blocking_lock();
    let server = PreparedServer::new(|_, grpc_port| {
        Ok(ServerTomlOptions::new(
            true,
            0,
            grpc_port,
            "upgrade-permission-denied",
        ))
    })?;

    let mut child = server.spawn_piped()?;
    let command = format!("upgrade runtime executable {SERVER_BOOTSTRAP_BIN}");
    write_stdin_lines(&mut child, &[command.as_str()])?;
    drop(child.stdin.take());
    thread::sleep(Duration::from_millis(500));
    kill_server(&mut child)?;

    let (stdout, _stderr) = read_child_output(&mut child)?;
    assert!(stdout.contains("permission denied"));
    assert!(stdout.contains("upgrade-runtime"));
    Ok(())
}

#[tokio::test]
async fn grpc_upgrade_permission_denied_without_upgrade_runtime_grant() -> TestResult<()> {
    let _guard = upgrade_test_lock().lock().await;
    let server = PreparedServer::new(|_, grpc_port| {
        Ok(ServerTomlOptions::new(
            true,
            0,
            grpc_port,
            "grpc-upgrade-denied",
        ))
    })?;

    let (mut child, _logs) = server.spawn_logged("grpc-upgrade-denied")?;
    let mut client = server.wait_for_client(Duration::from_secs(5)).await?;
    expect_upgrade_error(
        &mut client,
        SERVER_BOOTSTRAP_BIN,
        Code::PermissionDenied,
        "upgrade should be permission denied",
    )
    .await?;
    kill_server(&mut child)
}
