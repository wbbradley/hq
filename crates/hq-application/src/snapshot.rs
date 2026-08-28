//! Representation-independent application query values.

use std::{
    borrow::Borrow,
    collections::{BTreeMap, BTreeSet},
};

use hq_domain::{
    AccountId, AgentId, CommandDigest, CommandId, ContentText, DispatchId, EncryptionPublicKey,
    FactId, GrantId, InstallationId, MailboxAddress, MailboxKind, MessageId, ProjectId, ProviderId,
    ProviderSessionId, RemoteCommandResult, ResourceHealth, ResourceId, ResourceLocator, Revision,
    RuntimeObservation, ShortText, SigningPublicKey, Timestamp,
};
use hq_reducer::{
    ActivityView, AgentAggregateKey, AgentLifecycle, AgentProjection, AgentProjectionKey,
    AgentReport, AuthorityAggregateKey, AuthorityProjection, AuthorityProjectionKey,
    AuthorityReport, ConversationAggregateKey, ConversationProjection, ConversationProjectionKey,
    ConversationReport, MembershipState, MessageView, PeerRouteState, ProjectAggregateKey,
    ProjectLifecycle, ProjectOutputStatus, ProjectProjection, ProjectProjectionKey, ProjectReport,
    RemoteCommandStage,
};

