//! Real HTTP boundary for sparse defaults; independent of runtime availability.
use hennery_kernel::{hosts::Hosts, operator::Operator, secret::unix_now};
use hennery_sessions::{AppState, store::Store};
use hennery_testkit::{PUBLIC_URL, owner_phc};
use serde_json::{Value, json};

struct Fixture {
    _dir: tempfile::TempDir,
    state: AppState,
    url: String,
    token: String,
    hat: String,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let state = AppState::new(
            Store::open(&db).unwrap(),
            Hosts::open(&db).unwrap(),
            Operator::open(&db).unwrap(),
        );
        hennery_testkit::operator_client(&state.operator);
        let token = state
            .operator
            .open_session("test", &owner_phc(&state.operator), unix_now())
            .unwrap()
            .unwrap();
        let hat = state.hosts.hats().unwrap()[0].id.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self {
            _dir: dir,
            state,
            url,
            token,
            hat,
        }
    }
    fn request(&self, method: &str, hat: &str, agent: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .request(
                method.parse().unwrap(),
                format!("{}/api/hats/{hat}/session-defaults?agent={agent}", self.url),
            )
            .header("origin", PUBLIC_URL)
            .header("cookie", format!("__Host-hennery_session={}", self.token))
    }
}
#[tokio::test]
async fn persist_reset_conflict_and_never_enable_delegation() {
    let f = Fixture::new().await;
    let response = f.request("GET", &f.hat, "custom").send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    assert_eq!(response.json::<Value>().await.unwrap()["revision"], 0);
    let config = json!({"model":"a","mode":"full","axes":{"fast":false}});
    let response = f
        .request("PUT", &f.hat, "custom")
        .json(&json!({"revision":0,"config":config}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let stored: Value = response.json().await.unwrap();
    assert_eq!(stored["config"], config);
    assert_eq!(stored["revision"], 1);
    let response = f
        .request("PUT", &f.hat, "custom")
        .json(&json!({"revision":0,"config":{}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(response.json::<Value>().await.unwrap()["code"], "defaults_changed");
    let response = f
        .request("PUT", &f.hat, "custom")
        .json(&json!({"revision":1,"config":{}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.json::<Value>().await.unwrap()["revision"], 2);
    let stored: Value = f
        .request("GET", &f.hat, "custom")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stored["config"], json!({}));
    let conn = rusqlite::Connection::open(f._dir.path().join("hennery.db")).unwrap();
    let enabled: i64 = conn
        .query_row(
            "SELECT count(*) FROM hat_session_tools WHERE delegation_enabled = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(enabled, 0);
}
#[tokio::test]
async fn every_put_including_reset_needs_fresh_step_up_and_origin() {
    let mut f = Fixture::new().await;
    let stale = f
        .state
        .operator
        .open_session("stale", &owner_phc(&f.state.operator), unix_now() - 301)
        .unwrap()
        .unwrap();
    let fresh = f.token.clone();
    f.token = stale;
    for config in [json!({}), json!({"mode":"full"})] {
        let response = f
            .request("PUT", &f.hat, "custom")
            .json(&json!({"revision":0,"config":config}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
        assert_eq!(response.json::<Value>().await.unwrap()["code"], "step_up_required");
    }
    let response = f
        .request("PUT", &f.hat, "custom")
        .header("content-type", "application/json")
        .body("invalid json")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403, "route step-up must precede body extraction");
    f.token = fresh;
    let response = reqwest::Client::new()
        .put(format!("{}/api/hats/{}/session-defaults?agent=custom", f.url, f.hat))
        .header("cookie", format!("__Host-hennery_session={}", f.token))
        .header("origin", "https://foreign.example")
        .json(&json!({"revision":0,"config":{}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let response = reqwest::Client::new()
        .get(format!("{}/api/hats/{}/session-defaults?agent=custom", f.url, f.hat))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    let response = reqwest::Client::new()
        .put(format!("{}/api/hats/{}/session-defaults?agent=custom", f.url, f.hat))
        .header("cookie", format!("__Host-hennery_session={}", f.token))
        .json(&json!({"revision":0,"config":{}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let conn = rusqlite::Connection::open(f._dir.path().join("hennery.db")).unwrap();
    conn.execute_batch("INSERT INTO owners(id,created_at) VALUES ('foreign-owner',0); INSERT INTO hats(id,owner_id,name,colour,created_at) VALUES ('foreign-hat','foreign-owner','Other','#64748b',0);").unwrap();
    for method in ["GET", "PUT"] {
        let mut request = f.request(method, "foreign-hat", "custom");
        if method == "PUT" {
            request = request.json(&json!({"revision":0,"config":{}}));
        }
        assert_eq!(request.send().await.unwrap().status(), 404);
    }
    for method in ["GET", "PUT"] {
        let mut request = f.request(method, "missing", "custom");
        if method == "PUT" {
            request = request.json(&json!({"revision":0,"config":{}}));
        }
        assert_eq!(request.send().await.unwrap().status(), 404);
    }
}
#[tokio::test]
async fn bounds_strict_fields_and_frozen_hats() {
    let f = Fixture::new().await;
    for config in [
        json!({"unexpected":"x"}),
        json!({"model":""}),
        json!({"axes":{"mode":"x"}}),
        json!({"axes":{"a":3}}),
    ] {
        let response = f
            .request("PUT", &f.hat, "custom")
            .json(&json!({"revision":0,"config":config}))
            .send()
            .await
            .unwrap();
        assert!([400, 422].contains(&response.status().as_u16()));
    }
    let response = f
        .request("PUT", &f.hat, "custom")
        .header("content-type", "application/json")
        .body(" ".repeat(16385))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 413);
    for query in ["", "?agent=custom&extra=1", "?agent=custom&agent=other"] {
        let response = reqwest::Client::new()
            .get(format!("{}/api/hats/{}/session-defaults{query}", f.url, f.hat))
            .header("cookie", format!("__Host-hennery_session={}", f.token))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(response.json::<Value>().await.unwrap()["code"], "invalid_defaults");
    }
    let response = f.request("GET", &f.hat, "").send().await.unwrap();
    assert_eq!(response.status(), 400);
    let hat = match f.state.hosts.create_hat("Temporary", None, unix_now()).unwrap() {
        hennery_kernel::hats::HatChange::Done(hat) => hat.id,
        other => panic!("{other:?}"),
    };
    f.state.hosts.begin_purge(&hat, unix_now()).unwrap();
    let response = f
        .request("PUT", &hat, "custom")
        .json(&json!({"revision":0,"config":{}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(response.json::<Value>().await.unwrap()["code"], "hat_purging");
}
