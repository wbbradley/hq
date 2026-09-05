//! Stable conversation identities shared across domain boundaries.

use crate::{MailboxAddress, ProjectId, ProviderId, ProviderSessionId, ThreadId};

/// Closed identity of one immutable conversation transcript.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ConversationId {
    /// One independently initiated project exchange.
    ProjectThread {
        /// Stable project receiving the initiating input.
        project_id: ProjectId,
        /// Stable causal thread shared by input, output, and activity.
        thread: ThreadId,
    },
    /// One uncorrelated causal thread with an exact counterparty mailbox.
    Thread {
        /// Installation-qualified mailbox at the other side of the local human view.
        counterparty: MailboxAddress,
        /// Stable causal thread identity.
        thread: ThreadId,
    },
    /// One provider-scoped durable session with an exact source/counterparty mailbox.
    ProviderSession {
        /// Installation-qualified mailbox associated with the session.
        counterparty: MailboxAddress,
        /// Provider namespace.
        provider: ProviderId,
        /// Provider-scoped session identity.
        session: ProviderSessionId,
    },
}