use crate::ApplicationValueError;

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

    /// Derives the closed client-facing projection catalog without persistence or wire layouts.
    #[allow(clippy::too_many_lines)]
    pub fn client_projections(&self) -> Result<Vec<ClientProjection>, ApplicationValueError> {
        let mut items = Vec::new();
        let selected_accounts = self
            .authority
            .projections()
            .values()
            .filter_map(|projection| match projection {
                AuthorityProjection::AccountSelection { active, .. } => *active,
                AuthorityProjection::Installation(_)
                | AuthorityProjection::Mailbox(_)
                | AuthorityProjection::PeerRoute(_)
                | AuthorityProjection::MailboxCapability(_)
                | AuthorityProjection::Account { .. }
                | AuthorityProjection::Membership(_) => None,
            })
            .collect::<BTreeSet<_>>();

        for (key, projection) in self.authority.projections() {
            let item = match (key, projection) {
                (
                    AuthorityProjectionKey::Installation(installation_id),
                    AuthorityProjection::Installation(view),
                ) => ClientProjection::Installation {
                    installation_id: *installation_id,
                    signing_key: view.signing_key,
                    encryption_key: view.encryption_key,
                    label: view.label.clone(),
                },
                (AuthorityProjectionKey::Mailbox(address), AuthorityProjection::Mailbox(view)) => {
                    ClientProjection::Mailbox {
                        address: *address,
                        kind: view.kind,
                        label: view.label.clone(),
                    }
                }
                (
                    AuthorityProjectionKey::PeerRoute { owner, peer },
                    AuthorityProjection::PeerRoute(view),
                ) => ClientProjection::PeerRoute {
                    owner: *owner,
                    peer: *peer,
                    state: match view.state() {
                        PeerRouteState::Routable => ClientPeerRouteState::Routable,
                        PeerRouteState::Blocked => ClientPeerRouteState::Blocked,
                        PeerRouteState::Conflicted => ClientPeerRouteState::Conflicted,
                    },
                    frontier: view.frontier().clone(),
                },
                (
                    AuthorityProjectionKey::MailboxCapability(grant_id),
                    AuthorityProjection::MailboxCapability(view),
                ) => ClientProjection::MailboxCapability {
                    grant_id: *grant_id,
                    mailbox: view.mailbox,
                    grantee_installation: view.grantee.installation_id(),
                    active: view.is_active(),
                },
                (
                    AuthorityProjectionKey::Account(account_id),
                    AuthorityProjection::Account { creator, label, .. },
                ) => ClientProjection::Account {
                    account_id: *account_id,
                    creator_installation: creator.installation_id(),
                    label: label.clone(),
                    selected: selected_accounts.contains(account_id),
                },
                (
                    AuthorityProjectionKey::Membership { account, device },
                    AuthorityProjection::Membership(view),
                ) => ClientProjection::Membership {
                    account_id: *account,
                    device: *device,
                    state: match view.state() {
                        MembershipState::Pending => ClientMembershipState::Pending,
                        MembershipState::Active => ClientMembershipState::Active,
                        MembershipState::Revoked => ClientMembershipState::Revoked,
                    },
                    active_acceptances: view.active_acceptances.clone(),
                },
                (
                    AuthorityProjectionKey::AccountSelection(installation_id),
                    AuthorityProjection::AccountSelection { candidates, active },
                ) => ClientProjection::AccountSelection {
                    installation_id: *installation_id,
                    candidates: candidates.clone(),
                    active: *active,
                },
                _ => return Err(ApplicationValueError::InvalidEncoding),
            };
            items.push(item);
        }

        for projection in self.conversation.projections().values() {
            match projection {
                ConversationProjection::Thread(_)
                | ConversationProjection::Message(_)
                | ConversationProjection::ActionGroup(_)
                | ConversationProjection::Activity(_)
                | ConversationProjection::ActivityRetention(_) => {}
            }
        }

        for (key, projection) in self.agent.projections() {
            match (key, projection) {
                (AgentProjectionKey::Agent(agent_id), AgentProjection::Agent(view)) => {
                    items.push(ClientProjection::Agent {
                        agent_id: *agent_id,
                        names: view.names.clone(),
                        lifecycle: match view.lifecycle {
                            AgentLifecycle::Active => ClientAgentLifecycle::Active,
                            AgentLifecycle::Conflicted => ClientAgentLifecycle::Conflicted,
                            AgentLifecycle::Retired => ClientAgentLifecycle::Retired,
                        },
                        runnable: view.runnable,
                    });
                }
                (AgentProjectionKey::Session(session), AgentProjection::Session(view)) => {
                    items.push(ClientProjection::AgentSession {
                        provider: session.provider.clone(),
                        session: session.session.clone(),
                        mailbox: view.mailbox,
                        conflicted: view.conflicted,
                    });
                }
                (AgentProjectionKey::Selection(agent_id), AgentProjection::Selection(view)) => {
                    items.push(ClientProjection::AgentSelection {
                        agent_id: *agent_id,
                        selected: view.active.as_ref().map(|active| {
                            (
                                active.session.provider.clone(),
                                active.session.session.clone(),
                            )
                        }),
                        conflicted: view.conflicted,
                    });
                }
                (AgentProjectionKey::Rename { agent, session }, AgentProjection::Rename(view)) => {
                    items.push(ClientProjection::AgentSessionName {
                        agent_id: *agent,
                        provider: session.provider.clone(),
                        session: session.session.clone(),
                        resolved: view.resolved,
                        display_name: view.display_name.clone(),
                    });
                }
                (AgentProjectionKey::Name(_), AgentProjection::Name(_))
                | (AgentProjectionKey::Context(_), AgentProjection::Context(_))
                | (AgentProjectionKey::DirectSession { .. }, AgentProjection::DirectSession(_)) => {
                }
                _ => return Err(ApplicationValueError::InvalidEncoding),
            }
        }

        for (key, projection) in self.project.projections() {
            let item = match (key, projection) {
                (ProjectProjectionKey::Project(project_id), ProjectProjection::Project(view)) => {
                    items.push(ClientProjection::Project {
                        project_id: *project_id,
                        home: view.home,
                        name: view.name.clone(),
                        lifecycle: match view.lifecycle {
                            ProjectLifecycle::Open => ClientProjectLifecycle::Open,
                            ProjectLifecycle::Closing => ClientProjectLifecycle::Closing,
                            ProjectLifecycle::Closed => ClientProjectLifecycle::Closed,
                        },
                        archived: view.archived,
                        claimable: view.claimable,
                        head: view.head,
                        input_sequence: view.input_sequence,
                    });
                    for (resource_id, resource) in &view.resources {
                        items.push(ClientProjection::ProjectResource {
                            project_id: *project_id,
                            resource_id: *resource_id,
                            display_locator: resource.display_locator.clone(),
                            canonical_locator: resource.canonical_locator.clone(),
                            health: resource.health,
                            primary: view.primary == Some(*resource_id),
                            active_claim: view.active_claims.contains(resource_id),
                            conflicting_projects: view
                                .claim_conflicts
                                .get(resource_id)
                                .cloned()
                                .unwrap_or_default(),
                        });
                    }
                    continue;
                }
                (ProjectProjectionKey::Input(_), ProjectProjection::Input(view)) => {
                    ClientProjection::ProjectInput {
                        project_id: view.project_id,
                        message_id: view.message_id,
                        sequence: view.sequence,
                        accepted_fact: view.accepted_fact,
                    }
                }
                (ProjectProjectionKey::Dispatch(_), ProjectProjection::Dispatch(view)) => {
                    ClientProjection::ProjectDispatch {
                        dispatch_id: view.dispatch_id,
                        message_id: view.message_id,
                        sequence: view.sequence,
                        fact_id: view.fact_id,
                        conflicted: view.conflicted,
                    }
                }
                (ProjectProjectionKey::Output(_), ProjectProjection::Output(view)) => {
                    ClientProjection::ProjectOutput {
                        output_id: view.output_id,
                        dispatch_id: view.dispatch_id,
                        status: match view.status {
                            ProjectOutputStatus::Current => ClientProjectOutputStatus::Current,
                            ProjectOutputStatus::LateFromInactive => {
                                ClientProjectOutputStatus::LateFromInactive
                            }
                            ProjectOutputStatus::Conflicted => {
                                ClientProjectOutputStatus::Conflicted
                            }
                        },
                        content: view.message.body.clone(),
                    }
                }
                (ProjectProjectionKey::Command(command_id), ProjectProjection::Command(view)) => {
                    ClientProjection::RemoteCommand {
                        command_id: *command_id,
                        request_digest: view.digest,
                        account_id: view.account_id,
                        project_id: view.project_id,
                        target_home: view.target_home,
                        expected_head: view.expected_head,
                        operation: view.operation.clone(),
                        body: view.body.clone(),
                        issued_at: view.issued_at,
                        request_fact: view.request_fact,
                        stage: Box::new(match &view.stage {
                            RemoteCommandStage::Queued => ClientRemoteCommandStage::Queued,
                            RemoteCommandStage::Received {
                                receipt_fact,
                                received_head,
                                received_at,
                            } => ClientRemoteCommandStage::Received {
                                receipt_fact: *receipt_fact,
                                received_head: *received_head,
                                received_at: *received_at,
                            },
                            RemoteCommandStage::Terminal {
                                receipt_fact,
                                received_head,
                                received_at,
                                outcome_fact,
                                result,
                                runtime,
                            } => ClientRemoteCommandStage::Terminal {
                                receipt_fact: *receipt_fact,
                                received_head: *received_head,
                                received_at: *received_at,
                                outcome_fact: *outcome_fact,
                                result: result.clone(),
                                runtime: runtime.clone(),
                            },
                            RemoteCommandStage::Conflicted => ClientRemoteCommandStage::Conflicted,
                        }),
                    }
                }
                _ => return Err(ApplicationValueError::InvalidEncoding),
            };
            items.push(item);
        }
        Ok(items)
    }
}

