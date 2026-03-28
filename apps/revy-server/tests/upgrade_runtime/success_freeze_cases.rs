use crate::common::upgrade_test_lock;
use crate::success_support::{
    assert_freeze_buffers_play_packet, assert_freeze_mutations_rejected, finish_freeze_scenario,
    spawn_freeze_upgrade, start_freeze_harness, wait_for_parent_freeze,
};
use crate::support::TestResult;

#[tokio::test]
async fn grpc_upgrade_freeze_blocks_mutating_admin_requests_and_preserves_buffered_bytes()
-> TestResult<()> {
    let _guard = upgrade_test_lock().lock().await;
    let mut harness = start_freeze_harness().await?;
    let upgrade_task = spawn_freeze_upgrade(&mut harness)?;
    wait_for_parent_freeze(&mut harness).await?;
    assert_freeze_mutations_rejected(&mut harness).await?;
    assert_freeze_buffers_play_packet(&mut harness)?;
    finish_freeze_scenario(&mut harness, upgrade_task).await
}
