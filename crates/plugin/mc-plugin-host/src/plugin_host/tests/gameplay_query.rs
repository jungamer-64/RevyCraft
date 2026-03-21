use super::*;

#[test]
fn gameplay_query_tls_restores_previous_query_when_nested() {
    let outer = StubGameplayQuery {
        level_name: "outer",
    };
    let inner = StubGameplayQuery {
        level_name: "inner",
    };

    let observed = with_gameplay_query(&outer, || {
        let outer_name = with_current_gameplay_query(|query| Ok(query.world_meta().level_name))?;
        let inner_name = with_gameplay_query(&inner, || {
            with_current_gameplay_query(|query| Ok(query.world_meta().level_name))
        })?;
        let restored_name = with_current_gameplay_query(|query| Ok(query.world_meta().level_name))?;
        Ok((outer_name, inner_name, restored_name))
    })
    .expect("nested gameplay queries should succeed");

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
fn gameplay_query_tls_requires_an_active_query() {
    let error = with_current_gameplay_query(|query| Ok(query.world_meta().level_name))
        .expect_err("gameplay query access should fail outside callback scope");
    assert!(error.contains("without an active query"));
}
