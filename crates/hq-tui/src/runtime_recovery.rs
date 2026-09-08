//! Passive runtime recovery presentation, separate from connectivity and message acceptance.

/// Current worker availability derived from exact runtime evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiRuntimeAvailability {
    /// No current worker.
    Stopped,
    /// A current-generation recovery attempt has begun.
    Starting,
    /// A later recovery attempt is scheduled.
    Waiting,
    /// Current worker owns its lease.
    Ready,
    /// Current worker has a running operation.
    Working,
    /// Recovery requires intervention.
    Blocked,
    /// Current liveness could not be observed.
    Checking,
}

impl UiRuntimeAvailability {
    /// Ordinary status wording without internal identifiers.
    pub const fn text(self) -> &'static str {
        match self {
            Self::Stopped => "Agent is stopped",
            Self::Starting => "Agent is restarting",
            Self::Waiting => "Waiting to restart",
            Self::Ready => "Agent is ready",
            Self::Working => "Agent is working",
            Self::Blocked => "Agent needs attention",
            Self::Checking => "Checking agent status",
        }
    }
}

/// Passive result for the exact project conversation being viewed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiRuntimeRecovery {
    /// Stable project identity.
    pub project_id: [u8; 32],
    /// Exact conversation to which this observation applies.
    pub thread_id: [u8; 32],
    /// Current availability, independent of device connectivity.
    pub availability: UiRuntimeAvailability,
    /// Name resolved from the exact observed worker identity.
    pub agent_name: Option<String>,
    /// Exact canonical pending input still exists; this does not imply provider acceptance.
    pub input_saved: bool,
    /// Exact diagnostic evidence for the details view.
    pub details: Vec<(String, String)>,
    /// Exact authorized action, absent unless the displayed revision permits retry.
    pub retry: Option<UiRuntimeRetryTarget>,
}

/// Exact authority and blocked revision offered by the recovery reader.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct UiRuntimeRetryTarget {
    pub account_id: [u8; 32],
    pub home: [u8; 32],
    pub project_id: [u8; 32],
    pub assignment_id: [u8; 32],
    pub agent_id: [u8; 32],
    pub provider: String,
    pub session: String,
    pub thread_id: [u8; 32],
    pub operation_id: [u8; 32],
    pub expected_revision: u64,
}

/// Result of the same explicit retry intent, separate from runtime success.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiRuntimeRetryOutcome {
    /// The exact retry has been durably scheduled.
    Scheduled,
    /// A definite rejection; a new current observation is required.
    Rejected(String),
    /// Response was not established; reconcile this same intent.
    Unknown,
}

impl UiRuntimeRecovery {
    /// Names the observed worker without conflating runtime status and input acceptance.
    pub fn status_text(&self) -> String {
        let name = self.agent_name.as_deref().unwrap_or("Agent");
        match self.availability {
            UiRuntimeAvailability::Stopped => format!("{name} is stopped"),
            UiRuntimeAvailability::Starting => format!("{name} is restarting"),
            UiRuntimeAvailability::Waiting => format!("{name} is waiting to restart"),
            UiRuntimeAvailability::Ready => format!("{name} is ready"),
            UiRuntimeAvailability::Working => format!("{name} is working"),
            UiRuntimeAvailability::Blocked => format!("{name} needs attention"),
            UiRuntimeAvailability::Checking => format!("Checking {name}'s status"),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn named_runtime_status_does_not_claim_provider_acceptance() {
        let recovery = super::UiRuntimeRecovery {
            project_id: [1; 32],
            thread_id: [2; 32],
            agent_name: Some("Alice".to_owned()),
            input_saved: true,
            availability: super::UiRuntimeAvailability::Starting,
            details: Vec::new(),
            retry: None,
        };
        assert_eq!(recovery.status_text(), "Alice is restarting");
        assert!(recovery.input_saved);
    }
}
