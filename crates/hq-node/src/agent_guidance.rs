//! Concise installed guidance for agents using HQ's local messaging control plane.

/// Closed installed guidance topics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentGuidanceTopic {
    /// Complete concise guidance.
    Overview,
    /// Ask, send, wait, and poll usage.
    Messaging,
    /// Safe retries and stable identity.
    Retry,
    /// Explicit synchronization behavior.
    Synchronization,
    /// At-least-once delivery and completion.
    Delivery,
    /// Dependency-incomplete causal history.
    Causality,
    /// Human-owned authority and lifecycle boundaries.
    Administration,
}

impl AgentGuidanceTopic {
    /// Parses one stable installed topic name.
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value {
            None => Some(Self::Overview),
            Some("messaging") => Some(Self::Messaging),
            Some("retry") => Some(Self::Retry),
            Some("synchronization" | "sync") => Some(Self::Synchronization),
            Some("delivery") => Some(Self::Delivery),
            Some("causality" | "causal") => Some(Self::Causality),
            Some("administration" | "admin") => Some(Self::Administration),
            _ => None,
        }
    }

    /// Returns the stable machine-readable topic label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Messaging => "messaging",
            Self::Retry => "retry",
            Self::Synchronization => "synchronization",
            Self::Delivery => "delivery",
            Self::Causality => "causality",
            Self::Administration => "administration",
        }
    }

    /// Returns concise installed guidance without transport or identity internals.
    pub const fn text(self) -> &'static str {
        match self {
            Self::Overview => {
                "HQ agent guidance\n\nUse `hq ask` when a reply is required, `hq send` for asynchronous delivery, `hq wait` for a known question, and `hq poll` for ready inbound work. Retry with the same displayed message identity when an outcome is unknown. Use `hq relay sync` to prompt synchronization; absence after sync can still mean causal history is incomplete. Ready delivery is at least once and may repeat a stable identity after interruption. Humans own installation identity, authority, agent creation, session selection, and retirement.\n"
            }
            Self::Messaging => {
                "Use `hq ask` for a question and `hq send` for asynchronous work. `hq wait MESSAGE_ID` resumes waiting for one exact question; `hq poll` returns currently ready inbound work without blocking. Provider/session discovery must be unambiguous.\n"
            }
            Self::Retry => {
                "Treat an unknown outcome as reconcilable, not failed. Preserve the exact message or operation identity when retrying; creating a new identity can duplicate work.\n"
            }
            Self::Synchronization => {
                "Use `hq relay sync` to prompt configured relays. Synchronization is a wake request, not proof that every causal parent is already present or that a remote agent is online.\n"
            }
            Self::Delivery => {
                "Ready delivery is at least once. HQ writes output before recording reversible completion, so interruption can repeat the same stable message identity but must not silently lose it.\n"
            }
            Self::Causality => {
                "A record marked incomplete is inert because required causal history is absent or unusable. Do not infer rejection, absence, or authority from it; synchronize and inspect again.\n"
            }
            Self::Administration => {
                "Humans own installation identity, peer and mailbox authority, named-agent creation, durable session selection, and retirement. Agents may use the messaging commands but must not guess or change administrative state.\n"
            }
        }
    }
}
