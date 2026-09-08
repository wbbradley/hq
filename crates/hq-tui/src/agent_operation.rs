//! Exact recipient controls and cancellation presentation, independent of transport ownership.

use std::num::NonZeroU64;

use crate::{UiActivityStatus, UiConversationId};

/// Actor and conversation evidence retained verbatim for request replay.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiAgentOperationQuery {
    pub account_id: [u8; 32],
    pub home: [u8; 32],
    pub conversation: UiConversationId,
    pub tracked_operation: Option<[u8; 32]>,
}

/// Exact project assignment, absent for direct agent conversations.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiAgentOperationProject {
    pub project_id: [u8; 32],
    pub assignment_id: [u8; 32],
    pub thread_id: [u8; 32],
}

/// Canonical recipient binding, independent of provider wire methods or display names.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiAgentOperationScope {
    pub agent_id: [u8; 32],
    pub mailbox_installation: [u8; 32],
    pub mailbox_id: [u8; 32],
    pub provider: String,
    pub session: String,
    pub project: Option<UiAgentOperationProject>,
}

/// Complete authorized operation target supplied by the daemon's capability reader.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiAgentOperationTarget {
    pub query: UiAgentOperationQuery,
    pub scope: UiAgentOperationScope,
    pub generation: [u8; 32],
    pub owner: [u8; 32],
    pub operation_id: [u8; 32],
    pub sequence: NonZeroU64,
}

/// Canonical work status, separate from acknowledgement of the stop request.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiAgentOperationStatus {
    pub operation_id: [u8; 32],
    pub sequence: NonZeroU64,
    pub status: UiActivityStatus,
}

/// Current capability and evidence for an explicitly tracked prior operation.
#[allow(missing_docs)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UiAgentOperationView {
    pub target: Option<UiAgentOperationTarget>,
    pub tracked: Option<UiAgentOperationStatus>,
}

/// Stable pure-model intent; the shell binds its identity to one exact wire request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiAgentCancellationIntent {
    /// Unique model effect identity at initial submission, preserved across retries.
    pub id: NonZeroU64,
    /// Immutable complete target, including the original observation query.
    pub target: UiAgentOperationTarget,
}

/// Delivery progress without a claim about the final work result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiAgentCancellationOutcome {
    /// Admitted to the daemon's independent control queue.
    Queued,
    /// Provider acknowledged the interrupt, but work may still be stopping.
    Requested,
    /// Request found no remaining work; canonical evidence determines how it ended.
    AlreadyFinished,
    /// Definite refusal; refresh capabilities before offering a new intent.
    Rejected,
    /// Delivery is unknown; only the exact prior intent can be retried.
    Uncertain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Cancellation {
    intent: UiAgentCancellationIntent,
    outcome: Option<UiAgentCancellationOutcome>,
    terminal: Option<UiAgentOperationStatus>,
    latest_sequence: NonZeroU64,
}

/// Conversation-scoped control presentation. Correlate asynchronous effects before applying reads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiAgentOperationControl {
    conversation: UiConversationId,
    connected: bool,
    available: Option<UiAgentOperationTarget>,
    cancellation: Option<Cancellation>,
}

impl UiAgentOperationControl {
    /// Starts without assuming any recipient capability.
    pub const fn new(conversation: UiConversationId) -> Self {
        Self {
            conversation,
            connected: false,
            available: None,
            cancellation: None,
        }
    }

    /// Exact conversation to which reads and actions must be correlated.
    pub const fn conversation(&self) -> &UiConversationId {
        &self.conversation
    }

