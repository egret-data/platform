//! Minimal in-process event bus. Seams define their own event types (each
//! `impl Event`) and the bus routes by concrete type over broadcast channels —
//! no central event enum, so seams stay isolated. In-proc now; a remote adapter
//! is a future internal swap behind this same surface.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

/// A domain event carried by the bus.
pub trait Event: Clone + Send + Sync + 'static {}

const DEFAULT_CAPACITY: usize = 64;

/// A cheaply-cloneable handle to the in-process bus. Clones share the same
/// channels. (Concrete, not a `dyn` trait — the generic publish/subscribe
/// methods are not object-safe.)
#[derive(Clone, Default)]
pub struct EventBus {
    channels: Arc<Mutex<HashMap<TypeId, Box<dyn Any + Send + Sync>>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Deliver `event` to all current subscribers of `E`. No-op if there are
    /// none. Never blocks (a lagging subscriber drops old events).
    pub fn publish<E: Event>(&self, event: E) {
        let map = self.channels.lock().unwrap();
        if let Some(boxed) = map.get(&TypeId::of::<E>()) {
            if let Some(tx) = boxed.downcast_ref::<broadcast::Sender<E>>() {
                let _ = tx.send(event);
            }
        }
    }

    /// Subscribe to future `E` events.
    pub fn subscribe<E: Event>(&self) -> broadcast::Receiver<E> {
        let mut map = self.channels.lock().unwrap();
        let boxed = map.entry(TypeId::of::<E>()).or_insert_with(|| {
            let (tx, _rx) = broadcast::channel::<E>(DEFAULT_CAPACITY);
            Box::new(tx)
        });
        boxed
            .downcast_ref::<broadcast::Sender<E>>()
            .expect("channel stored under its own TypeId")
            .subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct Ping(u32);
    impl Event for Ping {}
    #[derive(Clone, Debug, PartialEq)]
    struct Pong;
    impl Event for Pong {}

    #[tokio::test]
    async fn subscriber_receives_published_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe::<Ping>();
        bus.publish(Ping(7));
        assert_eq!(rx.recv().await.unwrap(), Ping(7));
    }

    #[tokio::test]
    async fn event_types_are_routed_independently() {
        let bus = EventBus::new();
        let mut ping = bus.subscribe::<Ping>();
        let mut pong = bus.subscribe::<Pong>();
        bus.publish(Pong);
        assert_eq!(pong.recv().await.unwrap(), Pong);
        assert!(ping.try_recv().is_err());
    }

    #[tokio::test]
    async fn publish_with_no_subscriber_is_a_noop() {
        let bus = EventBus::new();
        bus.publish(Ping(1));
        let mut rx = bus.subscribe::<Ping>();
        bus.publish(Ping(2));
        assert_eq!(rx.recv().await.unwrap(), Ping(2));
    }

    #[tokio::test]
    async fn clones_share_channels() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe::<Ping>();
        let bus2 = bus.clone();
        bus2.publish(Ping(9));
        assert_eq!(rx.recv().await.unwrap(), Ping(9));
    }
}
