//! Typed causal parent and historical-authority references.

use std::collections::BTreeMap;

use crate::{BoundedSet, FactId, ValidatedValueError};

/// Semantic role played by a cited authority fact.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AuthorityRole {
    /// The installation root authorizing local control state.
    LocalInstallation,
    /// The installation and mailbox root authorizing a directional access grant.
    MailboxOwner,
    /// The exact directional mailbox capability grant.
    MailboxGrant,
    /// The unique creator root of a human account.
    AccountCreator,
    /// The exact creator-issued grant to a human device.
    DeviceGrant,
    /// A creator root or active device acceptance authorizing an account action.
    AccountMembership,
    /// The immediately previous home-linear state.
    PreviousState,
    /// The immutable installation root authoritative for a project.
    ProjectHome,
    /// Active human-account membership authorizing a project or remote command.
    ActiveHuman,
    /// The active project assignment.
    Assignment,
    /// The accepted input dispatch.
    Dispatch,
    /// A signed remote-control request.
    Request,
    /// The exact dispatch, assignment, and thread provenance for project output.
    OutputBinding,
}

impl AuthorityRole {
    /// Every authority role in stable semantic order.
    pub const ALL: [Self; 13] = [
        Self::LocalInstallation,
        Self::MailboxOwner,
        Self::MailboxGrant,
        Self::AccountCreator,
        Self::DeviceGrant,
        Self::AccountMembership,
        Self::PreviousState,
        Self::ProjectHome,
        Self::ActiveHuman,
        Self::Assignment,
        Self::Dispatch,
        Self::Request,
        Self::OutputBinding,
    ];
}

/// One typed authority edge to a required parent.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AuthorityReference {
    role: AuthorityRole,
    fact_id: FactId,
}

impl AuthorityReference {
    /// Creates a typed authority reference.
    pub const fn new(role: AuthorityRole, fact_id: FactId) -> Self {
        Self { role, fact_id }
    }

    /// Returns the semantic authority role.
    pub const fn role(self) -> AuthorityRole {
        self.role
    }

    /// Returns the cited fact identity.
    pub const fn fact_id(self) -> FactId {
        self.fact_id
    }
}

/// Required parents plus the subset assigned typed authority roles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CausalReferences<const MAXIMUM_PARENTS: usize, const MAXIMUM_AUTHORITIES: usize> {
    parents: BoundedSet<FactId, MAXIMUM_PARENTS>,
    authorities: BTreeMap<AuthorityRole, FactId>,
}

impl<const MAXIMUM_PARENTS: usize, const MAXIMUM_AUTHORITIES: usize>
    CausalReferences<MAXIMUM_PARENTS, MAXIMUM_AUTHORITIES>
{
    /// Validates that authority roles are unique and cite declared parents.
    pub fn new(
        parents: BoundedSet<FactId, MAXIMUM_PARENTS>,
        authorities: impl IntoIterator<Item = AuthorityReference>,
    ) -> Result<Self, ValidatedValueError> {
        let authorities = authorities.into_iter().collect::<Vec<_>>();
        if authorities.len() > MAXIMUM_AUTHORITIES {
            return Err(ValidatedValueError::TooMany {
                maximum: MAXIMUM_AUTHORITIES,
                actual: authorities.len(),
            });
        }
        let mut by_role = BTreeMap::new();
        for authority in authorities {
            if !parents.contains(&authority.fact_id) {
                return Err(ValidatedValueError::AuthorityNotParent);
            }
            if by_role.insert(authority.role, authority.fact_id).is_some() {
                return Err(ValidatedValueError::DuplicateAuthorityRole);
            }
        }
        Ok(Self {
            parents,
            authorities: by_role,
        })
    }

    /// Returns the required parent set.
    pub const fn parents(&self) -> &BoundedSet<FactId, MAXIMUM_PARENTS> {
        &self.parents
    }

    /// Returns the authority fact for a role, when present.
    pub fn authority(&self, role: AuthorityRole) -> Option<FactId> {
        self.authorities.get(&role).copied()
    }
}
