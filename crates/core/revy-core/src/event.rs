use crate::{ConnectionId, PlayerId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventTarget {
    Connection(ConnectionId),
    Player(PlayerId),
    EveryoneExcept(PlayerId),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound(serialize = "E: Serialize", deserialize = "E: Deserialize<'de>"))]
pub struct RoutedEvent<E> {
    pub target: EventTarget,
    pub event: E,
}
