use crate::common::upgrade_test_lock;
use crate::success_support::{
    assert_online_bootstrap_and_roundtrip, begin_pending_online_login, build_online_runtime,
    complete_cutover_online_login, cutover_logged_runtime, shutdown_logged_runtime,
    start_logged_runtime,
};
use crate::support::TestResult;
use std::time::Duration;

#[tokio::test]
async fn grpc_upgrade_online_login_session_survives_cutover() -> TestResult<()> {
    let _guard = upgrade_test_lock().lock().await;
    let mut runtime = start_logged_runtime(
        build_online_runtime()?,
        "grpc-upgrade-online-login",
        Duration::from_secs(10),
    )
    .await?;
    let mut pending = begin_pending_online_login(
        runtime.game_addr,
        "up-online-a",
        "initial online login handshake failed",
    )?;
    let mut child_client = cutover_logged_runtime(&mut runtime, Duration::from_secs(10)).await?;
    let mut encryption = complete_cutover_online_login(&mut pending)?;
    assert_online_bootstrap_and_roundtrip(&mut pending, &mut encryption)?;
    shutdown_logged_runtime(&mut runtime, &mut child_client).await
}
