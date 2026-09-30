//! Seams for tests that must hold the session actor still (feature
//! `test-hooks`, never enabled in a real build).
//!
//! Some races (a switch's answer queued behind a backlog when the Prompt is
//! read, or when the switch's deadline fires) depend on the actor *not*
//! having consumed something yet. A fake adapter can only shape what goes on
//! the wire, not how fast the actor drains it, so without these a test can
//! set such a race up only by wall clock — and a loaded CI runner then makes
//! the setup false, not the code.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::watch;

/// How many updates the actor handles in a row before its other arms get a
/// turn: a backlog longer than this guarantees the actor reads its commands
/// (and polls its deadlines) before reaching whatever waits behind it.
pub const UPDATE_BURST: usize = super::UPDATE_BURST;

/// Shared between a test and the actor it passes these to (in
/// `SessionOptions::test_hooks`).
#[derive(Clone, Debug)]
pub struct TestHooks {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    /// One-shot: the next `set_config` that sends a switch holds the actor.
    hold_armed: AtomicBool,
    released: watch::Sender<bool>,
    /// Switch answers pushed onto the actor's inbound channel so far.
    answers_queued: watch::Sender<u64>,
}

impl TestHooks {
    /// Hold the actor right after the first `set_config` it sends to the
    /// adapter, until `release`: nothing (updates, answers, commands) is
    /// consumed meanwhile, while the connection task keeps queueing what the
    /// adapter sends.
    pub fn hold_after_first_switch() -> Self {
        Self {
            inner: Arc::new(Inner {
                hold_armed: AtomicBool::new(true),
                released: watch::Sender::new(false),
                answers_queued: watch::Sender::new(0),
            }),
        }
    }

    /// Let a held actor go on (and never hold it again).
    pub fn release(&self) {
        self.inner.hold_armed.store(false, Ordering::SeqCst);
        self.inner.released.send_replace(true);
    }

    /// Resolves once at least `n` switch answers are in the actor's inbound
    /// channel (queued, not necessarily handled).
    pub async fn answers_queued(&self, n: u64) {
        let mut queued = self.inner.answers_queued.subscribe();
        // The sender lives in `inner`, which `self` keeps alive.
        let _ = queued.wait_for(|count| *count >= n).await;
    }

    /// Called by the actor's switch-answer callback, right after the answer
    /// is pushed onto the inbound channel.
    pub(super) fn answer_queued(&self) {
        self.inner.answers_queued.send_modify(|count| *count += 1);
    }

    /// Called by the actor right after a `set_config` sent a switch.
    pub(super) async fn hold_if_armed(&self) {
        if self.inner.hold_armed.swap(false, Ordering::SeqCst) {
            let mut released = self.inner.released.subscribe();
            let _ = released.wait_for(|released| *released).await;
        }
    }
}
