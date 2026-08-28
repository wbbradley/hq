use std::collections::{BTreeMap, BTreeSet, VecDeque};

use hq_domain::{
    AccountId, AuthorityRole, EncryptionPublicKey, ErrorCode, Fact, FactId, FactScope, GrantId,
    InstallationAddress, InstallationId, MailboxAddress, MailboxId, MailboxKind, RelayHints,
    SemanticPayload, ShortText, SigningPublicKey,
};

use crate::{
    ConflictObservation, ConflictReason, DomainDecision, DomainReducer, DomainReductionReport,
    ProjectionContribution, ReductionContext,
};

/// Explicit installation-local inputs to authority reduction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthorityPolicy {
    local_installation: InstallationId,
    local_human_mailbox: MailboxId,
}

impl AuthorityPolicy {
    /// Creates authority policy without consulting local configuration or ambient state.
    pub const fn new(local_installation: InstallationId, local_human_mailbox: MailboxId) -> Self {
        Self {
            local_installation,
            local_human_mailbox,
        }
    }

    /// Returns the installation whose local choices may become active.
    pub const fn local_installation(self) -> InstallationId {
        self.local_installation
    }

    /// Returns the reserved local human mailbox identity.
    pub const fn local_human_mailbox(self) -> MailboxId {
        self.local_human_mailbox
    }
}

/// Typed authority aggregate used for causal frontier derivation.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AuthorityAggregateKey {
    /// Competing declarations of one installation.
    Installation(InstallationId),
    /// Competing creations of one installation-qualified mailbox.
    Mailbox(MailboxAddress),
    /// Route register from one local installation to one peer.
    PeerRoute {
        /// Local route owner.
        owner: InstallationId,
        /// Remote installation.
        peer: InstallationId,
    },
    /// One directional mailbox grant lineage.
    MailboxCapability(GrantId),
    /// Competing roots of one human account.
    Account(AccountId),
    /// Membership history for one device in one account.
    Membership {
        /// Human account.
        account: AccountId,
        /// Non-creator device installation.
        device: InstallationId,
    },
    /// Local default-account selection register.
    AccountSelection(InstallationId),
}

/// Typed key of one normalized authority projection.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AuthorityProjectionKey {
    /// Installation root view.
    Installation(InstallationId),
    /// Mailbox root view.
    Mailbox(MailboxAddress),
    /// Directional peer-route view.
    PeerRoute {
        /// Local route owner.
        owner: InstallationId,
        /// Remote installation.
        peer: InstallationId,
    },
    /// Directional capability lineage.
    MailboxCapability(GrantId),
    /// Human-account root.
    Account(AccountId),
    /// Device membership.
    Membership {
        /// Human account.
        account: AccountId,
        /// Device installation.
        device: InstallationId,
    },
    /// Local account-selection register.
    AccountSelection(InstallationId),
}

/// Closed authority rejection and conflict reasons.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AuthorityReason {
    /// Signed scope does not match the payload family or subject.
    ScopeMismatch,
    /// Verified author is not the exact required signer.
    SignerMismatch,
    /// Payload subject does not match its cited authority.
    SubjectMismatch,
    /// A required typed authority role was omitted.
    MissingAuthority(AuthorityRole),
    /// The cited authority fact has the wrong semantic family.
    AuthorityKindMismatch(AuthorityRole),
    /// A root fact illegally declared causal parents.
    RootHasParents,
    /// Unequal facts claim one unique semantic root.
    UniqueRootConflict,
    /// A multivalue register retains multiple causal-maximal candidates.
    ConcurrentRegisterConflict,
    /// A state update did not descend from every relevant revoke maximum.
    IncompleteRevocationFrontier,
    /// The cited mailbox grant is revoked at the action's causal point.
    RevokedMailboxGrant,
    /// The cited account acceptance is not active and causal-maximal.
    InactiveAccountMembership,
}

/// Normalized installation identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallationView {
    /// Root supporting the installation.
    pub root_fact: FactId,
    /// Verified root signer.
    pub signing_key: SigningPublicKey,
    /// Advertised encryption key.
    pub encryption_key: EncryptionPublicKey,
    /// Optional bounded display label.
    pub label: Option<ShortText>,
}

/// Normalized installation-qualified mailbox identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MailboxView {
    /// Creation fact.
    pub create_fact: FactId,
    /// Fixed mailbox kind.
    pub kind: MailboxKind,
    /// Optional bounded display label.
    pub label: Option<ShortText>,
}

/// Current state of a directional peer-route register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerRouteState {
    /// Exactly one causal-maximal route set is usable.
    Routable,
    /// At least one causal-maximal block removes routing.
    Blocked,
    /// Multiple route-set maxima remain without a block.
    Conflicted,
}

/// Normalized directional peer-route history and current frontier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerRouteView {
    state: PeerRouteState,
    frontier: BTreeSet<FactId>,
    /// Every usable route-set fact and its exact typed routing value.
    pub routes: BTreeMap<FactId, PeerRouteCandidate>,
    /// Every usable block fact and its stable reason.
    pub blocks: BTreeMap<FactId, ErrorCode>,
}

impl PeerRouteView {
    /// Reconstructs a typed route view from explicitly decoded persisted parts.
    pub fn from_parts(
        state: PeerRouteState,
        frontier: BTreeSet<FactId>,
        routes: BTreeMap<FactId, PeerRouteCandidate>,
        blocks: BTreeMap<FactId, ErrorCode>,
    ) -> Option<Self> {
        let every_frontier_fact_is_retained = frontier
            .iter()
            .all(|fact| routes.contains_key(fact) || blocks.contains_key(fact));
        let frontier_blocks = frontier
            .iter()
            .filter(|fact| blocks.contains_key(fact))
            .count();
        let frontier_routes = frontier
            .iter()
            .filter(|fact| routes.contains_key(fact))
            .count();
        let expected = if frontier_blocks > 0 {
            PeerRouteState::Blocked
        } else if frontier_routes == 1 {
            PeerRouteState::Routable
        } else {
            PeerRouteState::Conflicted
        };
        let histories_are_disjoint = routes.keys().all(|fact| !blocks.contains_key(fact));
        (every_frontier_fact_is_retained && histories_are_disjoint && state == expected).then_some(
            Self {
                state,
                frontier,
                routes,
                blocks,
            },
        )
    }

    /// Returns remove-wins current route state.
    pub const fn state(&self) -> PeerRouteState {
        self.state
    }

    /// Returns every exact usable route maximum.
    pub const fn frontier(&self) -> &BTreeSet<FactId> {
        &self.frontier
    }
}

/// Typed routing value retained for one peer-route set fact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerRouteCandidate {
    /// Exact remote installation and signer address.
    pub peer: InstallationAddress,
    /// Remote encryption key.
    pub encryption_key: EncryptionPublicKey,
    /// Optional display label.
    pub label: Option<ShortText>,
    /// Bounded relay routing hints.
    pub relay_hints: RelayHints,
}

