//! Independent, bounded control RPCs sharing the sole provider reader and frame writer.

use std::{
    collections::BTreeMap,
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError, SyncSender},
    },
    time::Duration,
};

use hq_harness::{HarnessError, HarnessErrorClass};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{protocol::WireMessage, transport::MAX_CODEX_FRAME_BYTES};

const MAX_CONTROL_REQUESTS: usize = 8;
const CONTROL_ID_PREFIX: &str = "hq-control-";

#[derive(Default)]
pub(crate) struct FrameWriter(Mutex<Option<Box<dyn Write + Send>>>);

impl FrameWriter {
    pub(crate) fn bind(&self, input: Box<dyn Write + Send>) -> Result<(), HarnessError> {
        *self.0.lock().map_err(|_| unavailable())? = Some(input);
        Ok(())
    }

    pub(crate) fn write<T: Serialize>(&self, value: &T) -> Result<(), HarnessError> {
        let mut encoded = serde_json::to_vec(value)
            .map_err(|_| HarnessError::new(HarnessErrorClass::InvalidInput))?;
        if encoded.is_empty() || encoded.len() > MAX_CODEX_FRAME_BYTES {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        }
        encoded.push(b'\n');
        let mut input = self.0.lock().map_err(|_| unavailable())?;
        let input = input
            .as_mut()
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::IntakeClosed))?;
        input
            .write_all(&encoded)
            .and_then(|()| input.flush())
            .map_err(|_| unavailable())
    }

    pub(crate) fn close(&self) {
        if let Ok(mut input) = self.0.lock() {
            input.take();
        }
    }
}

#[derive(Default)]
struct PendingControls {
    closed: bool,
    responses: BTreeMap<String, SyncSender<WireMessage>>,
}

#[derive(Default)]
pub(crate) struct ControlResponses {
    next_id: AtomicU64,
    pending: Mutex<PendingControls>,
}

impl ControlResponses {
    /// Routes only this transport's control namespace; late replies cannot reach ordinary RPCs.
    pub(crate) fn route(&self, message: WireMessage) -> Option<WireMessage> {
        let Some(id) = message
            .id
            .as_ref()
            .and_then(Value::as_str)
            .filter(|id| id.starts_with(CONTROL_ID_PREFIX))
            .filter(|_| message.method.is_none())
        else {
            return Some(message);
        };
        if let Ok(mut pending) = self.pending.lock()
            && let Some(response) = pending.responses.remove(id)
        {
            let _ = response.try_send(message);
        }
        None
    }

    pub(crate) fn close(&self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.closed = true;
            pending.responses.clear();
        }
    }
}

/// Closes response waiters on every reader exit, including malformed input.
pub(crate) struct ControlReaderGuard(pub(crate) Arc<ControlResponses>);

impl Drop for ControlReaderGuard {
    fn drop(&mut self) {
        self.0.close();
    }
}

#[derive(Clone)]
pub(crate) struct TransportControl {
    writer: Arc<FrameWriter>,
    responses: Arc<ControlResponses>,
}

impl TransportControl {
    pub(crate) const fn new(writer: Arc<FrameWriter>, responses: Arc<ControlResponses>) -> Self {
        Self { writer, responses }
    }

    pub(crate) fn respond(&self, id: &Value, result: &Value) -> Result<(), HarnessError> {
        self.writer.write(&json!({"id": id, "result": result}))
    }

    pub(crate) fn request<T: Serialize>(
        &self,
        method: &str,
        params: T,
        timeout: Duration,
    ) -> Result<WireMessage, HarnessError> {
        let serial = self
            .responses
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| unavailable())?;
        let id = format!("{CONTROL_ID_PREFIX}{serial}");
        let (sender, receiver) = mpsc::sync_channel(1);
        {
            let mut pending = self.responses.pending.lock().map_err(|_| unavailable())?;
            if pending.closed {
                return Err(HarnessError::new(HarnessErrorClass::TransportClosed));
            }
            if pending.responses.len() >= MAX_CONTROL_REQUESTS {
                return Err(HarnessError::new(HarnessErrorClass::Backpressure));
            }
            pending.responses.insert(id.clone(), sender);
        }
        let result = self
            .writer
            .write(&json!({"id": id, "method": method, "params": params}))
            .and_then(|()| {
                receiver.recv_timeout(timeout).map_err(|error| {
                    HarnessError::new(match error {
                        RecvTimeoutError::Timeout => HarnessErrorClass::Unavailable,
                        RecvTimeoutError::Disconnected => HarnessErrorClass::TransportClosed,
                    })
                })
            });
        if let Ok(mut pending) = self.responses.pending.lock() {
            pending.responses.remove(&id);
        }
        result
    }
}

fn unavailable() -> HarnessError {
    HarnessError::new(HarnessErrorClass::Unavailable)
}
