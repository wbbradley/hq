//! Pure directional peer-route and mailbox-capability planning.

use std::collections::BTreeSet;

use hq_domain::{
    AuthorityReference, AuthorityRole, BoundedSet, CausalReferences, EncryptionPublicKey,
    ErrorCode, FactId, FactScope, GrantId, InstallationAddress, InstallationId,
    MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, MailboxAddress, RelayHints, SemanticPayload, ShortText,
};

use crate::{
    ApplicationError, ApplicationErrorCode, FactPlan, LocalFactInputs, LocalInstallationAuthority,
};

/// Passive complete intent for one directional peer-route set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerRouteRequest {
    /// Exact peer installation and signing key.
    pub peer: InstallationAddress,
    /// Exact peer encryption key used only for transport.
    pub encryption_key: EncryptionPublicKey,
    /// Optional signed display label.
    pub label: Option<ShortText>,
    /// Signed non-authority relay hints.
    pub relay_hints: RelayHints,
    /// Complete causal-maximal directional route history.
    pub route_frontier: BTreeSet<FactId>,
}

/// Passive complete intent for one directional mailbox capability grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MailboxGrantRequest {
    /// Stable capability identity.
    pub grant_id: GrantId,
    /// Exact locally owned mailbox.
    pub mailbox: MailboxAddress,
    /// Exact mailbox creation authority.
    pub mailbox_fact: FactId,
    /// Exact grantee installation and signing key.
    pub grantee: InstallationAddress,
    /// Complete prior revoke lineage for this mailbox and grantee.
    pub lineage_frontier: BTreeSet<FactId>,
}

/// Passive complete intent for one exact mailbox capability revocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MailboxRevokeRequest {
    /// Stable capability identity.
    pub grant_id: GrantId,
    /// Exact grant fact being revoked.
    pub grant_fact: FactId,
    /// Exact locally owned mailbox.
    pub mailbox: MailboxAddress,
    /// Exact grantee installation.
    pub grantee_id: InstallationId,
    /// Complete retained observation/revoke support for the grant.
    pub capability_frontier: BTreeSet<FactId>,
}

/// Plans one directional peer-route set or full-frontier recovery after a block.
pub fn plan_peer_route_set(
    authority: LocalInstallationAuthority,
    inputs: LocalFactInputs,
    request: PeerRouteRequest,
) -> Result<FactPlan, ApplicationError> {
    if request.peer.installation_id() == authority.installation_id {
        return Err(invalid_request());
    }
    Ok(FactPlan::new(
        authority.installation_id,
        inputs.authored_at,
        FactScope::InstallationPrivate(authority.installation_id),
        local_causal(authority, request.route_frontier)?,
        SemanticPayload::PeerRouteSet {
            peer: request.peer,
            encryption_key: request.encryption_key,
            label: request.label,
            relay_hints: request.relay_hints,
        },
        inputs.auxiliary_randomness,
    ))
}

/// Plans one directional, full-frontier peer-route block.
pub fn plan_peer_route_block(
    authority: LocalInstallationAuthority,
    inputs: LocalFactInputs,
    peer_id: InstallationId,
    reason: ErrorCode,
    route_frontier: BTreeSet<FactId>,
) -> Result<FactPlan, ApplicationError> {
    if peer_id == authority.installation_id {
        return Err(invalid_request());
    }
    Ok(FactPlan::new(
        authority.installation_id,
        inputs.authored_at,
        FactScope::InstallationPrivate(authority.installation_id),
        local_causal(authority, route_frontier)?,
        SemanticPayload::PeerRouteBlocked { peer_id, reason },
        inputs.auxiliary_randomness,
    ))
}

/// Plans one directional mailbox capability grant or full-lineage regrant.
pub fn plan_mailbox_grant(
    authority: LocalInstallationAuthority,
    inputs: LocalFactInputs,
    request: MailboxGrantRequest,
) -> Result<FactPlan, ApplicationError> {
    if request.mailbox.installation_id() != authority.installation_id
        || request.grantee.installation_id() == authority.installation_id
    {
        return Err(invalid_request());
    }
    let mut parents = request.lineage_frontier;
    parents.insert(request.mailbox_fact);
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        BoundedSet::new(parents).map_err(|_| invalid_request())?,
        [AuthorityReference::new(
            AuthorityRole::MailboxOwner,
            request.mailbox_fact,
        )],
    )
    .map_err(|_| invalid_request())?;
    Ok(FactPlan::new(
        authority.installation_id,
        inputs.authored_at,
        FactScope::PeerAddressed(request.mailbox),
        causal,
        SemanticPayload::MailboxAccessGranted {
            grant_id: request.grant_id,
            mailbox: request.mailbox,
            grantee: request.grantee,
        },
        inputs.auxiliary_randomness,
    ))
}