/// Normalized directional mailbox capability lineage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityView {
    /// Exact supporting capability-grant fact.
    pub grant_fact: FactId,
    /// Target mailbox.
    pub mailbox: MailboxAddress,
    /// Exact grantee installation and signing key.
    pub grantee: InstallationAddress,
    active: bool,
    /// Causal-maximal revoke facts for this grant.
    pub revoke_frontier: BTreeSet<FactId>,
    /// Owner-observed action identities retained by history.
    pub observed_actions: BTreeSet<FactId>,
}

impl CapabilityView {
    /// Reconstructs a typed capability view from explicitly decoded persisted parts.
    pub fn from_parts(
        grant_fact: FactId,
        mailbox: MailboxAddress,
        grantee: InstallationAddress,
        active: bool,
        revoke_frontier: BTreeSet<FactId>,
        observed_actions: BTreeSet<FactId>,
    ) -> Option<Self> {
        (active == revoke_frontier.is_empty()
            && !revoke_frontier.contains(&grant_fact)
            && !observed_actions.contains(&grant_fact)
            && revoke_frontier.is_disjoint(&observed_actions))
        .then_some(Self {
            grant_fact,
            mailbox,
            grantee,
            active,
            revoke_frontier,
            observed_actions,
        })
    }

    /// Reports whether this grant is currently active.
    pub const fn is_active(&self) -> bool {
        self.active
    }
}

/// Current device membership classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MembershipState {
    /// At least one grant exists but no active exact acceptance does.
    Pending,
    /// One or more causal-maximal exact acceptances grant authority.
    Active,
    /// A maximal revoke removes every known acceptance.
    Revoked,
}

/// Normalized device membership history and frontier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MembershipView {
    state: MembershipState,
    /// Every causal-maximal usable acceptance or revoke.
    pub frontier: BTreeSet<FactId>,
    /// Creator-issued grant history by stable grant identity.
    pub grants: BTreeMap<GrantId, DeviceGrantView>,
    /// Every usable exact acceptance fact.
    pub acceptances: BTreeSet<FactId>,
    /// Every usable exact revoke fact.
    pub revokes: BTreeSet<FactId>,
    /// Exact active acceptance authorities.
    pub active_acceptances: BTreeSet<FactId>,
    /// Grant identities cited by current active acceptances.
    pub active_grants: BTreeSet<GrantId>,
}

impl MembershipView {
    /// Reconstructs a typed membership view from explicitly decoded persisted parts.
    pub fn from_parts(
        state: MembershipState,
        frontier: BTreeSet<FactId>,
        grants: BTreeMap<GrantId, DeviceGrantView>,
        acceptances: BTreeSet<FactId>,
        revokes: BTreeSet<FactId>,
        active_acceptances: BTreeSet<FactId>,
        active_grants: BTreeSet<GrantId>,
    ) -> Option<Self> {
        let grant_facts = grants
            .values()
            .map(|grant| grant.grant_fact)
            .collect::<BTreeSet<_>>();
        let retained = grant_facts
            .iter()
            .copied()
            .chain(acceptances.iter().copied())
            .chain(revokes.iter().copied())
            .collect::<BTreeSet<_>>();
        let expected = if !active_acceptances.is_empty() {
            MembershipState::Active
        } else if !revokes.is_empty() {
            MembershipState::Revoked
        } else {
            MembershipState::Pending
        };
        let histories_are_disjoint = grant_facts.is_disjoint(&acceptances)
            && grant_facts.is_disjoint(&revokes)
            && acceptances.is_disjoint(&revokes);
        (state == expected
            && active_acceptances.is_subset(&acceptances)
            && active_grants.is_subset(&grants.keys().copied().collect())
            && active_acceptances.is_empty() == active_grants.is_empty()
            && histories_are_disjoint
            && frontier.is_subset(&retained))
        .then_some(Self {
            state,
            frontier,
            grants,
            acceptances,
            revokes,
            active_acceptances,
            active_grants,
        })
    }

    /// Returns current remove-wins membership state.
    pub const fn state(&self) -> MembershipState {
        self.state
    }
}

/// Creator-issued human-device grant retained in membership history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceGrantView {
    /// Exact supporting grant fact.
    pub grant_fact: FactId,
    /// Invited installation and signing key.
    pub device: InstallationAddress,
    /// Optional device label.
    pub label: Option<ShortText>,
    /// Bounded relay hints for account fanout.
    pub relay_hints: RelayHints,
}

/// One normalized authority projection value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthorityProjection {
    /// Installation root.
    Installation(InstallationView),
    /// Mailbox root.
    Mailbox(MailboxView),
    /// Directional route register.
    PeerRoute(PeerRouteView),
    /// Directional mailbox capability.
    MailboxCapability(CapabilityView),
    /// Human-account creator root.
    Account {
        /// Root fact.
        root_fact: FactId,
        /// Permanent creator address.
        creator: InstallationAddress,
        /// Optional display label.
        label: Option<ShortText>,
    },
    /// Human device membership.
    Membership(MembershipView),
    /// Local default-account candidates; `active` is set only for one authorized maximum.
    AccountSelection {
        /// Every causal-maximal selected account.
        candidates: BTreeSet<AccountId>,
        /// Singular active default account.
        active: Option<AccountId>,
    },
}

/// Pure reducer for installation, peer, mailbox-capability, and human-account authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthorityReducer {
    policy: AuthorityPolicy,
}

impl AuthorityReducer {
    /// Creates the reducer from explicit local policy.
    pub const fn new(policy: AuthorityPolicy) -> Self {
        Self { policy }
    }
}

/// Complete normalized authority report.
pub type AuthorityReport = DomainReductionReport<AuthorityReducer>;

impl DomainReducer for AuthorityReducer {
    type AggregateKey = AuthorityAggregateKey;
    type ProjectionKey = AuthorityProjectionKey;
    type ProjectionValue = AuthorityProjection;
    type Reason = AuthorityReason;

    fn classify(
        &self,
        fact: &Fact,
        context: &ReductionContext<'_, Self::Reason>,
    ) -> DomainDecision<Self::Reason> {
        classify_fact(self.policy, fact, context)
    }

    fn aggregate_keys(
        &self,
        fact: &Fact,
        _context: &ReductionContext<'_, Self::Reason>,
    ) -> Vec<Self::AggregateKey> {
        aggregate_key(fact).into_iter().collect()
    }

    fn projections(
        &self,
        context: &ReductionContext<'_, Self::Reason>,
    ) -> Vec<ProjectionContribution<Self::ProjectionKey, Self::ProjectionValue>> {
        derive_authority_projections(self.policy, context)
    }

    fn conflicts(
        &self,
        context: &ReductionContext<'_, Self::Reason>,
    ) -> Vec<ConflictObservation<Self::Reason>> {
        derive_authority_conflicts(context)
    }
}

