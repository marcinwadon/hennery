//! Failure-based rate limiting per client address (kernel spec §3.2, §4.1):
//! a few free failures within a window, then an exponentially growing
//! lockout.
//!
//! - **Every attempt counts as a failure the moment it is made**
//!   (`attempt`), under the same lock as the lockout check, so a burst of
//!   parallel requests cannot get more guesses in than the policy allows.
//!   Only a success (`succeeded`) takes it back, and clears the address.
//! - **The key is the network, not the socket address:** an IPv4-mapped
//!   IPv6 address counts as its IPv4 address, and IPv6 addresses count per
//!   /64, the block one subscriber usually holds.
//! - **The window runs from the end of the last lockout**, so waiting out a
//!   lockout does not reset the count; the next wrong attempt doubles it.
//! - **Bounded, and a live entry is never evicted (amendment 2026-10-01).**
//!   At most `capacity` addresses are tracked. Past it, forgotten entries are
//!   pruned first; if the table is still full of live ones, the new
//!   address's attempt is instead counted against its prefix's entry (an
//!   IPv4 /24, an IPv6 /48) in a second table of at most `aggregates`
//!   prefixes, and only past that against one shared "overflow" entry, each
//!   with the same budget and lockout, rather than evicting an unrelated
//!   address's real count. An earlier version evicted the oldest
//!   not-locked-out entry (then the oldest of all) to make room — but an
//!   attacker holding more addresses than `capacity` (a `/52` of IPv6 is
//!   4097 distinct `/64`s) could rotate through them so that no single
//!   address ever accumulated enough failures to lock out, turning the
//!   per-address budget into a throughput limit only. Past capacity, a
//!   flood from one prefix locks that prefix out together; only a flood
//!   spanning more prefixes than the second table holds reaches the shared
//!   overflow budget (security audit 2026-10-29, F3), and a whole flood
//!   gets at most (`capacity` + `aggregates` + known + 2) budgets a window.
//! - **Addresses that succeeded are known (F3).** The last `known`
//!   addresses that succeeded keep an entry of their own past capacity, as
//!   loopback does, so a flood cannot push the owner's usual address into
//!   a prefix or overflow budget it has locked. It is no exemption: the
//!   budget is the same as anyone's, and every credential is still
//!   checked. Only a success, which needs the credential, makes an address
//!   known. Limiters whose successes prove the same thing (password and
//!   passkey login: the owner signed in) share one set of known addresses
//!   (`KnownAddresses`), so a sign-in by either method protects the other;
//!   each keeps its own counts, which a success by the other never clears.
//! - **In memory only:** a collector restart clears every count and
//!   lockout, the prefix and overflow entries, and the known addresses.
//!   Each *tracked* address still has its own separate budget; only
//!   addresses past capacity share their prefix's, or the overflow's.
//! - **Loopback is one address with its own entry (3b decision 9).** Every
//!   `127.0.0.0/8` address counts as `127.0.0.1` (on Linux the whole block
//!   reaches `lo`, so per-address entries would give a local process
//!   millions of budgets), and loopback always gets its own entry, past
//!   capacity too: `hennery up`'s own host and an unconfigured reverse proxy on the same
//!   machine are never pushed into the shared overflow budget by a flood
//!   from elsewhere. It is not exempt: its budget is the same as anyone's.
//!
//! Callers pass `now`, so the policy is testable without sleeping.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Failures allowed within `window`; the last of them starts the first
    /// lockout.
    pub free_failures: u32,
    /// Failures are forgotten this long after the last one, or after the
    /// end of its lockout.
    pub window: Duration,
    /// The first lockout; each further failure doubles it.
    pub first_lockout: Duration,
    pub max_lockout: Duration,
}

impl Policy {
    /// Host enrollment: 5 wrong codes per 10 minutes (kernel spec §4.1).
    pub const ENROLL: Policy = Policy {
        free_failures: 5,
        window: Duration::from_secs(10 * 60),
        first_lockout: Duration::from_secs(60),
        max_lockout: Duration::from_secs(60 * 60),
    };

    /// Operator login and step-up: 5 wrong passwords per minute, then
    /// exponential backoff (kernel spec §3.2).
    pub const LOGIN: Policy = Policy {
        free_failures: 5,
        window: Duration::from_secs(60),
        first_lockout: Duration::from_secs(60),
        max_lockout: Duration::from_secs(60 * 60),
    };

