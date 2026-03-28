use crate::common::upgrade_test_lock;
use crate::success_support::{
    assert_status_session_survives, cutover_logged_runtime, shutdown_logged_runtime,
    start_status_runtime,
};
use crate::support::TestResult;
use std::time::Duration;

#[tokio::test]
async fn grpc_upgrade_status_session_survives_cutover() -> TestResult<()> {
    let _guard = upgrade_test_lock().lock().await;
    let (mut runtime, mut status_session) = start_status_runtime().await?;
    let mut child_client = cutover_logged_runtime(&mut runtime, Duration::from_secs(5)).await?;
    assert_status_session_survives(&mut runtime, &mut status_session)?;
    drop(status_session);
    shutdown_logged_runtime(&mut runtime, &mut child_client).await
}