pub(crate) fn classify_fact(
    policy: AuthorityPolicy,
    fact: &Fact,
    context: &ReductionContext<'_, impl Sized>,
) -> DomainDecision<AuthorityReason> {
    let result = match fact.payload() {
        SemanticPayload::InstallationDeclared {
            installation_id, ..
        } => validate_installation_root(fact, *installation_id, context),
        SemanticPayload::MailboxCreated {
            mailbox_id, kind, ..
        } => validate_mailbox_root(policy, fact, *mailbox_id, *kind, context),
        SemanticPayload::MailboxSessionBound { mailbox_id, .. }
        | SemanticPayload::MailboxContextRecorded { mailbox_id, .. } => {
            validate_mailbox_binding(fact, *mailbox_id, context)
        }
        SemanticPayload::PeerRouteSet { peer, .. } => {
            validate_local_fact(fact, context).and_then(|()| {
                (peer.installation_id() != fact.author().installation_id())
                    .then_some(())
                    .ok_or(AuthorityReason::SubjectMismatch)
            })
        }
        SemanticPayload::PeerRouteBlocked { peer_id, .. } => validate_local_fact(fact, context)
            .and_then(|()| {
                (*peer_id != fact.author().installation_id())
                    .then_some(())
                    .ok_or(AuthorityReason::SubjectMismatch)
            }),
        SemanticPayload::MailboxAccessGranted {
            grant_id,
            mailbox,
            grantee,
        } => validate_mailbox_grant(fact, *grant_id, *mailbox, *grantee, context),
        SemanticPayload::MailboxAccessRevoked {
            grant_id,
            mailbox,
            grantee_id,
        } => validate_mailbox_revoke(fact, *grant_id, *mailbox, *grantee_id, context),
        SemanticPayload::MailboxActionObserved {
            grant_id,
            action_id,
        } => validate_mailbox_observation(fact, *grant_id, *action_id, context),
        SemanticPayload::HumanAccountCreated {
            account_id,
            creator,
            ..
        } => validate_account_root(fact, *account_id, *creator, context),
        SemanticPayload::HumanDeviceGranted {
            account_id,
            grant_id,
            device,
            ..
        } => validate_device_grant(fact, *account_id, *grant_id, *device, context),
        SemanticPayload::HumanDeviceAccepted {
            account_id,
            grant_id,
            device,
        } => validate_device_acceptance(fact, *account_id, *grant_id, *device, context),
        SemanticPayload::HumanDeviceRevoked {
            account_id,
            grant_id,
            device_id,
        } => validate_device_revoke(fact, *account_id, *grant_id, *device_id, context),
        SemanticPayload::HumanAccountSelected { account_id } => validate_local_fact(fact, context)
            .and_then(|()| validate_account_action(fact, *account_id, context)),
        _ => validate_scoped_action(fact, context),
    };
    match result {
        Ok(()) => DomainDecision::Projected,
        Err(reason @ AuthorityReason::UniqueRootConflict) => DomainDecision::Conflicted {
            reason,
            participants: unique_conflict_participants(fact, context),
        },
        Err(
            reason @ (AuthorityReason::ScopeMismatch
            | AuthorityReason::SignerMismatch
            | AuthorityReason::SubjectMismatch
            | AuthorityReason::RootHasParents),
        ) => DomainDecision::Invalid { reason },
        Err(reason) => DomainDecision::Unauthorized {
            failed_authorities: failed_role(reason),
            reason,
        },
    }
}

fn validate_installation_root(
    fact: &Fact,
    installation_id: InstallationId,
    context: &ReductionContext<'_, impl Sized>,
) -> Result<(), AuthorityReason> {
    if fact.causal().parents().iter().next().is_some() {
        return Err(AuthorityReason::RootHasParents);
    }
    let conflicts = context
        .facts()
        .facts()
        .filter(|candidate| matches!(candidate.payload(), SemanticPayload::InstallationDeclared { installation_id: candidate_id, .. } if *candidate_id == installation_id))
        .count();
    if conflicts > 1 {
        Err(AuthorityReason::UniqueRootConflict)
    } else {
        Ok(())
    }
}

fn validate_mailbox_root(
    policy: AuthorityPolicy,
    fact: &Fact,
    mailbox_id: MailboxId,
    kind: MailboxKind,
    context: &ReductionContext<'_, impl Sized>,
) -> Result<(), AuthorityReason> {
    validate_local_fact(fact, context)?;
    if kind == MailboxKind::Human
        && fact.author().installation_id() == policy.local_installation
        && mailbox_id != policy.local_human_mailbox
    {
        return Err(AuthorityReason::SubjectMismatch);
    }
    let address = MailboxAddress::new(fact.author().installation_id(), mailbox_id);
    let conflicts = context
        .facts()
        .facts()
        .filter(|candidate| mailbox_subject(candidate) == Some(address))
        .count();
    let human_conflicts = matches!(
        fact.payload(),
        SemanticPayload::MailboxCreated {
            kind: MailboxKind::Human,
            ..
        }
    ) && fact.author().installation_id() != policy.local_installation
        && context
            .facts()
            .facts()
            .filter(|candidate| {
                candidate.author().installation_id() == fact.author().installation_id()
                    && matches!(
                        candidate.payload(),
                        SemanticPayload::MailboxCreated {
                            kind: MailboxKind::Human,
                            ..
                        }
                    )
            })
            .count()
            > 1;
    if conflicts > 1 || human_conflicts {
        Err(AuthorityReason::UniqueRootConflict)
    } else {
        Ok(())
    }
}

fn validate_mailbox_binding(
    fact: &Fact,
    mailbox_id: MailboxId,
    context: &ReductionContext<'_, impl Sized>,
) -> Result<(), AuthorityReason> {
    validate_local_fact(fact, context)?;
    let expected = MailboxAddress::new(fact.author().installation_id(), mailbox_id);
    if fact.causal().parents().iter().any(|parent| {
        context
            .facts()
            .get(*parent)
            .is_some_and(|candidate| mailbox_subject(candidate) == Some(expected))
    }) {
        Ok(())
    } else {
        Err(AuthorityReason::SubjectMismatch)
    }
}

fn validate_local_fact(
    fact: &Fact,
    context: &ReductionContext<'_, impl Sized>,
) -> Result<(), AuthorityReason> {
    let installation_id = match fact.scope() {
        FactScope::InstallationPrivate(installation_id) => *installation_id,
        _ => return Err(AuthorityReason::ScopeMismatch),
    };
    if fact.author().installation_id() != installation_id {
        return Err(AuthorityReason::SignerMismatch);
    }
    let authority = required_authority(fact, AuthorityRole::LocalInstallation)?;
    match context.facts().get(authority).map(Fact::payload) {
        Some(SemanticPayload::InstallationDeclared {
            installation_id: root_id,
            signing_key,
            ..
        }) if *root_id == installation_id && *signing_key == fact.author().signing_key() => Ok(()),
        Some(_) => Err(AuthorityReason::AuthorityKindMismatch(
            AuthorityRole::LocalInstallation,
        )),
        None => Err(AuthorityReason::MissingAuthority(
            AuthorityRole::LocalInstallation,
        )),
    }
}

