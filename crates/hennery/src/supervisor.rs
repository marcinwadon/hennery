//! `hennery up`'s restart policy (distribution spec §5.2): a child that
//! crashes is started again after a backoff, and one that crashes too often
//! is given up on. The loop is generic over its children, so the tests drive
//! it with fakes on tokio's paused clock.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::future::Future;
use std::path::Path;
use std::process::ExitStatus;
use std::time::Duration;
use tokio::time::Instant;

/// `hennery host run`'s exit code once the collector says the host was
/// revoked (sysexits' `EX_CONFIG`).
pub const REVOKED_EXIT: u8 = 78;

/// Where `up` reports its children, in its data directory.
pub const STATE_FILE: &str = "supervisor.json";

/// How long `up` waits for a child it asked to stop.
const STOP_GRACE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Which {
    Collector,
    Host,
}

impl std::fmt::Display for Which {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Collector => "collector",
            Self::Host => "host",
        })
    }
}

/// When to restart a child, and when to stop trying.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    /// The wait before the first restart; it doubles with every crash in
    /// the window.
    pub first_delay: Duration,
    pub max_delay: Duration,
    /// Crashes older than this are forgotten.
    pub window: Duration,
    /// This many crashes in the window, and the child is given up on.
    pub limit: usize,
    /// A child's first run that ends sooner than this failed to start: that
    /// ends `up` rather than being retried.
    pub startup_grace: Duration,
}

/// Distribution spec §5.2: 1 s doubling to 60 s; ten crashes in five
/// minutes stop the restarts.
pub const POLICY: Policy = Policy {
    first_delay: Duration::from_secs(1),
    max_delay: Duration::from_secs(60),
    window: Duration::from_secs(300),
    limit: 10,
    startup_grace: Duration::from_secs(5),
};

/// One child's recent crashes.
#[derive(Debug, Default)]
pub struct Crashes {
    times: VecDeque<Instant>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    RestartAfter(Duration),
    GiveUp,
}

impl Crashes {
    /// Record a crash at `now`, and say what to do about it.
    pub fn record(&mut self, now: Instant, policy: &Policy) -> Next {
        while self
            .times
            .front()
            .is_some_and(|&at| now.duration_since(at) >= policy.window)
        {
            self.times.pop_front();
        }
        self.times.push_back(now);
        let n = self.times.len();
        if n >= policy.limit {
            return Next::GiveUp;
        }
        // 1 s, 2 s, 4 s …: `n` is at most `limit`, so the shift is small.
        let delay = policy.first_delay.saturating_mul(1u32 << (n - 1).min(16));
        Next::RestartAfter(delay.min(policy.max_delay))
    }

    pub fn in_window(&self) -> usize {
        self.times.len()
    }
}

/// A running child process.
pub trait Child {
    /// Resolves once it has exited. Cancel-safe.
    fn wait(&mut self) -> impl Future<Output = std::io::Result<ExitStatus>>;
    /// Ask it to stop (SIGTERM).
    fn terminate(&self);
}

/// What `up` supervises: starting either child again, and what it needs to
/// know about the host.
pub trait Children {
    type Child: Child;
    /// Start `which` again, with no pairing descriptor: by then the host is
    /// paired.
    fn spawn(&mut self, which: Which) -> anyhow::Result<Self::Child>;
    /// Whether the host's data directory holds a whole pairing.
    fn host_paired(&self) -> bool;
    /// Say how to recover from a revoke (logged once per revoke).
    fn revoked(&self);
}

impl Child for tokio::process::Child {
    async fn wait(&mut self) -> std::io::Result<ExitStatus> {
        tokio::process::Child::wait(self).await
    }

    fn terminate(&self) {
        if let Some(pid) = self.id() {
            // SAFETY: plain kill(2) on a pid we spawned and still own.
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
        }
    }
}

/// How `supervise` ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Asked to (a signal): both children were stopped.
    Stopped,
    /// `up` cannot go on; the other child was stopped too.
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildState {
    Running,
    /// Crashed; waiting for its next start.
    Restarting,
    /// The host only: the collector revoked it; it stays down.
    Revoked,
    /// Crashed too often; it stays down until `up` is started again.
    GaveUp,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildReport {
    pub state: ChildState,
    pub crashes_in_window: usize,
    pub restarts: u32,
    /// How it last exited, as `ExitStatus` prints it.
    pub last_exit: Option<String>,
}

/// What `up` writes to `STATE_FILE` at every change: for `hennery service
/// status`, and later `doctor`. No secret is in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub pid: u32,
    /// Seconds since the Unix epoch.
    pub updated_at: u64,
    pub collector: ChildReport,
    pub host: ChildReport,
}

