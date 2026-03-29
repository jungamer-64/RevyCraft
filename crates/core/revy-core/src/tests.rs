use crate::ConnectionId;
use crate::event::{EventTarget, RoutedEvent};
use crate::overlay::{apply_optional_entries, apply_optional_entry};
use crate::revision::Revisioned;
use crate::routing::{ConnectionIdSource, SessionRoutes};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[test]
fn apply_optional_entry_materializes_overlay_updates() {
    let mut map = BTreeMap::new();
    apply_optional_entry(&mut map, 1, Some("one"));
    assert_eq!(map.get(&1), Some(&"one"));
    apply_optional_entry::<_, &str>(&mut map, 1, None);
    assert!(!map.contains_key(&1));
}

#[test]
fn apply_optional_entries_materialize_batch_overlay_updates() {
    let mut map = BTreeMap::new();
    apply_optional_entries(&mut map, [(1, Some("one")), (2, Some("two"))]);
    assert_eq!(map.get(&1), Some(&"one"));
    assert_eq!(map.get(&2), Some(&"two"));
    apply_optional_entries::<_, _, _>(&mut map, [(1, None), (3, Some("three"))]);
    assert!(!map.contains_key(&1));
    assert_eq!(map.get(&3), Some(&"three"));
}

#[test]
fn revisioned_rejects_stale_commit() {
    let mut state = Revisioned::new(Vec::<u8>::new());
    let (revision, ()) = state
        .try_apply(0, |buffer| buffer.push(7))
        .expect("first apply should succeed");
    assert_eq!(revision, 1);
    assert_eq!(state.state(), &vec![7]);
    let conflict = state
        .try_apply(0, |buffer| buffer.push(9))
        .expect_err("stale apply should conflict");
    assert_eq!(conflict.expected, 0);
    assert_eq!(conflict.actual, 1);
    assert_eq!(state.state(), &vec![7]);
}

#[test]
fn revisioned_can_skip_revision_bumps_for_noop_mutations() {
    let mut state = Revisioned::new(Vec::<u8>::new());
    let (revision, ()) = state
        .try_apply_if(0, |_buffer| (), |_| false)
        .expect("noop apply should succeed");
    assert_eq!(revision, 0);
    assert_eq!(state.state(), &Vec::<u8>::new());
}

#[test]
fn connection_id_source_allocates_monotonically_and_observes_seen_ids() {
    let source = ConnectionIdSource::default();
    assert_eq!(source.next_connection_id(), ConnectionId(1));
    assert_eq!(source.next_connection_id(), ConnectionId(2));
    source.observe_connection_id(ConnectionId(7));
    assert_eq!(source.next_connection_id(), ConnectionId(8));
}

#[test]
fn session_routes_track_pending_login_connections() {
    let mut routes = SessionRoutes::default();
    routes.insert_pending_login_route(3_u64, 9_u64);
    routes.insert_pending_login_route(4_u64, 10_u64);
    assert_eq!(routes.pending_login_route(3), Some(9));
    assert_eq!(
        routes.snapshot_pending_login_routes(),
        BTreeMap::from([(3_u64, 9_u64), (4_u64, 10_u64)])
    );
    assert_eq!(routes.clear_pending_login_route(3), Some(9));
    assert_eq!(routes.pending_login_route(3), None);
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum DummyEvent {
    Ping { value: u8 },
}

#[test]
fn routed_event_round_trips_through_serde() {
    let event = RoutedEvent {
        target: EventTarget::Connection(ConnectionId(4)),
        event: DummyEvent::Ping { value: 7 },
    };

    let encoded = serde_json::to_string(&event).expect("dummy event should serialize");
    let decoded: RoutedEvent<DummyEvent> =
        serde_json::from_str(&encoded).expect("dummy event should deserialize");
    assert_eq!(decoded, event);
}
