# Web Push (plan 10b-ii): delivery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** the collector sends its notices (kernel spec §6). Each notice goes to every subscription of the owner's, under the hat's policy:
- encrypted to the browser (RFC 8291);
- signed with the collector's VAPID key (RFC 8292);
- padded so its size tells the push service little;
- sent through the shared egress client, public addresses only (kernel spec §7.1, plan 8b);
- retried when the service is busy or failing, and the subscription removed when it is gone or expired.

**Architecture** (`hennery-kernel`, and the binary):
- `delivery.rs`:
  - `Transport`, the seam a request goes through, with its implementation for `egress::EgressClient`;
  - `payload_for` (the policy) and `padded`;
  - `Delivery` (`deliver`, `run`) and its `RetryPolicy`;
  - `spawn(hosts, operator, vapid, &egress) -> Push`, which takes `egress.client(Allowance::PublicOnly)` itself.
- `push.rs`: `Hosts::record_push_success`, `record_push_error`, `remove_gone_subscription` (each naming the endpoint the push went to), `prune_expired_subscriptions`.
- **Wire:** `PushPayload { title, body, url, tag }`, what the service worker reads.
- `main.rs`: one `Egress` for the collector, shared with the gateway (agreed with plan 8); `start_push` before anything serves.

**Tech Stack:** Rust (edition 2024, MSRV 1.88). New: `web-push-native =0.5.0` with default features off, for RFC 8291's encryption only. Its `vapid` feature would pull in `jwt-simple`, `superboring` and `rsa` (plan 10a decision 2); the token is 10a's own.

**Spec:** kernel §6 ("`web-push-native` builds RFC 8291 (aes128gcm) requests with VAPID (RFC 8292), sent with the shared HTTP client under the egress policy (§7.1; push endpoints must be public). TTL 1 hour, urgency `high` for 'needs your answer', `normal` otherwise. Non-2xx responses are logged with status; 404/410 delete the subscription."), §7.1; RFC 8030 §5 (TTL, Urgency, Topic), RFC 8291, RFC 8292.

