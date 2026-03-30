use super::*;

#[test]
fn gameplay_transaction_tls_restores_previous_query_when_nested() {
    let outer_core = stub_server_core("outer");
    let inner_core = stub_server_core("inner");
    let mut outer_scope = super::GameplayInvocationScope::new(
        boxed_gameplay_read_view(outer_core),
        PluginBufferLimits::default(),
    );
    let mut inner_scope = super::GameplayInvocationScope::new(
        boxed_gameplay_read_view(inner_core),
        PluginBufferLimits::default(),
    );

    let observed = with_gameplay_invocation_and_limits(&mut outer_scope, || {
        let outer_name = with_current_gameplay_query(|tx| Ok(tx.world_meta().level_name))?;
        let inner_name = with_gameplay_invocation_and_limits(&mut inner_scope, || {
            with_current_gameplay_query(|tx| Ok(tx.world_meta().level_name))
        })?;
        let restored_name = with_current_gameplay_query(|tx| Ok(tx.world_meta().level_name))?;
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
    let error = with_current_gameplay_query(|tx| Ok(tx.world_meta().level_name))
        .expect_err("gameplay transaction access should fail outside callback scope");
    assert!(error.contains("without an active invocation scope"));
}
