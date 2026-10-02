//! Lifecycle hooks (kernel spec §5.5): what the kernel's own operations
//! require of the modules that own session and gateway state. They
//! implement the trait, so the kernel never imports them.

/// Called by the kernel's lifecycle operations once the kernel's own part
/// is done.
pub trait LifecycleHooks: Send + Sync {
    /// The host is revoked and its connection is gone (kernel spec §4.3):
    /// nothing it held will ever be reconciled. Must be idempotent: a
    /// repeated revoke calls it again.
    fn on_host_revoked(&self, host_id: &str) -> anyhow::Result<()>;

    /// The hat is frozen for its purge (kernel spec §5.5; plan 9c decision
    /// 10d): delete everything of it the module keeps, before the kernel
    /// deletes the hat's row. Runs in transactions of its own. Must be
    /// idempotent: a purge that stopped is resumed by running it again. An
    /// error leaves the hat frozen, for a later purge to resume.
    fn on_hat_purged(&self, hat_id: &str) -> anyhow::Result<()>;
}
