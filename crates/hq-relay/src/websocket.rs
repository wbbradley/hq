//! Bounded blocking WebSocket transport for NIP-01 and NIP-42 frames.

use std::{
    io,
    net::{TcpStream, ToSocketAddrs},
    os::fd::{AsFd, BorrowedFd},
    time::{Duration, Instant},
};

use serde_json::value::RawValue;
use tungstenite::{
    HandshakeError, Message, WebSocket, client::IntoClientRequest, client_tls_with_config,
    error::Error as WebSocketError, http::header::LOCATION, protocol::WebSocketConfig,
    stream::MaybeTlsStream,
};

use crate::{
    MAX_GIFT_WRAP_BYTES, MAX_RELAY_STATUS_BYTES, RelayConnection, RelayConnector, RelayFrame,
    RelayPortError, RelayReceive, RelayUrl,
};

const MAX_SUBSCRIPTION_BYTES: usize = 128;
const RELAY_FRAME_OVERHEAD_BYTES: usize = 8 * 1024;
const DEFAULT_MAX_MESSAGE_BYTES: usize = MAX_GIFT_WRAP_BYTES + RELAY_FRAME_OVERHEAD_BYTES;
const MAX_IO_TIMEOUT: Duration = Duration::from_secs(60);

/// Explicit memory and redirect bounds for real relay WebSocket connections.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebSocketRelayConfig {
    /// Maximum complete incoming WebSocket message bytes.
    pub max_message_bytes: usize,
    /// Maximum buffered outgoing WebSocket bytes after a write failure.
    pub max_write_buffer_bytes: usize,
    /// Maximum HTTP redirects accepted during the WebSocket handshake.
    pub max_redirects: u8,
    /// Total TCP, TLS, redirect, and WebSocket handshake deadline.
    pub connect_timeout: Duration,
    /// Maximum duration of one blocking socket write.
    pub write_timeout: Duration,
}

impl Default for WebSocketRelayConfig {
    fn default() -> Self {
        Self {
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            max_write_buffer_bytes: DEFAULT_MAX_MESSAGE_BYTES * 2,
            max_redirects: 3,
            connect_timeout: Duration::from_secs(10),
            write_timeout: Duration::from_secs(10),
        }
    }
}

impl WebSocketRelayConfig {
    fn validate(self) -> Result<Self, RelayPortError> {
        if self.max_message_bytes < MAX_GIFT_WRAP_BYTES
            || self.max_message_bytes > MAX_GIFT_WRAP_BYTES + RELAY_FRAME_OVERHEAD_BYTES
            || self.max_write_buffer_bytes < self.max_message_bytes
            || self.max_write_buffer_bytes > self.max_message_bytes.saturating_mul(4)
            || self.max_redirects > 8
            || self.connect_timeout.is_zero()
            || self.connect_timeout > MAX_IO_TIMEOUT
            || self.write_timeout.is_zero()
            || self.write_timeout > MAX_IO_TIMEOUT
        {
            return Err(RelayPortError::InvalidInput);
        }
        Ok(self)
    }
}

/// Blocking Tungstenite connector with Rustls support and bounded protocol buffers.
#[derive(Clone, Copy, Debug, Default)]
pub struct WebSocketRelayConnector {
    config: WebSocketRelayConfig,
}

impl WebSocketRelayConnector {
    /// Constructs a connector after validating every explicit memory bound.
    pub fn new(config: WebSocketRelayConfig) -> Result<Self, RelayPortError> {
        Ok(Self {
            config: config.validate()?,
        })
    }
}

impl RelayConnector for WebSocketRelayConnector {
    fn connect(&self, url: &RelayUrl) -> Result<Box<dyn RelayConnection>, RelayPortError> {
        let socket_config = WebSocketConfig::default()
            .read_buffer_size(8 * 1024)
            .write_buffer_size(0)
            .max_write_buffer_size(self.config.max_write_buffer_bytes)
            .max_message_size(Some(self.config.max_message_bytes))
            .max_frame_size(Some(self.config.max_message_bytes));
        let socket = connect_bounded(url, socket_config, self.config)?;
        let readiness = clone_tcp_stream(socket.get_ref())?;
        Ok(Box::new(WebSocketRelayConnection { socket, readiness }))
    }
}