/// Write `state` to `dir`'s `STATE_FILE`, private, whole or not at all.
pub fn write_state(dir: &Path, state: &State) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = dir.join(format!("{STATE_FILE}.tmp"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(&serde_json::to_vec_pretty(state)?)?;
    file.write_all(b"\n")?;
    drop(file);
    std::fs::rename(&tmp, dir.join(STATE_FILE))?;
    Ok(())
}

enum Slot<C> {
    Running { child: C, since: Instant, first: bool },
    Waiting { until: Instant },
    Down,
}

struct Supervised<C> {
    which: Which,
    slot: Slot<C>,
    crashes: Crashes,
    report: ChildReport,
}

enum Event {
    Exited(std::io::Result<ExitStatus>),
    Due,
}

impl<C: Child> Supervised<C> {
    fn new(which: Which, child: C) -> Self {
        Self {
            which,
            slot: Slot::Running {
                child,
                since: Instant::now(),
                first: true,
            },
            crashes: Crashes::default(),
            report: ChildReport {
                state: ChildState::Running,
                crashes_in_window: 0,
                restarts: 0,
                last_exit: None,
            },
        }
    }

    /// The child's next exit, or its restart falling due; never, when it is
    /// down.
    async fn next(&mut self) -> Event {
        match &mut self.slot {
            Slot::Running { child, .. } => Event::Exited(child.wait().await),
            Slot::Waiting { until } => {
                tokio::time::sleep_until(*until).await;
                Event::Due
            }
            Slot::Down => std::future::pending().await,
        }
    }

    /// Stop it, if it runs, waiting at most `STOP_GRACE`.
    async fn stop(&mut self) {
        if let Slot::Running { child, .. } = &mut self.slot {
            child.terminate();
            let _ = tokio::time::timeout(STOP_GRACE, child.wait()).await;
            self.report.state = ChildState::Stopped;
        } else if matches!(self.slot, Slot::Waiting { .. }) {
            self.report.state = ChildState::Stopped;
        }
        self.slot = Slot::Down;
    }
}

/// Supervise `up`'s two children, already started, until `shutdown`
/// resolves or `up` cannot go on; then stop them, the host first (it parks
/// its sessions and stops its adapters), then the collector. `report` is
/// called at every change.
///
/// - A host that exits 78 was revoked: it stays down, and the collector
///   keeps serving (plan 3a, decision 11).
/// - Any exit while the host holds no pairing ends `up`: the code that
///   pairs it travels once, over the pipe `up` made before the first start
///   (kernel spec §4.2), so only a new `up` can pair it.
/// - A child's first run that ends within `startup_grace` failed to start
///   (a bad configuration, a data directory in use): that ends `up` too.
/// - Any other exit, also a clean one `up` did not ask for, is a crash: the
///   child is started again after a backoff. A host crashing too often is
///   given up on and the collector keeps serving; a collector crashing too
///   often ends `up`, which a service manager then restarts.
pub async fn supervise<S: Children>(
    children: &mut S,
    collector: S::Child,
    host: S::Child,
    policy: &Policy,
    shutdown: impl Future<Output = ()>,
    mut report: impl FnMut(&ChildReport, &ChildReport),
) -> Outcome {
    let mut collector = Supervised::new(Which::Collector, collector);
    let mut host = Supervised::new(Which::Host, host);
    report(&collector.report, &host.report);
    tokio::pin!(shutdown);
    let outcome = loop {
        let (which, event) = tokio::select! {
            event = collector.next() => (Which::Collector, event),
            event = host.next() => (Which::Host, event),
            () = &mut shutdown => break Outcome::Stopped,
        };
        let child = match which {
            Which::Collector => &mut collector,
            Which::Host => &mut host,
        };
        let failed = match event {
            Event::Due => start_again(children, child, policy),
            Event::Exited(status) => exited(children, child, status, policy),
        };
        report(&collector.report, &host.report);
        if let Some(why) = failed {
            break Outcome::Failed(why);
        }
    };
    host.stop().await;
    collector.stop().await;
    report(&collector.report, &host.report);
    outcome
}

/// Start `child` again, now its backoff is over: `Some(why)` when `up`
/// cannot go on.
fn start_again<S: Children>(children: &mut S, child: &mut Supervised<S::Child>, policy: &Policy) -> Option<String> {
    let now = Instant::now();
    match children.spawn(child.which) {
        Ok(process) => {
            tracing::info!(child = %child.which, "started again");
            child.slot = Slot::Running {
                child: process,
                since: now,
                first: false,
            };
            child.report.state = ChildState::Running;
            child.report.restarts += 1;
            None
        }
        // Counted as a crash: a binary that cannot be started any more
        // (moved by an upgrade, say) is given up on like one that crashes.
        Err(err) => {
            let why = format!("could not start: {err:#}");
            tracing::warn!(child = %child.which, error = %why, "restart failed");
            child.report.last_exit = Some(why);
            after_crash(child, now, policy)
        }
    }
}

/// Handle `child`'s exit: `Some(why)` when `up` cannot go on.
fn exited<S: Children>(
    children: &S,
    child: &mut Supervised<S::Child>,
    status: std::io::Result<ExitStatus>,
    policy: &Policy,
) -> Option<String> {
    let now = Instant::now();
    let Slot::Running { since, first, .. } = child.slot else {
        unreachable!("only a running child exits");
    };
    let shown = match &status {
        Ok(status) => status.to_string(),
        Err(err) => format!("wait failed: {err}"),
    };
    child.report.last_exit = Some(shown.clone());
    let code = status.as_ref().ok().and_then(ExitStatus::code);
    if child.which == Which::Host && code == Some(i32::from(REVOKED_EXIT)) {
        child.slot = Slot::Down;
        child.report.state = ChildState::Revoked;
        children.revoked();
        return None;
    }
    tracing::warn!(child = %child.which, status = %shown, "exited");
    if !children.host_paired() {
        child.slot = Slot::Down;
        child.report.state = ChildState::Stopped;
        return Some(format!(
            "the {} exited ({shown}) before the all-in-one host was paired; start `hennery up` again to pair it",
            child.which
        ));
    }
    if first && now.duration_since(since) < policy.startup_grace {
        child.slot = Slot::Down;
        child.report.state = ChildState::Stopped;
        return Some(format!("the {} failed to start ({shown})", child.which));
    }
    after_crash(child, now, policy)
}

/// Back `child` off after a crash at `now`, or give up on it: `Some(why)`
/// when `up` cannot go on.
fn after_crash<C>(child: &mut Supervised<C>, now: Instant, policy: &Policy) -> Option<String> {
    let next = child.crashes.record(now, policy);
    child.report.crashes_in_window = child.crashes.in_window();
    if let Next::RestartAfter(delay) = next {
        tracing::warn!(
            child = %child.which,
            restart_in_secs = delay.as_secs_f64(),
            crashes = child.crashes.in_window(),
            "starting it again after a backoff"
        );
        child.slot = Slot::Waiting { until: now + delay };
        child.report.state = ChildState::Restarting;
        return None;
    }
    child.slot = Slot::Down;
    child.report.state = ChildState::GaveUp;
    let crashes = format!("{} crashes in {} minutes", policy.limit, policy.window.as_secs() / 60);
    match child.which {
        Which::Host => {
            tracing::error!(
                "the all-in-one host crashed {crashes}; it is not started again, and the collector keeps serving. \
                 Restart `hennery up` (or its service) once the cause is fixed"
            );
            None
        }
        Which::Collector => Some(format!("the collector crashed {crashes}; giving up")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::os::unix::process::ExitStatusExt;
    use std::rc::Rc;

    /// What a fake child does once started.
    #[derive(Debug, Clone, Copy)]
    enum Run {
        /// Exit with this code, this long after its start.
        Exit(Duration, i32),
        /// Run until asked to stop.
        Serve,
    }

    /// Everything the fakes saw, in order, with the paused clock's time
    /// since the test began.
    type Log = Rc<RefCell<Vec<(Duration, String)>>>;

    struct FakeChild {
        which: Which,
        run: Run,
        started: Instant,
        stop: Rc<tokio::sync::Notify>,
        log: Log,
        epoch: Instant,
    }

    impl Child for FakeChild {
        async fn wait(&mut self) -> std::io::Result<ExitStatus> {
            let code = match self.run {
                Run::Exit(after, code) => {
                    tokio::time::sleep_until(self.started + after).await;
                    code
                }
                Run::Serve => {
                    self.stop.notified().await;
                    0
                }
            };
            Ok(ExitStatus::from_raw(code << 8))
        }

        fn terminate(&self) {
            self.log
                .borrow_mut()
                .push((self.epoch.elapsed(), format!("terminate {}", self.which)));
            self.stop.notify_one();
        }
    }

    /// Fake children: each start of a child takes the next `Run` of its
    /// script, and `Serve` once the script is used up.
    struct Fakes {
        scripts: HashMap<Which, VecDeque<Run>>,
        paired: bool,
        log: Log,
        epoch: Instant,
        /// How many spawns fail before one succeeds.
        failing_spawns: usize,
    }

    impl Fakes {
        fn new(collector: &[Run], host: &[Run]) -> Self {
            Self {
                scripts: HashMap::from([
                    (Which::Collector, collector.iter().copied().collect()),
                    (Which::Host, host.iter().copied().collect()),
                ]),
                paired: true,
                log: Rc::default(),
                epoch: Instant::now(),
                failing_spawns: 0,
            }
        }

        fn child(&mut self, which: Which) -> FakeChild {
            let run = self
                .scripts
                .get_mut(&which)
                .and_then(VecDeque::pop_front)
                .unwrap_or(Run::Serve);
            self.log
                .borrow_mut()
                .push((self.epoch.elapsed(), format!("start {which}")));
            FakeChild {
                which,
                run,
                started: Instant::now(),
                stop: Rc::default(),
                log: self.log.clone(),
                epoch: self.epoch,
            }
        }

        /// The times `which` was started at, in whole seconds.
        fn starts(&self, which: Which) -> Vec<u64> {
            let wanted = format!("start {which}");
            self.log
                .borrow()
                .iter()
                .filter(|(_, what)| *what == wanted)
                .map(|(at, _)| at.as_secs())
                .collect()
        }

        fn events(&self) -> Vec<String> {
            self.log.borrow().iter().map(|(_, what)| what.clone()).collect()
        }
    }

    impl Children for Fakes {
        type Child = FakeChild;

        fn spawn(&mut self, which: Which) -> anyhow::Result<FakeChild> {
            if self.failing_spawns > 0 {
                self.failing_spawns -= 1;
                self.log
                    .borrow_mut()
                    .push((self.epoch.elapsed(), format!("failed start {which}")));
                anyhow::bail!("no such file");
            }
            Ok(self.child(which))
        }

        fn host_paired(&self) -> bool {
            self.paired
        }

        fn revoked(&self) {
            self.log.borrow_mut().push((self.epoch.elapsed(), "revoked".into()));
        }
    }

    const SECS: fn(u64) -> Duration = Duration::from_secs;

    /// Supervise `fakes` until `stop_after` (or until `up` cannot go on),
    /// returning how it ended and the last report of each child.
    async fn supervise_fakes(fakes: &mut Fakes, stop_after: Duration) -> (Outcome, ChildReport, ChildReport) {
        let collector = fakes.child(Which::Collector);
        let host = fakes.child(Which::Host);
        let last = RefCell::new(None);
        let outcome = supervise(
            fakes,
            collector,
            host,
            &POLICY,
            tokio::time::sleep(stop_after),
            |collector: &ChildReport, host: &ChildReport| {
                *last.borrow_mut() = Some((collector.clone(), host.clone()));
            },
        )
        .await;
        let (collector, host) = last.into_inner().expect("reported");
        (outcome, collector, host)
    }

    /// Distribution spec §5.2: a crashed host is started again after 1 s,
    /// then 2 s, then 4 s; on a signal the host is stopped before the
    /// collector.
    #[tokio::test(start_paused = true)]
    async fn a_crashed_host_is_started_again_with_a_doubling_backoff() {
        let crash = Run::Exit(SECS(10), 1);
        let mut fakes = Fakes::new(&[], &[crash, crash, crash, crash]);
        let (outcome, collector, host) = supervise_fakes(&mut fakes, SECS(100)).await;
        assert_eq!(outcome, Outcome::Stopped);
        // Crashes at 10, 21, 33 and 47; starts 1, 2, 4 and 8 s after each.
        assert_eq!(fakes.starts(Which::Host), [0, 11, 23, 37, 55]);
        assert_eq!(fakes.starts(Which::Collector), [0]);
        let events = fakes.events();
        let tail = &events[events.len() - 2..];
        assert_eq!(tail, ["terminate host", "terminate collector"]);
        assert_eq!(host.state, ChildState::Stopped);
        assert_eq!(host.restarts, 4);
        assert_eq!(host.crashes_in_window, 4);
        assert_eq!(host.last_exit.as_deref(), Some("exit status: 1"));
        assert_eq!(collector.state, ChildState::Stopped);
    }

    /// A clean exit `up` did not ask for is a crash too.
    #[tokio::test(start_paused = true)]
    async fn an_unasked_for_clean_exit_is_a_crash() {
        let mut fakes = Fakes::new(&[Run::Exit(SECS(10), 0)], &[]);
        let (outcome, collector, _) = supervise_fakes(&mut fakes, SECS(30)).await;
        assert_eq!(outcome, Outcome::Stopped);
        assert_eq!(fakes.starts(Which::Collector), [0, 11]);
        assert_eq!(collector.restarts, 1);
    }

    /// Ten crashes in five minutes: the host is given up on, and the
    /// collector keeps serving until the signal.
    #[tokio::test(start_paused = true)]
    async fn a_host_crashing_too_often_is_given_up_on_and_the_collector_keeps_serving() {
        let first = Run::Exit(SECS(6), 1);
        let again = Run::Exit(Duration::from_millis(10), 1);
        let mut script = vec![first];
        script.extend([again; 20]);
        let mut fakes = Fakes::new(&[], &script);
        let (outcome, collector, host) = supervise_fakes(&mut fakes, SECS(600)).await;
        assert_eq!(outcome, Outcome::Stopped);
        // The first start and nine restarts; the tenth crash gives up.
        assert_eq!(fakes.starts(Which::Host).len(), 10);
        assert_eq!(host.state, ChildState::GaveUp);
        assert_eq!(host.crashes_in_window, 10);
        assert_eq!(collector.state, ChildState::Stopped);
        let events = fakes.events();
        assert_eq!(events.last().map(String::as_str), Some("terminate collector"));
        assert!(!events.contains(&"terminate host".to_string()), "{events:?}");
    }

    /// A collector crashing too often ends `up`, stopping the host.
    #[tokio::test(start_paused = true)]
    async fn a_collector_crashing_too_often_ends_up() {
        let mut script = vec![Run::Exit(SECS(6), 1)];
        script.extend([Run::Exit(Duration::from_millis(10), 1); 20]);
        let mut fakes = Fakes::new(&script, &[]);
        let (outcome, collector, host) = supervise_fakes(&mut fakes, SECS(3600)).await;
        let Outcome::Failed(why) = outcome else {
            panic!("{outcome:?}");
        };
        assert!(why.contains("10 crashes in 5 minutes"), "{why}");
        assert_eq!(fakes.starts(Which::Collector).len(), 10);
        assert_eq!(collector.state, ChildState::GaveUp);
        assert_eq!(host.state, ChildState::Stopped);
        assert!(fakes.events().contains(&"terminate host".to_string()));
    }

    /// A revoked host (exit 78) stays down, also when it exits at once, and
    /// the collector keeps serving (plan 3a, decision 11).
    #[tokio::test(start_paused = true)]
    async fn a_revoked_host_stays_down_and_does_not_end_up() {
        let mut fakes = Fakes::new(&[], &[Run::Exit(SECS(1), i32::from(REVOKED_EXIT))]);
        let (outcome, _, host) = supervise_fakes(&mut fakes, SECS(120)).await;
        assert_eq!(outcome, Outcome::Stopped);
        assert_eq!(fakes.starts(Which::Host), [0]);
        assert_eq!(host.state, ChildState::Revoked);
        assert_eq!(host.crashes_in_window, 0);
        assert!(fakes.events().contains(&"revoked".to_string()));

        // Judged before the pairing check too: a host revoked by the
        // collector may find no whole pairing left, and still does not end
        // `up`.
        let mut fakes = Fakes::new(&[], &[Run::Exit(SECS(1), i32::from(REVOKED_EXIT))]);
        fakes.paired = false;
        let (outcome, _, host) = supervise_fakes(&mut fakes, SECS(120)).await;
        assert_eq!(outcome, Outcome::Stopped);
        assert_eq!(host.state, ChildState::Revoked);
    }

    /// A child whose first run ends within the startup grace failed to
    /// start: `up` ends at once, rather than retrying a bad configuration
    /// for minutes.
    #[tokio::test(start_paused = true)]
    async fn a_child_that_fails_to_start_ends_up() {
        let mut fakes = Fakes::new(&[Run::Exit(SECS(1), 1)], &[]);
        let (outcome, collector, host) = supervise_fakes(&mut fakes, SECS(120)).await;
        assert_eq!(
            outcome,
            Outcome::Failed("the collector failed to start (exit status: 1)".into())
        );
        assert_eq!(fakes.starts(Which::Collector), [0]);
        assert_eq!(collector.state, ChildState::Stopped);
        assert_eq!(host.state, ChildState::Stopped);
    }

    /// The code that pairs the host travels once: any exit while the host
    /// is not paired ends `up`, which a new start pairs again.
    #[tokio::test(start_paused = true)]
    async fn an_exit_before_the_host_is_paired_ends_up() {
        let mut fakes = Fakes::new(&[], &[Run::Exit(SECS(30), 1)]);
        fakes.paired = false;
        let (outcome, _, _) = supervise_fakes(&mut fakes, SECS(120)).await;
        let Outcome::Failed(why) = outcome else {
            panic!("{outcome:?}");
        };
        assert!(why.contains("before the all-in-one host was paired"), "{why}");
        assert_eq!(fakes.starts(Which::Host), [0]);
    }

    /// A signal during a backoff ends it: nothing is started after it.
    #[tokio::test(start_paused = true)]
    async fn a_signal_during_a_backoff_starts_nothing() {
        let mut fakes = Fakes::new(&[], &[Run::Exit(SECS(10), 1)]);
        let (outcome, _, host) = supervise_fakes(&mut fakes, Duration::from_millis(10_500)).await;
        assert_eq!(outcome, Outcome::Stopped);
        assert_eq!(fakes.starts(Which::Host), [0]);
        assert_eq!(host.state, ChildState::Stopped);
        assert!(!fakes.events().contains(&"terminate host".to_string()));
    }

    /// A restart that cannot even be spawned counts as a crash.
    #[tokio::test(start_paused = true)]
    async fn a_failed_spawn_is_backed_off_like_a_crash() {
        let mut fakes = Fakes::new(&[], &[Run::Exit(SECS(10), 1)]);
        fakes.failing_spawns = 2;
        let (outcome, _, host) = supervise_fakes(&mut fakes, SECS(60)).await;
        assert_eq!(outcome, Outcome::Stopped);
        // Crash at 10; spawns fail at 11 and 13; started at 17.
        assert_eq!(fakes.starts(Which::Host), [0, 17]);
        assert_eq!(host.crashes_in_window, 3);
        assert_eq!(host.state, ChildState::Stopped);
    }

    /// The backoff doubles to 60 s and the tenth crash in the window gives
    /// up; crashes further apart than the window keep it at 1 s.
    #[test]
    fn the_backoff_doubles_to_a_minute_and_forgets_old_crashes() {
        let start = Instant::now();
        let mut crashes = Crashes::default();
        let delays: Vec<Next> = (0..10).map(|i| crashes.record(start + SECS(i), &POLICY)).collect();
        let expected: Vec<Next> = [1, 2, 4, 8, 16, 32, 60, 60, 60]
            .into_iter()
            .map(|s| Next::RestartAfter(SECS(s)))
            .chain([Next::GiveUp])
            .collect();
        assert_eq!(delays, expected);

        let mut spread = Crashes::default();
        for i in 0..20 {
            assert_eq!(
                spread.record(start + SECS(300 * i), &POLICY),
                Next::RestartAfter(SECS(1))
            );
        }
    }

    /// The state file is private, and replaced whole.
    #[test]
    fn the_state_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let report = ChildReport {
            state: ChildState::GaveUp,
            crashes_in_window: 10,
            restarts: 9,
            last_exit: Some("exit status: 1".into()),
        };
        let state = State {
            pid: 42,
            updated_at: 1,
            collector: report.clone(),
            host: report,
        };
        write_state(dir.path(), &state).unwrap();
        let path = dir.path().join(STATE_FILE);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        let read: State = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(read, state);
        assert!(std::fs::read_to_string(&path).unwrap().contains("\"gave_up\""));
        assert!(!dir.path().join(format!("{STATE_FILE}.tmp")).exists());
    }
}
