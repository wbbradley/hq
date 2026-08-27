//! Representation-independent application query values.

use std::{
    borrow::Borrow,
    collections::{BTreeMap, BTreeSet},
};

use hq_domain::{FactId, Revision};
use hq_reducer::{
    ActivityView, AgentAggregateKey, AgentProjection, AgentProjectionKey, AgentReport,
    AuthorityAggregateKey, AuthorityProjection, AuthorityProjectionKey, AuthorityReport,
    ConversationAggregateKey, ConversationProjection, ConversationProjectionKey,
    ConversationReport, MessageView, ProjectAggregateKey, ProjectProjection, ProjectProjectionKey,
    ProjectReport,
};

/// One normalized projection package independent of persistence layout and transport encoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectionSnapshot<A, K, V> {
    frontiers: BTreeMap<A, BTreeSet<FactId>>,
    projections: BTreeMap<K, V>,
    support: BTreeMap<K, BTreeSet<FactId>>,
}

impl<A, K, V> ProjectionSnapshot<A, K, V> {
    /// Constructs a snapshot from already normalized reducer-owned collections.
    pub const fn new(
        frontiers: BTreeMap<A, BTreeSet<FactId>>,
        projections: BTreeMap<K, V>,
        support: BTreeMap<K, BTreeSet<FactId>>,
    ) -> Self {
        Self {
            frontiers,
            projections,
            support,
        }
    }

    /// Returns every exact usable causal maximum by typed aggregate.
    pub const fn frontiers(&self) -> &BTreeMap<A, BTreeSet<FactId>> {
        &self.frontiers
    }

    /// Returns every typed projection.
    pub const fn projections(&self) -> &BTreeMap<K, V> {
        &self.projections
    }

    /// Returns one typed projection.
    pub fn projection<Q>(&self, key: Q) -> Option<&V>
    where
        K: Ord,
        Q: Borrow<K>,
    {
        self.projections.get(key.borrow())
    }

    /// Returns transitive usable support for every projection.
    pub const fn support(&self) -> &BTreeMap<K, BTreeSet<FactId>> {
        &self.support
    }
}

/// Full rebuildable authority view.
pub type AuthorityProjectionSnapshot =
    ProjectionSnapshot<AuthorityAggregateKey, AuthorityProjectionKey, AuthorityProjection>;
/// Full rebuildable conversation and activity view.
pub type ConversationProjectionSnapshot =
    ProjectionSnapshot<ConversationAggregateKey, ConversationProjectionKey, ConversationProjection>;
/// Full rebuildable named-agent view.
pub type AgentProjectionSnapshot =
    ProjectionSnapshot<AgentAggregateKey, AgentProjectionKey, AgentProjection>;
/// Full rebuildable project view.
pub type ProjectProjectionSnapshot =
    ProjectionSnapshot<ProjectAggregateKey, ProjectProjectionKey, ProjectProjection>;

/// All authoritative application projection packages from one serialized state point.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainSnapshot {
    authority: AuthorityProjectionSnapshot,
    conversation: ConversationProjectionSnapshot,
    agent: AgentProjectionSnapshot,
    project: ProjectProjectionSnapshot,
}

impl DomainSnapshot {
    /// Constructs the complete application snapshot from normalized packages.
    pub const fn new(
        authority: AuthorityProjectionSnapshot,
        conversation: ConversationProjectionSnapshot,
        agent: AgentProjectionSnapshot,
        project: ProjectProjectionSnapshot,
    ) -> Self {
        Self {
            authority,
            conversation,
            agent,
            project,
        }
    }

    /// Constructs an empty snapshot for bootstrapping and scripted adapters.
    pub fn empty() -> Self {
        Self::new(
            ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
            ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
            ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
            ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
        )
    }

    /// Derives application projections from fresh complete reducer reports.
    pub fn from_reports(
        authority: &AuthorityReport,
        conversation: &ConversationReport,
        agent: &AgentReport,
        project: &ProjectReport,
    ) -> Self {
        Self::new(
            ProjectionSnapshot::new(
                authority.frontiers().clone(),
                authority.projections().clone(),
                authority.support().clone(),
            ),
            ProjectionSnapshot::new(
                conversation.frontiers().clone(),
                conversation.projections().clone(),
                conversation.support().clone(),
            ),
            ProjectionSnapshot::new(
                agent.frontiers().clone(),
                agent.projections().clone(),
                agent.support().clone(),
            ),
            ProjectionSnapshot::new(
                project.frontiers().clone(),
                project.projections().clone(),
                project.support().clone(),
            ),
        )
    }

    /// Returns the authority package.
    pub const fn authority(&self) -> &AuthorityProjectionSnapshot {
        &self.authority
    }

    /// Returns the conversation and activity package.
    pub const fn conversation(&self) -> &ConversationProjectionSnapshot {
        &self.conversation
    }

    /// Returns the named-agent package.
    pub const fn agent(&self) -> &AgentProjectionSnapshot {
        &self.agent
    }

    /// Returns the project package.
    pub const fn project(&self) -> &ProjectProjectionSnapshot {
        &self.project
    }
}

impl Default for DomainSnapshot {
    fn default() -> Self {
        Self::empty()
    }
}

/// An authoritative snapshot paired with its monotonic local revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthoritativeSnapshot {
    revision: Revision,
    domain: DomainSnapshot,
}

impl AuthoritativeSnapshot {
    /// Constructs one revisioned authoritative view.
    pub const fn new(revision: Revision, domain: DomainSnapshot) -> Self {
        Self { revision, domain }
    }

    /// Returns the serialized state revision.
    pub const fn revision(&self) -> Revision {
        self.revision
    }

    /// Returns all normalized domain projection packages.
    pub const fn domain(&self) -> &DomainSnapshot {
        &self.domain
    }
}

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