fn validate_mailbox_grant(
    fact: &Fact,
    grant_id: GrantId,
    mailbox: MailboxAddress,
    grantee: InstallationAddress,
    context: &ReductionContext<'_, impl Sized>,
) -> Result<(), AuthorityReason> {
    if !matches!(fact.scope(), FactScope::PeerAddressed(scope) if *scope == mailbox)
        || fact.author().installation_id() != mailbox.installation_id()
        || grantee.installation_id() == mailbox.installation_id()
    {
        return Err(AuthorityReason::ScopeMismatch);
    }
    let owner = required_authority(fact, AuthorityRole::MailboxOwner)?;
    match context.facts().get(owner) {
        Some(candidate)
            if mailbox_subject(candidate) == Some(mailbox)
                && candidate.author() == fact.author() => {}
        Some(_) => {
            return Err(AuthorityReason::AuthorityKindMismatch(
                AuthorityRole::MailboxOwner,
            ));
        }
        None => {
            return Err(AuthorityReason::MissingAuthority(
                AuthorityRole::MailboxOwner,
            ));
        }
    }
    if context
        .facts()
        .facts()
        .filter(|candidate| matches!(candidate.payload(), SemanticPayload::MailboxAccessGranted { grant_id: candidate_id, .. } if *candidate_id == grant_id))
        .count()
        > 1
    {
        return Err(AuthorityReason::UniqueRootConflict);
    }
    for revoke in mailbox_lineage_revokes(context, mailbox, grantee.installation_id()) {
        if !context.graph().structurally_reaches(revoke.id(), fact.id())
            && !context.graph().structurally_reaches(fact.id(), revoke.id())
        {
            return Err(AuthorityReason::IncompleteRevocationFrontier);
        }
    }
    Ok(())
}

fn validate_mailbox_revoke(
    fact: &Fact,
    grant_id: GrantId,
    mailbox: MailboxAddress,
    grantee_id: InstallationId,
    context: &ReductionContext<'_, impl Sized>,
) -> Result<(), AuthorityReason> {
    if !matches!(fact.scope(), FactScope::PeerAddressed(scope) if *scope == mailbox)
        || fact.author().installation_id() != mailbox.installation_id()
    {
        return Err(AuthorityReason::ScopeMismatch);
    }
    let grant = required_authority(fact, AuthorityRole::MailboxGrant)?;
    match context.facts().get(grant) {
        Some(grant_fact)
            if grant_fact.author() == fact.author()
                && matches!(grant_fact.payload(), SemanticPayload::MailboxAccessGranted {
                    grant_id: expected,
                    mailbox: expected_mailbox,
                    grantee,
                } if *expected == grant_id
                    && *expected_mailbox == mailbox
                    && grantee.installation_id() == grantee_id) =>
        {
            Ok(())
        }
        Some(_) => Err(AuthorityReason::SubjectMismatch),
        None => Err(AuthorityReason::MissingAuthority(
            AuthorityRole::MailboxGrant,
        )),
    }
}

fn validate_mailbox_observation(
    fact: &Fact,
    grant_id: GrantId,
    action_id: FactId,
    context: &ReductionContext<'_, impl Sized>,
) -> Result<(), AuthorityReason> {
    let target = match fact.scope() {
        FactScope::PeerAddressed(target) => *target,
        _ => return Err(AuthorityReason::ScopeMismatch),
    };
    if fact.author().installation_id() != target.installation_id()
        || !fact.causal().parents().contains(&action_id)
    {
        return Err(AuthorityReason::SubjectMismatch);
    }
    let grant = required_authority(fact, AuthorityRole::MailboxGrant)?;
    validate_grant_for_action(grant, grant_id, target, context)?;
    if context
        .facts()
        .get(grant)
        .is_some_and(|grant_fact| grant_fact.author() != fact.author())
    {
        return Err(AuthorityReason::SignerMismatch);
    }
    let action = context
        .facts()
        .get(action_id)
        .ok_or(AuthorityReason::SubjectMismatch)?;
    if action.causal().authority(AuthorityRole::MailboxGrant) != Some(grant)
        || !matches!(action.scope(), FactScope::PeerAddressed(scope) if *scope == target)
    {
        return Err(AuthorityReason::SubjectMismatch);
    }
    Ok(())
}

fn validate_account_root(
    fact: &Fact,
    account_id: AccountId,
    creator: InstallationAddress,
    context: &ReductionContext<'_, impl Sized>,
) -> Result<(), AuthorityReason> {
    validate_local_fact(fact, context)?;
    if creator != fact.author() {
        return Err(AuthorityReason::SubjectMismatch);
    }
    let count = context
        .facts()
        .facts()
        .filter(|candidate| account_root_subject(candidate) == Some(account_id))
        .count();
    if count > 1 {
        Err(AuthorityReason::UniqueRootConflict)
    } else {
        Ok(())
    }
}

fn validate_device_grant(
    fact: &Fact,
    account_id: AccountId,
    grant_id: GrantId,
    device: InstallationAddress,
    context: &ReductionContext<'_, impl Sized>,
) -> Result<(), AuthorityReason> {
    if !matches!(fact.scope(), FactScope::AccountAddressed(scope) if *scope == account_id) {
        return Err(AuthorityReason::ScopeMismatch);
    }
    let creator = required_authority(fact, AuthorityRole::AccountCreator)?;
    let creator_address = match context.facts().get(creator).map(Fact::payload) {
        Some(SemanticPayload::HumanAccountCreated {
            account_id: expected,
            creator,
            ..
        }) if *expected == account_id => *creator,
        Some(_) => {
            return Err(AuthorityReason::AuthorityKindMismatch(
                AuthorityRole::AccountCreator,
            ));
        }
        None => {
            return Err(AuthorityReason::MissingAuthority(
                AuthorityRole::AccountCreator,
            ));
        }
    };
    if creator_address != fact.author()
        || device.installation_id() == creator_address.installation_id()
    {
        return Err(AuthorityReason::SignerMismatch);
    }
    if context
        .facts()
        .facts()
        .filter(|candidate| matches!(candidate.payload(), SemanticPayload::HumanDeviceGranted { grant_id: candidate_id, .. } if *candidate_id == grant_id))
        .count()
        > 1
    {
        return Err(AuthorityReason::UniqueRootConflict);
    }
    for revoke in account_revokes(context, account_id, device.installation_id()) {
        if !context.graph().structurally_reaches(revoke.id(), fact.id())
            && !context.graph().structurally_reaches(fact.id(), revoke.id())
        {
            return Err(AuthorityReason::IncompleteRevocationFrontier);
        }
    }
    Ok(())
}