fn connect_bounded(
    url: &RelayUrl,
    socket_config: WebSocketConfig,
    config: WebSocketRelayConfig,
) -> Result<WebSocket<MaybeTlsStream<TcpStream>>, RelayPortError> {
    let deadline = Instant::now()
        .checked_add(config.connect_timeout)
        .ok_or(RelayPortError::InvalidInput)?;
    let mut target = url.as_str().to_owned();
    for attempt in 0..=config.max_redirects {
        let request = target
            .as_str()
            .into_client_request()
            .map_err(|_| RelayPortError::Connection)?;
        let uri = request.uri();
        let host = uri.host().ok_or(RelayPortError::Connection)?;
        let host = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host);
        let port = uri.port_u16().unwrap_or(match uri.scheme_str() {
            Some("ws") => 80,
            Some("wss") => 443,
            _ => return Err(RelayPortError::Connection),
        });
        let addresses = (host, port)
            .to_socket_addrs()
            .map_err(|_| RelayPortError::Connection)?;
        let stream = connect_addresses(addresses, deadline)?;
        stream
            .set_nodelay(true)
            .map_err(|_| RelayPortError::Connection)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RelayPortError::Connection);
        }
        stream
            .set_read_timeout(Some(remaining))
            .and_then(|()| stream.set_write_timeout(Some(remaining)))
            .map_err(|_| RelayPortError::Connection)?;

        match client_tls_with_config(request, stream, Some(socket_config), None) {
            Ok((mut socket, _)) => {
                set_write_timeout(socket.get_mut(), config.write_timeout)?;
                return Ok(socket);
            }
            Err(HandshakeError::Failure(WebSocketError::Http(response)))
                if response.status().is_redirection() && attempt < config.max_redirects =>
            {
                response
                    .headers()
                    .get(LOCATION)
                    .and_then(|location| location.to_str().ok())
                    .ok_or(RelayPortError::Connection)?
                    .clone_into(&mut target);
            }
            Err(HandshakeError::Failure(_) | HandshakeError::Interrupted(_)) => {
                return Err(RelayPortError::Connection);
            }
        }
    }
    Err(RelayPortError::Connection)
}

fn connect_addresses(
    addresses: impl IntoIterator<Item = std::net::SocketAddr>,
    deadline: Instant,
) -> Result<TcpStream, RelayPortError> {
    for address in addresses {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        if let Ok(stream) = TcpStream::connect_timeout(&address, remaining) {
            return Ok(stream);
        }
    }
    Err(RelayPortError::Connection)
}

struct WebSocketRelayConnection {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    readiness: TcpStream,
}

impl RelayConnection for WebSocketRelayConnection {
    fn readiness(&self) -> BorrowedFd<'_> {
        self.readiness.as_fd()
    }

    fn send(&mut self, frame: RelayFrame) -> Result<(), RelayPortError> {
        set_nonblocking(self.socket.get_mut(), false)?;
        let exact = encode_frame(&frame)?;
        if exact.len() > self.socket.get_config().max_message_size.unwrap_or(0) {
            return Err(RelayPortError::InvalidInput);
        }
        self.socket
            .send(Message::text(exact))
            .map_err(|_| RelayPortError::Connection)
    }

    fn receive(&mut self) -> Result<RelayReceive, RelayPortError> {
        set_nonblocking(self.socket.get_mut(), true)?;
        loop {
            match self.socket.read() {
                Ok(Message::Text(exact)) => {
                    return decode_frame(exact.as_bytes()).map(RelayReceive::Frame);
                }
                Ok(Message::Close(_)) | Err(WebSocketError::ConnectionClosed) => {
                    return Ok(RelayReceive::Closed);
                }
                Ok(Message::Ping(_) | Message::Pong(_)) => {}
                Ok(Message::Binary(_) | Message::Frame(_)) => {
                    return Err(RelayPortError::Connection);
                }
                Err(WebSocketError::Io(error)) if timed_out(&error) => {
                    return Ok(RelayReceive::Pending);
                }
                Err(_) => return Err(RelayPortError::Connection),
            }
        }
    }

    fn close(&mut self) -> Result<(), RelayPortError> {
        set_nonblocking(self.socket.get_mut(), false)?;
        match self.socket.close(None) {
            Ok(()) | Err(WebSocketError::ConnectionClosed | WebSocketError::AlreadyClosed) => {
                Ok(())
            }
            Err(_) => Err(RelayPortError::Connection),
        }
    }
}

