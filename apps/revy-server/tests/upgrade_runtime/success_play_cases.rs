use crate::common::upgrade_test_lock;
use crate::success_support::{
    assert_new_player_can_join, assert_play_roundtrip, assert_post_cutover_play_roundtrip,
    cutover_logged_runtime, shutdown_logged_runtime, start_play_runtime,
    start_reloaded_play_runtime,
};
use crate::support::TestResult;
use std::time::Duration;

#[tokio::test]
async fn grpc_upgrade_play_session_survives_cutover_and_accepts_new_connections() -> TestResult<()>
{
    let _guard = upgrade_test_lock().lock().await;
    let (mut runtime, mut session) = start_play_runtime(
        "grpc-upgrade-play-continuity",
        "grpc-upgrade-play-continuity",
        "up-play-a",
        Duration::from_secs(5),
        "initial play login failed",
        "initial held-item bootstrap failed",
    )
    .await?;
    assert_play_roundtrip(
        &mut session,
        3,
        "pre-upgrade held-item write failed",
        "pre-upgrade held-item echo failed",
    )?;
    let mut child_client = cutover_logged_runtime(&mut runtime, Duration::from_secs(5)).await?;
    assert_post_cutover_play_roundtrip(
        &mut session,
        4,
        "login success should not reappear after cutover",
        "post-upgrade held-item write failed",
        "post-upgrade held-item echo failed",
    )?;
    assert_new_player_can_join(
        runtime.game_addr,
        "up-play-b",
        "fresh post-upgrade login failed",
    )?;
    shutdown_logged_runtime(&mut runtime, &mut child_client).await
}

#[tokio::test]
async fn grpc_upgrade_after_full_reload_preserves_play_session() -> TestResult<()> {
    let _guard = upgrade_test_lock().lock().await;
    let (mut runtime, mut session) = start_reloaded_play_runtime().await?;
    assert_play_roundtrip(
        &mut session,
        2,
        "post-reload held-item write failed",
        "post-reload held-item echo failed",
    )?;
    let mut child_client = cutover_logged_runtime(&mut runtime, Duration::from_secs(5)).await?;
    assert_post_cutover_play_roundtrip(
        &mut session,
        6,
        "login success should not reappear after reload cutover",
        "post-reload post-upgrade held-item write failed",
        "post-reload post-upgrade held-item echo failed",
    )?;
    shutdown_logged_runtime(&mut runtime, &mut child_client).await
}