    /// Passkey logins begun and not finished: 30 a minute, then the same
    /// backoff (plan 3c decision 8). A budget of its own, so a password
    /// lockout does not stop passkey login (3b-i's obligation). Every start
    /// counts, and a page that offers passkeys on load starts one each
    /// time, hence more than a password's five.
    pub const PASSKEY_LOGIN: Policy = Policy {
        free_failures: 30,
        window: Duration::from_secs(60),
        first_lockout: Duration::from_secs(60),
        max_lockout: Duration::from_secs(60 * 60),
    };
}

/// Addresses tracked by default.
pub const DEFAULT_CAPACITY: usize = 4096;

/// Prefixes tracked by default once the address table is full.
pub const DEFAULT_AGGREGATES: usize = 1024;

/// Addresses that succeeded, remembered by default.
pub const DEFAULT_KNOWN: usize = 64;

#[derive(Debug, Clone, Copy)]
struct Entry {
    failures: u32,
    last_failure: Instant,
    locked_until: Option<Instant>,
}

impl Entry {
    fn locked(&self, now: Instant) -> bool {
        self.locked_until.is_some_and(|until| now < until)
    }

    /// Nothing within `window` of the last failure or, if later, of the end
    /// of the lockout: waiting out a long lockout does not reset the count.
    fn forgotten(&self, policy: &Policy, now: Instant) -> bool {
        let since = self
            .locked_until
            .map_or(self.last_failure, |u| u.max(self.last_failure));
        !self.locked(now) && now.saturating_duration_since(since) >= policy.window
    }
}

/// What a client address is counted as: its IPv4 address (also when it
/// arrives IPv4-mapped), or its IPv6 /64. Every IPv4 loopback address is
/// `127.0.0.1`, and `::1` is itself.
pub fn key(addr: IpAddr) -> IpAddr {
    match addr.to_canonical() {
        IpAddr::V4(v4) if v4.is_loopback() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V4(v4) => IpAddr::V4(v4),
        // Not masked: its /64 would be `::`, which is not loopback.
        IpAddr::V6(v6) if v6.is_loopback() => IpAddr::V6(v6),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
    }
}

/// The prefix a key past capacity is counted under: an IPv4 /24, an IPv6
/// /48 (a site's usual allocation).
pub fn aggregate(key: IpAddr) -> IpAddr {
    match key {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            IpAddr::V4(Ipv4Addr::new(a, b, c, 0))
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], 0, 0, 0, 0, 0))
        }
    }
}

/// The last addresses (keys) that succeeded, at most `capacity`, most
/// recent first (F3). Shared between limiters whose successes prove the
/// same thing; it only recognises an address, and holds no counts.
pub struct KnownAddresses {
    capacity: usize,
    recent: Mutex<VecDeque<IpAddr>>,
}

impl KnownAddresses {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            recent: Mutex::new(VecDeque::new()),
        }
    }

    /// Whether `addr`'s key is one of them.
    pub fn contains(&self, addr: IpAddr) -> bool {
        self.recent.lock().expect("known lock").contains(&key(addr))
    }

    /// `addr`'s key succeeded: it is the most recent, and past `capacity`
    /// the oldest is forgotten.
    fn remember(&self, addr: IpAddr) {
        let addr = key(addr);
        let mut recent = self.recent.lock().expect("known lock");
        recent.retain(|known| *known != addr);
        if self.capacity > 0 {
            recent.push_front(addr);
            recent.truncate(self.capacity);
        }
    }
}

/// The per-address table, the per-prefix table new addresses fall back to
/// once it is full of live entries, and the one shared budget past both
/// (amendment 2026-10-01, F3).
struct State {
    entries: HashMap<IpAddr, Entry>,
    aggregates: HashMap<IpAddr, Entry>,
    overflow: Option<Entry>,
}

pub struct Limiter {
    policy: Policy,
    capacity: usize,
    aggregates: usize,
    /// Its own, or shared (`with_shared_known`). Never locked while
    /// `state` is.
    known: Arc<KnownAddresses>,
    state: Mutex<State>,
}

impl Limiter {
    pub fn new(policy: Policy) -> Self {
        Self::with_capacity(policy, DEFAULT_CAPACITY)
    }

