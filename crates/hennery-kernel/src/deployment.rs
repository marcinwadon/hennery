//! Whether the collector's deployment deserves kernel spec §10's warning
//! (plan 4d-B3): "when gateway credentials exist for more than one hat and
//! the collector shares its OS user with the host child". One verdict, read
//! by the collector's start line, `GET /api/settings` and `hennery doctor`
//! (over the admin socket), so no client works it out again.
//!
//! The collector cannot see another process's user. It knows it shares
//! `hennery up`'s host child's because `up` says so (`--beside-host`): `up`
//! starts both children as its own user, always. A collector started on its
//! own does not know, and says nothing: a host run by hand as the
//! collector's user is the same exposure, and is not detected (kernel §10's
//! recommendation stands for it).
//!
//! Only counts cross into the kernel: never a hat's id or name.

use anyhow::Result;
use std::sync::Arc;

/// Kernel spec §10's recommendation: the fix wherever the warning is shown.
pub const RECOMMENDATION: &str = "run the collector as a separate OS user or in a container (the Docker image), \
                                  with the hosts paired to it like any remote host";

/// What the verdict is drawn from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    /// Started by `hennery up` beside its host child (`--beside-host`), so
    /// as the OS user of every agent of that host.
    pub beside_host: bool,
    /// How many hats have a gateway credential stored.
    pub hats: usize,
}

/// The verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    /// Not started by `hennery up`: no host child shares its OS user, as far
    /// as the collector knows.
    NotUnderUp,
    /// Beside `up`'s host child, with credentials for one hat at most.
    OneHat,
    /// Beside `up`'s host child, with credentials for more than one hat:
    /// any agent of that host can read every one of them (warn).
    SeveralHats,
}

impl Facts {
    pub fn isolation(self) -> Isolation {
        if !self.beside_host {
            Isolation::NotUnderUp
        } else if self.hats > 1 {
            Isolation::SeveralHats
        } else {
            Isolation::OneHat
        }
    }
}

impl Isolation {
    /// Whether kernel spec §10's warning is due.
    pub fn warns(self) -> bool {
        self == Self::SeveralHats
    }
}

/// How many hats have a gateway credential stored, read when asked.
pub type HatCount = Arc<dyn Fn() -> Result<usize> + Send + Sync>;

/// Where the facts come from: `--beside-host`, and the gateway's store.
#[derive(Clone)]
pub struct Deployment {
    beside_host: bool,
    hats: HatCount,
}

impl Deployment {
    pub fn new(beside_host: bool, hats: impl Fn() -> Result<usize> + Send + Sync + 'static) -> Self {
        Self {
            beside_host,
            hats: Arc::new(hats),
        }
    }

    /// A collector on its own with no gateway: what a router starts with
    /// until the collector gives it its own.
    pub fn alone() -> Self {
        Self::new(false, || Ok(0))
    }

    /// The facts as they are now: the count is read on every call, since
    /// credentials come and go while the collector runs.
    pub fn facts(&self) -> Result<Facts> {
        Ok(Facts {
            beside_host: self.beside_host,
            hats: (self.hats)()?,
        })
    }
}

impl std::fmt::Debug for Deployment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deployment")
            .field("beside_host", &self.beside_host)
            .finish_non_exhaustive()
    }
}
