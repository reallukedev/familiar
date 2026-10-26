//! Shared application state.
use crate::{
    auth::ratelimit::Limiter, config::Config, crypto::MasterKey, db::Db, events::Events,
    packs::Catalog, vm::Fleet,
};
use std::sync::Arc;

/// Everything a request handler can reach.
pub struct Inner {
    /// Settings from the config file.
    pub config: Arc<Config>,
    /// Database.
    pub db: Db,
    /// Seals secrets at rest.
    pub key: MasterKey,
    /// Rate limits.
    pub limiter: Limiter,
    /// Live portal events.
    pub events: Events,
    /// Every Mac and its supervisor.
    pub fleet: Fleet,
    /// Built-in app packs.
    pub catalog: Arc<Catalog>,
    /// Waiters for owner approvals.
    pub approvals: crate::tools::approvals::Waiters,
    /// Web Push for approvals (VAPID key and client).
    pub push: crate::push::Push,
    /// No users yet: the setup page is open (with its token).
    pub setup_required: std::sync::atomic::AtomicBool,
    /// Admin policy: everyone must use an authenticator app.
    pub require_totp: std::sync::atomic::AtomicBool,
    /// Auto-updates.
    pub updates: crate::updates::Updater,
    /// Recent storage measurements per Mac.
    pub storage: crate::storage::Cache,
    /// Server start time.
    pub started: std::time::Instant,
}

/// Cheap to clone.
pub type App = Arc<Inner>;
