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
//! - **Bounded:** at most `capacity` addresses are tracked. Past it,
//!   forgotten entries are dropped first, then the oldest that are not
//!   locked out, then the oldest of all.
//! - **In memory only:** a collector restart clears every count and
//!   lockout. There is no global budget and no exemption for loopback.
//!
//! Callers pass `now`, so the policy is testable without sleeping.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::Mutex;
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
}

/// Addresses tracked by default.
pub const DEFAULT_CAPACITY: usize = 4096;

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
/// arrives IPv4-mapped), or its IPv6 /64.
pub fn key(addr: IpAddr) -> IpAddr {
    match addr.to_canonical() {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
    }
}

pub struct Limiter {
    policy: Policy,
    capacity: usize,
    entries: Mutex<HashMap<IpAddr, Entry>>,
}

impl Limiter {
    pub fn new(policy: Policy) -> Self {
        Self::with_capacity(policy, DEFAULT_CAPACITY)
    }

    pub fn with_capacity(policy: Policy, capacity: usize) -> Self {
        Self {
            policy,
            capacity: capacity.max(1),
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Start an attempt from `addr`: `Err(retry_after)` while it is locked
    /// out, else the attempt is counted as a failure until `succeeded`.
    pub fn attempt(&self, addr: IpAddr, now: Instant) -> Result<(), Duration> {
        let policy = self.policy;
        let addr = key(addr);
        let mut entries = self.entries.lock().expect("limiter lock");
        if let Some(until) = entries.get(&addr).and_then(|e| e.locked_until)
            && now < until
        {
            return Err(until - now);
        }
        if !entries.contains_key(&addr) && entries.len() >= self.capacity {
            make_room(&mut entries, &policy, now, self.capacity);
        }
        let entry = entries.entry(addr).or_insert(Entry {
            failures: 0,
            last_failure: now,
            locked_until: None,
        });
        if entry.forgotten(&policy, now) {
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

    /// The attempt succeeded: `addr`'s count and lockout are cleared.
    pub fn succeeded(&self, addr: IpAddr) {
        self.entries.lock().expect("limiter lock").remove(&key(addr));
    }

    /// Addresses currently tracked.
    pub fn tracked(&self) -> usize {
        self.entries.lock().expect("limiter lock").len()
    }
}

/// Bring `entries` under `capacity`, leaving room for one more.
fn make_room(entries: &mut HashMap<IpAddr, Entry>, policy: &Policy, now: Instant, capacity: usize) {
    entries.retain(|_, e| !e.forgotten(policy, now));
    let excess = (entries.len() + 1).saturating_sub(capacity);
    if excess == 0 {
        return;
    }
    // Not locked out first (oldest failure first), then the rest.
    let mut order: Vec<(bool, Instant, IpAddr)> = entries
        .iter()
        .map(|(addr, e)| (e.locked(now), e.last_failure, *addr))
        .collect();
    order.sort();
    for (_, _, addr) in order.into_iter().take(excess) {
        entries.remove(&addr);
    }
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
    fn past_its_capacity_it_evicts_the_oldest_addresses_that_are_not_locked_out() {
        let limiter = Limiter::with_capacity(Policy::ENROLL, 3);
        let t0 = Instant::now();
        for _ in 0..5 {
            limiter.attempt(A, t0).unwrap();
        }
        let addr = |n: u8| IpAddr::V4(std::net::Ipv4Addr::new(198, 51, 100, n));
        for n in 1..=10 {
            limiter.attempt(addr(n), t0 + secs(u64::from(n))).unwrap();
            assert!(limiter.tracked() <= 3);
        }
        // The locked-out address survived every eviction.
        assert!(limiter.attempt(A, t0 + secs(11)).is_err());
    }
}