fn validate_device_acceptance(
    fact: &Fact,
    account_id: AccountId,
    grant_id: GrantId,
    device: InstallationAddress,
    context: &ReductionContext<'_, impl Sized>,
) -> Result<(), AuthorityReason> {
    if !matches!(fact.scope(), FactScope::AccountAddressed(scope) if *scope == account_id)
        || fact.author() != device
    {
        return Err(AuthorityReason::SignerMismatch);
    }
    let grant = required_authority(fact, AuthorityRole::DeviceGrant)?;
    match context.facts().get(grant).map(Fact::payload) {
        Some(SemanticPayload::HumanDeviceGranted {
            account_id: expected_account,
            grant_id: expected_grant,
            device: expected_device,
            ..
        }) if *expected_account == account_id
            && *expected_grant == grant_id
            && *expected_device == device => {}
        Some(_) => return Err(AuthorityReason::SubjectMismatch),
        None => {
            return Err(AuthorityReason::MissingAuthority(
                AuthorityRole::DeviceGrant,
            ));
        }
    }
    for revoke in account_revokes(context, account_id, device.installation_id()) {
        if reaches_with_candidate(context, revoke.id(), fact.id(), fact.id())
            && !context.usably_reaches(revoke.id(), grant)
        {
            return Err(AuthorityReason::InactiveAccountMembership);
        }
    }
    Ok(())
}

fn validate_device_revoke(
    fact: &Fact,
    account_id: AccountId,
    grant_id: GrantId,
    device_id: InstallationId,
    context: &ReductionContext<'_, impl Sized>,
) -> Result<(), AuthorityReason> {
    if !matches!(fact.scope(), FactScope::AccountAddressed(scope) if *scope == account_id) {
        return Err(AuthorityReason::ScopeMismatch);
    }
    let creator = required_authority(fact, AuthorityRole::AccountCreator)?;
    let grant = required_authority(fact, AuthorityRole::DeviceGrant)?;
    let creator_matches = matches!(
        context.facts().get(creator).map(Fact::payload),
        Some(SemanticPayload::HumanAccountCreated { account_id: expected, creator, .. })
            if *expected == account_id && *creator == fact.author()
    );
    let grant_matches = matches!(
        context.facts().get(grant).map(Fact::payload),
        Some(SemanticPayload::HumanDeviceGranted { account_id: expected_account, grant_id: expected_grant, device, .. })
            if *expected_account == account_id && *expected_grant == grant_id && device.installation_id() == device_id
    );
    if creator_matches && grant_matches {
        Ok(())
    } else {
        Err(AuthorityReason::SubjectMismatch)
    }
}

fn validate_scoped_action(
    fact: &Fact,
    context: &ReductionContext<'_, impl Sized>,
) -> Result<(), AuthorityReason> {
    match fact.scope() {
        FactScope::InstallationPrivate(_) => validate_local_fact(fact, context),
        FactScope::PeerAddressed(target) => validate_peer_action(fact, *target, context),
        FactScope::AccountAddressed(account_id) | FactScope::RemoteControl { account_id, .. } => {
            validate_account_action(fact, *account_id, context)
        }
    }
}

fn validate_peer_action(
    fact: &Fact,
    target: MailboxAddress,
    context: &ReductionContext<'_, impl Sized>,
) -> Result<(), AuthorityReason> {
    let grant = required_authority(fact, AuthorityRole::MailboxGrant)?;
    let grant_id = match context.facts().get(grant).map(Fact::payload) {
        Some(SemanticPayload::MailboxAccessGranted {
            grant_id,
            mailbox,
            grantee,
        }) if *mailbox == target && *grantee == fact.author() => *grant_id,
        Some(_) => return Err(AuthorityReason::SubjectMismatch),
        None => {
            return Err(AuthorityReason::MissingAuthority(
                AuthorityRole::MailboxGrant,
            ));
        }
    };
    validate_grant_for_action(grant, grant_id, target, context)?;
    if mailbox_revokes(context, grant_id)
        .into_iter()
        .any(|revoke| !reaches_with_candidate(context, fact.id(), revoke.id(), fact.id()))
    {
        return Err(AuthorityReason::RevokedMailboxGrant);
    }
    Ok(())
}

fn validate_account_action(
    fact: &Fact,
    account_id: AccountId,
    context: &ReductionContext<'_, impl Sized>,
) -> Result<(), AuthorityReason> {
    let authority = required_authority(fact, AuthorityRole::AccountMembership)?;
    match context.facts().get(authority).map(Fact::payload) {
        Some(SemanticPayload::HumanAccountCreated {
            account_id: expected,
            creator,
            ..
        }) if *expected == account_id && *creator == fact.author() => Ok(()),
        Some(SemanticPayload::HumanDeviceAccepted {
            account_id: expected,
            device,
            ..
        }) if *expected == account_id
            && *device == fact.author()
            && account_action_survives_revokes(
                fact.id(),
                authority,
                account_id,
                device.installation_id(),
                context,
            ) =>
        {
            Ok(())
        }
        Some(_) => Err(AuthorityReason::InactiveAccountMembership),
        None => Err(AuthorityReason::MissingAuthority(
            AuthorityRole::AccountMembership,
        )),
    }
}

fn validate_grant_for_action(
    grant: FactId,
    grant_id: GrantId,
    target: MailboxAddress,
    context: &ReductionContext<'_, impl Sized>,
) -> Result<(), AuthorityReason> {
    match context.facts().get(grant).map(Fact::payload) {
        Some(SemanticPayload::MailboxAccessGranted {
            grant_id: expected,
            mailbox,
            ..
        }) if *expected == grant_id && *mailbox == target => Ok(()),
        Some(_) => Err(AuthorityReason::SubjectMismatch),
        None => Err(AuthorityReason::MissingAuthority(
            AuthorityRole::MailboxGrant,
        )),
    }
}

fn required_authority(fact: &Fact, role: AuthorityRole) -> Result<FactId, AuthorityReason> {
    fact.causal()
        .authority(role)
        .ok_or(AuthorityReason::MissingAuthority(role))
}

fn failed_role(reason: AuthorityReason) -> BTreeSet<AuthorityRole> {
    match reason {
        AuthorityReason::MissingAuthority(role) | AuthorityReason::AuthorityKindMismatch(role) => {
            BTreeSet::from([role])
        }
        AuthorityReason::RevokedMailboxGrant => BTreeSet::from([AuthorityRole::MailboxGrant]),
        AuthorityReason::InactiveAccountMembership => {
            BTreeSet::from([AuthorityRole::AccountMembership])
        }
        _ => BTreeSet::new(),
    }
}

