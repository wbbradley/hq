//! Bounded newline-delimited JSON transport for one app-server child.

use std::{
    io::{Read, Write},
    sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

use hq_harness::{HarnessError, HarnessErrorClass, HarnessEventNotifier};
use serde::Serialize;

use crate::protocol::WireMessage;

pub(crate) const MAX_CODEX_FRAME_BYTES: usize = 4 * 1024 * 1024;

pub(crate) enum TransportRead {
    Message(WireMessage),
    TimedOut,
    Closed,
    Failed(TransportFailure),
}

#[derive(Clone, Copy)]
pub(crate) enum TransportFailure {
    Malformed,
    Oversized,
    Truncated,
    Read,
}

impl TransportFailure {
    pub(crate) const fn class(self) -> HarnessErrorClass {
        match self {
            Self::Malformed | Self::Oversized | Self::Truncated => {
                HarnessErrorClass::ProtocolViolation
            }
            Self::Read => HarnessErrorClass::TransportClosed,
        }
    }
}

pub(crate) struct JsonlTransport {
    input: Option<Box<dyn Write + Send>>,
    incoming: Receiver<TransportRead>,
    reader: Option<JoinHandle<()>>,
    notifier: Arc<Mutex<Option<HarnessEventNotifier>>>,
}

impl JsonlTransport {
    pub(crate) fn start(
        output: Box<dyn Read + Send>,
        capacity: usize,
    ) -> Result<Self, HarnessError> {
        if capacity == 0 {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        }
        let (sender, incoming) = mpsc::sync_channel(capacity);
        let notifier = Arc::new(Mutex::new(None));
        let reader_notifier = Arc::clone(&notifier);
        let reader = thread::Builder::new()
            .name("hq-codex-jsonl".to_owned())
            .spawn(move || read_frames(output, &sender, &reader_notifier))
            .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?;
        Ok(Self {
            input: None,
            incoming,
            reader: Some(reader),
            notifier,
        })
    }

    pub(crate) fn register_event_notifier(
        &self,
        notifier: &HarnessEventNotifier,
    ) -> Result<(), HarnessError> {
        *self
            .notifier
            .lock()
            .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))? =
            Some(notifier.clone());
        notifier.notify()
    }

    pub(crate) fn bind_input(&mut self, input: Box<dyn Write + Send>) {
        self.input = Some(input);
    }

    pub(crate) fn write<T: Serialize>(&mut self, value: &T) -> Result<(), HarnessError> {
        let mut encoded = serde_json::to_vec(value)
            .map_err(|_| HarnessError::new(HarnessErrorClass::InvalidInput))?;
        if encoded.is_empty() || encoded.len() > MAX_CODEX_FRAME_BYTES {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        }
        encoded.push(b'\n');
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::IntakeClosed))?;
        input
            .write_all(&encoded)
            .and_then(|()| input.flush())
            .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))
    }

    pub(crate) fn receive(&self, wait: Duration) -> TransportRead {
        match self.incoming.recv_timeout(wait) {
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => TransportRead::TimedOut,
            Err(RecvTimeoutError::Disconnected) => TransportRead::Closed,
        }
    }

    pub(crate) fn try_receive(&self) -> TransportRead {
        match self.incoming.try_recv() {
            Ok(message) => message,
            Err(TryRecvError::Empty) => TransportRead::TimedOut,
            Err(TryRecvError::Disconnected) => TransportRead::Closed,
        }
    }

    pub(crate) fn close_input(&mut self) {
        self.input.take();
    }

    pub(crate) fn join_reader(&mut self) -> Result<(), HarnessError> {
        self.reader.take().map_or(Ok(()), |reader| {
            reader
                .join()
                .map_err(|_| HarnessError::new(HarnessErrorClass::CleanupFailed))
        })
    }
}

fn read_frames(
    mut output: Box<dyn Read + Send>,
    sender: &SyncSender<TransportRead>,
    notifier: &Mutex<Option<HarnessEventNotifier>>,
) {
    let mut pending = Vec::new();
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        let count = match output.read(&mut chunk) {
            Ok(0) => {
                let terminal = if pending.is_empty() {
                    TransportRead::Closed
                } else {
                    TransportRead::Failed(TransportFailure::Truncated)
                };
                send_incoming(sender, notifier, terminal);
                return;
            }
            Ok(count) => count,
            Err(_) => {
                send_incoming(
                    sender,
                    notifier,
                    TransportRead::Failed(TransportFailure::Read),
                );
                return;
            }
        };
        for byte in &chunk[..count] {
            if *byte == b'\n' {
                if pending.is_empty() {
                    send_incoming(
                        sender,
                        notifier,
                        TransportRead::Failed(TransportFailure::Malformed),
                    );
                    return;
                }
                let parsed = serde_json::from_slice::<WireMessage>(&pending);
                pending.clear();
                match parsed {
                    Ok(message) if valid_envelope(&message) => {
                        if !send_incoming(sender, notifier, TransportRead::Message(message)) {
                            return;
                        }
                    }
                    Ok(_) | Err(_) => {
                        send_incoming(
                            sender,
                            notifier,
                            TransportRead::Failed(TransportFailure::Malformed),
                        );
                        return;
                    }
                }
            } else {
                if pending.len() == MAX_CODEX_FRAME_BYTES {
                    send_incoming(
                        sender,
                        notifier,
                        TransportRead::Failed(TransportFailure::Oversized),
                    );
                    return;
                }
                pending.push(*byte);
            }
        }
    }
}

