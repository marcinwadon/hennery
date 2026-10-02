//! A loopback mirror is reached directly, whatever proxy the environment
//! names (decision 7). Its own test binary: the proxy variables are set
//! before any thread exists.

mod support;

use hennery_host::runtime::install::{self, Selection};
use std::collections::BTreeSet;

#[test]
fn a_loopback_mirror_bypasses_the_environments_proxy() {
    for var in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        // SAFETY: the only test in this binary, before its runtime starts:
        // no other thread reads the environment.
        unsafe { std::env::set_var(var, "http://127.0.0.1:1") };
    }
    for var in ["NO_PROXY", "no_proxy"] {
        // SAFETY: as above. Cleared, so it cannot be what spares loopback.
        unsafe { std::env::remove_var(var) };
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let server = support::Server::start().await;
        let fixture = support::Fixture::new("1.0.0");
        fixture.serve(&server);
        let (_dir, layout) = support::data_dir();
        let selection = Selection::new(&fixture.manifest, &fixture.hash(), support::here(), &BTreeSet::new()).unwrap();
        install::install(&layout, &selection, &server.sources(), &support::quiet)
            .await
            .expect("installed through no proxy");
    });
}
