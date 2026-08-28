//! Pure local human-account fact planning.

use std::collections::BTreeSet;

use hq_domain::{
    AccountId, AuthorityReference, AuthorityRole, BoundedSet, CausalReferences, FactId, FactScope,
    InstallationAddress, InstallationId, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, MailboxId,
    MailboxKind, SemanticPayload, ShortText, SigningPublicKey, Timestamp,
};

use crate::{ApplicationError, ApplicationErrorCode, FactPlan};

/// Exact local installation authority observed from one authoritative snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalInstallationAuthority {
    /// Installation authoring the local-control fact.
    pub installation_id: InstallationId,
    /// Signing key projected from the unique installation root.
    pub signing_key: SigningPublicKey,
    /// Exact unique installation-root fact.
    pub root_fact: FactId,
}

/// Explicit non-authority inputs supplied to one local fact plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalFactInputs {
    /// Semantic authored time selected by the caller.
    pub authored_at: Timestamp,
    /// Caller-selected BIP-340 auxiliary input, fresh or deliberately replay-stable.
    pub auxiliary_randomness: [u8; 32],
}

/// Plans creation of the reserved local human mailbox.
pub fn plan_human_mailbox_creation(
    authority: LocalInstallationAuthority,
    inputs: LocalFactInputs,
    mailbox_id: MailboxId,
    label: Option<ShortText>,
) -> Result<FactPlan, ApplicationError> {
    Ok(FactPlan::new(
        authority.installation_id,
        inputs.authored_at,
        FactScope::InstallationPrivate(authority.installation_id),
        local_references(authority, [])?,
        SemanticPayload::MailboxCreated {
            mailbox_id,
            kind: MailboxKind::Human,
            label,
        },
        inputs.auxiliary_randomness,
    ))
}

/// Plans creation of one human account permanently administered by the local installation.
pub fn plan_human_account_creation(
    authority: LocalInstallationAuthority,
    inputs: LocalFactInputs,
    account_id: AccountId,
    label: Option<ShortText>,
) -> Result<FactPlan, ApplicationError> {
    Ok(FactPlan::new(
        authority.installation_id,
        inputs.authored_at,
        FactScope::InstallationPrivate(authority.installation_id),
        local_references(authority, [])?,
        SemanticPayload::HumanAccountCreated {
            account_id,
            creator: InstallationAddress::new(authority.installation_id, authority.signing_key),
            label,
        },
        inputs.auxiliary_randomness,
    ))
}

/// Plans one frontier-complete local default-account selection.
pub fn plan_human_account_selection(
    authority: LocalInstallationAuthority,
    inputs: LocalFactInputs,
    account_id: AccountId,
    membership_fact: FactId,
    selection_frontier: BTreeSet<FactId>,
) -> Result<FactPlan, ApplicationError> {
    let mut parents = selection_frontier;
    parents.insert(authority.root_fact);
    parents.insert(membership_fact);
    let parents = BoundedSet::<FactId, MAX_FACT_PARENTS>::new(parents).map_err(invalid_request)?;
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        parents,
        [
            AuthorityReference::new(AuthorityRole::LocalInstallation, authority.root_fact),
            AuthorityReference::new(AuthorityRole::AccountMembership, membership_fact),
        ],
    )
    .map_err(invalid_request)?;
    Ok(FactPlan::new(
        authority.installation_id,
        inputs.authored_at,
        FactScope::InstallationPrivate(authority.installation_id),
        causal,
        SemanticPayload::HumanAccountSelected { account_id },
        inputs.auxiliary_randomness,
    ))
}

fn local_references(
    authority: LocalInstallationAuthority,
    additional_parents: impl IntoIterator<Item = FactId>,
) -> Result<CausalReferences<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>, ApplicationError> {
    let parents = additional_parents
        .into_iter()
        .chain([authority.root_fact])
        .collect::<BTreeSet<_>>();
    CausalReferences::new(
        BoundedSet::new(parents).map_err(invalid_request)?,
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            authority.root_fact,
        )],
    )
    .map_err(invalid_request)
}

fn invalid_request<T>(_error: T) -> ApplicationError {
    ApplicationError::new(ApplicationErrorCode::InvalidRequest)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    fn authority() -> LocalInstallationAuthority {
        LocalInstallationAuthority {
            installation_id: InstallationId::from_bytes([1; 32]),
            signing_key: SigningPublicKey::from_bytes([2; 32]),
            root_fact: FactId::from_bytes([3; 32]),
        }
    }

    fn inputs() -> LocalFactInputs {
        LocalFactInputs {
            authored_at: Timestamp::from_unix_millis(10),
            auxiliary_randomness: [4; 32],
        }
    }

    #[test]
    fn creator_plans_bind_the_exact_local_root_and_public_author() {
        let account = AccountId::from_bytes([5; 32]);
        let plan = plan_human_account_creation(authority(), inputs(), account, None)
            .expect("account plan");
        assert_eq!(plan.author(), authority().installation_id);
        assert_eq!(
            plan.causal().authority(AuthorityRole::LocalInstallation),
            Some(authority().root_fact)
        );
        assert!(matches!(
            plan.payload(),
            SemanticPayload::HumanAccountCreated {
                account_id,
                creator,
                label: None,
            } if *account_id == account
                && *creator == InstallationAddress::new(
                    authority().installation_id,
                    authority().signing_key,
                )
        ));

        let mailbox = plan_human_mailbox_creation(
            authority(),
            inputs(),
            MailboxId::from_bytes([6; 32]),
            None,
        )
        .expect("mailbox plan");
        assert!(matches!(
            mailbox.payload(),
            SemanticPayload::MailboxCreated {
                kind: MailboxKind::Human,
                ..
            }
        ));
    }

    #[test]
    fn selection_cites_membership_root_and_the_complete_prior_frontier() {
        let membership = FactId::from_bytes([7; 32]);
        let frontier = [FactId::from_bytes([8; 32]), FactId::from_bytes([9; 32])]
            .into_iter()
            .collect::<BTreeSet<_>>();
        let plan = plan_human_account_selection(
            authority(),
            inputs(),
            AccountId::from_bytes([10; 32]),
            membership,
            frontier.clone(),
        )
        .expect("selection plan");
        assert_eq!(
            plan.causal().authority(AuthorityRole::AccountMembership),
            Some(membership)
        );
        assert!(
            frontier
                .iter()
                .all(|fact| plan.causal().parents().contains(fact))
        );
        assert!(plan.causal().parents().contains(&authority().root_fact));
    }
}
