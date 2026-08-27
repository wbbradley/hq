//! Typed reducer-ordered conversation page values.

use hq_domain::FactId;
use hq_reducer::{ActivityView, MessageView};

/// One actionable message or non-actionable activity in canonical conversation order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConversationEntry {
    /// Typed projected message state.
    Message(Box<MessageView>),
    /// Typed selected or durable activity value.
    Activity(ActivityView),
}

impl ConversationEntry {
    /// Returns the stable canonical fact identity anchoring this entry.
    pub const fn fact_id(&self) -> FactId {
        match self {
            Self::Message(message) => message.fact_id,
            Self::Activity(activity) => activity.fact_id,
        }
    }
}
