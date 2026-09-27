//! The host's single path to the collector.
//!
//! Sequenced session frames go through the outbox (`emit`); correlated
//! responses to collector requests go straight to the socket (`reply`). The
//! connection task drains both.

use crate::outbox::Outbox;
use anyhow::Result;
use hennery_proto::frames::{HostFrame, SessionBody};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, mpsc};

#[derive(Clone)]
pub struct Uplink {
    outbox: Arc<Mutex<Outbox>>,
    outbox_changed: Arc<Notify>,
    replies: mpsc::UnboundedSender<HostFrame>,
}

impl Uplink {
    pub fn new(outbox: Outbox) -> (Self, mpsc::UnboundedReceiver<HostFrame>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let uplink = Self {
            outbox: Arc::new(Mutex::new(outbox)),
            outbox_changed: Arc::new(Notify::new()),
            replies: tx,
        };
        (uplink, rx)
    }

    /// Persist a session frame and wake the sender. Synchronous on purpose:
    /// callers rely on frames being ordered exactly as `emit` is called.
    pub fn emit(&self, session_id: &str, body: SessionBody) -> Result<()> {
        self.outbox.lock().expect("outbox lock").enqueue(session_id, body)?;
        self.outbox_changed.notify_one();
        Ok(())
    }

    /// Send a correlated response. Dropped responses are recovered by the
    /// collector's request timeout, so a closed channel is not an error.
    pub fn reply(&self, frame: HostFrame) {
        let _ = self.replies.send(frame);
    }

    pub fn pending(&self) -> Result<Vec<HostFrame>> {
        self.outbox.lock().expect("outbox lock").pending()
    }

    pub fn ack(&self, session_id: &str, ack_seq: u64) -> Result<()> {
        self.outbox.lock().expect("outbox lock").ack(session_id, ack_seq)?;
        Ok(())
    }

    pub fn fast_forward(&self, session_id: &str, seq: u64) -> Result<()> {
        self.outbox.lock().expect("outbox lock").fast_forward(session_id, seq)
    }

    pub fn last_seq(&self, session_id: &str) -> Result<u64> {
        self.outbox.lock().expect("outbox lock").last_seq(session_id)
    }

    pub async fn changed(&self) {
        self.outbox_changed.notified().await;
    }
}
