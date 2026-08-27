use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    num::NonZeroU64,
};

use hq_domain::{
    CommandId, DispatchId, FactId, InstallationId, MailboxId, MessageId, OperationId, ProviderId,
    ProviderSessionId, ShortText, Timestamp,
};

use crate::CausalGraph;

/// Presentation family rank; declaration order puts messages before activity on an exact tie.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PresentationFamily {
    /// Actionable or conversational message content.
    Message,
    /// Non-actionable harness activity.
    Activity,
}

/// Typed item or request correlation used by the canonical ready key.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PresentationItemId {
    /// Conversation message identity.
    Message(MessageId),
    /// Remote command identity.
    Command(CommandId),
    /// Project dispatch identity.
    Dispatch(DispatchId),
    /// Semantic fact identity when no narrower public identifier exists.
    Fact(FactId),
}

/// Typed stable public identity used after all semantic correlation fields.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PresentationPublicId {
    /// Conversation message identity.
    Message(MessageId),
    /// Remote command identity.
    Command(CommandId),
    /// Project dispatch identity.
    Dispatch(DispatchId),
}

/// Exact reducer-owned Kahn ready key for messages and activity.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PresentationKey {
    authored_at: Timestamp,
    presentation_occurrence: Timestamp,
    family: PresentationFamily,
    source_installation: InstallationId,
    source_mailbox: Option<MailboxId>,
    provider: Option<ProviderId>,
    session: Option<ProviderSessionId>,
    operation: Option<OperationId>,
    item_or_request: Option<PresentationItemId>,
    runtime: Option<ShortText>,
    source_sequence: Option<NonZeroU64>,
    stable_public_id: Option<PresentationPublicId>,
}

impl PresentationKey {
    /// Creates the complete canonical ready key from explicit semantic fields.
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        authored_at: Timestamp,
        presentation_occurrence: Timestamp,
        family: PresentationFamily,
        source_installation: InstallationId,
        source_mailbox: Option<MailboxId>,
        provider: Option<ProviderId>,
        session: Option<ProviderSessionId>,
        operation: Option<OperationId>,
        item_or_request: Option<PresentationItemId>,
        runtime: Option<ShortText>,
        source_sequence: Option<NonZeroU64>,
        stable_public_id: Option<PresentationPublicId>,
    ) -> Self {
        Self {
            authored_at,
            presentation_occurrence,
            family,
            source_installation,
            source_mailbox,
            provider,
            session,
            operation,
            item_or_request,
            runtime,
            source_sequence,
            stable_public_id,
        }
    }

    /// Creates a key with empty correlations, primarily for causal framework consumers and tests.
    pub const fn minimal(authored_at: Timestamp, family: PresentationFamily) -> Self {
        Self::new(
            authored_at,
            authored_at,
            family,
            InstallationId::from_bytes([0; 32]),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
    }
}

/// One selected fact and its semantic presentation ready key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationEntry {
    fact_id: FactId,
    key: PresentationKey,
}

impl PresentationEntry {
    /// Creates a selected presentation entry.
    pub const fn new(fact_id: FactId, key: PresentationKey) -> Self {
        Self { fact_id, key }
    }

    /// Returns the supporting fact identity.
    pub const fn fact_id(&self) -> FactId {
        self.fact_id
    }

    /// Returns the exact semantic ready key.
    pub const fn key(&self) -> &PresentationKey {
        &self.key
    }
}

/// Invalid selected input to the canonical presentation traversal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentationError {
    /// One fact identity appeared more than once in the selected set.
    DuplicateEntry(FactId),
    /// A selected identity was not a vertex in the supplied causal graph.
    UnknownFact(FactId),
    /// The selected induced graph retained a causal cycle.
    CausalCycle,
}

impl fmt::Display for PresentationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateEntry(_) => formatter.write_str("duplicate presentation entry"),
            Self::UnknownFact(_) => formatter.write_str("presentation entry is absent from graph"),
            Self::CausalCycle => {
                formatter.write_str("presentation selection contains a causal cycle")
            }
        }
    }
}

impl Error for PresentationError {}

/// Orders selected entries with a deterministic Kahn traversal and the one canonical ready key.
pub fn canonical_presentation_order(
    graph: &CausalGraph,
    entries: impl IntoIterator<Item = PresentationEntry>,
) -> Result<Vec<FactId>, PresentationError> {
    let mut selected = BTreeMap::new();
    for entry in entries {
        let fact_id = entry.fact_id;
        if !graph.vertices().contains(&fact_id) {
            return Err(PresentationError::UnknownFact(fact_id));
        }
        if selected.insert(fact_id, entry.key).is_some() {
            return Err(PresentationError::DuplicateEntry(fact_id));
        }
    }

    let selected_ids = selected.keys().copied().collect::<BTreeSet<_>>();
    let mut indegree = selected_ids
        .iter()
        .copied()
        .map(|fact_id| {
            let degree = graph.parents(fact_id).intersection(&selected_ids).count();
            (fact_id, degree)
        })
        .collect::<BTreeMap<_, _>>();
    let mut ready = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(fact_id, _)| (selected[fact_id].clone(), *fact_id))
        .collect::<BTreeSet<_>>();
    let mut ordered = Vec::with_capacity(selected.len());

    while let Some((_, fact_id)) = ready.pop_first() {
        ordered.push(fact_id);
        for child in graph.children(fact_id).intersection(&selected_ids) {
            if let Some(degree) = indegree.get_mut(child) {
                *degree -= 1;
                if *degree == 0 {
                    ready.insert((selected[child].clone(), *child));
                }
            }
        }
    }

    if ordered.len() == selected.len() {
        Ok(ordered)
    } else {
        Err(PresentationError::CausalCycle)
    }
}
