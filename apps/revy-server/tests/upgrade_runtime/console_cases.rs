use crate::common::{
    PreparedServer, SERVER_BOOTSTRAP_BIN, grpc_client, kill_server, remote_admin_upgrade_options,
    upgrade_test_lock, wait_for_clean_parent_exit, wait_for_output_contains, wait_for_tcp_closed,
    write_stdin_lines,
};
use crate::support::TestResult;
use std::fs;
use std::time::Duration;

#[test]
fn console_upgrade_reaches_child_and_child_console_keeps_working() -> TestResult<()> {
    let _guard = upgrade_test_lock().blocking_lock();
    let server = PreparedServer::new(|_, grpc_port| {
        Ok(remote_admin_upgrade_options(grpc_port, "console-upgrade"))
    })?;

    let stdout_path = server.temp_path().join("server.stdout.log");
    let stderr_path = server.temp_path().join("server.stderr.log");
    let mut child = server.spawn_with_log_files(&stdout_path, &stderr_path)?;
    let upgrade_command = format!("upgrade runtime executable {SERVER_BOOTSTRAP_BIN}");

    crate::support::wait_for_tcp_ready(server.grpc_addr, Duration::from_secs(5))?;
    write_stdin_lines(&mut child, &["status", upgrade_command.as_str()])?;
    wait_for_output_contains(
        &stdout_path,
        "upgrade runtime: executable=",
        Duration::from_secs(5),
    )?;

    crate::support::wait_for_tcp_ready(server.grpc_addr, Duration::from_secs(5))?;
    let runtime = tokio::runtime::Runtime::new()?;
    let mut client = runtime.block_on(async { grpc_client(server.grpc_addr).await })?;
    let status = runtime.block_on(async {
        client
            .get_status(crate::common::authorized_request(
                mc_plugin_admin_grpc::admin::GetStatusRequest {},
            ))
            .await
    })?;
    assert!(status.into_inner().status.is_some());

    write_stdin_lines(&mut child, &["status", "shutdown"])?;
    wait_for_tcp_closed(server.grpc_addr, Duration::from_secs(5))?;
    let logs = crate::support::PersistedServerLogCapture {
        stdout_path: stdout_path.clone(),
        stderr_path: stderr_path.clone(),
    };
    wait_for_clean_parent_exit(
        &mut child,
        &logs,
        Duration::from_secs(5),
        "original parent process should exit after successful cutover",
    )?;

    let stdout =
        wait_for_output_contains(&stdout_path, "shutdown: scheduled", Duration::from_secs(5))?;
    let stderr = fs::read_to_string(&stderr_path).unwrap_or_default();
    assert!(stderr.is_empty() || !stderr.contains("error:"));
    assert!(stdout.contains("upgrade runtime: executable="));
    assert!(
        stdout.matches("runtime active-generation=").count() >= 2,
        "expected status output from both pre-upgrade and post-upgrade consoles; stdout={stdout}"
    );

    let _ = kill_server(&mut child);
    Ok(())
}