    pub fn with_capacity(policy: Policy, capacity: usize) -> Self {
        Self::with_capacities(policy, capacity, DEFAULT_AGGREGATES, DEFAULT_KNOWN)
    }

    /// `capacity` addresses, `aggregates` prefixes, `known` addresses that
    /// succeeded.
    pub fn with_capacities(policy: Policy, capacity: usize, aggregates: usize, known: usize) -> Self {
        Self::sharing(policy, capacity, aggregates, Arc::new(KnownAddresses::new(known)))
    }

    /// The default capacities, recognising the addresses of `known`, which
    /// other limiters share: a success here or there makes an address
    /// known to both, and each keeps its own counts.
    pub fn with_shared_known(policy: Policy, known: Arc<KnownAddresses>) -> Self {
        Self::sharing(policy, DEFAULT_CAPACITY, DEFAULT_AGGREGATES, known)
    }

    /// `capacity` addresses and `aggregates` prefixes, recognising the
    /// addresses of `known`.
    pub fn sharing(policy: Policy, capacity: usize, aggregates: usize, known: Arc<KnownAddresses>) -> Self {
        Self {
            policy,
            capacity: capacity.max(1),
            aggregates: aggregates.max(1),
            known,
            state: Mutex::new(State {
                entries: HashMap::new(),
                aggregates: HashMap::new(),
                overflow: None,
            }),
        }
    }

    /// Start an attempt from `addr`: `Err(retry_after)` while it (or the
    /// prefix or overflow budget it falls back to) is locked out, else the
    /// attempt is counted as a failure until `succeeded`.
    pub fn attempt(&self, addr: IpAddr, now: Instant) -> Result<(), Duration> {
        let policy = self.policy;
        let addr = key(addr);
        // Before `state` is locked: the two locks are never held together.
        let known = self.known.contains(addr);
        let mut state = self.state.lock().expect("limiter lock");
        if let Some(entry) = state.entries.get_mut(&addr) {
            return record_attempt(entry, &policy, now);
        }
        let fresh = || Entry {
            failures: 0,
            last_failure: now,
            locked_until: None,
        };
        // A new address: forgotten entries are pruned to make room, but a
        // live one is never evicted for it (amendment 2026-10-01). Loopback
        // (3b decision 9) and a known address (F3) get their own entry
        // regardless.
        state.entries.retain(|_, e| !e.forgotten(&policy, now));
        if state.entries.len() < self.capacity || addr.is_loopback() || known {
            let entry = state.entries.entry(addr).or_insert_with(fresh);
            return record_attempt(entry, &policy, now);
        }
        // The table is still full of live entries: share the prefix's budget
        // instead of evicting someone else's real count, the same way.
        let prefix = aggregate(addr);
        if !state.aggregates.contains_key(&prefix) {
            state.aggregates.retain(|_, e| !e.forgotten(&policy, now));
        }
        if state.aggregates.contains_key(&prefix) || state.aggregates.len() < self.aggregates {
            let entry = state.aggregates.entry(prefix).or_insert_with(fresh);
            return record_attempt(entry, &policy, now);
        }
        // Both tables are full of live entries: share the one overflow.
        let overflow = state.overflow.get_or_insert_with(fresh);
        record_attempt(overflow, &policy, now)
    }

    /// The attempt succeeded: `addr`'s own count and lockout here are
    /// cleared, and it becomes known, to every limiter sharing this one's
    /// known addresses (whose own counts stay). A prefix or the overflow
    /// budget is shared, and not reset by any one address's success.
    pub fn succeeded(&self, addr: IpAddr) {
        let addr = key(addr);
        self.state.lock().expect("limiter lock").entries.remove(&addr);
        self.known.remember(addr);
    }

    /// Forget every address, prefix and the overflow budget: a lockout ends
    /// now. Known addresses stay known.
    pub fn clear(&self) {
        let mut state = self.state.lock().expect("limiter lock");
        state.entries.clear();
        state.aggregates.clear();
        state.overflow = None;
    }

    /// Whether `addr` is one of the addresses that last succeeded, here or
    /// at a limiter sharing this one's known addresses.
    pub fn is_known(&self, addr: IpAddr) -> bool {
        self.known.contains(addr)
    }

    /// Addresses currently tracked with their own entry (not counting the
    /// shared overflow budget, which is not itself an address).
    pub fn tracked(&self) -> usize {
        self.state.lock().expect("limiter lock").entries.len()
    }
}