fn mailbox_subject(fact: &Fact) -> Option<MailboxAddress> {
    match fact.payload() {
        SemanticPayload::MailboxCreated { mailbox_id, .. } => Some(MailboxAddress::new(
            fact.author().installation_id(),
            *mailbox_id,
        )),
        _ => None,
    }
}

fn account_root_subject(fact: &Fact) -> Option<AccountId> {
    match fact.payload() {
        SemanticPayload::HumanAccountCreated { account_id, .. } => Some(*account_id),
        _ => None,
    }
}

fn mailbox_revokes<'a>(
    context: &ReductionContext<'a, impl Sized>,
    grant_id: GrantId,
) -> Vec<&'a Fact> {
    context
        .facts()
        .facts()
        .filter(|candidate| matches!(candidate.payload(), SemanticPayload::MailboxAccessRevoked { grant_id: candidate_id, .. } if *candidate_id == grant_id))
        .collect()
}

fn mailbox_lineage_revokes<'a>(
    context: &ReductionContext<'a, impl Sized>,
    mailbox: MailboxAddress,
    grantee: InstallationId,
) -> Vec<&'a Fact> {
    context
        .facts()
        .facts()
        .filter(|candidate| matches!(candidate.payload(), SemanticPayload::MailboxAccessRevoked { mailbox: candidate_mailbox, grantee_id, .. } if *candidate_mailbox == mailbox && *grantee_id == grantee))
        .collect()
}

fn account_revokes<'a>(
    context: &ReductionContext<'a, impl Sized>,
    account_id: AccountId,
    device_id: InstallationId,
) -> Vec<&'a Fact> {
    context
        .facts()
        .facts()
        .filter(|candidate| matches!(candidate.payload(), SemanticPayload::HumanDeviceRevoked { account_id: candidate_account, device_id: candidate_device, .. } if *candidate_account == account_id && *candidate_device == device_id))
        .collect()
}

fn active_acceptance(
    acceptance: FactId,
    account_id: AccountId,
    device_id: InstallationId,
    context: &ReductionContext<'_, impl Sized>,
) -> bool {
    account_revokes(context, account_id, device_id)
        .into_iter()
        .all(|revoke| context.usably_reaches(revoke.id(), acceptance))
}

fn account_action_survives_revokes(
    action: FactId,
    acceptance: FactId,
    account_id: AccountId,
    device_id: InstallationId,
    context: &ReductionContext<'_, impl Sized>,
) -> bool {
    account_revokes(context, account_id, device_id)
        .into_iter()
        .all(|revoke| {
            reaches_with_candidate(context, action, revoke.id(), action)
                || context.usably_reaches(revoke.id(), acceptance)
        })
}

fn reaches_with_candidate(
    context: &ReductionContext<'_, impl Sized>,
    ancestor: FactId,
    descendant: FactId,
    candidate: FactId,
) -> bool {
    let usable = |fact_id| fact_id == candidate || context.is_projected(fact_id);
    if !usable(ancestor) || !usable(descendant) {
        return false;
    }
    if ancestor == descendant {
        return true;
    }
    let mut visited = BTreeSet::new();
    let mut pending = context
        .graph()
        .children(ancestor)
        .iter()
        .copied()
        .filter(|fact_id| usable(*fact_id))
        .collect::<VecDeque<_>>();
    while let Some(fact_id) = pending.pop_front() {
        if fact_id == descendant {
            return true;
        }
        if visited.insert(fact_id) {
            pending.extend(
                context
                    .graph()
                    .children(fact_id)
                    .iter()
                    .copied()
                    .filter(|child| usable(*child)),
            );
        }
    }
    false
}

fn unique_conflict_participants(
    fact: &Fact,
    context: &ReductionContext<'_, impl Sized>,
) -> BTreeSet<FactId> {
    context
        .facts()
        .facts()
        .filter(|candidate| match (fact.payload(), candidate.payload()) {
            (
                SemanticPayload::InstallationDeclared {
                    installation_id, ..
                },
                SemanticPayload::InstallationDeclared {
                    installation_id: candidate_id,
                    ..
                },
            ) => installation_id == candidate_id,
            (
                SemanticPayload::MailboxCreated {
                    mailbox_id, kind, ..
                },
                SemanticPayload::MailboxCreated {
                    mailbox_id: candidate_id,
                    kind: candidate_kind,
                    ..
                },
            ) => {
                (fact.author().installation_id() == candidate.author().installation_id()
                    && mailbox_id == candidate_id)
                    || (*kind == MailboxKind::Human
                        && *candidate_kind == MailboxKind::Human
                        && fact.author().installation_id() == candidate.author().installation_id())
            }
            (
                SemanticPayload::MailboxAccessGranted { grant_id, .. },
                SemanticPayload::MailboxAccessGranted {
                    grant_id: candidate_id,
                    ..
                },
            )
            | (
                SemanticPayload::HumanDeviceGranted { grant_id, .. },
                SemanticPayload::HumanDeviceGranted {
                    grant_id: candidate_id,
                    ..
                },
            ) => grant_id == candidate_id,
            (
                SemanticPayload::HumanAccountCreated { account_id, .. },
                SemanticPayload::HumanAccountCreated {
                    account_id: candidate_id,
                    ..
                },
            ) => account_id == candidate_id,
            _ => false,
        })
        .map(Fact::id)
        .collect()
}

fn aggregate_key(fact: &Fact) -> Option<AuthorityAggregateKey> {
    match fact.payload() {
        SemanticPayload::InstallationDeclared {
            installation_id, ..
        } => Some(AuthorityAggregateKey::Installation(*installation_id)),
        SemanticPayload::MailboxCreated { mailbox_id, .. } => Some(AuthorityAggregateKey::Mailbox(
            MailboxAddress::new(fact.author().installation_id(), *mailbox_id),
        )),
        SemanticPayload::PeerRouteSet { peer, .. } => Some(AuthorityAggregateKey::PeerRoute {
            owner: fact.author().installation_id(),
            peer: peer.installation_id(),
        }),
        SemanticPayload::PeerRouteBlocked { peer_id, .. } => {
            Some(AuthorityAggregateKey::PeerRoute {
                owner: fact.author().installation_id(),
                peer: *peer_id,
            })
        }
        SemanticPayload::MailboxAccessGranted { grant_id, .. }
        | SemanticPayload::MailboxAccessRevoked { grant_id, .. }
        | SemanticPayload::MailboxActionObserved { grant_id, .. } => {
            Some(AuthorityAggregateKey::MailboxCapability(*grant_id))
        }
        SemanticPayload::HumanAccountCreated { account_id, .. } => {
            Some(AuthorityAggregateKey::Account(*account_id))
        }
        SemanticPayload::HumanDeviceGranted {
            account_id, device, ..
        }
        | SemanticPayload::HumanDeviceAccepted {
            account_id, device, ..
        } => Some(AuthorityAggregateKey::Membership {
            account: *account_id,
            device: device.installation_id(),
        }),
        SemanticPayload::HumanDeviceRevoked {
            account_id,
            device_id,
            ..
        } => Some(AuthorityAggregateKey::Membership {
            account: *account_id,
            device: *device_id,
        }),
        SemanticPayload::HumanAccountSelected { .. } => Some(
            AuthorityAggregateKey::AccountSelection(fact.author().installation_id()),
        ),
        _ => None,
    }
}