/// Plans one exact, frontier-complete mailbox capability revocation.
pub fn plan_mailbox_revoke(
    authority: LocalInstallationAuthority,
    inputs: LocalFactInputs,
    request: MailboxRevokeRequest,
) -> Result<FactPlan, ApplicationError> {
    if request.mailbox.installation_id() != authority.installation_id
        || request.grantee_id == authority.installation_id
    {
        return Err(invalid_request());
    }
    let mut parents = request.capability_frontier;
    parents.insert(request.grant_fact);
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        BoundedSet::new(parents).map_err(|_| invalid_request())?,
        [AuthorityReference::new(
            AuthorityRole::MailboxGrant,
            request.grant_fact,
        )],
    )
    .map_err(|_| invalid_request())?;
    Ok(FactPlan::new(
        authority.installation_id,
        inputs.authored_at,
        FactScope::PeerAddressed(request.mailbox),
        causal,
        SemanticPayload::MailboxAccessRevoked {
            grant_id: request.grant_id,
            mailbox: request.mailbox,
            grantee_id: request.grantee_id,
        },
        inputs.auxiliary_randomness,
    ))
}

fn local_causal(
    authority: LocalInstallationAuthority,
    mut frontier: BTreeSet<FactId>,
) -> Result<CausalReferences<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>, ApplicationError> {
    frontier.insert(authority.root_fact);
    CausalReferences::new(
        BoundedSet::new(frontier).map_err(|_| invalid_request())?,
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            authority.root_fact,
        )],
    )
    .map_err(|_| invalid_request())
}

fn invalid_request() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCode::InvalidRequest)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use hq_domain::{MailboxId, SigningPublicKey, Timestamp};

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
            authored_at: Timestamp::from_unix_millis(4),
            auxiliary_randomness: [5; 32],
        }
    }

    #[test]
    fn peer_plans_bind_local_root_and_complete_route_frontier() {
        let frontier = BTreeSet::from([FactId::from_bytes([6; 32])]);
        let peer = InstallationAddress::new(
            InstallationId::from_bytes([7; 32]),
            SigningPublicKey::from_bytes([8; 32]),
        );
        let set = plan_peer_route_set(
            authority(),
            inputs(),
            PeerRouteRequest {
                peer,
                encryption_key: EncryptionPublicKey::from_bytes([9; 32]),
                label: None,
                relay_hints: RelayHints::new([]).expect("relays"),
                route_frontier: frontier.clone(),
            },
        )
        .expect("route set");
        assert!(
            frontier
                .iter()
                .all(|fact| set.causal().parents().contains(fact))
        );
        let block = plan_peer_route_block(
            authority(),
            inputs(),
            peer.installation_id(),
            ErrorCode::new("operator-distrust").expect("reason"),
            frontier,
        )
        .expect("route block");
        assert_eq!(
            block.causal().authority(AuthorityRole::LocalInstallation),
            Some(authority().root_fact)
        );
    }

    #[test]
    fn mailbox_plans_bind_exact_owner_grant_and_complete_lineage() {
        let mailbox =
            MailboxAddress::new(authority().installation_id, MailboxId::from_bytes([10; 32]));
        let grantee = InstallationAddress::new(
            InstallationId::from_bytes([11; 32]),
            SigningPublicKey::from_bytes([12; 32]),
        );
        let grant_id = GrantId::from_bytes([13; 32]);
        let mailbox_fact = FactId::from_bytes([14; 32]);
        let lineage = BTreeSet::from([FactId::from_bytes([15; 32])]);
        let grant = plan_mailbox_grant(
            authority(),
            inputs(),
            MailboxGrantRequest {
                grant_id,
                mailbox,
                mailbox_fact,
                grantee,
                lineage_frontier: lineage,
            },
        )
        .expect("mailbox grant");
        assert_eq!(
            grant.causal().authority(AuthorityRole::MailboxOwner),
            Some(mailbox_fact)
        );
        let support = BTreeSet::from([FactId::from_bytes([16; 32]), FactId::from_bytes([17; 32])]);
        let grant_fact = FactId::from_bytes([18; 32]);
        let revoke = plan_mailbox_revoke(
            authority(),
            inputs(),
            MailboxRevokeRequest {
                grant_id,
                grant_fact,
                mailbox,
                grantee_id: grantee.installation_id(),
                capability_frontier: support.clone(),
            },
        )
        .expect("mailbox revoke");
        assert_eq!(
            revoke.causal().authority(AuthorityRole::MailboxGrant),
            Some(grant_fact)
        );
        assert!(
            support
                .iter()
                .all(|fact| revoke.causal().parents().contains(fact))
        );
    }
}
