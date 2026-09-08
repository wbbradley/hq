//! Exact-operation cancellation state shared with the sole mutable session owner.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::protocol::TurnInterruptParams;
use hq_domain::OperationId;
use hq_harness::{
    HarnessCancellationOutcome, HarnessError, HarnessErrorClass, HarnessOperationControl,
};
use serde_json::Value;

use crate::transport_control::TransportControl;

#[derive(Clone, Eq, PartialEq)]
struct ActiveOperation {
    thread: String,
    turn: String,
    operation: OperationId,
}

#[derive(Default)]
struct ControlState {
    active: Option<ActiveOperation>,
    pending: Option<(OperationId, HarnessCancellationOutcome)>,
    closed: bool,
    in_flight: bool,
    submitting: Option<OperationId>,
    permissions: BTreeMap<String, PendingPermission>,
}

struct PendingPermission {
    operation: OperationId,
    wire_id: Value,
    cancelled: Value,
}

pub(crate) struct CodexOperationControl {
    transport: TransportControl,
    timeout: Duration,
    state: Mutex<ControlState>,
}

impl CodexOperationControl {
    pub(crate) fn new(transport: TransportControl, timeout: Duration) -> Self {
        Self {
            transport,
            timeout,
            state: Mutex::new(ControlState::default()),
        }
    }

    pub(crate) fn admit_submission(
        self: &Arc<Self>,
        operation: OperationId,
    ) -> Result<SubmissionGuard, HarnessError> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if state.closed {
            return Err(HarnessError::new(HarnessErrorClass::IntakeClosed));
        }
        if state.pending.is_some() || state.submitting.is_some() {
            return Err(HarnessError::new(HarnessErrorClass::Backpressure));
        }
        state.submitting = Some(operation);
        Ok(SubmissionGuard(Arc::clone(self)))
    }

    pub(crate) fn register_permission(
        &self,
        key: String,
        operation: OperationId,
        wire_id: Value,
        cancelled: Value,
    ) -> Result<bool, HarnessError> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if state.closed
            || state
                .pending
                .is_some_and(|(pending, _)| pending == operation)
        {
            drop(state);
            self.transport.respond(&wire_id, &cancelled)?;
            return Ok(false);
        }
        state.permissions.insert(
            key,
            PendingPermission {
                operation,
                wire_id,
                cancelled,
            },
        );
        Ok(true)
    }

    pub(crate) fn answer_permission(&self, key: &str, result: &Value) -> Result<(), HarnessError> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if state.closed
            || state.permissions.get(key).is_some_and(|permission| {
                state
                    .pending
                    .is_some_and(|(operation, _)| operation == permission.operation)
            })
        {
            return Err(HarnessError::new(
                HarnessErrorClass::InteractiveAlreadyAnswered,
            ));
        }
        let permission = state
            .permissions
            .remove(key)
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::InteractiveAlreadyAnswered))?;
        self.transport.respond(&permission.wire_id, result)
    }

    pub(crate) fn cancel_permission(&self, key: &str) -> Result<(), HarnessError> {
        let permission = self
            .state
            .lock()
            .map_err(|_| unavailable())?
            .permissions
            .remove(key);
        if let Some(permission) = permission {
            self.transport
                .respond(&permission.wire_id, &permission.cancelled)?;
        }
        Ok(())
    }

    fn cancel_permissions(&self, operation: OperationId) -> Result<(), HarnessError> {
        let keys = self
            .state
            .lock()
            .map_err(|_| unavailable())?
            .permissions
            .iter()
            .filter(|(_, permission)| permission.operation == operation)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in keys {
            self.cancel_permission(&key)?;
        }
        Ok(())
    }

    pub(crate) fn activate(&self, thread: &str, turn: &str, operation: OperationId) {
        if let Ok(mut state) = self.state.lock() {
            let active = ActiveOperation {
                thread: thread.to_owned(),
                turn: turn.to_owned(),
                operation,
            };
            if state.active.as_ref() != Some(&active) {
                state.pending = None;
                state.in_flight = false;
                state.active = Some(active);
            }
        }
    }

    pub(crate) fn finish(&self, operation: OperationId) {
        if let Ok(mut state) = self.state.lock()
            && state
                .active
                .as_ref()
                .is_some_and(|active| active.operation == operation)
        {
            state.active = None;
            state.pending = None;
            state.in_flight = false;
        }
    }

    pub(crate) fn pending(&self) -> Result<Option<OperationId>, HarnessError> {
        self.state
            .lock()
            .map(|state| state.pending.map(|(operation, _)| operation))
            .map_err(|_| unavailable())
    }

    pub(crate) fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
        }
    }
}