fn derive_authority_projections(
    policy: AuthorityPolicy,
    context: &ReductionContext<'_, impl Sized>,
) -> Vec<ProjectionContribution<AuthorityProjectionKey, AuthorityProjection>> {
    let mut output = Vec::new();
    for fact in context
        .facts()
        .facts()
        .filter(|fact| context.is_projected(fact.id()))
    {
        match fact.payload() {
            SemanticPayload::InstallationDeclared {
                installation_id,
                signing_key,
                encryption_key,
                label,
            } => output.push(ProjectionContribution::new(
                AuthorityProjectionKey::Installation(*installation_id),
                AuthorityProjection::Installation(InstallationView {
                    root_fact: fact.id(),
                    signing_key: *signing_key,
                    encryption_key: *encryption_key,
                    label: label.clone(),
                }),
                [fact.id()],
            )),
            SemanticPayload::MailboxCreated {
                mailbox_id,
                kind,
                label,
            } => {
                let address = MailboxAddress::new(fact.author().installation_id(), *mailbox_id);
                output.push(ProjectionContribution::new(
                    AuthorityProjectionKey::Mailbox(address),
                    AuthorityProjection::Mailbox(MailboxView {
                        create_fact: fact.id(),
                        kind: *kind,
                        label: label.clone(),
                    }),
                    [fact.id()],
                ));
            }
            _ => {}
        }
    }
    output.extend(peer_route_projections(context));
    output.extend(capability_projections(context));
    output.extend(account_projections(context));
    output.extend(membership_projections(context));
    output.extend(selection_projections(policy, context));
    output
}

fn peer_route_projections(
    context: &ReductionContext<'_, impl Sized>,
) -> Vec<ProjectionContribution<AuthorityProjectionKey, AuthorityProjection>> {
    let mut groups = BTreeMap::<(InstallationId, InstallationId), BTreeSet<FactId>>::new();
    for fact in context
        .facts()
        .facts()
        .filter(|fact| context.is_projected(fact.id()))
    {
        if let Some(AuthorityAggregateKey::PeerRoute { owner, peer }) = aggregate_key(fact) {
            groups.entry((owner, peer)).or_default().insert(fact.id());
        }
    }
    groups
        .into_iter()
        .map(|((owner, peer), members)| {
            let frontier = maximal_members(&members, context);
            let blocks = frontier
                .iter()
                .filter(|fact_id| {
                    matches!(
                        context.facts().get(**fact_id).map(Fact::payload),
                        Some(SemanticPayload::PeerRouteBlocked { .. })
                    )
                })
                .count();
            let sets = frontier.len() - blocks;
            let state = if blocks > 0 {
                PeerRouteState::Blocked
            } else if sets == 1 {
                PeerRouteState::Routable
            } else {
                PeerRouteState::Conflicted
            };
            let routes = members
                .iter()
                .filter_map(
                    |fact_id| match context.facts().get(*fact_id).map(Fact::payload) {
                        Some(SemanticPayload::PeerRouteSet {
                            peer,
                            encryption_key,
                            label,
                            relay_hints,
                        }) => Some((
                            *fact_id,
                            PeerRouteCandidate {
                                peer: *peer,
                                encryption_key: *encryption_key,
                                label: label.clone(),
                                relay_hints: relay_hints.clone(),
                            },
                        )),
                        _ => None,
                    },
                )
                .collect();
            let block_reasons = members
                .iter()
                .filter_map(
                    |fact_id| match context.facts().get(*fact_id).map(Fact::payload) {
                        Some(SemanticPayload::PeerRouteBlocked { reason, .. }) => {
                            Some((*fact_id, reason.clone()))
                        }
                        _ => None,
                    },
                )
                .collect();
            ProjectionContribution::new(
                AuthorityProjectionKey::PeerRoute { owner, peer },
                AuthorityProjection::PeerRoute(PeerRouteView {
                    state,
                    frontier: frontier.clone(),
                    routes,
                    blocks: block_reasons,
                }),
                frontier,
            )
        })
        .collect()
}

fn capability_projections(
    context: &ReductionContext<'_, impl Sized>,
) -> Vec<ProjectionContribution<AuthorityProjectionKey, AuthorityProjection>> {
    context
        .facts()
        .facts()
        .filter(|fact| context.is_projected(fact.id()))
        .filter_map(|grant| match grant.payload() {
            SemanticPayload::MailboxAccessGranted {
                grant_id,
                mailbox,
                grantee,
            } => {
                let revokes = mailbox_revokes(context, *grant_id)
                    .into_iter()
                    .filter(|fact| context.is_projected(fact.id()))
                    .map(Fact::id)
                    .collect::<BTreeSet<_>>();
                let revoke_frontier = maximal_members(&revokes, context);
                let observed_actions = context
                    .facts()
                    .facts()
                    .filter(|fact| context.is_projected(fact.id()))
                    .filter_map(|fact| match fact.payload() {
                        SemanticPayload::MailboxActionObserved {
                            grant_id: observed_grant,
                            action_id,
                        } if observed_grant == grant_id => Some(*action_id),
                        _ => None,
                    })
                    .collect::<BTreeSet<_>>();
                let observations = context
                    .facts()
                    .facts()
                    .filter(|fact| context.is_projected(fact.id()))
                    .filter(|fact| matches!(fact.payload(), SemanticPayload::MailboxActionObserved { grant_id: observed_grant, .. } if observed_grant == grant_id))
                    .map(Fact::id)
                    .collect::<BTreeSet<_>>();
                let support = std::iter::once(grant.id())
                    .chain(revokes.iter().copied())
                    .chain(observations.iter().copied())
                    .collect::<BTreeSet<_>>();
                Some(ProjectionContribution::new(
                    AuthorityProjectionKey::MailboxCapability(*grant_id),
                    AuthorityProjection::MailboxCapability(CapabilityView {
                        grant_fact: grant.id(),
                        mailbox: *mailbox,
                        grantee: *grantee,
                        active: revokes.is_empty(),
                        revoke_frontier,
                        observed_actions,
                    }),
                    support,
                ))
            }
            _ => None,
        })
        .collect()
}