It builds on plans [10a](2026-10-14-push.md) (key, token, subscriptions, policies), [10b-i](2026-10-15-push-triggers.md) (`Notice`, the queue) and [8b](2026-10-13-egress.md) (the egress client). Every anchor was taken from `main` at `fc00485` (10b-iii and #67 merged).

**Status:** written 2026-10-02; executed 2026-10-02 (see "Execution status"). Amended after:
- the security review of 2026-10-02 (A1–A4);
- its scoped re-confirmation, "confirmed", with its notes taken where they cost a line;
- the whole-branch review, "approve with fixes" (both must-fixes taken).

**How the code blocks were made and checked:**
- The code was built and tested first; every block below was generated from its diff.
- The plan was replayed from its text onto `fc00485` step by step; the trees matched byte for byte.
- After every task the five checks passed: 1048 tests after Task 1, 1054 after Task 2.
- Every side-effect line and every guard was revert-probed: 37 probes, each failing its test (the lists in each task's Step 5).

## Execution status (2026-10-02)

**Executed** on `main` at `fc00485` (10b-iii, `4f71aed`, and #67 merged; 8b's egress, `ef75d4f`, before it). The code was built first against 8b's branch behind a fake transport, then rebased onto the merged `hennery_kernel::egress` and checked against it, and reviewed. It was then cut into the two tasks' commits (tests first, then code), and the plan was replayed from its text until the trees matched.

| Area | As built | Why |
|---|---|---|
| The security review (opus, on the maintainer's behalf) | Approved after A1–A4. A1: each try has 10 s and a subscription 30 s in all, and `Retry-After` is honoured up to 10 s, not a minute. A2: a push task that fails is logged and the others' outcomes are still recorded. A3: `keys.auth` is taken as 16 bytes without a panic, and the token cache is a `Tokens` map with an explicit clock, renewing an hour before expiry and dropping expired entries. A4: the wire test checks the padded length. | Notices go one after another: one silent push service could have held every later notice for minutes. A panicked task would have lost its siblings' outcomes. The cache had no bound in time. |
| Its re-confirmation | "Confirmed", no amendments. Taken from its notes: the failed task is logged by id, not by its panic's message; `send_retrying`'s comment names the timeout and the budget. Recorded: with 32 subscriptions at most, a notice whose every service is silent holds the queue about 2 minutes (decision 7). | A panic's message can carry what it unwrapped. |
| The whole-branch review (opus): approve with fixes | Must-fix 1: an outcome is recorded only on a subscription that still has the endpoint pushed to (`remove_gone_subscription`; the owner audit's minimum for `push.rs` is 20). Must-fix 2: `Urgency: normal` is checked. Also taken: `Content-Type` is checked; a `Retry-After` shorter than the scheduled delay is tested; the two comments before `start_push` are one; the probe rows say what they change; the spec write-back corrects §6's "with VAPID". | A browser rotating its subscription while a push to the old endpoint was answered "gone" would have lost the new one. Mapping every notice to `high` passed the suite. |

Checks:
- After each task the five checks passed: 1048 tests after Task 1, 1054 after Task 2.
  - Two tests outside this plan failed once, each in a run with other builds at once, and passed when run again: `host_connection.rs`'s `backoff_resets_to_reconnect_min_after_every_acked_frame`, and `images.rs`'s `a_prompt_at_the_limit_reaches_the_host_whole_and_is_stored_by_reference`, a known flake on `main` that another lane is fixing.
- The 37 revert-probes each failed their test.
- Before merging it was rebased onto `4366dab` (#68's `run_host` change, purge 9a and #65; only the owner audit's and the README's neighbouring lines meet this plan's): the plan replays there too, its blocks unchanged, and the five checks and CI ran on the result.
- No test reaches the network: the fake push service is on `127.0.0.1`, reached only by an `InternalNetwork` client the tests build.
- The run was macOS only, so ubuntu CI is the Linux check.

## Scope

**2 tasks:**
1. Delivery through a `Transport`, tested with a fake one.
2. The egress client as the transport, and the collector's delivery.

**Out:**
- the gateway's `Notifier` calling `AppState::push` (plan 8's wiring, 8d);
- `agent_failure` (10b-i's "After this plan");
- a live check against real browsers and push services (an operator item).

## Decisions

1. **One seam, and the collector's end of it is fixed.** Delivery sends through `Transport`. The collector's is the egress client, public addresses only. `spawn` takes it from the `Egress` itself, so no caller can hand delivery an `InternalNetwork` client. Tests use a fake transport, or an `InternalNetwork` client to a push service on loopback.
2. **The policy is applied in one place** (`payload_for`), as 10b-i's `Notice` documents:
   - muted: nothing is sent;
   - `generic_title`: the generic title, and no body unless `details` is set too (corrected in the spec write-back: `details` still puts the question's title in the body, as `the_policy_decides_what_a_payload_shows` pins; the doc comment on `payload_for` still says "no body");
   - `details`: the `detail`, when there is one, as the body.
3. **The payload** is `{title, body, url, tag}` as JSON.
   - It is padded with trailing spaces (still valid JSON) to the next 512 bytes, at most 3993, RFC 8291's room in a 4096-byte body.
   - The notices' caps keep it far below that; an oversized one is refused, not cut.
4. **The request:**
   - headers: `TTL: 3600`, `Urgency: high|normal`, `Content-Encoding: aes128gcm`, `Content-Type: application/octet-stream`, and `Authorization: vapid t=…, k=…`;
   - never `Topic`, which the push service would see;
   - the token is made once per (audience, subject) and reused until an hour before it expires; one past its expiry is dropped;
   - the keys are the stored ones, canonical since 10a.
5. **What each answer does:**
   - 2xx: `last_success_at`, and `last_error` cleared;
   - 404 and 410: the subscription is removed;
   - 429, 5xx, a timeout, a failed connection: tried again after 1 s and 4 s, or after the service's `Retry-After` (seconds only) when longer, up to 10 s;
   - each try is given 10 s, and a subscription 30 s in all: a retry that could not finish within that is not made, and the failure says it gave up;
   - any other answer, and the egress policy's refusal: final, nothing tried again, and the subscription kept (a name's addresses can change);
   - an outcome is recorded only on a subscription that still has the endpoint the push went to: a browser that rotated it meanwhile keeps its id (plan 10a), and the old endpoint's answer, "gone" included, is not the new one's;
   - a failure is recorded as a short reason of delivery's own (the status code, or "could not be reached"), never the endpoint or the service's body (plan 10a's review, O2).
6. **Expired subscriptions** (past their `expirationTime`) are removed before each notice and sent nothing.
7. **Concurrency:** notices one after another; a notice's subscriptions at most 8 at once.
   - Each is bounded by decision 5's 30 s, so a round of 8 holds the queue up 30 s at most. With the owner's 32 subscriptions at most (plan 10a), a notice whose every service is silent holds it about 2 minutes, the worst case.
   - A task that fails is logged by its id, not its panic's message, which could hold what it unwrapped; the other subscriptions' outcomes are still recorded.
   - The queue (10b-i) holds what waits.
8. **The collector's one `Egress`** is built in `main.rs`, before anything serves. The gateway shares it rather than building a second (agreed with plan 8's lane: it holds one connection pool per allowance).

## Global Constraints

- The five checks pass after each task. **New crate: `web-push-native` (Task 1). Wire type: `PushPayload` (Task 1).**
- The store's new statements name the owner (`push.rs` in the owner audit).
- **No test reaches the network** (#54): the fake push service listens on `127.0.0.1`, and the collector's public-only client refuses it before connecting.
- Commits: Conventional Commits, gmail identity, unsigned.

## Review Focus

1. **A push reaching inside the network.** Expected: never; `spawn`'s client is `PublicOnly`. Tests: `the_collectors_delivery_never_reaches_a_private_address`, `the_collectors_notices_go_to_a_public_only_delivery`.
2. **What the push service sees.** Expected: ciphertext padded to 512 bytes, `TTL`, `Urgency` (`high` for "needs your answer", `normal` otherwise), the content headers, the VAPID header; no `Topic`, nothing of the title. Tests: `a_push_reaches_the_browser_encrypted_padded_and_signed`, `a_push_arrives_on_the_wire_encrypted_and_signed`.
3. **The policy.** Tests: `the_policy_decides_what_a_payload_shows`, `the_hats_policy_decides_what_is_sent`.
4. **Answers.** Tests: `a_subscription_the_service_says_is_gone_is_removed`, `a_busy_or_failing_service_is_tried_again_then_given_up`, `a_refusal_is_final`, `retry_after_is_honoured_up_to_its_cap`, `the_services_answers_over_http_decide_the_subscription`, `a_service_that_cannot_be_reached_is_tried_again_then_recorded`, `an_answer_for_a_rotated_endpoint_leaves_the_new_one_alone`.
5. **Tokens.** Tests: `a_token_is_reused_per_push_service`, `a_token_is_renewed_an_hour_before_it_expires`, `without_a_contact_a_loopback_collector_names_no_subject`.
6. **A slow service holding the others up.** Expected: 30 s at most per round of 8 subscriptions. Tests: `a_slow_or_silent_service_is_given_up_within_the_budget`, `a_failed_push_task_does_not_lose_the_others`.

**Reading the steps.** Each block is one of:
- "Create `path`:" (a new file);
- "In `path`, replace:" with the exact text it replaces, which occurs once, then "with:".

Apply them in order.

---

### Task 1: Delivery through a transport

- [ ] **Step 1: Write the tests**

Create `crates/hennery-kernel/tests/delivery.rs`:

  ```rust
  //! Web Push delivery (kernel spec §6; plan 10b-ii), through a fake
  //! transport: what reaches the push service (encrypted to the browser,
  //! padded, signed), what the hat's policy lets through, and what each answer
  //! does to the subscription. Nothing here opens a connection.

  use base64::Engine;
  use base64::engine::general_purpose::URL_SAFE_NO_PAD;
  use hennery_kernel::delivery::{Delivery, Outcome, PAD_TO, PushRequest, RetryPolicy, Sent, Transport};
  use hennery_kernel::hats::HatChange;
  use hennery_kernel::hosts::Hosts;
  use hennery_kernel::operator::{Operator, SetupOutcome};
  use hennery_kernel::push::{NewSubscription, Notice, Push, PushPolicy, Subscribed, Urgency, VapidKey};
  use hennery_kernel::secret::unix_now;
  use hennery_proto::rest::PushPayload;
  use p256::ecdsa::signature::Verifier;
  use p256::elliptic_curve::sec1::ToEncodedPoint;
  use serde_json::{Value, json};
  use std::collections::{HashMap, VecDeque};
  use std::future::Future;
  use std::pin::Pin;
  use std::sync::{Arc, Mutex};
  use std::time::Duration;

  /// A push service per endpoint: answers from a script (201 once it runs
  /// out), and every request kept. An endpoint `silent` never answers; one
  /// `panicking` fails the task sending to it. `on_send` runs once, as the
  /// first request is sent.
  #[derive(Clone, Default)]
  struct Fake {
      script: Arc<Mutex<HashMap<String, VecDeque<Sent>>>>,
      silent: Arc<Mutex<Vec<String>>>,
      panicking: Arc<Mutex<Vec<String>>>,
      on_send: Arc<Mutex<Option<OnSend>>>,
      sent: Arc<Mutex<Vec<PushRequest>>>,
  }

  type OnSend = Box<dyn FnOnce() + Send>;

  impl Fake {
      fn answer(&self, endpoint: &str, answers: &[Sent]) {
          self.script
              .lock()
              .unwrap()
              .insert(endpoint.into(), answers.iter().cloned().collect());
      }

      fn sent(&self) -> Vec<PushRequest> {
          self.sent.lock().unwrap().clone()
      }

      fn silent(&self, endpoint: &str) {
          self.silent.lock().unwrap().push(endpoint.into());
      }

      fn panicking(&self, endpoint: &str) {
          self.panicking.lock().unwrap().push(endpoint.into());
      }

      fn on_send(&self, then: impl FnOnce() + Send + 'static) {
          *self.on_send.lock().unwrap() = Some(Box::new(then));
      }
  }

  impl Transport for Fake {
      fn send(&self, request: PushRequest) -> Pin<Box<dyn Future<Output = Sent> + Send + '_>> {
          let answer = self
              .script
              .lock()
              .unwrap()
              .get_mut(request.endpoint.as_str())
              .and_then(VecDeque::pop_front)
              .unwrap_or(Sent::Status {
                  code: 201,
                  retry_after: None,
              });
          let silent = self.silent.lock().unwrap().contains(&request.endpoint.to_string());
          let panics = self.panicking.lock().unwrap().contains(&request.endpoint.to_string());
          self.sent.lock().unwrap().push(request);
          let then = self.on_send.lock().unwrap().take();
          if let Some(then) = then {
              then();
          }
          Box::pin(async move {
              if silent {
                  std::future::pending::<()>().await;
              }
              assert!(!panics, "the push task fails");
              answer
          })
      }
  }

  /// A browser's subscription keys, and what it decrypts with.
  struct Browser {
      secret: p256::SecretKey,
      auth: [u8; 16],
  }

  impl Browser {
      fn new(seed: u8) -> Self {
          let secret = p256::SecretKey::from_bytes(&[seed; 32].into()).unwrap();
          Self {
              secret,
              auth: [seed; 16],
          }
      }

      fn p256dh(&self) -> String {
          URL_SAFE_NO_PAD.encode(self.secret.public_key().to_encoded_point(false).as_bytes())
      }

      fn auth(&self) -> String {
          URL_SAFE_NO_PAD.encode(self.auth)
      }

      /// The payload, as the browser reads it: decrypted, then parsed.
      fn read(&self, request: &PushRequest) -> PushPayload {
          let auth = web_push_native::Auth::clone_from_slice(&self.auth);
          let plain = web_push_native::decrypt(request.body.clone(), &self.secret, &auth).unwrap();
          assert_eq!(plain.len(), PAD_TO, "padded to its bucket");
          serde_json::from_slice(&plain).unwrap()
      }
  }

  struct Setup {
      hosts: Arc<Hosts>,
      operator: Arc<Operator>,
      vapid: Arc<VapidKey>,
      fake: Fake,
      session: String,
      _dir: tempfile::TempDir,
  }

  impl Setup {
      /// The owner set up at `public_url`, signed in once.
      fn new(public_url: &str) -> Self {
          let dir = tempfile::tempdir().unwrap();
          let path = dir.path().join("hennery.db");
          let operator = Operator::open(&path).unwrap();
          let hosts = Hosts::open(&path).unwrap();
          let now = unix_now();
          let token = operator.issue_setup_token(now).unwrap().unwrap();
          let SetupOutcome::Done { phc, .. } = operator
              .set_up(&token, "correct horse battery", public_url, now)
              .unwrap()
          else {
              panic!("setup failed");
          };
          let cookie = operator.open_session("browser", &phc, now).unwrap().unwrap();
          let session = operator.authenticate(&cookie, now).unwrap().unwrap().session_id;
          Self {
              hosts: Arc::new(hosts),
              operator: Arc::new(operator),
              vapid: Arc::new(VapidKey::generate()),
              fake: Fake::default(),
              session,
              _dir: dir,
          }
      }

      fn subscribe(&self, endpoint: &str, browser: &Browser, expires_at: Option<i64>) -> String {
          let (p256dh, auth) = (browser.p256dh(), browser.auth());
          let new = NewSubscription {
              endpoint,
              p256dh: &p256dh,
              auth: &auth,
              device_label: None,
              expires_at,
          };
          match self.hosts.subscribe(&new, &self.session, unix_now()).unwrap() {
              Subscribed::Created(sub) => sub.id,
              other => panic!("{other:?}"),
          }
      }

      fn delivery(&self, retry: RetryPolicy) -> Arc<Delivery<Fake>> {
          Delivery::new(
              self.hosts.clone(),
              self.operator.clone(),
              self.vapid.clone(),
              self.fake.clone(),
              retry,
          )
      }

      fn hat(&self) -> String {
          self.hosts.default_hat_for_new_hosts().unwrap()
      }
  }

  /// No waits between tries, and time enough for all of them.
  fn at_once() -> RetryPolicy {
      RetryPolicy {
          delays: vec![Duration::ZERO, Duration::ZERO],
          max_retry_after: Duration::ZERO,
          attempt_timeout: Duration::from_secs(60),
          budget: Duration::from_secs(3600),
      }
  }

  fn notice(hat_id: &str) -> Notice {
      Notice {
          hat_id: hat_id.into(),
          urgency: Urgency::High,
          title: "Fix the flaky test".into(),
          generic_title: "Session needs your answer".into(),
          body: "needs your answer".into(),
          detail: Some("Run cargo test".into()),
          url: "/sessions/s1".into(),
          tag: "s1".into(),
      }
  }

  fn header<'a>(request: &'a PushRequest, name: &str) -> Option<&'a str> {
      request
          .headers
          .iter()
          .find(|(n, _)| n.eq_ignore_ascii_case(name))
          .map(|(_, v)| v.as_str())
  }

  /// The VAPID header's claims, verified with the key it names: a JWS parse
  /// and p256's verify, independent of the signing code.
  fn vapid_claims(authorization: &str, vapid: &VapidKey) -> Value {
      let (t, k) = authorization.strip_prefix("vapid ").unwrap().split_once(", ").unwrap();
      let k = k.strip_prefix("k=").unwrap();
      assert_eq!(k, vapid.public_key());
      let parts: Vec<&str> = t.strip_prefix("t=").unwrap().split('.').collect();
      let signature = p256::ecdsa::Signature::from_slice(&URL_SAFE_NO_PAD.decode(parts[2]).unwrap()).unwrap();
      p256::ecdsa::VerifyingKey::from_sec1_bytes(&URL_SAFE_NO_PAD.decode(k).unwrap())
          .unwrap()
          .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
          .unwrap();
      serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap()
  }

  #[tokio::test]
  async fn a_push_reaches_the_browser_encrypted_padded_and_signed() {
      let setup = Setup::new("https://hennery.example");
      setup.operator.set_contact(Some("me@example.com")).unwrap().unwrap();
      let phone = Browser::new(7);
      let id = setup.subscribe("https://fcm.googleapis.com/fcm/send/phone", &phone, None);
      let outcomes = setup.delivery(at_once()).deliver(&notice(&setup.hat())).await.unwrap();
      assert_eq!(outcomes, [(id.clone(), Outcome::Delivered)]);

      let sent = setup.fake.sent();
      assert_eq!(sent.len(), 1);
      let request = &sent[0];
      assert_eq!(
          phone.read(request),
          PushPayload {
              title: "Fix the flaky test".into(),
              body: "needs your answer".into(),
              url: "/sessions/s1".into(),
              tag: "s1".into(),
          }
      );
      assert_eq!(header(request, "TTL"), Some("3600"));
      assert_eq!(header(request, "Urgency"), Some("high"));
      assert_eq!(header(request, "Content-Encoding"), Some("aes128gcm"));
      assert_eq!(header(request, "Content-Type"), Some("application/octet-stream"));
      assert_eq!(header(request, "Topic"), None, "the push service would see a topic");
      let claims = vapid_claims(header(request, "Authorization").unwrap(), &setup.vapid);
      assert_eq!(claims["aud"], "https://fcm.googleapis.com");
      assert_eq!(claims["sub"], "mailto:me@example.com");
      let lifetime = claims["exp"].as_i64().unwrap() - unix_now();
      assert!(lifetime > 0 && lifetime <= 24 * 60 * 60);
      // Nothing of the title in the clear: the body is ciphertext.
      assert!(!String::from_utf8_lossy(&request.body).contains("flaky"));

      let stored = &setup.hosts.subscriptions().unwrap()[0];
      assert!(stored.last_success_at.is_some());
      assert_eq!(stored.last_error, None);

      // Anything but "needs your answer" is normal (kernel spec §6).
      let finished = Notice {
          urgency: Urgency::Normal,
          ..notice(&setup.hat())
      };
      setup.delivery(at_once()).deliver(&finished).await.unwrap();
      assert_eq!(header(&setup.fake.sent()[1], "Urgency"), Some("normal"));
  }

  #[tokio::test]
  async fn the_hats_policy_decides_what_is_sent() {
      let setup = Setup::new("https://hennery.example");
      let phone = Browser::new(7);
      setup.subscribe("https://fcm.googleapis.com/fcm/send/phone", &phone, None);
      let hat = setup.hat();
      let delivery = setup.delivery(at_once());
      let policy = |muted, details, generic_title| PushPolicy {
          muted,
          details,
          generic_title,
      };
      setup.hosts.set_push_policy(&hat, policy(false, true, true)).unwrap();
      delivery.deliver(&notice(&hat)).await.unwrap();
      let shown = phone.read(&setup.fake.sent()[0]);
      assert_eq!(
          (shown.title.as_str(), shown.body.as_str()),
          ("Session needs your answer", "Run cargo test")
      );
      // Muted: nothing at all.
      setup.hosts.set_push_policy(&hat, policy(true, false, false)).unwrap();
      assert!(delivery.deliver(&notice(&hat)).await.unwrap().is_empty());
      assert_eq!(setup.fake.sent().len(), 1);
      // Another hat's policy is its own: a hat with none set has the default.
      let HatChange::Done(work) = setup.hosts.create_hat("Work", None, unix_now()).unwrap() else {
          panic!("a hat");
      };
      delivery.deliver(&notice(&work.id)).await.unwrap();
      let shown = phone.read(&setup.fake.sent()[1]);
      assert_eq!(
          (shown.title.as_str(), shown.body.as_str()),
          ("Fix the flaky test", "needs your answer")
      );
  }

  #[tokio::test]
  async fn a_subscription_the_service_says_is_gone_is_removed() {
      let setup = Setup::new("https://hennery.example");
      let (a, b, c) = (Browser::new(1), Browser::new(2), Browser::new(3));
      let gone = setup.subscribe("https://fcm.googleapis.com/fcm/send/gone", &a, None);
      let missing = setup.subscribe("https://fcm.googleapis.com/fcm/send/missing", &b, None);
      let fine = setup.subscribe("https://fcm.googleapis.com/fcm/send/fine", &c, None);
      setup.fake.answer(
          "https://fcm.googleapis.com/fcm/send/gone",
          &[Sent::Status {
              code: 410,
              retry_after: None,
          }],
      );
      setup.fake.answer(
          "https://fcm.googleapis.com/fcm/send/missing",
          &[Sent::Status {
              code: 404,
              retry_after: None,
          }],
      );
      let mut outcomes = setup.delivery(at_once()).deliver(&notice(&setup.hat())).await.unwrap();
      outcomes.sort_by(|a, b| a.0.cmp(&b.0));
      let mut expected = vec![
          (gone, Outcome::Removed),
          (missing, Outcome::Removed),
          (fine.clone(), Outcome::Delivered),
      ];
      expected.sort_by(|a, b| a.0.cmp(&b.0));
      assert_eq!(outcomes, expected);
      let left: Vec<String> = setup.hosts.subscriptions().unwrap().into_iter().map(|s| s.id).collect();
      assert_eq!(left, [fine]);
  }

  #[tokio::test]
  async fn a_busy_or_failing_service_is_tried_again_then_given_up() {
      let setup = Setup::new("https://hennery.example");
      let phone = Browser::new(7);
      let endpoint = "https://fcm.googleapis.com/fcm/send/phone";
      setup.subscribe(endpoint, &phone, None);
      let delivery = setup.delivery(at_once());
      // A 503 then a timeout, then taken: delivered on the third try.
      setup.fake.answer(
          endpoint,
          &[
              Sent::Status {
                  code: 503,
                  retry_after: None,
              },
              Sent::Timeout,
          ],
      );
      let outcomes = delivery.deliver(&notice(&setup.hat())).await.unwrap();
      assert_eq!(outcomes[0].1, Outcome::Delivered);
      assert_eq!(setup.fake.sent().len(), 3);
      // Three failures: given up, the reason kept, without the endpoint.
      setup.fake.answer(
          endpoint,
          &[
              Sent::Status {
                  code: 500,
                  retry_after: None,
              },
              Sent::Failed,
              Sent::Status {
                  code: 429,
                  retry_after: None,
              },
          ],
      );
      let outcomes = delivery.deliver(&notice(&setup.hat())).await.unwrap();
      assert!(
          matches!(&outcomes[0].1, Outcome::Failed(why) if why.contains("429")),
          "{outcomes:?}"
      );
      assert_eq!(setup.fake.sent().len(), 6);
      let stored = &setup.hosts.subscriptions().unwrap()[0];
      let error = stored.last_error.clone().unwrap();
      assert!(error.contains("429") && !error.contains("fcm"), "{error}");
      // A later success clears it.
      delivery.deliver(&notice(&setup.hat())).await.unwrap();
      assert_eq!(setup.hosts.subscriptions().unwrap()[0].last_error, None);
  }

  #[tokio::test]
  async fn a_refusal_is_final() {
      let setup = Setup::new("https://hennery.example");
      let phone = Browser::new(7);
      let endpoint = "https://fcm.googleapis.com/fcm/send/phone";
      setup.subscribe(endpoint, &phone, None);
      let delivery = setup.delivery(at_once());
      setup.fake.answer(
          endpoint,
          &[Sent::Status {
              code: 403,
              retry_after: None,
          }],
      );
      let outcomes = delivery.deliver(&notice(&setup.hat())).await.unwrap();
      assert!(matches!(&outcomes[0].1, Outcome::Failed(why) if why.contains("403")));
      assert_eq!(setup.fake.sent().len(), 1, "a 403 is not tried again");
      // The egress policy refusing the address: not tried again either, and
      // the subscription kept (a DNS answer can change).
      setup.fake.answer(endpoint, &[Sent::Refused]);
      let outcomes = delivery.deliver(&notice(&setup.hat())).await.unwrap();
      assert!(matches!(&outcomes[0].1, Outcome::Failed(why) if why.contains("public address")));
      assert_eq!(setup.fake.sent().len(), 2);
      assert_eq!(setup.hosts.subscriptions().unwrap().len(), 1);
  }

  /// `Retry-After` is honoured, up to its cap; the clock is paused, so the
  /// waits are counted, not slept.
  #[tokio::test(start_paused = true)]
  async fn retry_after_is_honoured_up_to_its_cap() {
      let setup = Setup::new("https://hennery.example");
      let phone = Browser::new(7);
      let endpoint = "https://fcm.googleapis.com/fcm/send/phone";
      setup.subscribe(endpoint, &phone, None);
      let delivery = setup.delivery(RetryPolicy {
          delays: vec![Duration::from_secs(1), Duration::from_secs(4), Duration::from_secs(4)],
          max_retry_after: Duration::from_secs(60),
          attempt_timeout: Duration::from_secs(10),
          budget: Duration::from_secs(3600),
      });
      setup.fake.answer(
          endpoint,
          &[
              Sent::Status {
                  code: 429,
                  retry_after: Some(Duration::ZERO),
              },
              Sent::Status {
                  code: 429,
                  retry_after: Some(Duration::from_secs(30)),
              },
              Sent::Status {
                  code: 503,
                  retry_after: Some(Duration::from_secs(3600)),
              },
          ],
      );
      let started = tokio::time::Instant::now();
      let outcomes = delivery.deliver(&notice(&setup.hat())).await.unwrap();
      assert_eq!(outcomes[0].1, Outcome::Delivered);
      // The 1 s scheduled rather than the none asked, 30 s as asked, then
      // 60 s for the hour asked.
      assert_eq!(started.elapsed(), Duration::from_secs(91));
  }

  #[tokio::test]
  async fn an_expired_subscription_is_removed_and_sent_nothing() {
      let setup = Setup::new("https://hennery.example");
      let phone = Browser::new(7);
      let laptop = Browser::new(8);
      setup.subscribe(
          "https://fcm.googleapis.com/fcm/send/phone",
          &phone,
          Some(unix_now() + 2),
      );
      let laptop_id = setup.subscribe("https://fcm.googleapis.com/fcm/send/laptop", &laptop, None);
      rusqlite::Connection::open(setup._dir.path().join("hennery.db"))
          .unwrap()
          .execute(
              "UPDATE push_subscriptions SET expires_at = 1 WHERE expires_at IS NOT NULL",
              [],
          )
          .unwrap();
      let outcomes = setup.delivery(at_once()).deliver(&notice(&setup.hat())).await.unwrap();
      assert_eq!(outcomes, [(laptop_id, Outcome::Delivered)]);
      assert_eq!(setup.fake.sent().len(), 1);
      assert_eq!(setup.hosts.subscriptions().unwrap().len(), 1);
  }

  /// Plan 10a's re-confirmation, N1: with no contact and an `http`
  /// `public_url`, the token names no subject.
  #[tokio::test]
  async fn without_a_contact_a_loopback_collector_names_no_subject() {
      let setup = Setup::new("http://localhost:7117");
      let phone = Browser::new(7);
      setup.subscribe("https://fcm.googleapis.com/fcm/send/phone", &phone, None);
      setup.delivery(at_once()).deliver(&notice(&setup.hat())).await.unwrap();
      let claims = vapid_claims(header(&setup.fake.sent()[0], "Authorization").unwrap(), &setup.vapid);
      assert_eq!(claims.get("sub"), None);
      // An https one is named.
      let setup = Setup::new("https://hennery.example");
      setup.subscribe("https://fcm.googleapis.com/fcm/send/phone", &phone, None);
      setup.delivery(at_once()).deliver(&notice(&setup.hat())).await.unwrap();
      let claims = vapid_claims(header(&setup.fake.sent()[0], "Authorization").unwrap(), &setup.vapid);
      assert_eq!(claims["sub"], json!("https://hennery.example"));
  }

  /// The token is made once per push service and reused (the
  /// re-confirmation's N2), one per service.
  #[tokio::test]
  async fn a_token_is_reused_per_push_service() {
      let setup = Setup::new("https://hennery.example");
      setup.subscribe("https://fcm.googleapis.com/fcm/send/a", &Browser::new(1), None);
      setup.subscribe("https://fcm.googleapis.com/fcm/send/b", &Browser::new(2), None);
      setup.subscribe("https://web.push.apple.com/c", &Browser::new(3), None);
      let delivery = setup.delivery(at_once());
      delivery.deliver(&notice(&setup.hat())).await.unwrap();
      // A second later a fresh token would differ (its `exp`); the signature
      // is deterministic (RFC 6979), so only a later one tells reuse apart.
      tokio::time::sleep(Duration::from_millis(1100)).await;
      delivery.deliver(&notice(&setup.hat())).await.unwrap();
      let mut by_service: HashMap<String, Vec<String>> = HashMap::new();
      for request in setup.fake.sent() {
          by_service
              .entry(request.endpoint.host_str().unwrap().into())
              .or_default()
              .push(header(&request, "Authorization").unwrap().into());
      }
      let fcm = &by_service["fcm.googleapis.com"];
      assert_eq!(fcm.len(), 4);
      assert!(fcm.iter().all(|h| h == &fcm[0]));
      let apple = &by_service["web.push.apple.com"];
      assert_ne!(apple[0], fcm[0]);
      assert_eq!(
          vapid_claims(&apple[0], &setup.vapid)["aud"],
          "https://web.push.apple.com"
      );
  }

  /// The queue's notices are delivered as they come.
  #[tokio::test]
  async fn delivery_drains_the_queue() {
      let setup = Setup::new("https://hennery.example");
      let phone = Browser::new(7);
      setup.subscribe("https://fcm.googleapis.com/fcm/send/phone", &phone, None);
      let (push, notices) = Push::new();
      tokio::spawn(setup.delivery(at_once()).run(notices));
      push.notify(notice(&setup.hat()));
      let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
      while setup.fake.sent().is_empty() {
          assert!(tokio::time::Instant::now() < deadline, "nothing delivered");
          tokio::time::sleep(Duration::from_millis(10)).await;
      }
      assert_eq!(phone.read(&setup.fake.sent()[0]).tag, "s1");
  }

  /// 10b-ii's review, A1: notices go out one after another, so one slow or
  /// silent push service must not hold up the others. A service asking to
  /// wait an hour is waited for no longer than the cap, and one that never
  /// answers is given up within the budget (its three timed-out tries and
  /// their waits would outlast it), while a healthy one is delivered.
  #[tokio::test(start_paused = true)]
  async fn a_slow_or_silent_service_is_given_up_within_the_budget() {
      let setup = Setup::new("https://hennery.example");
      let busy = setup.subscribe("https://fcm.googleapis.com/fcm/send/busy", &Browser::new(1), None);
      let silent = setup.subscribe("https://fcm.googleapis.com/fcm/send/silent", &Browser::new(2), None);
      let fine = setup.subscribe("https://fcm.googleapis.com/fcm/send/fine", &Browser::new(3), None);
      let wait_an_hour = Sent::Status {
          code: 503,
          retry_after: Some(Duration::from_secs(3600)),
      };
      setup.fake.answer(
          "https://fcm.googleapis.com/fcm/send/busy",
          &[wait_an_hour.clone(), wait_an_hour.clone(), wait_an_hour],
      );
      setup.fake.silent("https://fcm.googleapis.com/fcm/send/silent");
      let policy = RetryPolicy::default();
      let budget = policy.budget;
      let delivery = setup.delivery(policy);
      let started = tokio::time::Instant::now();
      let mut outcomes = delivery.deliver(&notice(&setup.hat())).await.unwrap();
      assert!(started.elapsed() <= budget, "{:?}", started.elapsed());
      outcomes.sort_by(|a, b| a.0.cmp(&b.0));
      let of = |id: &str| outcomes.iter().find(|(i, _)| i == id).unwrap().1.clone();
      assert_eq!(of(&fine), Outcome::Delivered);
      assert!(matches!(of(&busy), Outcome::Failed(_)), "{outcomes:?}");
      assert!(
          matches!(of(&silent), Outcome::Failed(why) if why.contains("in time")),
          "{outcomes:?}"
      );
      // The next notice is not held up either.
      let started = tokio::time::Instant::now();
      delivery.deliver(&notice(&setup.hat())).await.unwrap();
      assert!(started.elapsed() <= budget);
  }

  /// 10b-ii's review, A2: a push task that fails is logged, and the other
  /// subscriptions' outcomes are still recorded.
  #[tokio::test]
  async fn a_failed_push_task_does_not_lose_the_others() {
      let setup = Setup::new("https://hennery.example");
      setup.subscribe("https://fcm.googleapis.com/fcm/send/broken", &Browser::new(1), None);
      let fine = setup.subscribe("https://fcm.googleapis.com/fcm/send/fine", &Browser::new(2), None);
      setup.fake.panicking("https://fcm.googleapis.com/fcm/send/broken");
      let outcomes = setup.delivery(at_once()).deliver(&notice(&setup.hat())).await.unwrap();
      assert_eq!(outcomes, vec![(fine, Outcome::Delivered)]);
  }

  /// The whole-branch review's must-fix 1: a browser rotates its subscription
  /// (same id, a new endpoint) while a push to the old one is on its way. The
  /// old endpoint's answer, whatever it is, is not recorded on the new one,
  /// and a "gone" does not remove it.
  #[tokio::test]
  async fn an_answer_for_a_rotated_endpoint_leaves_the_new_one_alone() {
      let (old, new) = (
          "https://fcm.googleapis.com/fcm/send/old",
          "https://fcm.googleapis.com/fcm/send/new",
      );
      for code in [410, 403, 201] {
          let setup = Setup::new("https://hennery.example");
          let browser = Browser::new(1);
          let id = setup.subscribe(old, &browser, None);
          setup.fake.answer(
              old,
              &[Sent::Status {
                  code,
                  retry_after: None,
              }],
          );
          let (hosts, p256dh, auth) = (setup.hosts.clone(), browser.p256dh(), browser.auth());
          setup.fake.on_send(move || {
              let rotated = NewSubscription {
                  endpoint: new,
                  p256dh: &p256dh,
                  auth: &auth,
                  device_label: None,
                  expires_at: None,
              };
              let replaced = hosts.rotate(old, &rotated, unix_now()).unwrap();
              assert!(matches!(replaced, Subscribed::Replaced(_)), "{replaced:?}");
          });
          setup.delivery(at_once()).deliver(&notice(&setup.hat())).await.unwrap();
          let subs = setup.hosts.subscriptions().unwrap();
          assert_eq!(subs.len(), 1, "{code}: the rotated subscription is kept");
          let (sub_id, endpoint) = (&subs[0].id, &subs[0].endpoint);
          assert_eq!((sub_id, endpoint.as_str()), (&id, new), "{code}");
          assert_eq!(subs[0].last_error, None, "{code}");
          assert_eq!(subs[0].last_success_at, None, "{code}");
      }
  }
  ```

- [ ] **Step 2: Run them, and see them fail**: `nix develop -c cargo test -p hennery-kernel --test delivery` fails to compile: there is no `hennery_kernel::delivery`.

- [ ] **Step 3: Write delivery**

In `Cargo.toml`, replace:

  ```toml
  p256 = { version = "=0.13.2", features = ["ecdsa"] }
  reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "stream"] }
  ```

with:

  ```toml
  p256 = { version = "=0.13.2", features = ["ecdsa"] }
  # Web Push (plan 10b-ii): RFC 8291's encryption only. Default features off:
  # `vapid` pulls in `jwt-simple`, and with it `rsa` (plan 10a decision 2).
  web-push-native = { version = "=0.5.0", default-features = false }
  reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "stream"] }
  ```

In `crates/hennery-kernel/Cargo.toml`, replace:

  ```toml
  webauthn-rs.workspace = true
  zeroize.workspace = true
  ```

with:

  ```toml
  webauthn-rs.workspace = true
  web-push-native.workspace = true
  zeroize.workspace = true
  ```

In `crates/hennery-kernel/Cargo.toml`, replace:

  ```toml
  tempfile = "3"
  tower = { version = "0.5", features = ["util"] }
  ```

with:

  ```toml
  tempfile = "3"
  # Delivery's retry waits run on a paused clock (plan 10b-ii).
  tokio = { workspace = true, features = ["test-util"] }
  tower = { version = "0.5", features = ["util"] }
  ```

Create `crates/hennery-kernel/src/delivery.rs`:

  ```rust
  //! Web Push delivery (kernel spec §6; plan 10b-ii): drain the notice queue
  //! (`push::Notices`), apply each notice's hat policy, and send it to every
  //! subscription of the owner's, encrypted to the browser (RFC 8291) and
  //! signed with the collector's VAPID key (RFC 8292).
  //!
  //! Requests go out through a `Transport`. The collector's is the shared
  //! egress client, public addresses only (kernel spec §7.1, plan 8b); tests
  //! use a fake one. Nothing here opens a connection itself.

  use crate::hosts::Hosts;
  use crate::operator::Operator;
  use crate::push::{
      Notice, Notices, PushPolicy, Subscription, Urgency, VAPID_TOKEN_SECS, VapidKey, audience, parse_keys, subject,
  };
  use crate::secret::unix_now;
  use anyhow::Result;
  use base64::Engine;
  use base64::engine::general_purpose::URL_SAFE_NO_PAD;
  use hennery_proto::rest::PushPayload;
  use std::collections::HashMap;
  use std::future::Future;
  use std::pin::Pin;
  use std::sync::{Arc, Mutex};
  use std::time::Duration;

  /// How long a push service keeps a notification it could not deliver yet
  /// (kernel spec §6: one hour).
  pub const PUSH_TTL_SECS: u64 = 60 * 60;

  /// The plaintext is padded with spaces to a multiple of this, so its size
  /// tells the push service little of the title (plan 10a's review).
  pub const PAD_TO: usize = 512;

  /// The most plaintext one push carries: RFC 8291's 4096-byte body, less the
  /// 86-byte header, the 16-byte tag and the padding delimiter.
  pub const MAX_PLAINTEXT: usize = 4096 - 86 - 16 - 1;

  /// Subscriptions sent to at once, per notice.
  pub const MAX_IN_FLIGHT: usize = 8;

  /// One push, ready to send.
  #[derive(Clone)]
  pub struct PushRequest {
      pub endpoint: url::Url,
      /// `TTL`, `Urgency`, `Content-Encoding`, `Content-Type` and
      /// `Authorization`. Never `Topic`: the push service would see it.
      pub headers: Vec<(&'static str, String)>,
      /// The encrypted payload (`aes128gcm`).
      pub body: Vec<u8>,
  }

  impl std::fmt::Debug for PushRequest {
      fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
          f.debug_struct("PushRequest")
              .field("endpoint_host", &self.endpoint.host_str())
              .field("body_len", &self.body.len())
              .finish_non_exhaustive()
      }
  }

  /// What came of one request.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum Sent {
      /// The push service answered.
      Status {
          code: u16,
          /// Its `Retry-After`, when it gave one in seconds.
          retry_after: Option<Duration>,
      },
      /// The egress policy refused the address: nothing was sent.
      Refused,
      /// No answer in time.
      Timeout,
      /// The connection failed.
      Failed,
  }

  /// Sends one push request. The collector's sends through the egress client
  /// (`PublicOnly`); never one that reaches a private address.
  pub trait Transport: Send + Sync + 'static {
      fn send(&self, request: PushRequest) -> Pin<Box<dyn Future<Output = Sent> + Send + '_>>;
  }

  /// When to try again, and how long to wait.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct RetryPolicy {
      /// The wait before each retry: as many retries as there are waits.
      pub delays: Vec<Duration>,
      /// The longest `Retry-After` honoured; longer is cut to it.
      pub max_retry_after: Duration,
      /// How long one try may take before it counts as unanswered.
      pub attempt_timeout: Duration,
      /// The most one subscription's tries and waits may take together: a
      /// retry that could overrun it is not made (10b-ii's review, A1).
      /// Notices are delivered one after another, so a slow or silent push
      /// service must not hold the others up for minutes.
      pub budget: Duration,
  }

  impl Default for RetryPolicy {
      /// Two retries, after 1 s and 4 s, `Retry-After` up to 10 s; each try
      /// 10 s at most, and 30 s in all.
      fn default() -> Self {
          Self {
              delays: vec![Duration::from_secs(1), Duration::from_secs(4)],
              max_retry_after: Duration::from_secs(10),
              attempt_timeout: Duration::from_secs(10),
              budget: Duration::from_secs(30),
          }
      }
  }

  /// What became of one subscription's push.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum Outcome {
      /// The push service took it (2xx).
      Delivered,
      /// The subscription is gone (404, 410): removed.
      Removed,
      /// Not delivered; the reason is the subscription's `last_error`.
      Failed(String),
  }

  /// The notice's payload under `policy`, or `None` for a muted hat (kernel
  /// spec §6; `Notice`'s documented rules):
  /// - `generic_title`: the generic title, and no body;
  /// - `details`: the notice's `detail`, when it has one, as the body.
  pub fn payload_for(notice: &Notice, policy: PushPolicy) -> Option<PushPayload> {
      if policy.muted {
          return None;
      }
      let body = match (&notice.detail, policy.details) {
          (Some(detail), true) => detail.clone(),
          _ if policy.generic_title => String::new(),
          _ => notice.body.clone(),
      };
      let title = if policy.generic_title {
          notice.generic_title.clone()
      } else {
          notice.title.clone()
      };
      Some(PushPayload {
          title,
          body,
          url: notice.url.clone(),
          tag: notice.tag.clone(),
      })
  }

  /// `payload` as JSON, padded with trailing spaces (still valid JSON) to the
  /// next multiple of `PAD_TO`, at most `MAX_PLAINTEXT`. `Err` for one that
  /// cannot fit, which the notices' caps rule out.
  pub fn padded(payload: &PushPayload) -> Result<Vec<u8>> {
      let mut json = serde_json::to_vec(payload)?;
      anyhow::ensure!(json.len() <= MAX_PLAINTEXT, "a push payload of {} bytes", json.len());
      let target = json.len().div_ceil(PAD_TO).max(1) * PAD_TO;
      json.resize(target.min(MAX_PLAINTEXT), b' ');
      Ok(json)
  }

  /// Delivers notices: see the module docs.
  pub struct Delivery<T> {
      hosts: Arc<Hosts>,
      operator: Arc<Operator>,
      vapid: Arc<VapidKey>,
      transport: T,
      retry: RetryPolicy,
      /// `Authorization` headers by (audience, subject), with when each was
      /// made: reused until an hour before it expires (plan 10a's
      /// re-confirmation, N2).
      tokens: Mutex<Tokens>,
  }

  /// `Authorization` headers by (audience, subject), each with when it was
  /// made: one token per push service, reused until an hour before it
  /// expires, and dropped once it has.
  #[derive(Default)]
  struct Tokens(HashMap<(String, Option<String>), (String, i64)>);

  impl Tokens {
      fn authorization(&mut self, vapid: &VapidKey, endpoint: &url::Url, subject: Option<&str>, now: i64) -> String {
          let key = (audience(endpoint), subject.map(str::to_string));
          if let Some((header, made)) = self.0.get(&key)
              && now - made < VAPID_TOKEN_SECS - 60 * 60
          {
              return header.clone();
          }
          self.0.retain(|_, (_, made)| now - *made < VAPID_TOKEN_SECS);
          let header = vapid.authorization(endpoint, subject, now);
          self.0.insert(key, (header.clone(), now));
          header
      }
  }

  impl<T: Transport> Delivery<T> {
      pub fn new(
          hosts: Arc<Hosts>,
          operator: Arc<Operator>,
          vapid: Arc<VapidKey>,
          transport: T,
          retry: RetryPolicy,
      ) -> Arc<Self> {
          Arc::new(Self {
              hosts,
              operator,
              vapid,
              transport,
              retry,
              tokens: Mutex::new(Tokens::default()),
          })
      }

      /// Deliver every notice queued, one after another, until the process
      /// ends. A notice that cannot be delivered is logged; the next one is
      /// tried all the same.
      pub async fn run(self: Arc<Self>, mut notices: Notices) {
          loop {
              let notice = notices.recv().await;
              if let Err(err) = self.deliver(&notice).await {
                  tracing::warn!(tag = %notice.tag, error = %err, "push not delivered");
              }
          }
      }

      /// Deliver `notice` to every subscription, at most `MAX_IN_FLIGHT` at
      /// once: each subscription's outcome, by its id. Expired subscriptions
      /// are removed first and get nothing; a muted hat gets nothing.
      pub async fn deliver(self: &Arc<Self>, notice: &Notice) -> Result<Vec<(String, Outcome)>> {
          let now = unix_now();
          self.hosts.prune_expired_subscriptions(now)?;
          let Some(payload) = payload_for(notice, self.hosts.push_policy(&notice.hat_id)?) else {
              return Ok(Vec::new());
          };
          let plaintext = Arc::new(padded(&payload)?);
          let subject = match self.operator.public_url() {
              Some(public_url) => subject(self.operator.contact()?.as_deref(), &public_url),
              None => None,
          };
          let urgency = match notice.urgency {
              Urgency::High => "high",
              Urgency::Normal => "normal",
          };
          let permits = Arc::new(tokio::sync::Semaphore::new(MAX_IN_FLIGHT));
          let mut sending = tokio::task::JoinSet::new();
          for sub in self.hosts.subscriptions()? {
              let this = self.clone();
              let plaintext = plaintext.clone();
              let subject = subject.clone();
              let permits = permits.clone();
              sending.spawn(async move {
                  let _permit = permits.acquire_owned().await;
                  let outcome = this.send_one(&sub, &plaintext, urgency, subject.as_deref()).await;
                  (sub.id, outcome)
              });
          }
          let mut outcomes = Vec::new();
          // A task that panicked is logged; the others' outcomes are still
          // collected and recorded (10b-ii's review, A2).
          while let Some(done) = sending.join_next().await {
              match done {
                  Ok(outcome) => outcomes.push(outcome),
                  // Not the panic's message, which could hold what it unwrapped.
                  Err(err) => tracing::error!(task = %err.id(), panicked = err.is_panic(), "a push task failed"),
              }
          }
          Ok(outcomes)
      }

      /// One subscription's push, retried as `retry` says; its outcome is
      /// recorded on it.
      async fn send_one(&self, sub: &Subscription, plaintext: &[u8], urgency: &str, subject: Option<&str>) -> Outcome {
          let outcome = match self.request(sub, plaintext, urgency, subject) {
              Ok(request) => self.send_retrying(request).await,
              Err(err) => {
                  tracing::warn!(id = %sub.id, error = %err, "push not built");
                  Outcome::Failed("could not encrypt to this device".into())
              }
          };
          let recorded = match &outcome {
              Outcome::Delivered => self.hosts.record_push_success(&sub.id, &sub.endpoint, unix_now()),
              Outcome::Removed => self.hosts.remove_gone_subscription(&sub.id, &sub.endpoint).map(|_| ()),
              Outcome::Failed(why) => self.hosts.record_push_error(&sub.id, &sub.endpoint, why),
          };
          if let Err(err) = recorded {
              tracing::warn!(id = %sub.id, error = %err, "push outcome not recorded");
          }
          match &outcome {
              Outcome::Removed => {
                  tracing::info!(id = %sub.id, host = %sub.endpoint_host(), "push subscription gone; removed")
              }
              Outcome::Failed(why) => tracing::warn!(id = %sub.id, host = %sub.endpoint_host(), %why, "push failed"),
              Outcome::Delivered => {}
          }
          outcome
      }

      /// The push for `sub`: the plaintext encrypted to its keys, and the
      /// headers.
      fn request(
          &self,
          sub: &Subscription,
          plaintext: &[u8],
          urgency: &str,
          subject: Option<&str>,
      ) -> Result<PushRequest> {
          let endpoint = url::Url::parse(&sub.endpoint)?;
          let (p256dh, auth) = parse_keys(&sub.p256dh, &sub.auth).map_err(anyhow::Error::msg)?;
          let ua_public = p256::PublicKey::from_sec1_bytes(&URL_SAFE_NO_PAD.decode(p256dh)?)?;
          let auth: [u8; 16] = URL_SAFE_NO_PAD
              .decode(auth)?
              .try_into()
              .map_err(|_| anyhow::anyhow!("keys.auth is not 16 bytes"))?;
          let auth = web_push_native::Auth::from(auth);
          let body =
              web_push_native::encrypt(plaintext.to_vec(), &ua_public, &auth).map_err(|e| anyhow::anyhow!("{e}"))?;
          Ok(PushRequest {
              headers: vec![
                  ("TTL", PUSH_TTL_SECS.to_string()),
                  ("Urgency", urgency.to_string()),
                  ("Content-Encoding", "aes128gcm".to_string()),
                  ("Content-Type", "application/octet-stream".to_string()),
                  ("Authorization", self.authorization(&endpoint, subject)),
              ],
              endpoint,
              body,
          })
      }

      /// The `Authorization` header for `endpoint`'s push service, made once
      /// per audience and subject and reused until an hour before it expires.
      fn authorization(&self, endpoint: &url::Url, subject: Option<&str>) -> String {
          let mut tokens = self.tokens.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
          tokens.authorization(&self.vapid, endpoint, subject, unix_now())
      }

      /// Send `request`, retrying a 429, a 5xx, a timeout or a failed
      /// connection after each of `retry`'s delays (or the service's
      /// `Retry-After`, if longer, up to its cap). Any other answer is final.
      /// Each try has `attempt_timeout`, and no retry is made that could not
      /// finish within `budget` of the first.
      async fn send_retrying(&self, request: PushRequest) -> Outcome {
          let started = tokio::time::Instant::now();
          let mut delays = self.retry.delays.iter();
          loop {
              let sent = tokio::time::timeout(self.retry.attempt_timeout, self.transport.send(request.clone()))
                  .await
                  .unwrap_or(Sent::Timeout);
              let (retryable, failed, retry_after) = match sent {
                  Sent::Status { code: 200..=299, .. } => return Outcome::Delivered,
                  Sent::Status { code: 404 | 410, .. } => return Outcome::Removed,
                  Sent::Status { code, retry_after } if code == 429 || code >= 500 => {
                      (true, format!("the push service answered {code}"), retry_after)
                  }
                  Sent::Status { code, .. } => (false, format!("the push service refused it ({code})"), None),
                  Sent::Refused => (
                      false,
                      "refused: the push service is not at a public address".into(),
                      None,
                  ),
                  Sent::Timeout => (true, "the push service did not answer in time".into(), None),
                  Sent::Failed => (true, "the push service could not be reached".into(), None),
              };
              match delays.next() {
                  Some(delay) if retryable => {
                      let wait = retry_after.map_or(*delay, |after| after.min(self.retry.max_retry_after).max(*delay));
                      if started.elapsed() + wait + self.retry.attempt_timeout > self.retry.budget {
                          return Outcome::Failed(format!("{failed}; gave up within the time allowed"));
                      }
                      tokio::time::sleep(wait).await;
                  }
                  _ => return Outcome::Failed(failed),
              }
          }
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      fn notice() -> Notice {
          Notice {
              hat_id: "hat-1".into(),
              urgency: Urgency::High,
              title: "Fix the flaky test".into(),
              generic_title: "Session needs your answer".into(),
              body: "needs your answer".into(),
              detail: Some("Run cargo test".into()),
              url: "/sessions/s1".into(),
              tag: "s1".into(),
          }
      }

      /// 10b-ii's review, A3: a token is reused until an hour before it
      /// expires, then made again with a later expiry.
      #[test]
      fn a_token_is_renewed_an_hour_before_it_expires() {
          let vapid = VapidKey::generate();
          let endpoint = url::Url::parse("https://fcm.googleapis.com/fcm/send/a").unwrap();
          let mut tokens = Tokens::default();
          let made = 1_800_000_000;
          let first = tokens.authorization(&vapid, &endpoint, None, made);
          let renew_at = made + VAPID_TOKEN_SECS - 60 * 60;
          assert_eq!(tokens.authorization(&vapid, &endpoint, None, renew_at - 1), first);
          let renewed = tokens.authorization(&vapid, &endpoint, None, renew_at);
          assert_ne!(renewed, first);
          // Another subject is another token.
          assert_ne!(
              tokens.authorization(&vapid, &endpoint, Some("mailto:a@b.co"), renew_at),
              renewed
          );
          // Long past its expiry, an entry is dropped when another is made.
          let other = url::Url::parse("https://web.push.apple.com/b").unwrap();
          tokens.authorization(&vapid, &other, None, made + 3 * VAPID_TOKEN_SECS);
          assert_eq!(tokens.0.len(), 1);
      }

      #[test]
      fn the_policy_decides_what_a_payload_shows() {
          let default = payload_for(&notice(), PushPolicy::default()).unwrap();
          assert_eq!(
              (
                  default.title.as_str(),
                  default.body.as_str(),
                  default.url.as_str(),
                  default.tag.as_str()
              ),
              ("Fix the flaky test", "needs your answer", "/sessions/s1", "s1")
          );
          let generic = PushPolicy {
              generic_title: true,
              ..PushPolicy::default()
          };
          let shown = payload_for(&notice(), generic).unwrap();
          assert_eq!(
              (shown.title.as_str(), shown.body.as_str()),
              ("Session needs your answer", "")
          );
          let details = PushPolicy {
              details: true,
              ..PushPolicy::default()
          };
          assert_eq!(payload_for(&notice(), details).unwrap().body, "Run cargo test");
          let both = PushPolicy {
              details: true,
              generic_title: true,
              ..PushPolicy::default()
          };
          let shown = payload_for(&notice(), both).unwrap();
          assert_eq!(
              (shown.title.as_str(), shown.body.as_str()),
              ("Session needs your answer", "Run cargo test")
          );
          // `details` with no detail: the body as usual.
          let mut plain = notice();
          plain.detail = None;
          assert_eq!(payload_for(&plain, details).unwrap().body, "needs your answer");
          let muted = PushPolicy {
              muted: true,
              details: true,
              generic_title: false,
          };
          assert_eq!(payload_for(&notice(), muted), None);
      }

      #[test]
      fn a_payload_is_padded_to_its_bucket_and_stays_json() {
          let payload = payload_for(&notice(), PushPolicy::default()).unwrap();
          let short = padded(&payload).unwrap();
          assert_eq!(short.len(), PAD_TO);
          assert_eq!(serde_json::from_slice::<PushPayload>(&short).unwrap(), payload);
          // Titles of different lengths in one bucket look the same.
          let mut longer = payload.clone();
          longer.title = "x".repeat(100);
          assert_eq!(padded(&longer).unwrap().len(), PAD_TO);
          longer.title = "x".repeat(600);
          assert_eq!(padded(&longer).unwrap().len(), 2 * PAD_TO);
          longer.title = "x".repeat(3900);
          assert_eq!(padded(&longer).unwrap().len(), MAX_PLAINTEXT);
          longer.title = "x".repeat(4000);
          assert!(padded(&longer).is_err());
      }
  }
  ```

In `crates/hennery-kernel/src/lib.rs`, replace:

  ```rust
  pub mod db;
  pub mod egress;
  ```

with:

  ```rust
  pub mod db;
  pub mod delivery;
  pub mod egress;
  ```

In `crates/hennery-kernel/src/push.rs`, replace:

  ```rust
              params![endpoint.as_str(), self.owner_id()],
          )?;
          Ok(removed > 0)
      }

      /// Every hat's push policy, the hats oldest first: the default for a
  ```

with:

  ```rust
              params![endpoint.as_str(), self.owner_id()],
          )?;
          Ok(removed > 0)
      }

      // Delivery's three outcomes name the endpoint the push went to, as
      // well as the subscription: a browser rotating it meanwhile keeps the
      // id, and the old endpoint's answer is not the new one's (plan 10b-ii).

      /// Record that a push to `endpoint` reached its push service at `now`,
      /// clearing the subscription `id`'s last error (plan 10b-ii).
      pub fn record_push_success(&self, id: &str, endpoint: &str, now: i64) -> Result<()> {
          self.conn().execute(
              "UPDATE push_subscriptions SET last_success_at = ?2, last_error = NULL
               WHERE id = ?1 AND endpoint = ?4 AND owner_id = ?3",
              params![id, now, self.owner_id(), endpoint],
          )?;
          Ok(())
      }

      /// Record why a push to the subscription `id` at `endpoint` failed: a
      /// short reason of delivery's own, never the endpoint or the service's
      /// answer (plan 10a's review, O2).
      pub fn record_push_error(&self, id: &str, endpoint: &str, error: &str) -> Result<()> {
          self.conn().execute(
              "UPDATE push_subscriptions SET last_error = ?2 WHERE id = ?1 AND endpoint = ?4 AND owner_id = ?3",
              params![id, error, self.owner_id(), endpoint],
          )?;
          Ok(())
      }

      /// Remove the subscription `id` its push service says is gone, if it
      /// still has the `endpoint` the push went to (plan 10b-ii).
      pub fn remove_gone_subscription(&self, id: &str, endpoint: &str) -> Result<bool> {
          let removed = self.conn().execute(
              "DELETE FROM push_subscriptions WHERE id = ?1 AND endpoint = ?2 AND owner_id = ?3",
              params![id, endpoint, self.owner_id()],
          )?;
          Ok(removed > 0)
      }

      /// Remove the subscriptions whose `expirationTime` has passed at `now`
      /// (plan 10b-ii): how many.
      pub fn prune_expired_subscriptions(&self, now: i64) -> Result<usize> {
          Ok(self.conn().execute(
              "DELETE FROM push_subscriptions WHERE expires_at IS NOT NULL AND expires_at <= ?1 AND owner_id = ?2",
              params![now, self.owner_id()],
          )?)
      }

      /// Every hat's push policy, the hats oldest first: the default for a
  ```

In `crates/hennery-proto/src/codegen.rs`, replace:

  ```rust
          rest::SettingsResponse,
          rest::SettingsUpdateRequest,
      );
      // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
  ```

with:

  ```rust
          rest::SettingsResponse,
          rest::SettingsUpdateRequest,
          rest::PushPayload,
      );
      // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
  ```

In `crates/hennery-proto/src/codegen.rs`, replace:

  ```rust
          rest::SettingsUpdateRequest,
      );
  ```

with:

  ```rust
          rest::SettingsUpdateRequest,
          rest::PushPayload,
      );
  ```

In `crates/hennery-proto/src/rest.rs`, replace:

  ```rust
  pub struct SettingsUpdateRequest {
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "string | undefined", optional)]
      pub contact: Option<String>,
  }
  ```

with:

  ```rust
  pub struct SettingsUpdateRequest {
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "string | undefined", optional)]
      pub contact: Option<String>,
  }

  /// What a push carries to the browser's service worker (kernel spec §6;
  /// plan 10b-ii), encrypted to the browser (RFC 8291): the push service sees
  /// its size only, and that is padded. The service worker shows `title` and
  /// `body` as text, tags the notification `tag`, and on a click opens `url`,
  /// a path of this origin.
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  pub struct PushPayload {
      pub title: String,
      pub body: String,
      /// `/sessions/<id>`, or `/mcp`: always a path, never a URL.
      pub url: String,
      pub tag: String,
  }
  ```

In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

  ```rust
          include_str!("../../hennery-kernel/src/push.rs"),
          16,
      ),
  ```

with:

  ```rust
          include_str!("../../hennery-kernel/src/push.rs"),
          20,
      ),
  ```

  Then `nix develop -c cargo build --workspace` (the lock file gains `web-push-native`, `ece-native` and their RustCrypto kin), and `nix develop -c cargo run -p hennery-proto --bin gen`.

- [ ] **Step 4: Run the tests, and the five checks**

- [ ] **Step 5: Revert-probe**

  | Change | Fails |
  |---|---|
  | a muted hat sends | `the_hats_policy_decides_what_is_sent` |
  | `details` ignored | `the_policy_decides_what_a_payload_shows` |
  | `generic_title` ignored | `the_hats_policy_decides_what_is_sent` |
  | no padding | `a_push_reaches_the_browser_encrypted_padded_and_signed` |
  | a `Topic` sent | the same |
  | `Urgency` always normal | the same |
  | `Urgency` always high | the same |
  | no `Content-Type` | the same |
  | no subject | the same |
  | success not recorded | the same |
  | a gone subscription kept | `a_subscription_the_service_says_is_gone_is_removed` |
  | failures not recorded | `a_busy_or_failing_service_is_tried_again_then_given_up` |
  | no retry | the same |
  | a success keeping the last error | the same |
  | a 403 retried | `a_refusal_is_final` |
  | a refusal retried | the same |
  | `Retry-After` ignored | `retry_after_is_honoured_up_to_its_cap` |
  | `Retry-After` uncapped | the same |
  | a shorter `Retry-After` than the scheduled delay winning | the same |
  | a success recorded on a rotated endpoint | `an_answer_for_a_rotated_endpoint_leaves_the_new_one_alone` |
  | a failure recorded on it | the same |
  | a "gone" removing it | the same |
  | expired subscriptions not pruned | `an_expired_subscription_is_removed_and_sent_nothing` |
  | no token cache | `a_token_is_reused_per_push_service` |
  | a token never renewed | `a_token_is_renewed_an_hour_before_it_expires` |
  | expired tokens kept | the same |
  | a try's timeout an hour, not the policy's | `a_slow_or_silent_service_is_given_up_within_the_budget` |
  | no budget | the same |
  | a failed task failing the notice | `a_failed_push_task_does_not_lose_the_others` |

- [ ] **Step 6: Commit**: `test(push): delivery through a fake transport: what reaches the push service, and what its answers do`, then `feat(push): delivery: encrypted, padded, signed, the hat's policy applied, retried and cleaned up`.

### Task 2: The egress client, and the collector's delivery

- [ ] **Step 1: Write the tests**

Create `crates/hennery-kernel/tests/delivery_http.rs`:

  ```rust
  //! Web Push delivery over HTTP (plan 10b-ii), through the egress client, to a
  //! fake push service on loopback: what arrives on the wire, what the
  //! service's answers do, and that the collector's own client, public only,
  //! sends nothing to it. No test leaves this machine.

  use axum::Router;
  use axum::body::Bytes;
  use axum::extract::{Path, State};
  use axum::http::{HeaderMap, StatusCode};
  use axum::response::{IntoResponse, Response};
  use axum::routing::post;
  use base64::Engine;
  use base64::engine::general_purpose::URL_SAFE_NO_PAD;
  use hennery_kernel::delivery::{Delivery, Outcome, PAD_TO, RetryPolicy, spawn};
  use hennery_kernel::egress::{Allowance, Egress, Timeouts};
  use hennery_kernel::hosts::Hosts;
  use hennery_kernel::operator::{Operator, SetupOutcome};
  use hennery_kernel::push::{Notice, Urgency, VapidKey};
  use hennery_kernel::secret::unix_now;
  use hennery_proto::rest::PushPayload;
  use p256::elliptic_curve::sec1::ToEncodedPoint;
  use std::collections::{HashMap, VecDeque};
  use std::net::SocketAddr;
  use std::sync::{Arc, Mutex};
  use std::time::Duration;

  /// One request as the push service received it.
  #[derive(Clone, Debug)]
  struct Received {
      name: String,
      headers: HeaderMap,
      body: Vec<u8>,
  }

  /// A status, and the `Retry-After` it is sent with.
  type Answer = (u16, Option<&'static str>);

  /// A push service: per subscription, scripted (status, `Retry-After`)
  /// answers, 201 once they run out.
  #[derive(Clone, Default)]
  struct Service {
      script: Arc<Mutex<HashMap<String, VecDeque<Answer>>>>,
      received: Arc<Mutex<Vec<Received>>>,
  }

  async fn push(State(service): State<Service>, Path(name): Path<String>, headers: HeaderMap, body: Bytes) -> Response {
      let answer = service
          .script
          .lock()
          .unwrap()
          .get_mut(&name)
          .and_then(VecDeque::pop_front)
          .unwrap_or((201, None));
      service.received.lock().unwrap().push(Received {
          name,
          headers,
          body: body.to_vec(),
      });
      let mut response = StatusCode::from_u16(answer.0).unwrap().into_response();
      if let Some(after) = answer.1 {
          response.headers_mut().insert("retry-after", after.parse().unwrap());
      }
      response
  }

  impl Service {
      async fn start() -> (Service, SocketAddr) {
          let service = Service::default();
          let app = Router::new()
              .route("/push/{name}", post(push))
              .with_state(service.clone());
          let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
          let addr = listener.local_addr().unwrap();
          tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
          (service, addr)
      }

      fn answer(&self, name: &str, answers: &[Answer]) {
          self.script
              .lock()
              .unwrap()
              .insert(name.into(), answers.iter().copied().collect());
      }

      fn received(&self) -> Vec<Received> {
          self.received.lock().unwrap().clone()
      }
  }

  struct Browser {
      secret: p256::SecretKey,
      auth: [u8; 16],
  }

  impl Browser {
      fn new(seed: u8) -> Self {
          Self {
              secret: p256::SecretKey::from_bytes(&[seed; 32].into()).unwrap(),
              auth: [seed; 16],
          }
      }

      fn read(&self, body: &[u8]) -> PushPayload {
          let auth = web_push_native::Auth::clone_from_slice(&self.auth);
          let plain = web_push_native::decrypt(body.to_vec(), &self.secret, &auth).unwrap();
          serde_json::from_slice(&plain).unwrap()
      }
  }

  struct Setup {
      hosts: Arc<Hosts>,
      operator: Arc<Operator>,
      vapid: Arc<VapidKey>,
      dir: tempfile::TempDir,
  }

  impl Setup {
      fn new() -> Self {
          let dir = tempfile::tempdir().unwrap();
          let path = dir.path().join("hennery.db");
          let operator = Operator::open(&path).unwrap();
          let hosts = Hosts::open(&path).unwrap();
          let token = operator.issue_setup_token(unix_now()).unwrap().unwrap();
          let SetupOutcome::Done { .. } = operator
              .set_up(&token, "correct horse battery", "https://hennery.example", unix_now())
              .unwrap()
          else {
              panic!("setup failed");
          };
          Self {
              hosts: Arc::new(hosts),
              operator: Arc::new(operator),
              vapid: Arc::new(VapidKey::generate()),
              dir,
          }
      }

      /// A subscription at `endpoint`, written straight into the file: a
      /// loopback `http` endpoint is never one `subscribe` accepts.
      fn subscribe_at(&self, id: &str, endpoint: &str, browser: &Browser) {
          let p256dh = URL_SAFE_NO_PAD.encode(browser.secret.public_key().to_encoded_point(false).as_bytes());
          rusqlite::Connection::open(self.dir.path().join("hennery.db"))
              .unwrap()
              .execute(
                  "INSERT INTO push_subscriptions(id, owner_id, endpoint, p256dh, auth, device_label, auth_session, created_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, 'test', 's', ?6)",
                  rusqlite::params![id, self.hosts.owner_id(), endpoint, p256dh, URL_SAFE_NO_PAD.encode(browser.auth), unix_now()],
              )
              .unwrap();
      }

      fn last_error(&self, id: &str) -> Option<String> {
          self.hosts
              .subscriptions()
              .unwrap()
              .into_iter()
              .find(|s| s.id == id)?
              .last_error
      }
  }

  fn notice(hat_id: &str) -> Notice {
      Notice {
          hat_id: hat_id.into(),
          urgency: Urgency::High,
          title: "Fix the flaky test".into(),
          generic_title: "Session needs your answer".into(),
          body: "needs your answer".into(),
          detail: None,
          url: "/sessions/s1".into(),
          tag: "s1".into(),
      }
  }

  fn egress() -> Egress {
      Egress::new(Timeouts::DEFAULT).unwrap()
  }

  fn quick() -> RetryPolicy {
      RetryPolicy {
          delays: vec![Duration::ZERO, Duration::ZERO],
          max_retry_after: Duration::from_secs(2),
          attempt_timeout: Duration::from_secs(10),
          budget: Duration::from_secs(60),
      }
  }

  #[tokio::test]
  async fn a_push_arrives_on_the_wire_encrypted_and_signed() {
      let (service, addr) = Service::start().await;
      let setup = Setup::new();
      let phone = Browser::new(7);
      setup.subscribe_at("push-a", &format!("http://{addr}/push/a"), &phone);
      let delivery = Delivery::new(
          setup.hosts.clone(),
          setup.operator.clone(),
          setup.vapid.clone(),
          egress().client(Allowance::InternalNetwork),
          quick(),
      );
      let outcomes = delivery
          .deliver(&notice(&setup.hosts.default_hat_for_new_hosts().unwrap()))
          .await
          .unwrap();
      assert_eq!(outcomes, [("push-a".into(), Outcome::Delivered)]);
      let received = service.received();
      assert_eq!(received.len(), 1);
      let got = &received[0];
      let header = |name: &str| got.headers.get(name).map(|v| v.to_str().unwrap().to_string());
      assert_eq!(header("ttl").as_deref(), Some("3600"));
      assert_eq!(header("urgency").as_deref(), Some("high"));
      assert_eq!(header("content-encoding").as_deref(), Some("aes128gcm"));
      assert_eq!(header("topic"), None);
      assert!(header("authorization").unwrap().starts_with("vapid t="));
      assert_eq!(phone.read(&got.body).title, "Fix the flaky test");
      // The padding survives the real path: RFC 8291's 86-byte header, the
      // padded plaintext, the 16-byte tag and its delimiter (the review's A4).
      assert_eq!(got.body.len(), 86 + PAD_TO + 17);
      assert!(setup.hosts.subscriptions().unwrap()[0].last_success_at.is_some());
  }

  #[tokio::test]
  async fn the_services_answers_over_http_decide_the_subscription() {
      let (service, addr) = Service::start().await;
      let setup = Setup::new();
      setup.subscribe_at("push-gone", &format!("http://{addr}/push/gone"), &Browser::new(1));
      setup.subscribe_at("push-busy", &format!("http://{addr}/push/busy"), &Browser::new(2));
      setup.subscribe_at("push-refused", &format!("http://{addr}/push/refused"), &Browser::new(3));
      service.answer("gone", &[(410, None)]);
      service.answer("busy", &[(503, Some("1")), (429, None)]);
      service.answer("refused", &[(403, None)]);
      let delivery = Delivery::new(
          setup.hosts.clone(),
          setup.operator.clone(),
          setup.vapid.clone(),
          egress().client(Allowance::InternalNetwork),
          quick(),
      );
      let started = std::time::Instant::now();
      let mut outcomes = delivery
          .deliver(&notice(&setup.hosts.default_hat_for_new_hosts().unwrap()))
          .await
          .unwrap();
      outcomes.sort_by(|a, b| a.0.cmp(&b.0));
      assert_eq!(outcomes[0], ("push-busy".into(), Outcome::Delivered));
      assert_eq!(outcomes[1], ("push-gone".into(), Outcome::Removed));
      assert!(matches!(&outcomes[2], (id, Outcome::Failed(why)) if id == "push-refused" && why.contains("403")));
      // The 503's `Retry-After: 1` was waited out.
      assert!(started.elapsed() >= Duration::from_secs(1));
      let tries = |name: &str| service.received().iter().filter(|r| r.name == name).count();
      assert_eq!((tries("busy"), tries("gone"), tries("refused")), (3, 1, 1));
      let ids: Vec<String> = setup.hosts.subscriptions().unwrap().into_iter().map(|s| s.id).collect();
      assert!(!ids.contains(&"push-gone".to_string()));
  }

  #[tokio::test]
  async fn a_service_that_cannot_be_reached_is_tried_again_then_recorded() {
      // A port nothing listens on: bound, then closed.
      let addr = tokio::net::TcpListener::bind("127.0.0.1:0")
          .await
          .unwrap()
          .local_addr()
          .unwrap();
      let setup = Setup::new();
      setup.subscribe_at("push-a", &format!("http://{addr}/push/a"), &Browser::new(1));
      let delivery = Delivery::new(
          setup.hosts.clone(),
          setup.operator.clone(),
          setup.vapid.clone(),
          egress().client(Allowance::InternalNetwork),
          quick(),
      );
      let outcomes = delivery
          .deliver(&notice(&setup.hosts.default_hat_for_new_hosts().unwrap()))
          .await
          .unwrap();
      assert!(
          matches!(&outcomes[0].1, Outcome::Failed(why) if why.contains("could not be reached")),
          "{outcomes:?}"
      );
      let error = setup.last_error("push-a").unwrap();
      assert!(!error.contains("127.0.0.1"), "{error}");
  }

  /// The collector's delivery (`spawn`) is public only (kernel spec §7.1): a
  /// subscription at a loopback address is refused before anything is sent,
  /// and kept, with the reason.
  #[tokio::test]
  async fn the_collectors_delivery_never_reaches_a_private_address() {
      let (service, addr) = Service::start().await;
      let setup = Setup::new();
      setup.subscribe_at("push-a", &format!("http://{addr}/push/a"), &Browser::new(1));
      let push = spawn(
          setup.hosts.clone(),
          setup.operator.clone(),
          setup.vapid.clone(),
          &egress(),
      );
      push.notify(notice(&setup.hosts.default_hat_for_new_hosts().unwrap()));
      let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
      let error = loop {
          if let Some(error) = setup.last_error("push-a") {
              break error;
          }
          assert!(tokio::time::Instant::now() < deadline, "nothing recorded");
          tokio::time::sleep(Duration::from_millis(20)).await;
      };
      assert!(error.contains("public address"), "{error}");
      assert!(service.received().is_empty());
      assert_eq!(setup.hosts.subscriptions().unwrap().len(), 1);
  }
  ```

In `crates/hennery/Cargo.toml`, replace:

  ```toml
  hennery-testkit = { path = "../hennery-testkit" }
  tempfile.workspace = true
  ```

with:

  ```toml
  hennery-testkit = { path = "../hennery-testkit" }
  # Writes a subscription `subscribe` would refuse (plan 10b-ii's wiring test).
  rusqlite.workspace = true
  tempfile.workspace = true
  ```

In `crates/hennery/src/main.rs`, replace:

  ```rust

      /// Final review I1: `up` may itself have the old operator bearer in its
  ```

with:

  ```rust

      /// Plan 10b-ii: the collector's notices reach delivery, and delivery is
      /// public only. A subscription at a loopback address gets a notice
      /// refused, recorded on it; nothing is sent.
      #[tokio::test]
      async fn the_collectors_notices_go_to_a_public_only_delivery() {
          let dir = tempfile::tempdir().unwrap();
          let db = dir.path().join("hennery.db");
          let mut state = AppState::new(
              Store::open(&db).unwrap(),
              Hosts::open(&db).unwrap(),
              Operator::open(&db).unwrap(),
          );
          let now = hennery_kernel::secret::unix_now();
          let token = state.operator.issue_setup_token(now).unwrap().unwrap();
          state
              .operator
              .set_up(&token, "correct horse battery", "https://hennery.example", now)
              .unwrap();
          let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
          rusqlite::Connection::open(&db)
              .unwrap()
              .execute(
                  "INSERT INTO push_subscriptions(id, owner_id, endpoint, p256dh, auth, device_label, auth_session, created_at)
                   VALUES ('push-a', ?1, ?2,
                       'BLn9b-VR0ca83knDNZ32dCHGyjJp-1riX9ZTN40MqV8K_LpQmLqxC_DoHvqvFXO_nGdAB4W9dogZb_sM-uV4JbY',
                       '_ordMnz7uTCmrpBTeUV4Bw', 'test', 's', ?3)",
                  rusqlite::params![
                      state.hosts.owner_id(),
                      format!("http://{}/push", listener.local_addr().unwrap()),
                      now
                  ],
              )
              .unwrap();
          start_push(
              &mut state,
              &hennery_kernel::egress::Egress::new(hennery_kernel::egress::Timeouts::DEFAULT).unwrap(),
          );
          state.push.notify(hennery_kernel::push::Notice {
              hat_id: state.hosts.default_hat_for_new_hosts().unwrap(),
              urgency: hennery_kernel::push::Urgency::Normal,
              title: "t".into(),
              generic_title: "g".into(),
              body: "finished".into(),
              detail: None,
              url: "/sessions/s1".into(),
              tag: "s1".into(),
          });
          let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
          let error = loop {
              if let Some(error) = state.hosts.subscriptions().unwrap()[0].last_error.clone() {
                  break error;
              }
              assert!(
                  tokio::time::Instant::now() < deadline,
                  "the notice never reached delivery"
              );
              tokio::time::sleep(std::time::Duration::from_millis(20)).await;
          };
          assert!(error.contains("public address"), "{error}");
          listener.set_nonblocking(true).unwrap();
          assert!(listener.accept().is_err(), "nothing connected");
      }

      /// Final review I1: `up` may itself have the old operator bearer in its
  ```

- [ ] **Step 2: Run them, and see them fail**: they fail to compile: there is no `Transport` for `EgressClient`, no `delivery::spawn`, no `start_push`.

- [ ] **Step 3: The adapter, `spawn`, and the collector's wiring**

In `crates/hennery-kernel/src/delivery.rs`, replace:

  ```rust
  //! egress client, public addresses only (kernel spec §7.1, plan 8b); tests
  //! use a fake one. Nothing here opens a connection itself.

  ```

with:

  ```rust
  //! egress client, public addresses only (kernel spec §7.1, plan 8b): `spawn`
  //! takes it from an `Egress` itself, so no caller can hand delivery another.
  //! Tests use a fake one, or an `InternalNetwork` client to a push service on
  //! loopback.

  use crate::egress::{Allowance, Egress, EgressClient, EgressError, Method, Request, header};
  ```

In `crates/hennery-kernel/src/delivery.rs`, replace:

  ```rust
      fn send(&self, request: PushRequest) -> Pin<Box<dyn Future<Output = Sent> + Send + '_>>;
  }
  ```

with:

  ```rust
      fn send(&self, request: PushRequest) -> Pin<Box<dyn Future<Output = Sent> + Send + '_>>;
  }

  /// The egress client sends a push as any other request (kernel spec §7.1):
  /// its policy refuses a non-public address before anything is sent, and an
  /// error never carries the endpoint.
  impl Transport for EgressClient {
      fn send(&self, push: PushRequest) -> Pin<Box<dyn Future<Output = Sent> + Send + '_>> {
          Box::pin(async move {
              let mut request = Request::new(Method::POST, push.endpoint);
              for (name, value) in &push.headers {
                  let (Ok(name), Ok(value)) = (
                      header::HeaderName::from_bytes(name.as_bytes()),
                      header::HeaderValue::from_str(value),
                  ) else {
                      return Sent::Failed;
                  };
                  request.headers_mut().insert(name, value);
              }
              *request.body_mut() = Some(push.body.into());
              match EgressClient::send(self, request).await {
                  Ok(response) => Sent::Status {
                      code: response.status().as_u16(),
                      retry_after: retry_after(response.headers()),
                  },
                  Err(EgressError::Refused(_)) => Sent::Refused,
                  Err(EgressError::Timeout) => Sent::Timeout,
                  Err(EgressError::Http(_)) => Sent::Failed,
              }
          })
      }
  }

  /// `Retry-After` in seconds (RFC 9110 §10.2.3); an HTTP date, or anything
  /// else, is no hint.
  fn retry_after(headers: &header::HeaderMap) -> Option<Duration> {
      let secs = headers
          .get(header::RETRY_AFTER)?
          .to_str()
          .ok()?
          .trim()
          .parse::<u64>()
          .ok()?;
      Some(Duration::from_secs(secs))
  }

  /// Start the collector's delivery: a queue, and a task draining it through
  /// `egress`'s public-only client (kernel spec §6, §7.1: Web Push never
  /// reaches a private address). The queue's sending end is `AppState::push`.
  pub fn spawn(hosts: Arc<Hosts>, operator: Arc<Operator>, vapid: Arc<VapidKey>, egress: &Egress) -> crate::push::Push {
      let (push, notices) = crate::push::Push::new();
      let delivery = Delivery::new(
          hosts,
          operator,
          vapid,
          egress.client(Allowance::PublicOnly),
          RetryPolicy::default(),
      );
      tokio::spawn(delivery.run(notices));
      push
  }
  ```

In `crates/hennery-kernel/src/delivery.rs`, replace:

  ```rust
      #[test]
      fn the_policy_decides_what_a_payload_shows() {
  ```

with:

  ```rust
      #[test]
      fn retry_after_is_read_in_seconds_only() {
          let mut headers = header::HeaderMap::new();
          assert_eq!(retry_after(&headers), None);
          headers.insert(header::RETRY_AFTER, header::HeaderValue::from_static(" 30 "));
          assert_eq!(retry_after(&headers), Some(Duration::from_secs(30)));
          headers.insert(
              header::RETRY_AFTER,
              header::HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"),
          );
          assert_eq!(retry_after(&headers), None);
          headers.insert(header::RETRY_AFTER, header::HeaderValue::from_static("-1"));
          assert_eq!(retry_after(&headers), None);
      }

      #[test]
      fn the_policy_decides_what_a_payload_shows() {
  ```

In `crates/hennery/src/main.rs`, replace:

  ```rust
      state.vapid = std::sync::Arc::new(hennery_kernel::push::VapidKey::load_or_create(&args.data_dir)?);
      state.offline_threshold = std::time::Duration::from_secs(args.host_offline_secs);
  ```

with:

  ```rust
      state.vapid = std::sync::Arc::new(hennery_kernel::push::VapidKey::load_or_create(&args.data_dir)?);
      // The collector's one outbound HTTP policy (kernel spec §7.1), built
      // once: Web Push takes it here, and the gateway shares this same one
      // (agreed with plan 8), never a second. Delivery starts before anything
      // serves, so a notice queued meanwhile reaches it, not the queue nobody
      // reads (plan 10b-ii).
      let egress = hennery_kernel::egress::Egress::new(hennery_kernel::egress::Timeouts::DEFAULT)?;
      start_push(&mut state, &egress);
      state.offline_threshold = std::time::Duration::from_secs(args.host_offline_secs);
  ```

In `crates/hennery/src/main.rs`, replace:

  ```rust

  #[cfg(test)]
  ```

with:

  ```rust

  /// Web Push delivery (plan 10b-ii): the state's notices go to a task that
  /// sends them through `egress`, public addresses only.
  fn start_push(state: &mut AppState, egress: &hennery_kernel::egress::Egress) {
      state.push =
          hennery_kernel::delivery::spawn(state.hosts.clone(), state.operator.clone(), state.vapid.clone(), egress);
  }

  #[cfg(test)]
  ```

- [ ] **Step 4: Run the tests, and the five checks**

- [ ] **Step 5: Revert-probe**

  | Change | Fails |
  |---|---|
  | `spawn` taking `InternalNetwork` | `the_collectors_delivery_never_reaches_a_private_address` |
  | the policy's refusal read as a failed connection | the same |
  | `Retry-After` not read from the answer | `the_services_answers_over_http_decide_the_subscription` |
  | the headers not set | `a_push_arrives_on_the_wire_encrypted_and_signed` |
  | the body not set | the same |
  | no padding | the same |
  | a failed connection read as a refusal | `a_service_that_cannot_be_reached_is_tried_again_then_recorded` |
  | `start_push` starting nothing | `the_collectors_notices_go_to_a_public_only_delivery` |

  The call to `start_push` in `run_collector` is checked by reading, not by a probe: a test would have to run the collector and see a notice reach a push service, and the collector's push services are public, which no test may reach (#54).

- [ ] **Step 6: Commit**: `test(push): delivery over HTTP through the egress client, and the collector's own, public only`, then `feat(push): the collector delivers Web Push through the shared egress client, public addresses only`.

## After this plan

- **Plan 8 (gateway):**
  - its `Notifier` calls `state.push.notify(Notice { url: "/mcp", tag: "mcp-<connection>", urgency: Normal, … })`;
  - it builds no `Egress` of its own: the `egress` local in `run_collector`, built before anything serves, is the one 8d passes on.
- **The operator:** a live check with a real browser per push service (FCM, Mozilla, Apple, WNS): subscribe, receive, rotate, unsubscribe.
- **The spec write-back:** kernel §6 gains decisions 3–7, and its "`web-push-native` builds RFC 8291 (aes128gcm) requests with VAPID" becomes: the crate encrypts, and the VAPID token is plan 10a's own.

---

_Generated with Claude AI — please review before distribution._
