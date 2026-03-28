use crate::common::{
    JavaPlaySession, PreparedServer, SERVER_BOOTSTRAP_BIN, assert_process_alive,
    assert_upgrade_task_failed, expect_upgrade_error, fetch_status, kill_server,
    remote_admin_upgrade_options, spawn_upgrade_task, upgrade_test_lock, wait_for_upgrade_phase,
    write_stdin_lines,
};
use crate::support::{
    ServerTomlOptions, TestResult, UPGRADE_CONSOLE_PERMISSIONS, UPGRADE_REMOTE_PERMISSIONS,
    read_child_output,
};
use mc_plugin_admin_grpc::admin as proto;
use mc_proto_test_support::TestJavaPacket;
use std::thread;
use std::time::Duration;
use tonic::Code;

async fn assert_failed_upgrade_keeps_grpc(
    server: &PreparedServer,
    executable_path: &str,
    expected_code: Code,
    context: &str,
) -> TestResult<()> {
    let mut client = server.wait_for_client(Duration::from_secs(5)).await?;
    expect_upgrade_error(&mut client, executable_path, expected_code, context).await?;
    let _ = fetch_status(&mut client).await?;
    Ok(())
}

#[tokio::test]
async fn grpc_upgrade_invalid_executable_path_rolls_back_and_keeps_grpc_available() -> TestResult<()>
{
    let _guard = upgrade_test_lock().lock().await;
    let server = PreparedServer::remote_admin("grpc-upgrade-invalid-executable")?;

    let (mut child, _logs) = server.spawn_logged("grpc-upgrade-invalid-executable")?;
    assert_failed_upgrade_keeps_grpc(
        &server,
        &server.missing_bootstrap_path(),
        Code::FailedPrecondition,
        "upgrade should fail for a missing executable",
    )
    .await?;
    kill_server(&mut child)
}

#[tokio::test]
async fn grpc_upgrade_session_transfer_failure_rolls_back_and_keeps_grpc_available()
-> TestResult<()> {
    let _guard = upgrade_test_lock().lock().await;
    let server = PreparedServer::remote_admin("grpc-upgrade-session-transfer")?;

    let (mut child, _logs) = server.spawn_logged_with_envs(
        "grpc-upgrade-session-transfer",
        &[("REVY_UPGRADE_TEST_FAULT", "session-transfer-failure")],
    )?;
    assert_failed_upgrade_keeps_grpc(
        &server,
        SERVER_BOOTSTRAP_BIN,
        Code::FailedPrecondition,
        "upgrade should fail with injected session transfer error",
    )
    .await?;
    kill_server(&mut child)
}

#[cfg(unix)]
#[tokio::test]
async fn grpc_upgrade_child_import_failure_rolls_back_and_keeps_grpc_available() -> TestResult<()> {
    let _guard = upgrade_test_lock().lock().await;
    let server = PreparedServer::new(|_, grpc_port| {
        let mut options = ServerTomlOptions::new(true, 0, grpc_port, "grpc-upgrade-rollback");
        options.console_permissions = UPGRADE_CONSOLE_PERMISSIONS;
        options.remote_permissions = UPGRADE_REMOTE_PERMISSIONS;
        Ok(options)
    })?;

    let mut child =
        server.spawn_piped_with_envs(&[("REVY_UPGRADE_TEST_FAULT", "child-import-failure")])?;
    assert_failed_upgrade_keeps_grpc(
        &server,
        SERVER_BOOTSTRAP_BIN,
        Code::FailedPrecondition,
        "upgrade should fail with injected child import error",
    )
    .await?;

    write_stdin_lines(&mut child, &["status"])?;
    thread::sleep(Duration::from_millis(500));
    assert_process_alive(&mut child, "server exited unexpectedly after rollback")?;

    kill_server(&mut child)?;
    let (stdout, _stderr) = read_child_output(&mut child)?;
    assert!(stdout.contains("runtime active-generation="));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn grpc_upgrade_ready_timeout_rolls_back_and_keeps_grpc_available() -> TestResult<()> {
    let _guard = upgrade_test_lock().lock().await;
    let server = PreparedServer::remote_admin("grpc-upgrade-ready-timeout")?;

    let (mut child, _logs) = server.spawn_logged_with_envs(
        "grpc-upgrade-ready-timeout",
        &[
            ("REVY_UPGRADE_TEST_FAULT", "child-ready-timeout"),
            ("REVY_UPGRADE_TEST_READY_TIMEOUT_MS", "500"),
        ],
    )?;
    let mut upgrade_client = server.wait_for_client(Duration::from_secs(5)).await?;
    let mut status_client = server.wait_for_client(Duration::from_secs(5)).await?;
    let game_addr = crate::common::fetch_runtime_tcp_listener_addr(&mut upgrade_client).await?;
    let mut session =
        JavaPlaySession::connect(game_addr, "timeout-play-a", "ready-timeout login failed")?;
    session.wait_for_bootstrap("ready-timeout bootstrap held-item read failed")?;

    let upgrade_task = spawn_upgrade_task(upgrade_client);
    let waiting_status = wait_for_upgrade_phase(
        &mut status_client,
        proto::RuntimeUpgradeRole::Parent,
        proto::RuntimeUpgradePhase::ParentWaitingChildReady,
        Duration::from_secs(5),
    )
    .await?;
    assert!(waiting_status.upgrade.is_some());

    session.set_held_item(9, "ready-timeout held-item write failed")?;
    session.assert_no_packet(
        TestJavaPacket::HeldItemChange,
        Duration::from_millis(200),
        "ready-timeout held-item should stay buffered during rollback",
    )?;
    assert_upgrade_task_failed(
        upgrade_task,
        Code::FailedPrecondition,
        "upgrade should fail with injected child ready timeout",
    )
    .await?;

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

    kill_server(&mut child)
}

#[cfg(unix)]
#[tokio::test]
async fn grpc_upgrade_grpc_takeover_failure_rolls_back_and_keeps_grpc_available() -> TestResult<()>
{
    let _guard = upgrade_test_lock().lock().await;
    let server = PreparedServer::remote_admin("grpc-upgrade-grpc-takeover")?;

    let (mut child, _logs) = server.spawn_logged_with_envs(
        "grpc-upgrade-grpc-takeover",
        &[("REVY_UPGRADE_TEST_FAULT", "grpc-takeover-failure")],
    )?;
    assert_failed_upgrade_keeps_grpc(
        &server,
        SERVER_BOOTSTRAP_BIN,
        Code::FailedPrecondition,
        "upgrade should fail with injected child gRPC takeover error",
    )
    .await?;
    kill_server(&mut child)
}

#[tokio::test]
async fn grpc_upgrade_rejects_bedrock_enabled_runtime() -> TestResult<()> {
    let _guard = upgrade_test_lock().lock().await;
    let server = PreparedServer::new(|_, grpc_port| {
        let mut options = remote_admin_upgrade_options(grpc_port, "grpc-upgrade-bedrock-reject");
        options.bedrock_enabled = true;
        Ok(options)
    })?;

    let (mut child, _logs) = server.spawn_logged("grpc-upgrade-bedrock-reject")?;
    assert_failed_upgrade_keeps_grpc(
        &server,
        SERVER_BOOTSTRAP_BIN,
        Code::FailedPrecondition,
        "upgrade should reject bedrock-enabled topology",
    )
    .await?;
    kill_server(&mut child)
}