/// Stable client presentation of a peer-route register.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientPeerRouteState {
    Routable,
    Blocked,
    Conflicted,
}
/// Stable client presentation of account membership.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientMembershipState {
    Pending,
    Active,
    Revoked,
}
/// Stable client presentation of named-agent lifecycle.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientAgentLifecycle {
    Active,
    Conflicted,
    Retired,
}
/// Stable client presentation of project lifecycle.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientProjectLifecycle {
    Open,
    Closing,
    Closed,
}
/// Stable client presentation of project output provenance.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientProjectOutputStatus {
    Current,
    LateFromInactive,
    Conflicted,
}
/// Stable client presentation of remote-command progress.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientRemoteCommandStage {
    Queued,
    Received {
        receipt_fact: FactId,
        received_head: FactId,
        received_at: Timestamp,
    },
    Terminal {
        receipt_fact: FactId,
        received_head: FactId,
        received_at: Timestamp,
        outcome_fact: FactId,
        result: RemoteCommandResult,
        runtime: Option<RuntimeObservation>,
    },
    Conflicted,
}

/// Closed, representation-independent projection catalog consumed by local clients.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientProjection {
    Installation {
        installation_id: InstallationId,
        signing_key: SigningPublicKey,
        encryption_key: EncryptionPublicKey,
        label: Option<ShortText>,
    },
    Mailbox {
        address: MailboxAddress,
        kind: MailboxKind,
        label: Option<ShortText>,
    },
    Account {
        account_id: AccountId,
        creator_installation: InstallationId,
        label: Option<ShortText>,
        selected: bool,
    },
    PeerRoute {
        owner: InstallationId,
        peer: InstallationId,
        state: ClientPeerRouteState,
        frontier: BTreeSet<FactId>,
    },
    MailboxCapability {
        grant_id: GrantId,
        mailbox: MailboxAddress,
        grantee_installation: InstallationId,
        active: bool,
    },
    Membership {
        account_id: AccountId,
        device: InstallationId,
        state: ClientMembershipState,
        active_acceptances: BTreeSet<FactId>,
    },
    AccountSelection {
        installation_id: InstallationId,
        candidates: BTreeSet<AccountId>,
        active: Option<AccountId>,
    },
    Conversation {
        key: hq_reducer::ConversationKey,
        latest_fact: Option<FactId>,
        open_messages: u32,
    },
    Agent {
        agent_id: AgentId,
        names: BTreeSet<ShortText>,
        lifecycle: ClientAgentLifecycle,
        runnable: bool,
    },
    AgentSession {
        provider: ProviderId,
        session: ProviderSessionId,
        mailbox: Option<MailboxAddress>,
        conflicted: bool,
    },
    AgentSelection {
        agent_id: AgentId,
        selected: Option<(ProviderId, ProviderSessionId)>,
        conflicted: bool,
    },
    AgentSessionName {
        agent_id: AgentId,
        provider: ProviderId,
        session: ProviderSessionId,
        resolved: bool,
        display_name: Option<ShortText>,
    },
    Project {
        project_id: ProjectId,
        home: InstallationId,
        name: ShortText,
        lifecycle: ClientProjectLifecycle,
        archived: bool,
        claimable: bool,
        head: FactId,
        input_sequence: u64,
    },
    ProjectResource {
        project_id: ProjectId,
        resource_id: ResourceId,
        display_locator: ResourceLocator,
        canonical_locator: ResourceLocator,
        health: ResourceHealth,
        primary: bool,
        active_claim: bool,
        conflicting_projects: BTreeSet<ProjectId>,
    },
    ProjectInput {
        project_id: ProjectId,
        message_id: MessageId,
        sequence: u64,
        accepted_fact: FactId,
    },
    ProjectDispatch {
        dispatch_id: DispatchId,
        message_id: MessageId,
        sequence: u64,
        fact_id: FactId,
        conflicted: bool,
    },
    ProjectOutput {
        output_id: MessageId,
        dispatch_id: DispatchId,
        status: ClientProjectOutputStatus,
        content: ContentText,
    },
    RemoteCommand {
        command_id: CommandId,
        request_digest: CommandDigest,
        account_id: AccountId,
        project_id: ProjectId,
        target_home: InstallationId,
        expected_head: FactId,
        operation: hq_domain::OperationCorrelation,
        body: ContentText,
        issued_at: Timestamp,
        request_fact: FactId,
        stage: Box<ClientRemoteCommandStage>,
    },
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
    conversations: Vec<ConversationSummary>,
}

