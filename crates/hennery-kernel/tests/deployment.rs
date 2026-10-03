//! Kernel spec §10's warning (plan 4d-B3): each verdict at its boundaries,
//! and facts read when asked.

use hennery_kernel::deployment::{Deployment, Facts, Isolation};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn verdict(beside_host: bool, hats: usize) -> Isolation {
    Facts { beside_host, hats }.isolation()
}

/// A collector `up` did not start knows of no host child sharing its user:
/// quiet, however many hats have credentials.
#[test]
fn a_collector_not_under_up_is_quiet_whatever_it_holds() {
    for hats in [0, 1, 2, 7] {
        assert_eq!(verdict(false, hats), Isolation::NotUnderUp, "{hats}");
    }
    assert!(!Isolation::NotUnderUp.warns());
}

/// Beside `up`'s host child, credentials for one hat at most: quiet.
#[test]
fn beside_the_host_child_one_hat_at_most_is_quiet() {
    assert_eq!(verdict(true, 0), Isolation::OneHat);
    assert_eq!(verdict(true, 1), Isolation::OneHat);
    assert!(!Isolation::OneHat.warns());
}

/// Beside `up`'s host child, credentials for two hats or more: the warning.
#[test]
fn beside_the_host_child_two_hats_warn() {
    assert_eq!(verdict(true, 2), Isolation::SeveralHats);
    assert_eq!(verdict(true, 9), Isolation::SeveralHats);
    assert!(Isolation::SeveralHats.warns());
}

/// The count is read on every call, and a failed read is an error, never a
/// quiet zero.
#[test]
fn the_facts_are_read_when_asked() {
    let count = Arc::new(AtomicUsize::new(1));
    let seen = count.clone();
    let deployment = Deployment::new(true, move || Ok(seen.load(Ordering::SeqCst)));
    assert_eq!(
        deployment.facts().unwrap(),
        Facts {
            beside_host: true,
            hats: 1
        }
    );
    count.store(3, Ordering::SeqCst);
    assert_eq!(deployment.facts().unwrap().isolation(), Isolation::SeveralHats);

    let broken = Deployment::new(true, || anyhow::bail!("the gateway's store is gone"));
    assert!(broken.facts().is_err());

    assert_eq!(
        Deployment::alone().facts().unwrap(),
        Facts {
            beside_host: false,
            hats: 0
        }
    );
}
