//! Valid semantic fact fixtures built entirely from explicit inputs.

use std::{error::Error, fmt};

use hq_domain::{
    AuthorityReference, AuthorityRole, BoundedSet, CausalReferences, FactId, FactScope,
    InstallationAddress, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, MailboxKind, SemanticFact,
    SemanticFactError, SemanticPayload, ShortText, Timestamp, ValidatedValueError,
};

use crate::DeterministicValues;

/// Failures constructing deterministic semantic fixtures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixtureError {
    /// A generated bounded value violated its requested limit.
    Value(ValidatedValueError),
    /// Envelope and payload protocol classes were incompatible.
    Fact(SemanticFactError),
}

impl fmt::Display for FixtureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(error) => write!(formatter, "invalid fixture value: {error}"),
            Self::Fact(error) => write!(formatter, "invalid fixture fact: {error}"),
        }
    }
}

impl Error for FixtureError {}

impl From<ValidatedValueError> for FixtureError {
    fn from(value: ValidatedValueError) -> Self {
        Self::Value(value)
    }
}

impl From<SemanticFactError> for FixtureError {
    fn from(value: SemanticFactError) -> Self {
        Self::Fact(value)
    }
}

/// Builders for small, valid catalog fixtures.
pub struct FactBuilder;

impl FactBuilder {
    /// Wraps a payload with explicit causal references for reducer scenarios.
    pub fn with_causal(
        values: &mut DeterministicValues,
        author: InstallationAddress,
        authored_at: Timestamp,
        scope: FactScope,
        parents: impl IntoIterator<Item = FactId>,
        authorities: impl IntoIterator<Item = AuthorityReference>,
        payload: SemanticPayload,
    ) -> Result<SemanticFact, FixtureError> {
        SemanticFact::new(
            values.fact_id(),
            author,
            authored_at,
            scope,
            CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
                BoundedSet::new(parents)?,
                authorities,
            )?,
            payload,
        )
        .map_err(Into::into)
    }

    /// Wraps a payload as a root fixture under an explicit author and scope.
    pub fn root(
        values: &mut DeterministicValues,
        author: InstallationAddress,
        scope: FactScope,
        payload: SemanticPayload,
    ) -> Result<SemanticFact, FixtureError> {
        SemanticFact::new(
            values.fact_id(),
            author,
            Timestamp::from_unix_millis(0),
            scope,
            CausalReferences::new(BoundedSet::new([])?, [])?,
            payload,
        )
        .map_err(Into::into)
    }

    /// Builds a root installation declaration.
    pub fn installation_declared(
        values: &mut DeterministicValues,
        label: &str,
    ) -> Result<SemanticFact, FixtureError> {
        let id = values.fact_id();
        let installation_id = values.installation_id();
        let signing_key = values.signing_key();
        let author = InstallationAddress::new(installation_id, signing_key);
        SemanticFact::new(
            id,
            author,
            Timestamp::from_unix_millis(0),
            FactScope::InstallationPrivate(installation_id),
            CausalReferences::new(BoundedSet::new([])?, [])?,
            SemanticPayload::InstallationDeclared {
                installation_id,
                signing_key,
                encryption_key: values.encryption_key(),
                label: Some(ShortText::new(label)?),
            },
        )
        .map_err(Into::into)
    }

    /// Builds an agent mailbox creation descending from an installation root.
    pub fn mailbox_created(
        values: &mut DeterministicValues,
        installation: &SemanticFact,
        label: &str,
    ) -> Result<SemanticFact, FixtureError> {
        let author = installation.author();
        let parent = installation.id();
        SemanticFact::new(
            values.fact_id(),
            author,
            Timestamp::from_unix_millis(1),
            FactScope::InstallationPrivate(author.installation_id()),
            CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
                BoundedSet::new([parent])?,
                [AuthorityReference::new(
                    AuthorityRole::LocalInstallation,
                    parent,
                )],
            )?,
            SemanticPayload::MailboxCreated {
                mailbox_id: values.mailbox_id(),
                kind: MailboxKind::Agent,
                label: Some(ShortText::new(label)?),
            },
        )
        .map_err(Into::into)
    }
}