impl AuthoritativeSnapshot {
    /// Constructs one revisioned authoritative view.
    pub const fn new(revision: Revision, domain: DomainSnapshot) -> Self {
        Self {
            revision,
            domain,
            conversations: Vec::new(),
        }
    }

    /// Constructs one revisioned view with its indexed conversation summaries.
    pub const fn with_conversations(
        revision: Revision,
        domain: DomainSnapshot,
        conversations: Vec<ConversationSummary>,
    ) -> Self {
        Self {
            revision,
            domain,
            conversations,
        }
    }

    /// Returns the serialized state revision.
    pub const fn revision(&self) -> Revision {
        self.revision
    }

    /// Returns all normalized domain projection packages.
    pub const fn domain(&self) -> &DomainSnapshot {
        &self.domain
    }

    /// Returns indexed conversation discovery summaries in stable key order.
    pub fn conversations(&self) -> &[ConversationSummary] {
        &self.conversations
    }

    /// Derives the complete client-facing projection catalog.
    pub fn client_projections(&self) -> Result<Vec<ClientProjection>, ApplicationValueError> {
        let mut projections = self.domain.client_projections()?;
        projections.extend(self.conversations.iter().map(|summary| {
            ClientProjection::Conversation {
                key: summary.key.clone(),
                latest_fact: summary.latest_fact,
                open_messages: summary.open_messages,
            }
        }));
        Ok(projections)
    }
}

/// Plain indexed conversation discovery summary owned by the application boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationSummary {
    /// Stable typed conversation identity.
    pub key: hq_reducer::ConversationKey,
    /// Canonically latest presented fact, when the conversation is nonempty.
    pub latest_fact: Option<FactId>,
    /// Number of currently open actionable messages.
    pub open_messages: u32,
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