    /// Disables actions on disconnect while retaining uncertain request identity.
    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
        if !connected {
            self.available = None;
        }
    }

    /// Returns the prior operation to include in subsequent canonical status queries.
    pub fn tracked_operation(&self) -> Option<[u8; 32]> {
        self.cancellation
            .as_ref()
            .map(|value| value.intent.target.operation_id)
    }

    /// Retained intent for passive acknowledgement observation, never implicit resubmission.
    pub fn pending_intent(&self) -> Option<&UiAgentCancellationIntent> {
        self.cancellation
            .as_ref()
            .filter(|value| {
                value.terminal.is_none()
                    && matches!(
                        value.outcome,
                        None | Some(
                            UiAgentCancellationOutcome::Queued
                                | UiAgentCancellationOutcome::Uncertain
                        )
                    )
            })
            .map(|value| &value.intent)
    }

    /// Applies a correlated authoritative read, never deriving a capability from activity text.
    pub fn observe(&mut self, view: UiAgentOperationView) {
        self.available = view.target.filter(|target| {
            self.connected
                && target.query.conversation == self.conversation
                && !matches!(self.conversation, UiConversationId::Thread { .. })
        });
        if let Some(cancellation) = &mut self.cancellation
            && cancellation.terminal.is_none()
            && let Some(status) = view.tracked
            && status.operation_id == cancellation.intent.target.operation_id
            && status.sequence > cancellation.latest_sequence
        {
            cancellation.latest_sequence = status.sequence;
            if matches!(
                status.status,
                UiActivityStatus::Succeeded
                    | UiActivityStatus::Failed { .. }
                    | UiActivityStatus::Interrupted
            ) {
                cancellation.terminal = Some(status);
            }
        }
        if let Some(cancellation) = &self.cancellation
            && cancellation.terminal.is_some()
            && self.available.as_ref().is_some_and(|target| {
                target.operation_id == cancellation.intent.target.operation_id
            })
        {
            self.available = None;
        }
    }

    /// Whether the focused surface can offer an explicit stop or exact-request retry.
    pub fn can_cancel(&self) -> bool {
        if !self.connected {
            return false;
        }
        match &self.cancellation {
            Some(value) if value.terminal.is_none() => match value.outcome {
                Some(UiAgentCancellationOutcome::Uncertain) => true,
                Some(UiAgentCancellationOutcome::Rejected) => self.available.is_some(),
                _ => false,
            },
            _ => self.available.is_some(),
        }
    }

    /// Whether the next explicit stop must retry an uncertain original request.
    pub fn retry_is_uncertain(&self) -> bool {
        self.cancellation.as_ref().is_some_and(|value| {
            value.terminal.is_none() && value.outcome == Some(UiAgentCancellationOutcome::Uncertain)
        })
    }

    /// Begins one explicit action. An uncertain retry always returns the original identity/target.
    pub fn begin(&mut self, id: NonZeroU64) -> Option<UiAgentCancellationIntent> {
        if !self.can_cancel() {
            return None;
        }
        if let Some(value) = &mut self.cancellation
            && value.terminal.is_none()
            && value.outcome == Some(UiAgentCancellationOutcome::Uncertain)
        {
            value.outcome = None;
            return Some(value.intent.clone());
        }
        let intent = UiAgentCancellationIntent {
            id,
            target: self.available.take()?,
        };
        self.cancellation = Some(Cancellation {
            intent: intent.clone(),
            outcome: None,
            latest_sequence: intent.target.sequence,
            terminal: None,
        });
        Some(intent)
    }

    /// Applies only the matching intent's receipt. Terminal canonical evidence wins every race.
    pub fn acknowledge(&mut self, id: NonZeroU64, outcome: UiAgentCancellationOutcome) {
        if let Some(value) = &mut self.cancellation
            && value.intent.id == id
            && value.terminal.is_none()
            && !matches!(
                value.outcome,
                Some(
                    UiAgentCancellationOutcome::Requested
                        | UiAgentCancellationOutcome::AlreadyFinished
                )
            )
        {
            value.outcome = Some(outcome);
            if outcome == UiAgentCancellationOutcome::Rejected {
                self.available = None;
            }
        }
    }

    /// Ordinary status wording; an acknowledged interrupt never renders as completed work.
    pub fn notice(&self) -> Option<&'static str> {
        let value = self.cancellation.as_ref()?;
        if let Some(terminal) = &value.terminal {
            if self
                .available
                .as_ref()
                .is_some_and(|target| target.operation_id != terminal.operation_id)
            {
                return None;
            }
            return Some(match terminal.status {
                UiActivityStatus::Interrupted => "Agent stopped",
                UiActivityStatus::Succeeded => "Agent finished before it was stopped",
                UiActivityStatus::Failed { .. } => "Agent ended with an error",
                UiActivityStatus::Snapshot | UiActivityStatus::Running => return None,
            });
        }
        Some(match value.outcome {
            None | Some(UiAgentCancellationOutcome::Queued) => "Requesting agent stop…",
            Some(UiAgentCancellationOutcome::Requested) => "Stopping agent…",
            Some(UiAgentCancellationOutcome::AlreadyFinished) => "Checking how the agent finished…",
            Some(UiAgentCancellationOutcome::Rejected) => "Could not stop the agent",
            Some(UiAgentCancellationOutcome::Uncertain) => "Stop response unknown",
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::expect_used)]
    use super::*;

    fn sequence(value: u64) -> NonZeroU64 {
        NonZeroU64::new(value).expect("nonzero test sequence")
    }

    pub(crate) fn target() -> UiAgentOperationTarget {
        UiAgentOperationTarget {
            query: UiAgentOperationQuery {
                account_id: [1; 32],
                home: [2; 32],
                conversation: UiConversationId::ProviderSession {
                    counterparty_installation: [2; 32],
                    counterparty_mailbox: [3; 32],
                    provider: "provider".to_owned(),
                    session: "session".to_owned(),
                },
                tracked_operation: None,
            },
            scope: UiAgentOperationScope {
                agent_id: [4; 32],
                mailbox_installation: [2; 32],
                mailbox_id: [3; 32],
                provider: "provider".to_owned(),
                session: "session".to_owned(),
                project: None,
            },
            generation: [5; 32],
            owner: [6; 32],
            operation_id: [7; 32],
            sequence: sequence(10),
        }
    }

    fn ready() -> UiAgentOperationControl {
        let target = target();
        let mut control = UiAgentOperationControl::new(target.query.conversation.clone());
        control.set_connected(true);
        control.observe(UiAgentOperationView {
            target: Some(target),
            tracked: None,
        });
        control
    }

    fn terminal(
        operation_id: [u8; 32],
        value: u64,
        status: UiActivityStatus,
    ) -> UiAgentOperationView {
        UiAgentOperationView {
            target: None,
            tracked: Some(UiAgentOperationStatus {
                operation_id,
                sequence: sequence(value),
                status,
            }),
        }
    }

    #[test]
    fn terminal_evidence_cannot_go_behind_a_later_running_observation() {
        let mut control = ready();
        let intent = control.begin(sequence(1)).expect("intent");
        control.acknowledge(intent.id, UiAgentCancellationOutcome::Requested);
        control.observe(terminal([7; 32], 20, UiActivityStatus::Running));
        control.observe(terminal([7; 32], 15, UiActivityStatus::Interrupted));
        control.acknowledge(intent.id, UiAgentCancellationOutcome::Uncertain);
        assert_eq!(control.notice(), Some("Stopping agent…"));
        assert!(!control.can_cancel());
        control.observe(terminal([7; 32], 21, UiActivityStatus::Interrupted));
        assert_eq!(control.notice(), Some("Agent stopped"));
    }

    #[test]
    fn acknowledgement_waits_for_exact_canonical_terminal_evidence() {
        let mut control = ready();
        let intent = control.begin(sequence(1)).expect("supported control");
        control.acknowledge(intent.id, UiAgentCancellationOutcome::Requested);
        assert_eq!(control.notice(), Some("Stopping agent…"));
        assert!(!control.can_cancel());
        control.observe(terminal([8; 32], 11, UiActivityStatus::Interrupted));
        control.observe(terminal([7; 32], 9, UiActivityStatus::Interrupted));
        control.observe(terminal([7; 32], 10, UiActivityStatus::Interrupted));
        control.observe(terminal([7; 32], 11, UiActivityStatus::Snapshot));
        assert_eq!(control.notice(), Some("Stopping agent…"));
        control.observe(terminal([7; 32], 12, UiActivityStatus::Interrupted));
        assert_eq!(control.notice(), Some("Agent stopped"));
        control.acknowledge(intent.id, UiAgentCancellationOutcome::Uncertain);
        control.observe(terminal([7; 32], 13, UiActivityStatus::Running));
        assert_eq!(control.notice(), Some("Agent stopped"));
    }

    #[test]
    fn uncertain_retry_retains_the_complete_original_intent() {
        let mut control = ready();
        let original = control.begin(sequence(1)).expect("first intent");
        control.acknowledge(original.id, UiAgentCancellationOutcome::Uncertain);
        let mut newer = target();
        newer.operation_id = [8; 32];
        newer.query.tracked_operation = Some(original.target.operation_id);
        control.observe(UiAgentOperationView {
            target: Some(newer.clone()),
            tracked: None,
        });
        let retry = control.begin(sequence(2)).expect("retry");
        assert_eq!(retry, original);
        control.observe(terminal([7; 32], 11, UiActivityStatus::Succeeded));
        assert_eq!(
            control.notice(),
            Some("Agent finished before it was stopped")
        );
        control.observe(UiAgentOperationView {
            target: Some(newer.clone()),
            tracked: None,
        });
        assert_eq!(
            control.notice(),
            None,
            "do not label newly running work as stopped"
        );
        let next = control.begin(sequence(3)).expect("new turn");
        assert_eq!(next.target, newer);
        assert_eq!(next.id, sequence(3));
        control.acknowledge(original.id, UiAgentCancellationOutcome::Rejected);
        assert_eq!(control.notice(), Some("Requesting agent stop…"));
    }

    #[test]
    fn unavailable_or_different_conversation_cannot_offer_control() {
        let mut control = UiAgentOperationControl::new(target().query.conversation);
        assert!(control.begin(sequence(1)).is_none());
        control.set_connected(true);
        control.observe(UiAgentOperationView::default());
        assert!(!control.can_cancel());
        let mut wrong = target();
        wrong.query.conversation = UiConversationId::Thread {
            counterparty_installation: [2; 32],
            counterparty_mailbox: [3; 32],
            thread_id: [9; 32],
        };
        control.observe(UiAgentOperationView {
            target: Some(wrong),
            tracked: None,
        });
        assert!(!control.can_cancel());
    }

    #[test]
    fn disconnect_retains_uncertain_identity_but_disables_action() {
        let mut control = ready();
        let original = control.begin(sequence(1)).expect("intent");
        control.acknowledge(original.id, UiAgentCancellationOutcome::Uncertain);
        control.set_connected(false);
        assert!(control.begin(sequence(2)).is_none());
        assert_eq!(
            control.tracked_operation(),
            Some(original.target.operation_id)
        );
        control.set_connected(true);
        assert_eq!(control.begin(sequence(3)), Some(original));
    }

    #[test]
    fn already_finished_receipt_does_not_claim_interruption() {
        let mut control = ready();
        let intent = control.begin(sequence(1)).expect("intent");
        control.acknowledge(intent.id, UiAgentCancellationOutcome::AlreadyFinished);
        assert_eq!(control.notice(), Some("Checking how the agent finished…"));
        control.observe(terminal(
            [7; 32],
            11,
            UiActivityStatus::Failed {
                reason: "failure".to_owned(),
            },
        ));
        assert_eq!(control.notice(), Some("Agent ended with an error"));
        control.observe(UiAgentOperationView {
            target: Some(target()),
            tracked: None,
        });
        assert!(
            !control.can_cancel(),
            "stale running view cannot revive terminal work"
        );
    }
}
