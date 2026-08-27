//! Bounded asynchronous byte I/O for one authenticated local session.

use std::{error::Error, fmt, future::Future, num::NonZeroUsize};

use hq_local_api::protocol::v1::{FrameDecoder, Id32, WireMessage};
use hq_local_api::{OutboundMessage, WriteTicket};
use tokio::{
    io::{AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::UnixStream,
    sync::{mpsc, watch},
};

use crate::AcceptedLocalStream;

/// Stable terminal cause for one local byte session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalSessionClose {
    /// The owning node explicitly closed this session.
    Requested,
    /// The peer cleanly closed after complete input frames.
    PeerClosed,
    /// Input was malformed, oversized, or truncated at EOF.
    Protocol,
    /// Reading from the authenticated stream failed.
    ReadFailure,
    /// A complete queued frame could not be written.
    WriteFailure,
    /// The central event receiver was dropped.
    EventReceiverClosed,
    /// Every encoded-write producer was dropped.
    WriteQueueClosed,
}

/// One bounded event emitted to the sole node session loop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalSessionEvent {
    /// One complete strictly decoded frame.
    Message {
        /// Connection identity assigned by the node loop.
        session_id: Id32,
        /// Complete decoded local protocol message.
        message: WireMessage,
    },
    /// Every byte of one exact session-owned response frame was written.
    Written {
        /// Connection identity assigned by the node loop.
        session_id: Id32,
        /// Ticket that may now be confirmed on its `ServerSession`.
        ticket: WriteTicket,
    },
    /// The descriptor and both byte loops have terminated.
    Closed {
        /// Connection identity assigned by the node loop.
        session_id: Id32,
        /// Redacted terminal cause.
        cause: LocalSessionClose,
    },
}

/// Immediate encoded-write admission rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalSessionSendError {
    /// The fixed write queue is occupied.
    Full,
    /// The session write queue has closed.
    Closed,
    /// The typed message could not be encoded within protocol bounds.
    Encode,
    /// Only invalidations may be sent without a response ticket.
    InvalidMessage,
}

/// Failure to attach authenticated nonblocking I/O to the active Tokio reactor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalSessionStartError {
    /// The stream could not be registered without exposing operating-system prose.
    RuntimeUnavailable,
}

impl fmt::Display for LocalSessionStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("authenticated local session I/O is unavailable")
    }
}

impl Error for LocalSessionStartError {}

#[derive(Debug)]
struct QueuedFrame {
    ticket: Option<WriteTicket>,
    bytes: Vec<u8>,
}

/// Sole bounded write and close capability for one authenticated session.
#[derive(Debug)]
pub struct LocalSessionHandle {
    writes: mpsc::Sender<QueuedFrame>,
    close: watch::Sender<bool>,
}

impl LocalSessionHandle {
    /// Attempts to enqueue one exact response produced by `ServerSession`.
    pub fn try_send_response(
        &self,
        outbound: OutboundMessage,
    ) -> Result<(), LocalSessionSendError> {
        let (ticket, message) = outbound.into_parts();
        self.try_send(Some(ticket), &message)
    }

    /// Attempts to enqueue one untracked coalesced revision invalidation.
    pub fn try_send_invalidation(
        &self,
        message: &WireMessage,
    ) -> Result<(), LocalSessionSendError> {
        if !matches!(message, WireMessage::Invalidation(_)) {
            return Err(LocalSessionSendError::InvalidMessage);
        }
        self.try_send(None, message)
    }

    /// Requests prompt descriptor close independently of write-queue capacity.
    pub fn close(&self) {
        self.close.send_replace(true);
    }

    fn try_send(
        &self,
        ticket: Option<WriteTicket>,
        message: &WireMessage,
    ) -> Result<(), LocalSessionSendError> {
        let bytes = message
            .encode_frame()
            .map_err(|_| LocalSessionSendError::Encode)?;
        self.writes
            .try_send(QueuedFrame { ticket, bytes })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => LocalSessionSendError::Full,
                mpsc::error::TrySendError::Closed(_) => LocalSessionSendError::Closed,
            })
    }
}

/// Prepares one joined read/write future over a peer-validated nonblocking stream.
pub fn prepare_local_session_io(
    accepted: AcceptedLocalStream,
    session_id: Id32,
    write_capacity: NonZeroUsize,
    events: mpsc::Sender<LocalSessionEvent>,
) -> Result<
    (
        LocalSessionHandle,
        impl Future<Output = LocalSessionClose> + Send + 'static,
    ),
    LocalSessionStartError,
> {
    let stream = UnixStream::from_std(accepted.into_inner())
        .map_err(|_| LocalSessionStartError::RuntimeUnavailable)?;
    let (writes, write_rx) = mpsc::channel(write_capacity.get());
    let (close, close_rx) = watch::channel(false);
    let handle = LocalSessionHandle { writes, close };
    let driver = drive_session(stream, session_id, write_rx, events, close_rx);
    Ok((handle, driver))
}