impl HarnessOperationControl for CodexOperationControl {
    fn cancel_operation(
        &self,
        operation_id: OperationId,
    ) -> Result<HarnessCancellationOutcome, HarnessError> {
        let target = {
            let mut state = self.state.lock().map_err(|_| unavailable())?;
            if state.closed {
                return Err(HarnessError::new(HarnessErrorClass::IntakeClosed));
            }
            if state
                .submitting
                .is_some_and(|operation| operation != operation_id)
            {
                return Ok(HarnessCancellationOutcome::Rejected(
                    HarnessErrorClass::Backpressure,
                ));
            }
            if let Some((pending, outcome)) = state.pending
                && pending == operation_id
                && (state.in_flight || outcome == HarnessCancellationOutcome::Requested)
            {
                return Ok(outcome);
            }
            let Some(target) = state
                .active
                .as_ref()
                .filter(|active| active.operation == operation_id)
                .cloned()
            else {
                return Ok(HarnessCancellationOutcome::AlreadyFinished);
            };
            state.in_flight = true;
            state.pending = Some((
                operation_id,
                HarnessCancellationOutcome::Uncertain(HarnessErrorClass::Unavailable),
            ));
            target
        };
        let permissions = self.cancel_permissions(operation_id);
        let outcome = match self.transport.request(
            "turn/interrupt",
            TurnInterruptParams {
                thread_id: &target.thread,
                turn_id: &target.turn,
            },
            self.timeout,
        ) {
            Ok(response) if response.error.is_some() => {
                HarnessCancellationOutcome::Rejected(HarnessErrorClass::Unavailable)
            }
            Ok(_) => HarnessCancellationOutcome::Requested,
            Err(error) => HarnessCancellationOutcome::Uncertain(error.class),
        };
        let outcome = match permissions {
            Ok(()) => outcome,
            Err(error) => HarnessCancellationOutcome::Uncertain(error.class),
        };
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if state.active.as_ref() == Some(&target) {
            state.in_flight = false;
            state.pending = if matches!(outcome, HarnessCancellationOutcome::Rejected(_)) {
                None
            } else {
                Some((operation_id, outcome))
            };
        }
        Ok(outcome)
    }
}

pub(crate) struct SubmissionGuard(Arc<CodexOperationControl>);

impl Drop for SubmissionGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.state.lock() {
            state.submitting = None;
        }
    }
}

fn unavailable() -> HarnessError {
    HarnessError::new(HarnessErrorClass::Unavailable)
}

#[cfg(test)]
mod tests {
    use std::{io::Cursor, sync::Arc, time::Duration};

    use hq_domain::OperationId;
    use hq_harness::{HarnessCancellationOutcome, HarnessErrorClass, HarnessOperationControl};

    use super::CodexOperationControl;
    use crate::transport::JsonlTransport;

    #[test]
    fn cancellation_cannot_cross_admission_of_a_different_submission()
    -> Result<(), Box<dyn std::error::Error>> {
        let transport = JsonlTransport::start(Box::new(Cursor::new(Vec::<u8>::new())), 8)?;
        let control = Arc::new(CodexOperationControl::new(
            transport.control(),
            Duration::from_secs(1),
        ));
        let old = OperationId::from_bytes([1; 32]);
        control.activate("thread", "turn", old);
        let guard = control.admit_submission(OperationId::from_bytes([2; 32]))?;
        assert_eq!(
            control.cancel_operation(old)?,
            HarnessCancellationOutcome::Rejected(HarnessErrorClass::Backpressure)
        );
        drop(guard);
        Ok(())
    }
}