fn clone_tcp_stream(stream: &MaybeTlsStream<TcpStream>) -> Result<TcpStream, RelayPortError> {
    match stream {
        MaybeTlsStream::Plain(stream) => stream.try_clone(),
        MaybeTlsStream::Rustls(stream) => stream.sock.try_clone(),
        _ => return Err(RelayPortError::Connection),
    }
    .map_err(|_| RelayPortError::Connection)
}

fn set_nonblocking(
    stream: &mut MaybeTlsStream<TcpStream>,
    nonblocking: bool,
) -> Result<(), RelayPortError> {
    match stream {
        MaybeTlsStream::Plain(stream) => stream.set_nonblocking(nonblocking),
        MaybeTlsStream::Rustls(stream) => stream.sock.set_nonblocking(nonblocking),
        _ => return Err(RelayPortError::Connection),
    }
    .map_err(|_| RelayPortError::Connection)
}

fn set_write_timeout(
    stream: &mut MaybeTlsStream<TcpStream>,
    wait: Duration,
) -> Result<(), RelayPortError> {
    match stream {
        MaybeTlsStream::Plain(stream) => stream.set_write_timeout(Some(wait)),
        MaybeTlsStream::Rustls(stream) => stream.sock.set_write_timeout(Some(wait)),
        _ => return Err(RelayPortError::Connection),
    }
    .map_err(|_| RelayPortError::Connection)
}