/// Check `entry`'s lockout first (an attempt while locked out is not
/// counted again), else count this attempt as a failure — forgetting a
/// stale count first — and set a fresh lockout if this failure reaches it.
/// Shared between a per-address, a prefix and the overflow `Entry`: the
/// same policy applies either way.
fn record_attempt(entry: &mut Entry, policy: &Policy, now: Instant) -> Result<(), Duration> {
    if let Some(until) = entry.locked_until
        && now < until
    {
        return Err(until - now);
    }
    if entry.forgotten(policy, now) {
        entry.failures = 0;
        entry.locked_until = None;
    }
    entry.failures += 1;
    entry.last_failure = now;
    if entry.failures >= policy.free_failures {
        let doublings = (entry.failures - policy.free_failures).min(20);
        let lockout = policy
            .first_lockout
            .saturating_mul(1 << doublings)
            .min(policy.max_lockout);
        entry.locked_until = Some(now + lockout);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const A: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1));
    const B: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 2));

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn the_fifth_attempt_locks_out_and_each_further_one_doubles_the_lockout() {
        let limiter = Limiter::new(Policy::ENROLL);
        let t0 = Instant::now();
        for _ in 0..5 {
            assert_eq!(limiter.attempt(A, t0), Ok(()));
        }
        assert_eq!(limiter.attempt(A, t0), Err(secs(60)));
        assert_eq!(limiter.attempt(A, t0 + secs(59)), Err(secs(1)));
        assert_eq!(limiter.attempt(A, t0 + secs(60)), Ok(()));
        assert_eq!(limiter.attempt(A, t0 + secs(60)), Err(secs(120)));
        assert_eq!(limiter.attempt(A, t0 + secs(180)), Ok(()));
        assert_eq!(limiter.attempt(A, t0 + secs(180)), Err(secs(240)));
        // Other addresses are not affected.
        assert_eq!(limiter.attempt(B, t0), Ok(()));
    }

    #[test]
    fn a_parallel_burst_gets_no_more_than_the_free_attempts() {
        let limiter = Arc::new(Limiter::new(Policy::ENROLL));
        let now = Instant::now();
        let barrier = Arc::new(std::sync::Barrier::new(32));
        let threads: Vec<_> = (0..32)
            .map(|_| {
                let limiter = limiter.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    limiter.attempt(A, now).is_ok()
                })
            })
            .collect();
        let allowed = threads.into_iter().map(|t| t.join().unwrap()).filter(|ok| *ok).count();
        assert_eq!(allowed, 5);
    }

    #[test]
    fn failures_older_than_the_window_are_forgotten() {
        let limiter = Limiter::new(Policy::ENROLL);
        let t0 = Instant::now();
        for _ in 0..4 {
            limiter.attempt(A, t0).unwrap();
        }
        let later = t0 + secs(10 * 60);
        for _ in 0..4 {
            assert_eq!(limiter.attempt(A, later), Ok(()));
        }
        assert_eq!(limiter.attempt(A, later), Ok(()));
        assert!(limiter.attempt(A, later).is_err());
    }

    #[test]
    fn a_success_clears_the_count_and_the_lockout_is_capped() {
        let limiter = Limiter::new(Policy::ENROLL);
        let t0 = Instant::now();
        for _ in 0..4 {
            limiter.attempt(A, t0).unwrap();
        }
        limiter.succeeded(A);
        for _ in 0..4 {
            assert_eq!(limiter.attempt(A, t0), Ok(()));
        }
        // Waiting out every lockout, each wrong attempt doubles the next
        // one, up to the cap.
        let mut t = t0;
        let mut longest = Duration::ZERO;
        for _ in 0..40 {
            if let Err(wait) = limiter.attempt(A, t) {
                longest = longest.max(wait);
                t += wait;
            }
        }
        assert_eq!(longest, secs(60 * 60));
    }

    #[test]
    fn an_ipv6_64_is_one_address_and_a_mapped_ipv4_is_its_ipv4() {
        let limiter = Limiter::new(Policy::ENROLL);
        let t0 = Instant::now();
        let v6 = |last: u16| IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 1, 2, 0, 0, 0, last));
        for last in 1..=5 {
            limiter.attempt(v6(last), t0).unwrap();
        }
        assert!(limiter.attempt(v6(99), t0).is_err(), "same /64");
        let other_64 = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 1, 3, 0, 0, 0, 1));
        assert_eq!(limiter.attempt(other_64, t0), Ok(()));

        for _ in 0..5 {
            limiter.attempt(A, t0).unwrap();
        }
        let mapped = IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xc000, 0x0201));
        assert_eq!(key(mapped), A);
        assert!(limiter.attempt(mapped, t0).is_err());
    }

    #[test]
    fn past_its_capacity_forgotten_entries_are_pruned_before_anything_else() {
        let limiter = Limiter::with_capacity(Policy::ENROLL, 3);
        let t0 = Instant::now();
        let addr = |n: u8| IpAddr::V4(std::net::Ipv4Addr::new(198, 51, 100, n));
        for n in 1..=3 {
            limiter.attempt(addr(n), t0).unwrap();
        }
        assert_eq!(limiter.tracked(), 3);
        // Every entry is forgotten by now: the table has room again without
        // anything being shared through the overflow budget.
        let later = t0 + secs(10 * 60);
        assert_eq!(limiter.attempt(addr(4), later), Ok(()));
        assert_eq!(limiter.tracked(), 1);
    }

    /// Amendment 2026-10-01 (round 2): a live (not-yet-forgotten) entry is
    /// never evicted to make room. Past capacity, a new address's attempt is
    /// instead counted against one address-agnostic overflow budget, so an
    /// attacker cannot dodge the per-address limit by rotating through more
    /// addresses than the table can hold.
    #[test]
    fn once_the_table_is_full_of_live_entries_new_addresses_share_one_overflow_budget() {
        let limiter = Limiter::with_capacity(Policy::ENROLL, 3);
        let t0 = Instant::now();
        let live = |n: u8| IpAddr::V4(std::net::Ipv4Addr::new(203, 0, 113, n));
        for n in 1..=3 {
            // One failure each: live and not locked, so none is forgotten
            // and none is locked either.
            assert_eq!(limiter.attempt(live(n), t0), Ok(()));
        }
        assert_eq!(limiter.tracked(), 3);

        // Five different new addresses, each turned away from a full table:
        // every one of them shares the same overflow entry.
        let flood = |n: u8| IpAddr::V4(std::net::Ipv4Addr::new(198, 51, 100, n));
        for n in 1..=5 {
            assert_eq!(limiter.attempt(flood(n), t0), Ok(()), "overflow attempt {n}");
        }
        // The table never grew: no live entry was evicted, and the overflow
        // budget is not itself a tracked address.
        assert_eq!(limiter.tracked(), 3);
        // The sixth address to overflow finds the shared budget locked out
        // for the policy's own first lockout, even though it never made an
        // attempt before — the same doubling policy as any per-address entry.
        assert_eq!(limiter.attempt(flood(6), t0), Err(secs(60)));

        // The three original addresses were never touched by the flood: each
        // still has exactly its first failure. Four more attempts each (five
        // total) are still free, and only the sixth locks them out — proving
        // the count survived, rather than merely returning `Ok` (which a
        // reset-to-zero entry would also do for its next four attempts).
        for n in 1..=3 {
            for _ in 0..4 {
                assert_eq!(
                    limiter.attempt(live(n), t0),
                    Ok(()),
                    "address {n} was reset by the flood"
                );
            }
            assert!(
                limiter.attempt(live(n), t0).is_err(),
                "address {n} was reset by the flood (should be locked after 5 total failures)"
            );
        }
        assert_eq!(limiter.tracked(), 3);
    }

    /// 3b decision 9 (M3 of 3a's final review): past capacity, loopback
    /// still gets its own entry, so a flood from elsewhere cannot use up
    /// `hennery up`'s own budget; and the whole of `127.0.0.0/8` is that
    /// one entry, so a local process cannot rotate through it.
    #[test]
    fn loopback_keeps_one_entry_of_its_own_past_capacity() {
        let limiter = Limiter::with_capacity(Policy::ENROLL, 1);
        let t0 = Instant::now();
        limiter.attempt(A, t0).unwrap();
        // The table is full of a live entry: a new address overflows.
        for _ in 0..5 {
            limiter.attempt(B, t0).unwrap();
        }
        assert!(limiter.attempt(B, t0).is_err(), "the overflow budget is used up");
        let lo = |d: u8| IpAddr::V4(std::net::Ipv4Addr::new(127, d, 0, 1));
        assert_eq!(key(lo(9)), IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        for d in 0..5 {
            assert_eq!(
                limiter.attempt(lo(d), t0),
                Ok(()),
                "loopback shared the overflow budget"
            );
        }
        assert!(limiter.attempt(lo(200), t0).is_err(), "127/8 is one address");
        assert_eq!(limiter.tracked(), 2);
        // `::1` is loopback too, with its own entry.
        assert_eq!(limiter.attempt(IpAddr::V6(Ipv6Addr::LOCALHOST), t0), Ok(()));
    }

    /// The all-locked case: every tracked address is already locked out
    /// (never forgotten, since `forgotten` exempts a locked entry), so
    /// pruning frees nothing. A flood of new addresses must still never
    /// evict any of them.
    #[test]
    fn locked_entries_are_never_evicted_even_by_a_large_flood() {
        let limiter = Limiter::with_capacity(Policy::ENROLL, 2);
        let t0 = Instant::now();
        let locked = |n: u8| IpAddr::V4(std::net::Ipv4Addr::new(203, 0, 113, n));
        for n in 1..=2 {
            for _ in 0..5 {
                limiter.attempt(locked(n), t0).unwrap();
            }
            assert!(limiter.attempt(locked(n), t0).is_err());
        }
        assert_eq!(limiter.tracked(), 2);

        let flood = |n: u16| IpAddr::V4(std::net::Ipv4Addr::new(198, 51, 100, (n % 256) as u8));
        for n in 1..=20 {
            let _ = limiter.attempt(flood(n), t0);
            assert_eq!(limiter.tracked(), 2, "a locked entry was evicted at flood attempt {n}");
        }
        // Both locked addresses are still locked, undisturbed by the flood.
        for n in 1..=2 {
            assert!(limiter.attempt(locked(n), t0).is_err(), "address {n} was reset");
        }
    }

    /// The `n`th /64 of `2001:db8:1::/48`.
    fn in_one_48(n: u16) -> IpAddr {
        IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 1, n, 0, 0, 0, 1))
    }

    /// Security audit 2026-10-29, F3: a flood from more /64s than the table
    /// holds, all in one /48, locks that /48 out, not an owner address
    /// elsewhere (once they shared one overflow budget). Rotating to
    /// further /64s of the same /48 gains nothing.
    #[test]
    fn a_flood_from_one_prefix_past_capacity_locks_only_that_prefix() {
        let limiter = Limiter::with_capacity(Policy::LOGIN, 3);
        let t0 = Instant::now();
        for n in 0..3 {
            limiter.attempt(in_one_48(n), t0).unwrap();
        }
        for n in 3..8 {
            assert_eq!(limiter.attempt(in_one_48(n), t0), Ok(()));
        }
        assert_eq!(limiter.attempt(in_one_48(9), t0), Err(secs(60)));
        assert_eq!(limiter.attempt(in_one_48(500), t0), Err(secs(60)));
        let owner = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 77));
        assert_eq!(
            limiter.attempt(owner, t0),
            Ok(()),
            "the owner shares the flood's lockout"
        );
        assert_eq!(limiter.tracked(), 3);
    }

    /// Past both tables, the shared overflow budget remains, so a flood
    /// across more prefixes than the second table holds still locks itself
    /// out rather than getting a budget per address.
    #[test]
    fn past_the_prefix_table_new_prefixes_share_the_overflow_budget() {
        let limiter = Limiter::with_capacities(Policy::LOGIN, 1, 2, 0);
        let t0 = Instant::now();
        let net = |n: u8| IpAddr::V4(Ipv4Addr::new(198, 51, n, 1));
        for n in 0..3 {
            limiter.attempt(net(n), t0).unwrap();
        }
        for n in 3..8 {
            assert_eq!(limiter.attempt(net(n), t0), Ok(()), "overflow attempt {n}");
        }
        assert_eq!(limiter.attempt(net(9), t0), Err(secs(60)));
        // The prefixes already tracked keep their own budgets.
        assert_eq!(limiter.attempt(net(1), t0), Ok(()));
    }

    /// F3: an address that succeeded keeps an entry of its own past both
    /// tables, as loopback does, so a flood that locked the shared budgets
    /// does not lock it out; its own budget is still the same as anyone's.
    #[test]
    fn a_known_address_keeps_its_own_budget_past_capacity() {
        let limiter = Limiter::with_capacities(Policy::LOGIN, 1, 1, 2);
        let t0 = Instant::now();
        let owner = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 77));
        limiter.attempt(owner, t0).unwrap();
        limiter.succeeded(owner);
        assert!(limiter.is_known(owner));
        let net = |n: u8| IpAddr::V4(Ipv4Addr::new(198, 51, n, 1));
        for n in 0..10 {
            let _ = limiter.attempt(net(n), t0);
        }
        assert!(limiter.attempt(net(20), t0).is_err(), "the shared budgets are locked");
        for _ in 0..Policy::LOGIN.free_failures {
            assert_eq!(limiter.attempt(owner, t0), Ok(()));
        }
        assert!(limiter.attempt(owner, t0).is_err(), "a known address is not exempt");
    }

    #[test]
    fn the_known_addresses_are_bounded_most_recent_first() {
        let limiter = Limiter::with_capacities(Policy::LOGIN, 8, 8, 2);
        let addr = |n: u8| IpAddr::V4(Ipv4Addr::new(192, 0, 2, n));
        for n in 1..=3 {
            limiter.succeeded(addr(n));
        }
        assert!(!limiter.is_known(addr(1)));
        assert!(limiter.is_known(addr(2)) && limiter.is_known(addr(3)));
        limiter.succeeded(addr(2));
        limiter.succeeded(addr(4));
        assert!(limiter.is_known(addr(2)) && limiter.is_known(addr(4)));
        assert!(!limiter.is_known(addr(3)));
        // `clear` ends lockouts, not what is known.
        limiter.clear();
        assert!(limiter.is_known(addr(2)));
    }

    /// Password and passkey login share known addresses, not counts: a
    /// sign-in by one method gives the address its own entry at the other,
    /// past a flood that filled that one's table and locked the address's
    /// prefix and the overflow; and genuine failures counted by one method
    /// are not cleared by a success of the other. Both ways round.
    #[test]
    fn known_addresses_are_shared_across_methods_and_counts_are_not() {
        let t0 = Instant::now();
        let owner = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7));
        let neighbour = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 8));
        let elsewhere = |n: u8| IpAddr::V4(Ipv4Addr::new(203, 0, n, 1));
        for (first, second) in [
            (Policy::LOGIN, Policy::PASSKEY_LOGIN),
            (Policy::PASSKEY_LOGIN, Policy::LOGIN),
        ] {
            let known = Arc::new(KnownAddresses::new(4));
            let signed_in = Limiter::sharing(first, 1, 1, known.clone());
            let other = Limiter::sharing(second, 1, 1, known.clone());
            // The other method's table, the owner's prefix and the
            // overflow, all filled and locked by the flood.
            other.attempt(elsewhere(0), t0).unwrap();
            for _ in 0..=second.free_failures {
                let _ = other.attempt(neighbour, t0);
                let _ = other.attempt(elsewhere(1), t0);
            }
            assert!(
                other.attempt(owner, t0).is_err(),
                "the flood did not lock the owner's prefix"
            );
            signed_in.attempt(owner, t0).unwrap();
            signed_in.succeeded(owner);
            assert!(other.is_known(owner));
            for n in 0..second.free_failures {
                assert_eq!(
                    other.attempt(owner, t0),
                    Ok(()),
                    "{n}: the owner shared the flood's lockout"
                );
            }
            assert!(other.attempt(owner, t0).is_err(), "a known address is not exempt");

            // Genuine failures at one method survive the other's success.
            let known = Arc::new(KnownAddresses::new(4));
            let failing = Limiter::sharing(first, 8, 8, known.clone());
            let succeeding = Limiter::sharing(second, 8, 8, known.clone());
            for _ in 0..2 {
                failing.attempt(owner, t0).unwrap();
            }
            succeeding.attempt(owner, t0).unwrap();
            succeeding.succeeded(owner);
            assert!(failing.is_known(owner));
            for _ in 2..first.free_failures {
                assert_eq!(failing.attempt(owner, t0), Ok(()));
            }
            assert!(
                failing.attempt(owner, t0).is_err(),
                "the other method's success cleared the count"
            );
        }
    }
}
