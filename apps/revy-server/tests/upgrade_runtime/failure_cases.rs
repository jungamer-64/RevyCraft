use crate::common::{
    PreparedServer, expect_upgrade_error, fetch_status, kill_server, upgrade_test_lock,
};
use crate::support::TestResult;
use std::time::Duration;
use tonic::Code;

#[tokio::test]
async fn grpc_upgrade_invalid_executable_path_preserves_active_runtime() -> TestResult<()> {
    let _guard = upgrade_test_lock().lock().await;
    let server = PreparedServer::remote_admin("grpc-upgrade-invalid-executable")?;
    let (mut child, _logs) = server.spawn_logged("grpc-upgrade-invalid-executable")?;
    let mut client = server.wait_for_client(Duration::from_secs(5)).await?;

    expect_upgrade_error(
        &mut client,
        &server.missing_bootstrap_path(),
        Code::FailedPrecondition,
        "upgrade should reject a missing executable before staging",
    )
    .await?;
    let status = fetch_status(&mut client).await?;
    assert!(status.upgrade.is_none());

    kill_server(&mut child)
}