fn send_incoming(
    sender: &SyncSender<TransportRead>,
    notifier: &Mutex<Option<HarnessEventNotifier>>,
    incoming: TransportRead,
) -> bool {
    if sender.send(incoming).is_err() {
        return false;
    }
    if let Ok(notifier) = notifier.lock()
        && let Some(notifier) = notifier.as_ref()
    {
        let _ = notifier.notify();
    }
    true
}

fn valid_envelope(message: &WireMessage) -> bool {
    let request = message.id.is_some()
        && message.method.is_some()
        && message.result.is_none()
        && message.error.is_none();
    let notification = message.id.is_none()
        && message.method.is_some()
        && message.result.is_none()
        && message.error.is_none();
    let response = message.id.is_some()
        && message.method.is_none()
        && (message.result.is_some() ^ message.error.is_some());
    request || notification || response
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Cursor, Read},
        time::Duration,
    };

    use hq_harness::HarnessEventNotifier;

    use super::{JsonlTransport, MAX_CODEX_FRAME_BYTES, TransportFailure, TransportRead};

    #[test]
    fn accepts_partial_reads_and_additive_fields() -> Result<(), Box<dyn std::error::Error>> {
        let input =
            Cursor::new(b"{\"id\":1,\"result\":{},\"jsonrpc\":\"2.0\",\"future\":true}\n".to_vec());
        let mut transport = JsonlTransport::start(Box::new(input), 1)?;
        assert!(matches!(
            transport.receive(Duration::from_secs(1)),
            TransportRead::Message(_)
        ));
        assert!(matches!(
            transport.receive(Duration::from_secs(1)),
            TransportRead::Closed
        ));
        transport.join_reader()?;
        Ok(())
    }

    #[test]
    fn rejects_malformed_truncated_and_oversized_frames() -> Result<(), Box<dyn std::error::Error>>
    {
        for (input, expected) in [
            (b"not-json\n".to_vec(), TransportFailure::Malformed),
            (b"{\"id\":1".to_vec(), TransportFailure::Truncated),
        ] {
            let mut transport = JsonlTransport::start(Box::new(Cursor::new(input)), 1)?;
            assert!(matches!(
                transport.receive(Duration::from_secs(1)),
                TransportRead::Failed(actual)
                    if std::mem::discriminant(&actual) == std::mem::discriminant(&expected)
            ));
            transport.join_reader()?;
        }

        let oversized = vec![b'x'; MAX_CODEX_FRAME_BYTES + 1];
        let mut transport = JsonlTransport::start(Box::new(Cursor::new(oversized)), 1)?;
        assert!(matches!(
            transport.receive(Duration::from_secs(2)),
            TransportRead::Failed(TransportFailure::Oversized)
        ));
        transport.join_reader()?;
        Ok(())
    }

    #[test]
    fn distinguishes_reader_failure_from_protocol_failure() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut transport = JsonlTransport::start(Box::new(FailingReader), 1)?;
        assert!(matches!(
            transport.receive(Duration::from_secs(1)),
            TransportRead::Failed(TransportFailure::Read)
        ));
        transport.join_reader()?;
        Ok(())
    }

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("scripted read failure"))
        }
    }

    #[test]
    fn reports_timeout_without_reclassifying_it_as_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let transport = JsonlTransport::start(Box::new(std::io::empty()), 1)?;
        assert!(matches!(
            transport.receive(Duration::ZERO),
            TransportRead::TimedOut | TransportRead::Closed
        ));
        Ok(())
    }

    #[test]
    fn notifier_covers_events_queued_before_registration() -> Result<(), Box<dyn std::error::Error>>
    {
        let input = Cursor::new(b"{\"id\":1,\"result\":{}}\n".to_vec());
        let mut transport = JsonlTransport::start(Box::new(input), 2)?;
        transport.join_reader()?;
        let notifier = HarnessEventNotifier::default();

        transport.register_event_notifier(&notifier)?;

        assert!(notifier.wait(Some(Duration::from_secs(1)))?);
        assert!(matches!(transport.try_receive(), TransportRead::Message(_)));
        assert!(matches!(transport.try_receive(), TransportRead::Closed));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn notifier_wakes_for_input_backpressure_and_terminal_closure()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::{io::Write, os::unix::net::UnixStream};

        let (reader, mut writer) = UnixStream::pair()?;
        let mut transport = JsonlTransport::start(Box::new(reader), 1)?;
        let notifier = HarnessEventNotifier::default();
        transport.register_event_notifier(&notifier)?;
        assert!(notifier.wait(Some(Duration::ZERO))?);

        writer.write_all(b"{\"id\":1,\"result\":{}}\n{\"id\":2,\"result\":{}}\n")?;
        assert!(notifier.wait(Some(Duration::from_secs(1)))?);
        assert!(matches!(transport.try_receive(), TransportRead::Message(_)));
        assert!(notifier.wait(Some(Duration::from_secs(1)))?);
        assert!(matches!(transport.try_receive(), TransportRead::Message(_)));

        drop(writer);
        assert!(notifier.wait(Some(Duration::from_secs(1)))?);
        assert!(matches!(transport.try_receive(), TransportRead::Closed));
        transport.join_reader()?;
        Ok(())
    }
}
