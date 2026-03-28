use super::*;

#[test]
fn gameplay_transaction_tls_restores_previous_query_when_nested() {
    let mut outer_core = stub_server_core("outer");
    let mut inner_core = stub_server_core("inner");
    let mut outer_tx = outer_core.begin_gameplay_transaction(0);
    let mut inner_tx = inner_core.begin_gameplay_transaction(0);

    let observed =
        with_gameplay_transaction_and_limits(&mut outer_tx, PluginBufferLimits::default(), || {
            let outer_name =
                with_current_gameplay_transaction(|tx| Ok(tx.world_meta().level_name))?;
            let inner_name = with_gameplay_transaction_and_limits(
                &mut inner_tx,
                PluginBufferLimits::default(),
                || with_current_gameplay_transaction(|tx| Ok(tx.world_meta().level_name)),
            )?;
            let restored_name =
                with_current_gameplay_transaction(|tx| Ok(tx.world_meta().level_name))?;
            Ok((outer_name, inner_name, restored_name))
        })
        .expect("nested gameplay transactions should succeed");

    assert_eq!(
        observed,
        (
            "outer".to_string(),
            "inner".to_string(),
            "outer".to_string()
        )
    );
}

#[test]
fn gameplay_transaction_tls_requires_an_active_transaction() {
    let error = with_current_gameplay_transaction(|tx| Ok(tx.world_meta().level_name))
        .expect_err("gameplay transaction access should fail outside callback scope");
    assert!(error.contains("without an active transaction"));
}
