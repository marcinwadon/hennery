//! Axum validates the HTTP handshake. A bounded frame-header gate protects
//! tungstenite's eager payload allocation until the host proves its key.
use axum::{
    body::Body,
    extract::{FromRequestParts, ws::WebSocketUpgrade},
    http::{HeaderValue, StatusCode, Version, header, request::Parts},
    response::{IntoResponse, Response},
};
use hyper::upgrade::{OnUpgrade, Upgraded};
use hyper_util::rt::TokioIo;
use std::{
    future::Future,
    io::{self, Cursor},
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
pub(crate) use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        handshake::derive_accept_key,
        protocol::{Role, WebSocketConfig, frame::FrameHeader},
    },
};
pub(crate) const HELLO_READ_BYTES: usize = 64 * 1024;
pub(crate) type WebSocket = WebSocketStream<FrameBudget<TokioIo<Upgraded>>>;

pub(crate) struct HostUpgrade {
    raw: OnUpgrade,
    key: HeaderValue,
}
impl<S: Send + Sync> FromRequestParts<S> for HostUpgrade {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        if parts.version > Version::HTTP_11 {
            return Err(StatusCode::BAD_REQUEST.into_response());
        }
        let raw = parts.extensions.get::<OnUpgrade>().cloned();
        let key = parts.headers.get(header::SEC_WEBSOCKET_KEY).cloned();
        // Reuse Axum's validation, including upgrade capability, method,
        // Connection/Upgrade and WebSocket version. Drop its unpolled future;
        // our clone is the only future that awaits the shared upgrade receiver.
        let validated = WebSocketUpgrade::from_request_parts(parts, state)
            .await
            .map_err(IntoResponse::into_response)?;
        drop(validated);
        Ok(Self {
            raw: raw.expect("validated upgrade"),
            key: key.expect("validated key"),
        })
    }
}
impl HostUpgrade {
    pub(crate) fn on_upgrade<F, Fut>(self, callback: F) -> Response
    where
        F: FnOnce(WebSocket) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        tokio::spawn(async move {
            if let Ok(Ok(raw)) = tokio::time::timeout(Duration::from_secs(10), self.raw).await {
                let config = WebSocketConfig::default()
                    .read_buffer_size(8192)
                    .max_message_size(Some(super::ws::MAX_FRAME))
                    .max_frame_size(Some(super::ws::MAX_FRAME));
                let socket = WebSocketStream::from_raw_socket(
                    FrameBudget::new(TokioIo::new(raw), HELLO_READ_BYTES),
                    Role::Server,
                    Some(config),
                )
                .await;
                callback(socket).await;
            }
        });
        Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "websocket")
            .header(header::SEC_WEBSOCKET_ACCEPT, derive_accept_key(self.key.as_bytes()))
            .body(Body::empty())
            .expect("fixed upgrade response")
    }
}

pub(crate) struct FrameBudget<T> {
    inner: T,
    remaining: Option<usize>,
    header: [u8; 14],
    read: usize,
    sent: usize,
    ready: bool,
    payload: usize,
}
impl<T> FrameBudget<T> {
    fn new(inner: T, bytes: usize) -> Self {
        Self {
            inner,
            remaining: Some(bytes),
            header: [0; 14],
            read: 0,
            sent: 0,
            ready: false,
            payload: 0,
        }
    }
    pub(crate) fn authenticated(&mut self) {
        self.remaining = None;
    }
}
impl<T: AsyncRead + Unpin> AsyncRead for FrameBudget<T> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, output: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if self.remaining.is_none() {
            return Pin::new(&mut self.inner).poll_read(cx, output);
        }
        while !self.ready {
            let mut byte = [0u8; 1];
            let mut input = ReadBuf::new(&mut byte);
            match Pin::new(&mut self.inner).poll_read(cx, &mut input) {
                Poll::Ready(Ok(())) if input.filled().is_empty() => return Poll::Ready(Ok(())),
                Poll::Ready(Ok(())) => {}
                other => return other,
            }
            let index = self.read;
            self.header[index] = byte[0];
            self.read += 1;
            let parsed = FrameHeader::parse(&mut Cursor::new(&self.header[..self.read]));
            match parsed {
                Ok(Some((_, length))) => {
                    let left = self.remaining.expect("restricted");
                    let length = usize::try_from(length).ok();
                    let Some(length) = length.filter(|n| *n <= left.saturating_sub(self.read) && self.read <= left)
                    else {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "unauthenticated frame exceeds read budget",
                        )));
                    };
                    // Charge the declared size BEFORE passing its header to
                    // tungstenite, which reserves that size without reading it.
                    self.remaining = Some(left - self.read - length);
                    self.payload = length;
                    self.ready = true;
                }
                Ok(None) if self.read < self.header.len() => {}
                _ => return Poll::Ready(Err(io::Error::new(io::ErrorKind::InvalidData, "invalid frame header"))),
            }
        }
        if self.sent < self.read {
            let end = (self.sent + output.remaining()).min(self.read);
            output.put_slice(&self.header[self.sent..end]);
            self.sent = end;
        } else if self.payload > 0 {
            let mut scratch = [0u8; 8192];
            let count = self.payload.min(output.remaining()).min(scratch.len());
            let mut input = ReadBuf::new(&mut scratch[..count]);
            match Pin::new(&mut self.inner).poll_read(cx, &mut input) {
                Poll::Ready(Ok(())) => {
                    self.payload -= input.filled().len();
                    output.put_slice(input.filled());
                }
                other => return other,
            }
        }
        if self.sent == self.read && self.payload == 0 {
            self.read = 0;
            self.sent = 0;
            self.ready = false;
        }
        Poll::Ready(Ok(()))
    }
}
impl<T: AsyncWrite + Unpin> AsyncWrite for FrameBudget<T> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, data)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn header(length: u64) -> Vec<u8> {
        let mut bytes = vec![0x82, 0xff];
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes
    }
    #[tokio::test]
    async fn declared_large_frame_is_refused_before_its_header_reaches_the_parser() {
        let bytes = header(32 * 1024 * 1024);
        let mut guarded = FrameBudget::new(bytes.as_slice(), HELLO_READ_BYTES);
        let mut forwarded = Vec::new();
        let error = guarded.read_to_end(&mut forwarded).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(forwarded.is_empty(), "parser must not see an allocation-sized header");
    }
    #[tokio::test]
    async fn fragments_share_one_total_budget() {
        let mut bytes = header(40_000);
        bytes.extend_from_slice(&vec![0; 40_000]);
        let allowed = bytes.len();
        bytes.extend_from_slice(&header(40_000));
        let mut guarded = FrameBudget::new(bytes.as_slice(), HELLO_READ_BYTES);
        let mut forwarded = Vec::new();
        assert_eq!(
            guarded.read_to_end(&mut forwarded).await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(forwarded.len(), allowed);
    }
    #[tokio::test]
    async fn authenticated_stream_preserves_large_frames() {
        let mut bytes = header(100_000);
        bytes.extend_from_slice(&vec![0; 100_000]);
        let mut guarded = FrameBudget::new(bytes.as_slice(), HELLO_READ_BYTES);
        guarded.authenticated();
        let mut forwarded = Vec::new();
        guarded.read_to_end(&mut forwarded).await.unwrap();
        assert_eq!(forwarded, bytes);
    }
}