fn account_projections(
    context: &ReductionContext<'_, impl Sized>,
) -> Vec<ProjectionContribution<AuthorityProjectionKey, AuthorityProjection>> {
    context
        .facts()
        .facts()
        .filter(|fact| context.is_projected(fact.id()))
        .filter_map(|fact| match fact.payload() {
            SemanticPayload::HumanAccountCreated {
                account_id,
                creator,
                label,
            } => Some(ProjectionContribution::new(
                AuthorityProjectionKey::Account(*account_id),
                AuthorityProjection::Account {
                    root_fact: fact.id(),
                    creator: *creator,
                    label: label.clone(),
                },
                [fact.id()],
            )),
            _ => None,
        })
        .collect()
}

fn membership_projections(
    context: &ReductionContext<'_, impl Sized>,
) -> Vec<ProjectionContribution<AuthorityProjectionKey, AuthorityProjection>> {
    let mut groups = BTreeMap::<(AccountId, InstallationId), BTreeSet<FactId>>::new();
    for fact in context
        .facts()
        .facts()
        .filter(|fact| context.is_projected(fact.id()))
    {
        if let Some(AuthorityAggregateKey::Membership { account, device }) = aggregate_key(fact) {
            groups
                .entry((account, device))
                .or_default()
                .insert(fact.id());
        }
    }
    groups
        .into_iter()
        .map(|((account, device), members)| {
            let frontier = maximal_members(&members, context);
            let grants = members
                .iter()
                .filter_map(
                    |fact_id| match context.facts().get(*fact_id).map(Fact::payload) {
                        Some(SemanticPayload::HumanDeviceGranted {
                            grant_id,
                            device,
                            label,
                            relay_hints,
                            ..
                        }) => Some((
                            *grant_id,
                            DeviceGrantView {
                                grant_fact: *fact_id,
                                device: *device,
                                label: label.clone(),
                                relay_hints: relay_hints.clone(),
                            },
                        )),
                        _ => None,
                    },
                )
                .collect();
            let acceptances = members
                .iter()
                .copied()
                .filter(|fact_id| {
                    matches!(
                        context.facts().get(*fact_id).map(Fact::payload),
                        Some(SemanticPayload::HumanDeviceAccepted { .. })
                    )
                })
                .collect::<BTreeSet<_>>();
            let revokes = members
                .iter()
                .copied()
                .filter(|fact_id| {
                    matches!(
                        context.facts().get(*fact_id).map(Fact::payload),
                        Some(SemanticPayload::HumanDeviceRevoked { .. })
                    )
                })
                .collect::<BTreeSet<_>>();
            let active_acceptances = members
                .iter()
                .copied()
                .filter(|fact_id| {
                    matches!(
                        context.facts().get(*fact_id).map(Fact::payload),
                        Some(SemanticPayload::HumanDeviceAccepted { .. })
                    ) && active_acceptance(*fact_id, account, device, context)
                })
                .collect::<BTreeSet<_>>();
            let active_grants = active_membership_grants(&active_acceptances, context);
            let has_revoke = members.iter().any(|fact_id| {
                matches!(
                    context.facts().get(*fact_id).map(Fact::payload),
                    Some(SemanticPayload::HumanDeviceRevoked { .. })
                )
            });
            let state = if !active_acceptances.is_empty() {
                MembershipState::Active
            } else if has_revoke {
                MembershipState::Revoked
            } else {
                MembershipState::Pending
            };
            ProjectionContribution::new(
                AuthorityProjectionKey::Membership { account, device },
                AuthorityProjection::Membership(MembershipView {
                    state,
                    frontier,
                    grants,
                    acceptances,
                    revokes,
                    active_acceptances,
                    active_grants,
                }),
                members,
            )
        })
        .collect()
}

fn active_membership_grants(
    acceptances: &BTreeSet<FactId>,
    context: &ReductionContext<'_, impl Sized>,
) -> BTreeSet<GrantId> {
    acceptances
        .iter()
        .filter_map(
            |fact_id| match context.facts().get(*fact_id).map(Fact::payload) {
                Some(SemanticPayload::HumanDeviceAccepted { grant_id, .. }) => Some(*grant_id),
                _ => None,
            },
        )
        .collect()
}

fn selection_projections(
    policy: AuthorityPolicy,
    context: &ReductionContext<'_, impl Sized>,
) -> Vec<ProjectionContribution<AuthorityProjectionKey, AuthorityProjection>> {
    let members = context
        .facts()
        .facts()
        .filter(|fact| context.is_projected(fact.id()))
        .filter(|fact| fact.author().installation_id() == policy.local_installation)
        .filter(|fact| matches!(fact.payload(), SemanticPayload::HumanAccountSelected { .. }))
        .map(Fact::id)
        .collect::<BTreeSet<_>>();
    if members.is_empty() {
        return Vec::new();
    }
    let frontier = maximal_members(&members, context);
    let candidates = frontier
        .iter()
        .filter_map(
            |fact_id| match context.facts().get(*fact_id).map(Fact::payload) {
                Some(SemanticPayload::HumanAccountSelected { account_id }) => Some(*account_id),
                _ => None,
            },
        )
        .collect::<BTreeSet<_>>();
    let active = (candidates.len() == 1)
        .then(|| candidates.iter().next().copied())
        .flatten();
    vec![ProjectionContribution::new(
        AuthorityProjectionKey::AccountSelection(policy.local_installation),
        AuthorityProjection::AccountSelection { candidates, active },
        frontier,
    )]
}

fn maximal_members(
    members: &BTreeSet<FactId>,
    context: &ReductionContext<'_, impl Sized>,
) -> BTreeSet<FactId> {
    members
        .iter()
        .copied()
        .filter(|candidate| {
            !members
                .iter()
                .copied()
                .any(|other| other != *candidate && context.usably_reaches(*candidate, other))
        })
        .collect()
}

fn derive_authority_conflicts(
    context: &ReductionContext<'_, impl Sized>,
) -> Vec<ConflictObservation<AuthorityReason>> {
    let mut groups = BTreeMap::<AuthorityAggregateKey, BTreeSet<FactId>>::new();
    for fact in context
        .facts()
        .facts()
        .filter(|fact| context.is_projected(fact.id()))
    {
        if let Some(key) = aggregate_key(fact) {
            groups.entry(key).or_default().insert(fact.id());
        }
    }
    groups
        .into_iter()
        .filter_map(|(key, members)| {
            let frontier = maximal_members(&members, context);
            let conflicted = match key {
                AuthorityAggregateKey::PeerRoute { .. } => {
                    frontier.len() > 1
                        && frontier.iter().all(|fact_id| {
                            matches!(
                                context.facts().get(*fact_id).map(Fact::payload),
                                Some(SemanticPayload::PeerRouteSet { .. })
                            )
                        })
                }
                AuthorityAggregateKey::AccountSelection(_) => frontier.len() > 1,
                _ => false,
            };
            conflicted.then(|| {
                ConflictObservation::new(
                    ConflictReason::Domain(AuthorityReason::ConcurrentRegisterConflict),
                    frontier,
                )
            })
        })
        .collect()
}
