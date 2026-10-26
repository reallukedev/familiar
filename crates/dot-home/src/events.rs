//! Live events for the portal (SSE). Fire and forget: nobody listening is fine.
use serde_json::Value;
use tokio::sync::broadcast;

/// Who may see an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Audience {
    /// The owner of this instance.
    Instance(String),
    /// Admins.
    Admins,
}

/// One event.
#[derive(Debug, Clone)]
pub struct Event {
    /// Who sees it.
    pub audience: Audience,
    /// SSE event name: `instance`, `activity`, `approval`, `image`.
    pub kind: &'static str,
    /// JSON payload.
    pub data: Value,
}

/// The bus.
#[derive(Clone)]
pub struct Events(broadcast::Sender<Event>);

impl Default for Events {
    fn default() -> Self {
        Self(broadcast::channel(1024).0)
    }
}

impl Events {
    /// Publish to an instance's owner.
    pub fn instance(&self, instance_id: &str, kind: &'static str, data: Value) {
        let _ = self.0.send(Event {
            audience: Audience::Instance(instance_id.to_owned()),
            kind,
            data,
        });
    }
    /// Publish to admins.
    pub fn admins(&self, kind: &'static str, data: Value) {
        let _ = self.0.send(Event {
            audience: Audience::Admins,
            kind,
            data,
        });
    }
    /// Listen.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.0.subscribe()
    }
}
