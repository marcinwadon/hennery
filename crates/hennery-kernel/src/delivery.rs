//! Web Push delivery (kernel spec §6; plan 10b-ii): drain the notice queue
//! (`push::Notices`), apply each notice's hat policy, and send it to every
//! subscription of the owner's, encrypted to the browser (RFC 8291) and
//! signed with the collector's VAPID key (RFC 8292).
//!
//! Requests go out through a `Transport`. The collector's is the shared
//! egress client, public addresses only (kernel spec §7.1, plan 8b): `spawn`
//! takes it from an `Egress` itself, so no caller can hand delivery another.
//! Tests use a fake one, or an `InternalNetwork` client to a push service on
//! loopback.

use crate::egress::{Allowance, Egress, EgressClient, EgressError, Method, Request, header};
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
/// - the title: the notice's generic title under `generic_title`, else its
///   title;
/// - the body: the notice's `detail` when `details` is on and it has one,
///   with `generic_title` too; otherwise none under `generic_title`, else
///   the notice's body.
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
