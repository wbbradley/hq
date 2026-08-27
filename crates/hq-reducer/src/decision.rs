use std::collections::{BTreeMap, BTreeSet};

use hq_domain::{AuthorityRole, FactId};

/// One of the six normalized semantic outcomes for a known identity.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DecisionStatus {
    /// Valid, resolved, authorized, and admitted semantic knowledge.
    Projected,
    /// At least one required dependency is missing or currently unusable.
    Unresolved,
    /// Historical signer, audience, or authority policy failed.
    Unauthorized,
    /// An explicit identity, root, register, fork, or cardinality conflict applies.
    Conflicted,
    /// The semantic value or graph shape is intrinsically impossible.
    Invalid,
    /// The verified protocol version or semantic family is not implemented.
    Unsupported,
}

/// Closed framework reasons plus a domain reducer's own closed reason enum.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DecisionReason<R> {
    /// Unequal exact facts carried one content identity.
    IdentityCollision,
    /// The fact participates in a present causal cycle.
    CausalCycle,
    /// A reason defined by the plugged-in pure domain reducer.
    Domain(R),
}

/// Domain-owned classification after graph dependencies are usable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DomainDecision<R> {
    /// Admit the fact as usable knowledge.
    Projected,
    /// Reject it for failed historical authority.
    Unauthorized {
        /// Closed domain reason.
        reason: R,
        /// Exact typed authority roles that failed.
        failed_authorities: BTreeSet<AuthorityRole>,
    },
    /// Mark this fact conflicted and record the complete participant set.
    Conflicted {
        /// Closed domain reason.
        reason: R,
        /// Complete normalized participant set.
        participants: BTreeSet<FactId>,
    },
    /// Reject an intrinsically impossible domain value or transition.
    Invalid {
        /// Closed domain reason.
        reason: R,
    },
    /// Retain a verified but unimplemented semantic family.
    Unsupported {
        /// Closed domain reason.
        reason: R,
    },
}

/// Normalized decision and dependency diagnostics for one identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FactDecision<R> {
    pub(crate) status: DecisionStatus,
    pub(crate) reason: Option<DecisionReason<R>>,
    pub(crate) missing_dependencies: BTreeSet<FactId>,
    pub(crate) unusable_dependencies: BTreeMap<FactId, DecisionStatus>,
    pub(crate) failed_authorities: BTreeSet<AuthorityRole>,
    pub(crate) conflict_participants: BTreeSet<FactId>,
    pub(crate) reverse_dependants: BTreeSet<FactId>,
}

impl<R> FactDecision<R> {
    /// Returns the normalized outcome category.
    pub const fn status(&self) -> DecisionStatus {
        self.status
    }

    /// Returns the closed framework or domain reason, when applicable.
    pub const fn reason(&self) -> Option<&DecisionReason<R>> {
        self.reason.as_ref()
    }

    /// Returns absent required parent identities.
    pub const fn missing_dependencies(&self) -> &BTreeSet<FactId> {
        &self.missing_dependencies
    }

    /// Returns present but unusable parents and their current outcomes.
    pub const fn unusable_dependencies(&self) -> &BTreeMap<FactId, DecisionStatus> {
        &self.unusable_dependencies
    }

    /// Returns typed authority roles rejected by domain policy.
    pub const fn failed_authorities(&self) -> &BTreeSet<AuthorityRole> {
        &self.failed_authorities
    }

    /// Returns every participant in the applicable conflict.
    pub const fn conflict_participants(&self) -> &BTreeSet<FactId> {
        &self.conflict_participants
    }

    /// Returns direct reverse dependants that require reconsideration if this decision changes.
    pub const fn reverse_dependants(&self) -> &BTreeSet<FactId> {
        &self.reverse_dependants
    }
}
