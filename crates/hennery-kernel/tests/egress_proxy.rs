//! Plan 8b (a): the egress clients never use a proxy from the environment.
//!
//! A proxy would make the connection the policy checked go elsewhere, and
//! hand the request (credentials included) to whatever the environment
//! names. This is its own test binary with a single test, because it sets
//! process-wide environment variables before any client exists.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use hennery_kernel::egress::{Allowance, Egress, Request, Timeouts};
use reqwest::{Method, Url};

#[test]
fn the_environment_s_proxy_is_never_used() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    // SAFETY: the only test in this binary, before any thread of its own
    // exists (the runtime is built below).
    unsafe {
        for name in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            std::env::set_var(name, &proxy);
        }
        std::env::remove_var("NO_PROXY");
        std::env::remove_var("no_proxy");
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let listener = {
        let _guard = runtime.enter();
        tokio::net::TcpListener::from_std(listener).unwrap()
    };
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    runtime.spawn(async move {
        // Accept and close: a proxied request then fails at once.
        while let Ok((stream, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });

    // Short deadlines, well inside the 10 s bound below, whatever the
    // network's resolver does with `.invalid`.
    let egress = Egress::new(Timeouts {
        connect: Duration::from_secs(2),
        request: Duration::from_secs(2),
    })
    .unwrap();
    runtime.block_on(async {
        // A public name that passes every check; it only fails to resolve.
        // Through a proxy, it would never be resolved here at all.
        let url = Url::parse("https://hennery-egress-test.invalid/").unwrap();
        for allowance in [Allowance::PublicOnly, Allowance::InternalNetwork] {
            let client = egress.client(allowance);
            let sent = tokio::time::timeout(
                Duration::from_secs(10),
                client.send(Request::new(Method::GET, url.clone())),
            )
            .await
            .expect("the request hung");
            assert!(sent.is_err(), "{allowance:?}: an unresolvable name answered");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    });
    assert_eq!(accepted.load(Ordering::SeqCst), 0, "a request went to the proxy");
}
