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
}