fn timed_out(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

fn encode_frame(frame: &RelayFrame) -> Result<String, RelayPortError> {
    match frame {
        RelayFrame::Event(exact) => encode_embedded("EVENT", None, exact),
        RelayFrame::Request {
            subscription,
            filter,
        } => encode_embedded("REQ", Some(subscription), filter.as_bytes()),
        RelayFrame::Close(subscription) => {
            validate_subscription(subscription)?;
            serde_json::to_string(&("CLOSE", subscription)).map_err(|_| RelayPortError::Corrupt)
        }
        RelayFrame::Auth(exact) => encode_embedded("AUTH", None, exact.as_bytes()),
        RelayFrame::Ok { .. }
        | RelayFrame::EndOfStoredEvents(_)
        | RelayFrame::Closed { .. }
        | RelayFrame::Notice(_)
        | RelayFrame::SubscriptionEvent { .. } => Err(RelayPortError::InvalidInput),
    }
}

fn encode_embedded(
    tag: &str,
    subscription: Option<&str>,
    exact_json: &[u8],
) -> Result<String, RelayPortError> {
    let exact = std::str::from_utf8(exact_json).map_err(|_| RelayPortError::Corrupt)?;
    RawValue::from_string(exact.to_owned()).map_err(|_| RelayPortError::Corrupt)?;
    let tag = serde_json::to_string(tag).map_err(|_| RelayPortError::Corrupt)?;
    match subscription {
        Some(subscription) => {
            validate_subscription(subscription)?;
            let subscription =
                serde_json::to_string(subscription).map_err(|_| RelayPortError::Corrupt)?;
            Ok(format!("[{tag},{subscription},{exact}]"))
        }
        None => Ok(format!("[{tag},{exact}]")),
    }
}

fn decode_frame(exact: &[u8]) -> Result<RelayFrame, RelayPortError> {
    let values: Vec<Box<RawValue>> =
        serde_json::from_slice(exact).map_err(|_| RelayPortError::Connection)?;
    let tag = values
        .first()
        .ok_or(RelayPortError::Connection)
        .and_then(|value| decode_string(value, 16))?;
    match tag.as_str() {
        "OK" if values.len() == 4 => Ok(RelayFrame::Ok {
            event_id: decode_hex32(decode_string(&values[1], 64)?.as_str())?,
            accepted: serde_json::from_str(values[2].get())
                .map_err(|_| RelayPortError::Connection)?,
            message: decode_string(&values[3], MAX_RELAY_STATUS_BYTES)?,
        }),
        "EVENT" if values.len() == 3 => {
            let subscription = decode_subscription(&values[1])?;
            let event = values[2].get().as_bytes();
            if event.is_empty() || event.len() > MAX_GIFT_WRAP_BYTES {
                return Err(RelayPortError::Connection);
            }
            Ok(RelayFrame::SubscriptionEvent {
                subscription,
                exact_event: event.to_vec(),
            })
        }
        "EOSE" if values.len() == 2 => Ok(RelayFrame::EndOfStoredEvents(decode_subscription(
            &values[1],
        )?)),
        "AUTH" if values.len() == 2 => Ok(RelayFrame::Auth(decode_string(
            &values[1],
            MAX_RELAY_STATUS_BYTES,
        )?)),
        "CLOSED" if values.len() == 3 => Ok(RelayFrame::Closed {
            subscription: decode_subscription(&values[1])?,
            message: decode_string(&values[2], MAX_RELAY_STATUS_BYTES)?,
        }),
        "NOTICE" if values.len() == 2 => Ok(RelayFrame::Notice(decode_string(
            &values[1],
            MAX_RELAY_STATUS_BYTES,
        )?)),
        _ => Err(RelayPortError::Connection),
    }
}

fn decode_subscription(value: &RawValue) -> Result<String, RelayPortError> {
    decode_string(value, MAX_SUBSCRIPTION_BYTES).and_then(|subscription| {
        validate_subscription(&subscription)?;
        Ok(subscription)
    })
}

fn validate_subscription(subscription: &str) -> Result<(), RelayPortError> {
    if subscription.is_empty()
        || subscription.len() > MAX_SUBSCRIPTION_BYTES
        || subscription.chars().any(char::is_control)
    {
        return Err(RelayPortError::InvalidInput);
    }
    Ok(())
}

fn decode_string(value: &RawValue, maximum: usize) -> Result<String, RelayPortError> {
    let value: String =
        serde_json::from_str(value.get()).map_err(|_| RelayPortError::Connection)?;
    if value.len() > maximum {
        return Err(RelayPortError::Connection);
    }
    Ok(value)
}

fn decode_hex32(value: &str) -> Result<[u8; 32], RelayPortError> {
    if value.len() != 64 {
        return Err(RelayPortError::Connection);
    }
    let bytes = value.as_bytes();
    let mut output = [0_u8; 32];
    for (index, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        output[index] = decode_nibble(pair[0])?
            .checked_mul(16)
            .and_then(|high| high.checked_add(decode_nibble(pair[1]).ok()?))
            .ok_or(RelayPortError::Connection)?;
    }
    Ok(output)
}

const fn decode_nibble(value: u8) -> Result<u8, RelayPortError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(RelayPortError::Connection),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{net::TcpListener, thread, time::Instant};

    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use tungstenite::{Message, accept};

    use super::*;

    #[test]
    fn frame_codec_preserves_embedded_json_and_rejects_non_client_frames() {
        let event = br#"{"id":"01"}"#.to_vec();
        assert_eq!(
            encode_frame(&RelayFrame::Event(event)).expect("event encodes"),
            r#"["EVENT",{"id":"01"}]"#
        );
        assert_eq!(
            encode_frame(&RelayFrame::Request {
                subscription: "sub-a".to_owned(),
                filter: r#"{"kinds":[1059]}"#.to_owned(),
            })
            .expect("request encodes"),
            r#"["REQ","sub-a",{"kinds":[1059]}]"#
        );
        assert_eq!(
            encode_frame(&RelayFrame::Notice("server-only".to_owned())),
            Err(RelayPortError::InvalidInput)
        );
    }

    #[test]
    fn frame_codec_is_closed_bounded_and_keeps_exact_event_bytes() {
        let exact = br#"{"id":"same bytes","content":"x"}"#;
        assert_eq!(
            decode_frame(
                format!(
                    r#"["EVENT","sub",{}]"#,
                    std::str::from_utf8(exact).expect("UTF-8")
                )
                .as_bytes()
            ),
            Ok(RelayFrame::SubscriptionEvent {
                subscription: "sub".to_owned(),
                exact_event: exact.to_vec(),
            })
        );
        let oversized = "x".repeat(MAX_RELAY_STATUS_BYTES + 1);
        assert_eq!(
            decode_frame(
                serde_json::to_string(&("NOTICE", oversized))
                    .expect("notice serializes")
                    .as_bytes()
            ),
            Err(RelayPortError::Connection)
        );
        assert_eq!(
            decode_frame(br#"["UNKNOWN"]"#),
            Err(RelayPortError::Connection)
        );
    }

    #[test]
    fn connector_rejects_unbounded_or_zero_resource_limits() {
        for invalid in [
            WebSocketRelayConfig {
                connect_timeout: Duration::ZERO,
                ..WebSocketRelayConfig::default()
            },
            WebSocketRelayConfig {
                write_timeout: MAX_IO_TIMEOUT + Duration::from_millis(1),
                ..WebSocketRelayConfig::default()
            },
            WebSocketRelayConfig {
                max_redirects: 9,
                ..WebSocketRelayConfig::default()
            },
        ] {
            assert!(matches!(
                WebSocketRelayConnector::new(invalid),
                Err(RelayPortError::InvalidInput)
            ));
        }
    }

    #[test]
    fn stalled_handshake_obeys_the_configured_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener binds");
        let address = listener.local_addr().expect("address exists");
        let server = thread::spawn(move || {
            let _stream = listener.accept().expect("client connects").0;
            thread::sleep(Duration::from_millis(100));
        });
        let connector = WebSocketRelayConnector::new(WebSocketRelayConfig {
            connect_timeout: Duration::from_millis(20),
            ..WebSocketRelayConfig::default()
        })
        .expect("config validates");
        let url = RelayUrl::new(format!("ws://{address}")).expect("URL validates");
        let started = Instant::now();
        assert!(connector.connect(&url).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        server.join().expect("server joins");
    }

    #[test]
    fn loopback_connection_encodes_request_receives_auth_and_observes_close() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener binds");
        let address = listener.local_addr().expect("address exists");
        let server = thread::spawn(move || {
            let stream = listener.accept().expect("client connects").0;
            let mut socket = accept(stream).expect("handshake succeeds");
            assert_eq!(
                socket.read().expect("request arrives"),
                Message::text(r#"["REQ","sub",{"kinds":[1059]}]"#)
            );
            socket
                .send(Message::Ping(vec![1, 2, 3].into()))
                .expect("ping sends");
            socket
                .send(Message::text(r#"["AUTH","challenge"]"#))
                .expect("auth sends");
            assert_eq!(
                socket.read().expect("automatic pong arrives"),
                Message::Pong(vec![1, 2, 3].into())
            );
            socket.close(None).expect("close sends");
        });
        let url = RelayUrl::new(format!("ws://{address}")).expect("URL validates");
        let connector = WebSocketRelayConnector::default();
        let mut connection = connector.connect(&url).expect("connection opens");
        connection
            .send(RelayFrame::Request {
                subscription: "sub".to_owned(),
                filter: r#"{"kinds":[1059]}"#.to_owned(),
            })
            .expect("request sends");
        assert_eq!(
            receive_ready(connection.as_mut(), Duration::from_secs(1)),
            RelayReceive::Frame(RelayFrame::Auth("challenge".to_owned()))
        );
        assert_eq!(
            receive_ready(connection.as_mut(), Duration::from_secs(1)),
            RelayReceive::Closed
        );
        server.join().expect("server joins");
    }

    fn receive_ready(connection: &mut dyn RelayConnection, timeout: Duration) -> RelayReceive {
        let mut descriptor = [PollFd::new(
            connection.readiness(),
            PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR,
        )];
        let timeout = PollTimeout::try_from(timeout).expect("timeout fits poll");
        assert_eq!(poll(&mut descriptor, timeout).expect("readiness waits"), 1);
        connection.receive().expect("ready frame receives")
    }
}
