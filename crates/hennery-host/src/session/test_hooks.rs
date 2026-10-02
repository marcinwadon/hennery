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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::watch;

/// How many updates the actor handles in a row before its other arms get a
/// turn: a backlog longer than this guarantees the actor reads its commands
/// (and polls its deadlines) before reaching whatever waits behind it.
pub const UPDATE_BURST: usize = super::UPDATE_BURST;

/// How many `config_timeout`s an orphaned switch is still tracked for.
pub const ORPHAN_GRACE: u32 = super::ORPHAN_GRACE;

/// Where a held actor stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HoldAt {
    /// Right after a `set_config` sent a switch.
    SwitchSent,
    /// Right as a switch's deadline fires, before the arm does anything.
    SwitchDeadline,
    /// Right after a switch's deadline orphaned it.
    Orphaned,
    /// Right after a turn's reply errored, before `exited_within` checks
    /// whether the adapter has already exited.
    PromptErrored,
}

/// Shared between a test and the actor it passes these to (in
/// `SessionOptions::test_hooks`).
#[derive(Clone, Debug)]
pub struct TestHooks {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    hold_at: HoldAt,
    /// One-shot: the actor holds the first time it reaches `hold_at`.
    hold_armed: AtomicBool,
    released: watch::Sender<bool>,
    /// Switch answers pushed onto the actor's inbound channel so far.
    answers_queued: watch::Sender<u64>,
    /// Set once the actor is actually parked at its armed hold point,
    /// waiting for `released`.
    holding: watch::Sender<bool>,
    /// Live updates pushed onto the actor's inbound channel so far.
    updates_queued: watch::Sender<u64>,
    /// Inbound items the actor has handled so far, by any path.
    handled: AtomicU64,
    /// `handled` when the actor went on from its hold (`NOT_YET` before).
    handled_at_release: AtomicU64,
    /// `handled` when the actor read the first `Cancel` it acted on
    /// (`NOT_YET` before).
    handled_at_cancel: AtomicU64,
    /// Each inbound item handled after that `Cancel`, up to `slow_count`
    /// of them, keeps the actor's thread busy this long first: a stand-in
    /// for an outbox commit on a slow disk.
    slow_commit: Duration,
    slow_count: u64,
}

const NOT_YET: u64 = u64::MAX;

impl TestHooks {
    /// Hold the actor right after the first `set_config` it sends to the
    /// adapter, until `release`: nothing (updates, answers, commands) is
    /// consumed meanwhile, while the connection task keeps queueing what the
    /// adapter sends.
    pub fn hold_after_first_switch() -> Self {
        Self::holding_at(HoldAt::SwitchSent)
    }

    /// Hold the actor right as the first switch deadline fires, before
    /// that arm decides anything (or drains anything), until `release`.
    pub fn hold_when_first_switch_deadline_fires() -> Self {
        Self::holding_at(HoldAt::SwitchDeadline)
    }

    /// Hold the actor right after the first switch its deadline orphans
    /// (its requester already answered `config_failed`), until `release`.
    pub fn hold_after_first_orphan() -> Self {
        Self::holding_at(HoldAt::Orphaned)
    }

    /// Hold the actor right after a turn's reply errors, before it checks
    /// whether the adapter has already exited, until `release`. Lets a test
    /// force the real exit only once the actor is provably waiting to check
    /// for it, instead of racing a fixed sleep against `EXIT_SETTLE`.
    pub fn hold_after_prompt_errors() -> Self {
        Self::holding_at(HoldAt::PromptErrored)
    }

    fn holding_at(hold_at: HoldAt) -> Self {
        Self {
            inner: Arc::new(Inner {
                hold_at,
                hold_armed: AtomicBool::new(true),
                released: watch::Sender::new(false),
                answers_queued: watch::Sender::new(0),
                holding: watch::Sender::new(false),
                updates_queued: watch::Sender::new(0),
                handled: AtomicU64::new(0),
                handled_at_release: AtomicU64::new(NOT_YET),
                handled_at_cancel: AtomicU64::new(NOT_YET),
                slow_commit: Duration::ZERO,
                slow_count: 0,
            }),
        }
    }

    /// Make each of the first `count` inbound items the actor handles after
    /// reading a `Cancel` keep its thread busy for `commit` first, as a slow
    /// disk's outbox commit would. Set before the hooks are shared.
    pub fn slow_commits_after_cancel(mut self, commit: Duration, count: u64) -> Self {
        let inner = Arc::get_mut(&mut self.inner).expect("configured before it is shared");
        inner.slow_commit = commit;
        inner.slow_count = count;
        self
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

    /// Live updates pushed onto the actor's inbound channel so far (queued,
    /// not necessarily handled).
    pub fn updates_queued_now(&self) -> u64 {
        *self.inner.updates_queued.borrow()
    }

    /// Resolves once at least `n` live updates have been pushed onto the
    /// actor's inbound channel.
    pub async fn updates_queued(&self, n: u64) {
        let mut queued = self.inner.updates_queued.subscribe();
        let _ = queued.wait_for(|count| *count >= n).await;
    }

    /// Called by the connection's notification handler, right after a live
    /// update is pushed onto the inbound channel.
    pub(super) fn update_queued(&self) {
        self.inner.updates_queued.send_modify(|count| *count += 1);
    }

    /// How many inbound items the actor handled between going on from its
    /// hold and reading its first `Cancel`; `None` until both happened.
    pub fn handled_from_release_to_cancel(&self) -> Option<u64> {
        let released = self.inner.handled_at_release.load(Ordering::SeqCst);
        let cancelled = self.inner.handled_at_cancel.load(Ordering::SeqCst);
        if released == NOT_YET || cancelled == NOT_YET {
            return None;
        }
        cancelled.checked_sub(released)
    }

    /// Called by the actor as it handles any inbound item.
    pub(super) fn inbound_handled(&self) {
        let handled = self.inner.handled.fetch_add(1, Ordering::SeqCst);
        let cancelled = self.inner.handled_at_cancel.load(Ordering::SeqCst);
        if cancelled != NOT_YET
            && handled
                .checked_sub(cancelled)
                .is_some_and(|after| after < self.inner.slow_count)
        {
            // Spun, not slept: a sleep's wake-up can be late by far more
            // than the commit is long, and a commit holds the thread busy.
            let started = std::time::Instant::now();
            while started.elapsed() < self.inner.slow_commit {
                std::hint::spin_loop();
            }
        }
    }

    /// Called by the actor when a `Cancel` makes it send `session/cancel`.
    pub(super) fn cancel_read(&self) {
        let handled = self.inner.handled.load(Ordering::SeqCst);
        let _ = self
            .inner
            .handled_at_cancel
            .compare_exchange(NOT_YET, handled, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// Resolves once the actor has reached its armed hold point and is
    /// actually waiting there for `release` — as opposed to a test assuming
    /// so from timing alone.
    pub async fn holding(&self) {
        let mut holding = self.inner.holding.subscribe();
        let _ = holding.wait_for(|holding| *holding).await;
    }

    /// Called by the actor at each hold point.
    pub(super) async fn hold_if_armed(&self, at: HoldAt) {
        if self.inner.hold_at == at && self.inner.hold_armed.swap(false, Ordering::SeqCst) {
            self.inner.holding.send_replace(true);
            let mut released = self.inner.released.subscribe();
            let _ = released.wait_for(|released| *released).await;
            let handled = self.inner.handled.load(Ordering::SeqCst);
            self.inner.handled_at_release.store(handled, Ordering::SeqCst);
        }
    }
}
