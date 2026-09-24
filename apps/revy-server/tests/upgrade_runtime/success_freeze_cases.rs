use crate::common::upgrade_test_lock;
use crate::success_support::{
    assert_upgrade_mutations_rejected, finish_concurrent_play_scenario,
    send_play_packet_during_upgrade, spawn_upgrade_from_harness, start_upgrade_harness,
    wait_for_parent_preparing,
};
use crate::support::TestResult;

#[tokio::test]
async fn grpc_upgrade_serializes_mutations_and_preserves_concurrent_play_packet() -> TestResult<()>
{
    let _guard = upgrade_test_lock().lock().await;
    let mut harness = start_upgrade_harness().await?;
    let upgrade_task = spawn_upgrade_from_harness(&mut harness)?;
    wait_for_parent_preparing(&mut harness).await?;
    assert_upgrade_mutations_rejected(&mut harness).await?;
    send_play_packet_during_upgrade(&mut harness)?;
    finish_concurrent_play_scenario(&mut harness, upgrade_task).await
}