async fn drive_session(
    stream: UnixStream,
    session_id: Id32,
    outbound: mpsc::Receiver<QueuedFrame>,
    events: mpsc::Sender<LocalSessionEvent>,
    close: watch::Receiver<bool>,
) -> LocalSessionClose {
    let (read_half, write_half) = stream.into_split();
    let cause = {
        let read = read_loop(read_half, session_id, events.clone(), close.clone());
        let write = write_loop(write_half, session_id, outbound, events.clone(), close);
        tokio::pin!(read, write);
        tokio::select! {
            cause = &mut read => cause,
            cause = &mut write => cause,
        }
    };
    let _ = events
        .send(LocalSessionEvent::Closed { session_id, cause })
        .await;
    cause
}

async fn read_loop(
    mut reader: tokio::net::unix::OwnedReadHalf,
    session_id: Id32,
    events: mpsc::Sender<LocalSessionEvent>,
    mut close: watch::Receiver<bool>,
) -> LocalSessionClose {
    let mut decoder = FrameDecoder::new();
    let mut bytes = [0_u8; 8_192];
    loop {
        let read = tokio::select! {
            biased;
            () = close_requested(&mut close) => return LocalSessionClose::Requested,
            result = reader.read(&mut bytes) => result,
        };
        let count = match read {
            Ok(0) if decoder.buffered_len() == 0 => return LocalSessionClose::PeerClosed,
            Ok(0) => return LocalSessionClose::Protocol,
            Ok(count) => count,
            Err(_) => return LocalSessionClose::ReadFailure,
        };
        let mut next = decoder.push(&bytes[..count]);
        loop {
            let message = match next {
                Ok(Some(message)) => message,
                Ok(None) => break,
                Err(_) => return LocalSessionClose::Protocol,
            };
            let sent = tokio::select! {
                biased;
                () = close_requested(&mut close) => return LocalSessionClose::Requested,
                result = events.send(LocalSessionEvent::Message { session_id, message }) => result,
            };
            if sent.is_err() {
                return LocalSessionClose::EventReceiverClosed;
            }
            next = decoder.push(&[]);
        }
    }
}

async fn write_loop<W>(
    mut writer: W,
    session_id: Id32,
    mut outbound: mpsc::Receiver<QueuedFrame>,
    events: mpsc::Sender<LocalSessionEvent>,
    mut close: watch::Receiver<bool>,
) -> LocalSessionClose
where
    W: AsyncWrite + Unpin,
{
    loop {
        let queued = tokio::select! {
            biased;
            () = close_requested(&mut close) => return LocalSessionClose::Requested,
            queued = outbound.recv() => queued,
        };
        let Some(queued) = queued else {
            return LocalSessionClose::WriteQueueClosed;
        };
        let written = tokio::select! {
            biased;
            () = close_requested(&mut close) => return LocalSessionClose::Requested,
            result = writer.write_all(&queued.bytes) => result,
        };
        if written.is_err() {
            return LocalSessionClose::WriteFailure;
        }
        if let Some(ticket) = queued.ticket {
            let sent = tokio::select! {
                biased;
                () = close_requested(&mut close) => return LocalSessionClose::Requested,
                result = events.send(LocalSessionEvent::Written { session_id, ticket }) => result,
            };
            if sent.is_err() {
                return LocalSessionClose::EventReceiverClosed;
            }
        }
    }
}

async fn close_requested(close: &mut watch::Receiver<bool>) {
    while !*close.borrow_and_update() {
        if close.changed().await.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use hq_local_api::protocol::v1::Id32;
    use tokio::io::AsyncReadExt;

    use super::{LocalSessionClose, QueuedFrame, write_loop};

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_after_a_partial_write_closes_without_a_completion_event() {
        let (writer, mut peer) = tokio::io::duplex(8);
        let (outbound_tx, outbound_rx) = tokio::sync::mpsc::channel(1);
        outbound_tx
            .send(QueuedFrame {
                ticket: None,
                bytes: vec![0x5a; 64],
            })
            .await
            .expect("queued frame");
        let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(1);
        let (close_tx, close_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(write_loop(
            writer,
            Id32::new([91; 32]),
            outbound_rx,
            events_tx,
            close_rx,
        ));

        let mut partial = [0_u8; 8];
        peer.read_exact(&mut partial)
            .await
            .expect("first partial bytes");
        close_tx.send_replace(true);
        assert_eq!(
            task.await.expect("write loop joins"),
            LocalSessionClose::Requested
        );
        assert!(events_rx.try_recv().is_err());
    }
}
