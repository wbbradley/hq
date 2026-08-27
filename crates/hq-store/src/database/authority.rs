//! Explicit relational codecs for rebuildable authority projections.

use std::collections::{BTreeMap, BTreeSet};

use hq_domain::{
    AccountId, BoundedText, EncryptionPublicKey, ErrorCode, FactId, GrantId, InstallationAddress,
    InstallationId, MailboxAddress, MailboxId, MailboxKind, RelayHints, ResourceLocator,
    ResourceScheme, ShortText, SigningPublicKey,
};
use hq_reducer::{
    AuthorityAggregateKey, AuthorityProjection, AuthorityProjectionKey, CapabilityView,
    DeviceGrantView, InstallationView, MailboxView, MembershipState, MembershipView,
    PeerRouteCandidate, PeerRouteState, PeerRouteView,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};

use crate::{AuthorityProjectionSnapshot, StoreError, StoreErrorClass};

const MAXIMUM_AUTHORITY_ROWS: i64 = 64_000_000;

pub(super) fn clear(transaction: &Transaction<'_>) -> Result<(), StoreError> {
    transaction
        .execute_batch(
            "DELETE FROM authority_state;
             DELETE FROM authority_account_selection_candidates;
             DELETE FROM authority_account_selections;
             DELETE FROM authority_membership_grant_relays;
             DELETE FROM authority_membership_grants;
             DELETE FROM authority_membership_facts;
             DELETE FROM authority_memberships;
             DELETE FROM authority_accounts;
             DELETE FROM authority_capability_facts;
             DELETE FROM authority_capabilities;
             DELETE FROM authority_peer_route_relays;
             DELETE FROM authority_peer_route_candidates;
             DELETE FROM authority_peer_route_facts;
             DELETE FROM authority_peer_routes;
             DELETE FROM authority_mailboxes;
             DELETE FROM authority_installations;
             DELETE FROM authority_support;
             DELETE FROM authority_frontiers;",
        )
        .map_err(database)
}

pub(super) fn insert(
    transaction: &Transaction<'_>,
    snapshot: &AuthorityProjectionSnapshot,
) -> Result<(), StoreError> {
    insert_frontiers(transaction, snapshot)?;
    insert_support(transaction, snapshot)?;
    for (key, projection) in &snapshot.projections {
        insert_projection(transaction, *key, projection)?;
    }
    let counts = Counts::read(transaction)?;
    counts.validate()?;
    if counts.frontier_count != length(snapshot.frontiers.values().map(BTreeSet::len))?
        || counts.projection_count
            != i64::try_from(snapshot.projections.len()).map_err(|_| corrupt())?
        || counts.support_count != length(snapshot.support.values().map(BTreeSet::len))?
    {
        return Err(corrupt());
    }
    let digest = row_digest(transaction)?;
    transaction
        .execute(
            "INSERT INTO authority_state(singleton, frontier_count, projection_count, \
                 support_count, row_count, row_digest) VALUES (1, ?1, ?2, ?3, ?4, ?5)",
            params![
                counts.frontier_count,
                counts.projection_count,
                counts.support_count,
                counts.row_count,
                digest.as_slice()
            ],
        )
        .map_err(database)?;
    Ok(())
}

pub(super) fn load(connection: &Connection) -> Result<AuthorityProjectionSnapshot, StoreError> {
    let Some(expected) = load_state(connection)? else {
        return if Counts::read(connection)?.row_count == 0 {
            Err(StoreError::new(StoreErrorClass::NotRepaired))
        } else {
            Err(corrupt())
        };
    };
    expected.counts.validate()?;
    if Counts::read(connection)? != expected.counts || row_digest(connection)? != expected.digest {
        return Err(corrupt());
    }
    let snapshot = AuthorityProjectionSnapshot {
        frontiers: load_keyed_facts(connection, "authority_frontiers", decode_aggregate_key)?,
        projections: load_projections(connection)?,
        support: load_keyed_facts(connection, "authority_support", decode_projection_key)?,
    };
    validate_snapshot(&snapshot, expected.counts)?;
    Ok(snapshot)
}

fn insert_frontiers(
    transaction: &Transaction<'_>,
    snapshot: &AuthorityProjectionSnapshot,
) -> Result<(), StoreError> {
    for (key, facts) in &snapshot.frontiers {
        let key = aggregate_key_parts(*key);
        for fact in facts {
            insert_keyed_fact(transaction, "authority_frontiers", key, *fact)?;
        }
    }
    Ok(())
}

fn insert_support(
    transaction: &Transaction<'_>,
    snapshot: &AuthorityProjectionSnapshot,
) -> Result<(), StoreError> {
    for (key, facts) in &snapshot.support {
        let key = projection_key_parts(*key);
        for fact in facts {
            insert_keyed_fact(transaction, "authority_support", key, *fact)?;
        }
    }
    Ok(())
}

fn insert_keyed_fact(
    transaction: &Transaction<'_>,
    table: &str,
    key: KeyParts,
    fact: FactId,
) -> Result<(), StoreError> {
    let sql = match table {
        "authority_frontiers" => {
            "INSERT INTO authority_frontiers(key_kind, key_a, key_b, fact_id) VALUES (?1, ?2, ?3, ?4)"
        }
        "authority_support" => {
            "INSERT INTO authority_support(key_kind, key_a, key_b, fact_id) VALUES (?1, ?2, ?3, ?4)"
        }
        _ => return Err(corrupt()),
    };
    transaction
        .execute(
            sql,
            params![
                key.kind,
                key.a.as_slice(),
                key.b.unwrap_or([0; 32]).to_vec(),
                fact.as_bytes().as_slice()
            ],
        )
        .map_err(database)?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn insert_projection(
    transaction: &Transaction<'_>,
    key: AuthorityProjectionKey,
    projection: &AuthorityProjection,
) -> Result<(), StoreError> {
    match (key, projection) {
        (
            AuthorityProjectionKey::Installation(installation),
            AuthorityProjection::Installation(view),
        ) => {
            transaction.execute(
                "INSERT INTO authority_installations(installation_id, root_fact, signing_key, encryption_key, label) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![installation.as_bytes().as_slice(), view.root_fact.as_bytes().as_slice(),
                    view.signing_key.as_bytes().as_slice(), view.encryption_key.as_bytes().as_slice(),
                    optional_text(view.label.as_ref())],
            ).map_err(database)?;
        }
        (AuthorityProjectionKey::Mailbox(address), AuthorityProjection::Mailbox(view)) => {
            transaction.execute(
                "INSERT INTO authority_mailboxes(owner_id, mailbox_id, create_fact, mailbox_kind, label) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![address.installation_id().as_bytes().as_slice(), address.mailbox_id().as_bytes().as_slice(),
                    view.create_fact.as_bytes().as_slice(), encode_mailbox_kind(view.kind), optional_text(view.label.as_ref())],
            ).map_err(database)?;
        }
        (
            AuthorityProjectionKey::PeerRoute { owner, peer },
            AuthorityProjection::PeerRoute(view),
        ) => {
            transaction.execute(
                "INSERT INTO authority_peer_routes(owner_id, peer_id, route_state) VALUES (?1, ?2, ?3)",
                params![owner.as_bytes().as_slice(), peer.as_bytes().as_slice(), encode_route_state(view.state())],
            ).map_err(database)?;
            for fact in view.frontier() {
                insert_route_fact(transaction, owner, peer, *fact, 1, None)?;
            }
            for (fact, reason) in &view.blocks {
                insert_route_fact(transaction, owner, peer, *fact, 2, Some(reason.as_str()))?;
            }
            for (fact, candidate) in &view.routes {
                transaction.execute(
                    "INSERT INTO authority_peer_route_candidates(owner_id, peer_id, fact_id, candidate_installation, \
                         candidate_signing_key, encryption_key, label) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![owner.as_bytes().as_slice(), peer.as_bytes().as_slice(), fact.as_bytes().as_slice(),
                        candidate.peer.installation_id().as_bytes().as_slice(), candidate.peer.signing_key().as_bytes().as_slice(),
                        candidate.encryption_key.as_bytes().as_slice(), optional_text(candidate.label.as_ref())],
                ).map_err(database)?;
                insert_relays(
                    transaction,
                    RelayOwner::Route(owner, peer, *fact),
                    &candidate.relay_hints,
                )?;
            }
        }
        (
            AuthorityProjectionKey::MailboxCapability(grant),
            AuthorityProjection::MailboxCapability(view),
        ) => {
            transaction.execute(
                "INSERT INTO authority_capabilities(grant_id, mailbox_owner, mailbox_id, grantee_installation, \
                     grantee_signing_key, active) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![grant.as_bytes().as_slice(), view.mailbox.installation_id().as_bytes().as_slice(),
                    view.mailbox.mailbox_id().as_bytes().as_slice(), view.grantee.installation_id().as_bytes().as_slice(),
                    view.grantee.signing_key().as_bytes().as_slice(), i64::from(view.is_active())],
            ).map_err(database)?;
            for fact in &view.revoke_frontier {
                insert_capability_fact(transaction, grant, *fact, 1)?;
            }
            for fact in &view.observed_actions {
                insert_capability_fact(transaction, grant, *fact, 2)?;
            }
        }
        (
            AuthorityProjectionKey::Account(account),
            AuthorityProjection::Account {
                root_fact,
                creator,
                label,
            },
        ) => {
            transaction.execute(
                "INSERT INTO authority_accounts(account_id, root_fact, creator_installation, creator_signing_key, label) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![account.as_bytes().as_slice(), root_fact.as_bytes().as_slice(),
                    creator.installation_id().as_bytes().as_slice(), creator.signing_key().as_bytes().as_slice(),
                    optional_text(label.as_ref())],
            ).map_err(database)?;
        }
        (
            AuthorityProjectionKey::Membership { account, device },
            AuthorityProjection::Membership(view),
        ) => {
            transaction.execute(
                "INSERT INTO authority_memberships(account_id, device_id, membership_state) VALUES (?1, ?2, ?3)",
                params![account.as_bytes().as_slice(), device.as_bytes().as_slice(), encode_membership_state(view.state())],
            ).map_err(database)?;
            for fact in &view.frontier {
                insert_membership_fact(transaction, account, device, *fact, 1)?;
            }
            for fact in &view.acceptances {
                insert_membership_fact(transaction, account, device, *fact, 2)?;
            }
            for fact in &view.revokes {
                insert_membership_fact(transaction, account, device, *fact, 3)?;
            }
            for fact in &view.active_acceptances {
                insert_membership_fact(transaction, account, device, *fact, 4)?;
            }
            for (grant, value) in &view.grants {
                transaction.execute(
                    "INSERT INTO authority_membership_grants(account_id, device_id, grant_id, grant_fact, granted_installation, \
                         granted_signing_key, label) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![account.as_bytes().as_slice(), device.as_bytes().as_slice(), grant.as_bytes().as_slice(),
                        value.grant_fact.as_bytes().as_slice(), value.device.installation_id().as_bytes().as_slice(),
                        value.device.signing_key().as_bytes().as_slice(), optional_text(value.label.as_ref())],
                ).map_err(database)?;
                insert_relays(
                    transaction,
                    RelayOwner::Grant(account, device, *grant),
                    &value.relay_hints,
                )?;
            }
        }
        (
            AuthorityProjectionKey::AccountSelection(installation),
            AuthorityProjection::AccountSelection { candidates, active },
        ) => {
            transaction.execute(
                "INSERT INTO authority_account_selections(installation_id, active_account) VALUES (?1, ?2)",
                params![installation.as_bytes().as_slice(), active.map(|account| account.as_bytes().to_vec())],
            ).map_err(database)?;
            for account in candidates {
                transaction.execute(
                    "INSERT INTO authority_account_selection_candidates(installation_id, account_id) VALUES (?1, ?2)",
                    params![installation.as_bytes().as_slice(), account.as_bytes().as_slice()],
                ).map_err(database)?;
            }
        }
        _ => return Err(corrupt()),
    }
    Ok(())
}

fn insert_route_fact(
    transaction: &Transaction<'_>,
    owner: InstallationId,
    peer: InstallationId,
    fact: FactId,
    relation: i64,
    reason: Option<&str>,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO authority_peer_route_facts(owner_id, peer_id, fact_id, relation, reason) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![owner.as_bytes().as_slice(), peer.as_bytes().as_slice(), fact.as_bytes().as_slice(), relation, reason],
    ).map_err(database)?;
    Ok(())
}

fn insert_capability_fact(
    transaction: &Transaction<'_>,
    grant: GrantId,
    fact: FactId,
    relation: i64,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO authority_capability_facts(grant_id, fact_id, relation) VALUES (?1, ?2, ?3)",
        params![grant.as_bytes().as_slice(), fact.as_bytes().as_slice(), relation],
    ).map_err(database)?;
    Ok(())
}

fn insert_membership_fact(
    transaction: &Transaction<'_>,
    account: AccountId,
    device: InstallationId,
    fact: FactId,
    relation: i64,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO authority_membership_facts(account_id, device_id, fact_id, relation) VALUES (?1, ?2, ?3, ?4)",
        params![account.as_bytes().as_slice(), device.as_bytes().as_slice(), fact.as_bytes().as_slice(), relation],
    ).map_err(database)?;
    Ok(())
}

#[derive(Clone, Copy)]
enum RelayOwner {
    Route(InstallationId, InstallationId, FactId),
    Grant(AccountId, InstallationId, GrantId),
}

fn insert_relays(
    transaction: &Transaction<'_>,
    owner: RelayOwner,
    relays: &RelayHints,
) -> Result<(), StoreError> {
    for (position, relay) in relays.as_slice().iter().enumerate() {
        let position = i64::try_from(position).map_err(|_| corrupt())?;
        match owner {
            RelayOwner::Route(route_owner, peer, fact) => transaction.execute(
                "INSERT INTO authority_peer_route_relays(owner_id, peer_id, fact_id, position, scheme, value) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![route_owner.as_bytes().as_slice(), peer.as_bytes().as_slice(), fact.as_bytes().as_slice(),
                    position, encode_scheme(relay.scheme()), relay.value()],
            ),
            RelayOwner::Grant(account, device, grant) => transaction.execute(
                "INSERT INTO authority_membership_grant_relays(account_id, device_id, grant_id, position, scheme, value) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![account.as_bytes().as_slice(), device.as_bytes().as_slice(), grant.as_bytes().as_slice(),
                    position, encode_scheme(relay.scheme()), relay.value()],
            ),
        }.map_err(database)?;
    }
    Ok(())
}

fn optional_text(value: Option<&ShortText>) -> Option<&str> {
    value.map(BoundedText::as_str)
}

type DecodeKey<K> = fn(i64, [u8; 32], Option<[u8; 32]>) -> Option<K>;

fn load_keyed_facts<K: Ord>(
    connection: &Connection,
    table: &str,
    decode: DecodeKey<K>,
) -> Result<BTreeMap<K, BTreeSet<FactId>>, StoreError> {
    let sql = match table {
        "authority_frontiers" => {
            "SELECT key_kind, key_a, key_b, fact_id FROM authority_frontiers \
             ORDER BY key_kind, key_a, key_b, fact_id"
        }
        "authority_support" => {
            "SELECT key_kind, key_a, key_b, fact_id FROM authority_support \
             ORDER BY key_kind, key_a, key_b, fact_id"
        }
        _ => return Err(corrupt()),
    };
    let mut output = BTreeMap::<K, BTreeSet<FactId>>::new();
    let mut statement = connection.prepare(sql).map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })
        .map_err(database)?;
    for row in rows {
        let (kind, a, b, fact) = row.map_err(database)?;
        let b = fixed(b)?;
        let b = matches!(kind, 2 | 3 | 6).then_some(b);
        let key = decode(kind, fixed(a)?, b).ok_or_else(corrupt)?;
        if !output
            .entry(key)
            .or_default()
            .insert(FactId::from_bytes(fixed(fact)?))
        {
            return Err(corrupt());
        }
    }
    Ok(output)
}

fn load_projections(
    connection: &Connection,
) -> Result<BTreeMap<AuthorityProjectionKey, AuthorityProjection>, StoreError> {
    let mut output = BTreeMap::new();
    load_installations(connection, &mut output)?;
    load_mailboxes(connection, &mut output)?;
    load_routes(connection, &mut output)?;
    load_capabilities(connection, &mut output)?;
    load_accounts(connection, &mut output)?;
    load_memberships(connection, &mut output)?;
    load_selections(connection, &mut output)?;
    Ok(output)
}

fn load_installations(
    connection: &Connection,
    output: &mut BTreeMap<AuthorityProjectionKey, AuthorityProjection>,
) -> Result<(), StoreError> {
    let rows = rows5(
        connection,
        "SELECT installation_id, root_fact, signing_key, encryption_key, label \
         FROM authority_installations ORDER BY installation_id",
    )?;
    for (installation, root, signing, encryption, label) in rows {
        insert_loaded(
            output,
            AuthorityProjectionKey::Installation(InstallationId::from_bytes(fixed(installation)?)),
            AuthorityProjection::Installation(InstallationView {
                root_fact: FactId::from_bytes(fixed(root)?),
                signing_key: SigningPublicKey::from_bytes(fixed(signing)?),
                encryption_key: EncryptionPublicKey::from_bytes(fixed(encryption)?),
                label: short_text(label)?,
            }),
        )?;
    }
    Ok(())
}

fn load_mailboxes(
    connection: &Connection,
    output: &mut BTreeMap<AuthorityProjectionKey, AuthorityProjection>,
) -> Result<(), StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT owner_id, mailbox_id, create_fact, mailbox_kind, label \
         FROM authority_mailboxes ORDER BY owner_id, mailbox_id",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(database)?;
    for row in rows {
        let (owner, mailbox, create, kind, label) = row.map_err(database)?;
        let address = MailboxAddress::new(
            InstallationId::from_bytes(fixed(owner)?),
            MailboxId::from_bytes(fixed(mailbox)?),
        );
        insert_loaded(
            output,
            AuthorityProjectionKey::Mailbox(address),
            AuthorityProjection::Mailbox(MailboxView {
                create_fact: FactId::from_bytes(fixed(create)?),
                kind: decode_mailbox_kind(kind).ok_or_else(corrupt)?,
                label: short_text(label)?,
            }),
        )?;
    }
    Ok(())
}

fn load_routes(
    connection: &Connection,
    output: &mut BTreeMap<AuthorityProjectionKey, AuthorityProjection>,
) -> Result<(), StoreError> {
    let mut statement = connection.prepare(
        "SELECT owner_id, peer_id, route_state FROM authority_peer_routes ORDER BY owner_id, peer_id",
    ).map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(database)?;
    for row in rows {
        let (owner, peer, state) = row.map_err(database)?;
        let owner = InstallationId::from_bytes(fixed(owner)?);
        let peer = InstallationId::from_bytes(fixed(peer)?);
        let (frontier, blocks) = load_route_facts(connection, owner, peer)?;
        let routes = load_route_candidates(connection, owner, peer)?;
        let view = PeerRouteView::from_parts(
            decode_route_state(state).ok_or_else(corrupt)?,
            frontier,
            routes,
            blocks,
        )
        .ok_or_else(corrupt)?;
        insert_loaded(
            output,
            AuthorityProjectionKey::PeerRoute { owner, peer },
            AuthorityProjection::PeerRoute(view),
        )?;
    }
    Ok(())
}

fn load_route_facts(
    connection: &Connection,
    owner: InstallationId,
    peer: InstallationId,
) -> Result<(BTreeSet<FactId>, BTreeMap<FactId, ErrorCode>), StoreError> {
    let mut frontier = BTreeSet::new();
    let mut blocks = BTreeMap::new();
    let mut statement = connection
        .prepare(
            "SELECT fact_id, relation, reason FROM authority_peer_route_facts \
         WHERE owner_id = ?1 AND peer_id = ?2 ORDER BY fact_id, relation",
        )
        .map_err(database)?;
    let rows = statement
        .query_map(
            params![owner.as_bytes().as_slice(), peer.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .map_err(database)?;
    for row in rows {
        let (fact, relation, reason) = row.map_err(database)?;
        let fact = FactId::from_bytes(fixed(fact)?);
        match (relation, reason) {
            (1, None) if frontier.insert(fact) => {}
            (2, Some(reason)) => {
                if blocks
                    .insert(fact, ErrorCode::new(reason).map_err(|_| corrupt())?)
                    .is_some()
                {
                    return Err(corrupt());
                }
            }
            _ => return Err(corrupt()),
        }
    }
    Ok((frontier, blocks))
}

fn load_route_candidates(
    connection: &Connection,
    owner: InstallationId,
    peer: InstallationId,
) -> Result<BTreeMap<FactId, PeerRouteCandidate>, StoreError> {
    let mut output = BTreeMap::new();
    let mut statement = connection.prepare(
        "SELECT fact_id, candidate_installation, candidate_signing_key, encryption_key, label \
         FROM authority_peer_route_candidates WHERE owner_id = ?1 AND peer_id = ?2 ORDER BY fact_id",
    ).map_err(database)?;
    let rows = statement
        .query_map(
            params![owner.as_bytes().as_slice(), peer.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .map_err(database)?;
    for row in rows {
        let (fact, installation, signing, encryption, label) = row.map_err(database)?;
        let fact = FactId::from_bytes(fixed(fact)?);
        let candidate = PeerRouteCandidate {
            peer: InstallationAddress::new(
                InstallationId::from_bytes(fixed(installation)?),
                SigningPublicKey::from_bytes(fixed(signing)?),
            ),
            encryption_key: EncryptionPublicKey::from_bytes(fixed(encryption)?),
            label: short_text(label)?,
            relay_hints: load_relays(connection, RelayOwner::Route(owner, peer, fact))?,
        };
        if candidate.peer.installation_id() != peer {
            return Err(corrupt());
        }
        if output.insert(fact, candidate).is_some() {
            return Err(corrupt());
        }
    }
    Ok(output)
}

fn load_capabilities(
    connection: &Connection,
    output: &mut BTreeMap<AuthorityProjectionKey, AuthorityProjection>,
) -> Result<(), StoreError> {
    let mut statement = connection.prepare(
        "SELECT grant_id, mailbox_owner, mailbox_id, grantee_installation, grantee_signing_key, active \
         FROM authority_capabilities ORDER BY grant_id",
    ).map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(database)?;
    for row in rows {
        let (grant, owner, mailbox, grantee, signing, active) = row.map_err(database)?;
        let grant = GrantId::from_bytes(fixed(grant)?);
        let (revokes, observations) = load_capability_facts(connection, grant)?;
        let view = CapabilityView::from_parts(
            MailboxAddress::new(
                InstallationId::from_bytes(fixed(owner)?),
                MailboxId::from_bytes(fixed(mailbox)?),
            ),
            InstallationAddress::new(
                InstallationId::from_bytes(fixed(grantee)?),
                SigningPublicKey::from_bytes(fixed(signing)?),
            ),
            decode_bool(active)?,
            revokes,
            observations,
        )
        .ok_or_else(corrupt)?;
        insert_loaded(
            output,
            AuthorityProjectionKey::MailboxCapability(grant),
            AuthorityProjection::MailboxCapability(view),
        )?;
    }
    Ok(())
}

fn load_capability_facts(
    connection: &Connection,
    grant: GrantId,
) -> Result<(BTreeSet<FactId>, BTreeSet<FactId>), StoreError> {
    let mut first = BTreeSet::new();
    let mut second = BTreeSet::new();
    let mut statement = connection.prepare(
        "SELECT fact_id, relation FROM authority_capability_facts WHERE grant_id = ?1 ORDER BY fact_id, relation",
    ).map_err(database)?;
    let rows = statement
        .query_map([grant.as_bytes().as_slice()], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(database)?;
    for row in rows {
        let (fact, relation) = row.map_err(database)?;
        let set = match relation {
            1 => &mut first,
            2 => &mut second,
            _ => return Err(corrupt()),
        };
        if !set.insert(FactId::from_bytes(fixed(fact)?)) {
            return Err(corrupt());
        }
    }
    Ok((first, second))
}

fn load_accounts(
    connection: &Connection,
    output: &mut BTreeMap<AuthorityProjectionKey, AuthorityProjection>,
) -> Result<(), StoreError> {
    let rows = rows5(
        connection,
        "SELECT account_id, root_fact, creator_installation, creator_signing_key, label \
        FROM authority_accounts ORDER BY account_id",
    )?;
    for (account, root, installation, signing, label) in rows {
        let account = AccountId::from_bytes(fixed(account)?);
        insert_loaded(
            output,
            AuthorityProjectionKey::Account(account),
            AuthorityProjection::Account {
                root_fact: FactId::from_bytes(fixed(root)?),
                creator: InstallationAddress::new(
                    InstallationId::from_bytes(fixed(installation)?),
                    SigningPublicKey::from_bytes(fixed(signing)?),
                ),
                label: short_text(label)?,
            },
        )?;
    }
    Ok(())
}

fn load_memberships(
    connection: &Connection,
    output: &mut BTreeMap<AuthorityProjectionKey, AuthorityProjection>,
) -> Result<(), StoreError> {
    let mut statement = connection.prepare(
        "SELECT account_id, device_id, membership_state FROM authority_memberships ORDER BY account_id, device_id",
    ).map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(database)?;
    for row in rows {
        let (account, device, state) = row.map_err(database)?;
        let account = AccountId::from_bytes(fixed(account)?);
        let device = InstallationId::from_bytes(fixed(device)?);
        let facts = load_membership_facts(connection, account, device)?;
        let grants = load_membership_grants(connection, account, device)?;
        let view = MembershipView::from_parts(
            decode_membership_state(state).ok_or_else(corrupt)?,
            facts[0].clone(),
            grants,
            facts[1].clone(),
            facts[2].clone(),
            facts[3].clone(),
        )
        .ok_or_else(corrupt)?;
        insert_loaded(
            output,
            AuthorityProjectionKey::Membership { account, device },
            AuthorityProjection::Membership(view),
        )?;
    }
    Ok(())
}

fn load_membership_facts(
    connection: &Connection,
    account: AccountId,
    device: InstallationId,
) -> Result<[BTreeSet<FactId>; 4], StoreError> {
    let mut sets = std::array::from_fn(|_| BTreeSet::new());
    let mut statement = connection.prepare(
        "SELECT fact_id, relation FROM authority_membership_facts WHERE account_id = ?1 AND device_id = ?2 \
         ORDER BY fact_id, relation",
    ).map_err(database)?;
    let rows = statement
        .query_map(
            params![account.as_bytes().as_slice(), device.as_bytes().as_slice()],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(database)?;
    for row in rows {
        let (fact, relation) = row.map_err(database)?;
        let index = usize::try_from(relation - 1).map_err(|_| corrupt())?;
        let set = sets.get_mut(index).ok_or_else(corrupt)?;
        if !set.insert(FactId::from_bytes(fixed(fact)?)) {
            return Err(corrupt());
        }
    }
    Ok(sets)
}

fn load_membership_grants(
    connection: &Connection,
    account: AccountId,
    device: InstallationId,
) -> Result<BTreeMap<GrantId, DeviceGrantView>, StoreError> {
    let mut output = BTreeMap::new();
    let mut statement = connection.prepare(
        "SELECT grant_id, grant_fact, granted_installation, granted_signing_key, label \
         FROM authority_membership_grants WHERE account_id = ?1 AND device_id = ?2 ORDER BY grant_id",
    ).map_err(database)?;
    let rows = statement
        .query_map(
            params![account.as_bytes().as_slice(), device.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .map_err(database)?;
    for row in rows {
        let (grant, fact, installation, signing, label) = row.map_err(database)?;
        let grant = GrantId::from_bytes(fixed(grant)?);
        let value = DeviceGrantView {
            grant_fact: FactId::from_bytes(fixed(fact)?),
            device: InstallationAddress::new(
                InstallationId::from_bytes(fixed(installation)?),
                SigningPublicKey::from_bytes(fixed(signing)?),
            ),
            label: short_text(label)?,
            relay_hints: load_relays(connection, RelayOwner::Grant(account, device, grant))?,
        };
        if value.device.installation_id() != device {
            return Err(corrupt());
        }
        if output.insert(grant, value).is_some() {
            return Err(corrupt());
        }
    }
    Ok(output)
}

fn load_relays(connection: &Connection, owner: RelayOwner) -> Result<RelayHints, StoreError> {
    let (sql, values): (&str, Vec<Vec<u8>>) = match owner {
        RelayOwner::Route(route_owner, peer, fact) => (
            "SELECT position, scheme, value FROM authority_peer_route_relays \
             WHERE owner_id = ?1 AND peer_id = ?2 AND fact_id = ?3 ORDER BY position",
            vec![
                route_owner.as_bytes().to_vec(),
                peer.as_bytes().to_vec(),
                fact.as_bytes().to_vec(),
            ],
        ),
        RelayOwner::Grant(account, device, grant) => (
            "SELECT position, scheme, value FROM authority_membership_grant_relays \
             WHERE account_id = ?1 AND device_id = ?2 AND grant_id = ?3 ORDER BY position",
            vec![
                account.as_bytes().to_vec(),
                device.as_bytes().to_vec(),
                grant.as_bytes().to_vec(),
            ],
        ),
    };
    let mut statement = connection.prepare(sql).map_err(database)?;
    let rows = statement
        .query_map(
            params![
                values[0].as_slice(),
                values[1].as_slice(),
                values[2].as_slice()
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(database)?;
    let mut relays = Vec::new();
    for row in rows {
        let (position, scheme, value) = row.map_err(database)?;
        if position != i64::try_from(relays.len()).map_err(|_| corrupt())? {
            return Err(corrupt());
        }
        let value = BoundedText::new(value).map_err(|_| corrupt())?;
        relays.push(ResourceLocator::new(
            decode_scheme(scheme).ok_or_else(corrupt)?,
            value,
        ));
    }
    RelayHints::new(relays).map_err(|_| corrupt())
}

fn load_selections(
    connection: &Connection,
    output: &mut BTreeMap<AuthorityProjectionKey, AuthorityProjection>,
) -> Result<(), StoreError> {
    let mut statement = connection.prepare(
        "SELECT installation_id, active_account FROM authority_account_selections ORDER BY installation_id",
    ).map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Option<Vec<u8>>>(1)?))
        })
        .map_err(database)?;
    for row in rows {
        let (installation, active) = row.map_err(database)?;
        let installation = InstallationId::from_bytes(fixed(installation)?);
        let active = optional_fixed(active)?.map(AccountId::from_bytes);
        let candidates = load_selection_candidates(connection, installation)?;
        let expected_active = (candidates.len() == 1)
            .then(|| candidates.iter().next().copied())
            .flatten();
        if active != expected_active {
            return Err(corrupt());
        }
        insert_loaded(
            output,
            AuthorityProjectionKey::AccountSelection(installation),
            AuthorityProjection::AccountSelection { candidates, active },
        )?;
    }
    Ok(())
}

fn load_selection_candidates(
    connection: &Connection,
    installation: InstallationId,
) -> Result<BTreeSet<AccountId>, StoreError> {
    let mut output = BTreeSet::new();
    let mut statement = connection.prepare(
        "SELECT account_id FROM authority_account_selection_candidates WHERE installation_id = ?1 ORDER BY account_id",
    ).map_err(database)?;
    let rows = statement
        .query_map([installation.as_bytes().as_slice()], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .map_err(database)?;
    for row in rows {
        if !output.insert(AccountId::from_bytes(fixed(row.map_err(database)?)?)) {
            return Err(corrupt());
        }
    }
    Ok(output)
}

fn insert_loaded(
    output: &mut BTreeMap<AuthorityProjectionKey, AuthorityProjection>,
    key: AuthorityProjectionKey,
    value: AuthorityProjection,
) -> Result<(), StoreError> {
    if output.insert(key, value).is_some() {
        Err(corrupt())
    } else {
        Ok(())
    }
}

type FiveColumns = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, Option<String>);

fn rows5(connection: &Connection, sql: &str) -> Result<Vec<FiveColumns>, StoreError> {
    let mut statement = connection.prepare(sql).map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .map_err(database)?;
    rows.map(|row| row.map_err(database)).collect()
}

#[derive(Clone, Copy)]
struct KeyParts {
    kind: i64,
    a: [u8; 32],
    b: Option<[u8; 32]>,
}

fn aggregate_key_parts(key: AuthorityAggregateKey) -> KeyParts {
    match key {
        AuthorityAggregateKey::Installation(value) => key_parts(1, *value.as_bytes(), None),
        AuthorityAggregateKey::Mailbox(value) => key_parts(
            2,
            *value.installation_id().as_bytes(),
            Some(*value.mailbox_id().as_bytes()),
        ),
        AuthorityAggregateKey::PeerRoute { owner, peer } => {
            key_parts(3, *owner.as_bytes(), Some(*peer.as_bytes()))
        }
        AuthorityAggregateKey::MailboxCapability(value) => key_parts(4, *value.as_bytes(), None),
        AuthorityAggregateKey::Account(value) => key_parts(5, *value.as_bytes(), None),
        AuthorityAggregateKey::Membership { account, device } => {
            key_parts(6, *account.as_bytes(), Some(*device.as_bytes()))
        }
        AuthorityAggregateKey::AccountSelection(value) => key_parts(7, *value.as_bytes(), None),
    }
}

fn projection_key_parts(key: AuthorityProjectionKey) -> KeyParts {
    match key {
        AuthorityProjectionKey::Installation(value) => key_parts(1, *value.as_bytes(), None),
        AuthorityProjectionKey::Mailbox(value) => key_parts(
            2,
            *value.installation_id().as_bytes(),
            Some(*value.mailbox_id().as_bytes()),
        ),
        AuthorityProjectionKey::PeerRoute { owner, peer } => {
            key_parts(3, *owner.as_bytes(), Some(*peer.as_bytes()))
        }
        AuthorityProjectionKey::MailboxCapability(value) => key_parts(4, *value.as_bytes(), None),
        AuthorityProjectionKey::Account(value) => key_parts(5, *value.as_bytes(), None),
        AuthorityProjectionKey::Membership { account, device } => {
            key_parts(6, *account.as_bytes(), Some(*device.as_bytes()))
        }
        AuthorityProjectionKey::AccountSelection(value) => key_parts(7, *value.as_bytes(), None),
    }
}

const fn key_parts(kind: i64, a: [u8; 32], b: Option<[u8; 32]>) -> KeyParts {
    KeyParts { kind, a, b }
}

fn decode_aggregate_key(
    kind: i64,
    a: [u8; 32],
    b: Option<[u8; 32]>,
) -> Option<AuthorityAggregateKey> {
    match (kind, b) {
        (1, None) => Some(AuthorityAggregateKey::Installation(
            InstallationId::from_bytes(a),
        )),
        (2, Some(b)) => Some(AuthorityAggregateKey::Mailbox(MailboxAddress::new(
            InstallationId::from_bytes(a),
            MailboxId::from_bytes(b),
        ))),
        (3, Some(b)) => Some(AuthorityAggregateKey::PeerRoute {
            owner: InstallationId::from_bytes(a),
            peer: InstallationId::from_bytes(b),
        }),
        (4, None) => Some(AuthorityAggregateKey::MailboxCapability(
            GrantId::from_bytes(a),
        )),
        (5, None) => Some(AuthorityAggregateKey::Account(AccountId::from_bytes(a))),
        (6, Some(b)) => Some(AuthorityAggregateKey::Membership {
            account: AccountId::from_bytes(a),
            device: InstallationId::from_bytes(b),
        }),
        (7, None) => Some(AuthorityAggregateKey::AccountSelection(
            InstallationId::from_bytes(a),
        )),
        _ => None,
    }
}

fn decode_projection_key(
    kind: i64,
    a: [u8; 32],
    b: Option<[u8; 32]>,
) -> Option<AuthorityProjectionKey> {
    match (kind, b) {
        (1, None) => Some(AuthorityProjectionKey::Installation(
            InstallationId::from_bytes(a),
        )),
        (2, Some(b)) => Some(AuthorityProjectionKey::Mailbox(MailboxAddress::new(
            InstallationId::from_bytes(a),
            MailboxId::from_bytes(b),
        ))),
        (3, Some(b)) => Some(AuthorityProjectionKey::PeerRoute {
            owner: InstallationId::from_bytes(a),
            peer: InstallationId::from_bytes(b),
        }),
        (4, None) => Some(AuthorityProjectionKey::MailboxCapability(
            GrantId::from_bytes(a),
        )),
        (5, None) => Some(AuthorityProjectionKey::Account(AccountId::from_bytes(a))),
        (6, Some(b)) => Some(AuthorityProjectionKey::Membership {
            account: AccountId::from_bytes(a),
            device: InstallationId::from_bytes(b),
        }),
        (7, None) => Some(AuthorityProjectionKey::AccountSelection(
            InstallationId::from_bytes(a),
        )),
        _ => None,
    }
}

const fn encode_mailbox_kind(value: MailboxKind) -> i64 {
    match value {
        MailboxKind::Human => 1,
        MailboxKind::Agent => 2,
    }
}

const fn decode_mailbox_kind(value: i64) -> Option<MailboxKind> {
    match value {
        1 => Some(MailboxKind::Human),
        2 => Some(MailboxKind::Agent),
        _ => None,
    }
}

const fn encode_route_state(value: PeerRouteState) -> i64 {
    match value {
        PeerRouteState::Routable => 1,
        PeerRouteState::Blocked => 2,
        PeerRouteState::Conflicted => 3,
    }
}

const fn decode_route_state(value: i64) -> Option<PeerRouteState> {
    match value {
        1 => Some(PeerRouteState::Routable),
        2 => Some(PeerRouteState::Blocked),
        3 => Some(PeerRouteState::Conflicted),
        _ => None,
    }
}

const fn encode_membership_state(value: MembershipState) -> i64 {
    match value {
        MembershipState::Pending => 1,
        MembershipState::Active => 2,
        MembershipState::Revoked => 3,
    }
}

const fn decode_membership_state(value: i64) -> Option<MembershipState> {
    match value {
        1 => Some(MembershipState::Pending),
        2 => Some(MembershipState::Active),
        3 => Some(MembershipState::Revoked),
        _ => None,
    }
}

const fn encode_scheme(value: ResourceScheme) -> i64 {
    match value {
        ResourceScheme::GitRepository => 1,
        ResourceScheme::WorkingTree => 2,
        ResourceScheme::Container => 3,
        ResourceScheme::Opaque => 4,
    }
}

const fn decode_scheme(value: i64) -> Option<ResourceScheme> {
    match value {
        1 => Some(ResourceScheme::GitRepository),
        2 => Some(ResourceScheme::WorkingTree),
        3 => Some(ResourceScheme::Container),
        4 => Some(ResourceScheme::Opaque),
        _ => None,
    }
}

fn short_text(value: Option<String>) -> Result<Option<ShortText>, StoreError> {
    value.map(ShortText::new).transpose().map_err(|_| corrupt())
}

fn decode_bool(value: i64) -> Result<bool, StoreError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(corrupt()),
    }
}

fn fixed(value: Vec<u8>) -> Result<[u8; 32], StoreError> {
    value.try_into().map_err(|_| corrupt())
}

fn optional_fixed(value: Option<Vec<u8>>) -> Result<Option<[u8; 32]>, StoreError> {
    value.map(fixed).transpose()
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Counts {
    frontier_count: i64,
    projection_count: i64,
    support_count: i64,
    row_count: i64,
}

impl Counts {
    fn read(connection: &Connection) -> Result<Self, StoreError> {
        connection.query_row(
            "SELECT (SELECT count(*) FROM authority_frontiers), \
                    (SELECT count(*) FROM authority_installations) + (SELECT count(*) FROM authority_mailboxes) + \
                    (SELECT count(*) FROM authority_peer_routes) + (SELECT count(*) FROM authority_capabilities) + \
                    (SELECT count(*) FROM authority_accounts) + (SELECT count(*) FROM authority_memberships) + \
                    (SELECT count(*) FROM authority_account_selections), \
                    (SELECT count(*) FROM authority_support), \
                    (SELECT count(*) FROM authority_frontiers) + (SELECT count(*) FROM authority_support) + \
                    (SELECT count(*) FROM authority_installations) + (SELECT count(*) FROM authority_mailboxes) + \
                    (SELECT count(*) FROM authority_peer_routes) + (SELECT count(*) FROM authority_peer_route_facts) + \
                    (SELECT count(*) FROM authority_peer_route_candidates) + (SELECT count(*) FROM authority_peer_route_relays) + \
                    (SELECT count(*) FROM authority_capabilities) + (SELECT count(*) FROM authority_capability_facts) + \
                    (SELECT count(*) FROM authority_accounts) + (SELECT count(*) FROM authority_memberships) + \
                    (SELECT count(*) FROM authority_membership_facts) + (SELECT count(*) FROM authority_membership_grants) + \
                    (SELECT count(*) FROM authority_membership_grant_relays) + (SELECT count(*) FROM authority_account_selections) + \
                    (SELECT count(*) FROM authority_account_selection_candidates)",
            [],
            |row| Ok(Self { frontier_count: row.get(0)?, projection_count: row.get(1)?, support_count: row.get(2)?, row_count: row.get(3)? }),
        ).map_err(database)
    }

    fn validate(self) -> Result<(), StoreError> {
        if [
            self.frontier_count,
            self.projection_count,
            self.support_count,
            self.row_count,
        ]
        .into_iter()
        .all(|value| (0..=MAXIMUM_AUTHORITY_ROWS).contains(&value))
        {
            Ok(())
        } else {
            Err(corrupt())
        }
    }
}

struct State {
    counts: Counts,
    digest: [u8; 32],
}

fn load_state(connection: &Connection) -> Result<Option<State>, StoreError> {
    connection.query_row(
        "SELECT frontier_count, projection_count, support_count, row_count, row_digest FROM authority_state WHERE singleton = 1",
        [], |row| Ok((Counts { frontier_count: row.get(0)?, projection_count: row.get(1)?, support_count: row.get(2)?, row_count: row.get(3)? }, row.get::<_, Vec<u8>>(4)?)),
    ).optional().map_err(database)?.map(|(counts, digest)| Ok(State { counts, digest: fixed(digest)? })).transpose()
}

fn validate_snapshot(
    snapshot: &AuthorityProjectionSnapshot,
    counts: Counts,
) -> Result<(), StoreError> {
    if counts.frontier_count != length(snapshot.frontiers.values().map(BTreeSet::len))?
        || counts.projection_count
            != i64::try_from(snapshot.projections.len()).map_err(|_| corrupt())?
        || counts.support_count != length(snapshot.support.values().map(BTreeSet::len))?
        || snapshot
            .support
            .keys()
            .any(|key| !snapshot.projections.contains_key(key))
        || snapshot
            .projections
            .keys()
            .any(|key| !snapshot.support.contains_key(key))
    {
        Err(corrupt())
    } else {
        Ok(())
    }
}

fn length(values: impl IntoIterator<Item = usize>) -> Result<i64, StoreError> {
    values.into_iter().try_fold(0_i64, |total, value| {
        total
            .checked_add(i64::try_from(value).map_err(|_| corrupt())?)
            .ok_or_else(corrupt)
    })
}

fn row_digest(connection: &Connection) -> Result<[u8; 32], StoreError> {
    let mut digest = Sha256::new();
    digest.update(b"hq-authority-rows-v1\0");
    for (table, query) in DIGEST_QUERIES {
        digest.update(table.as_bytes());
        digest.update([0]);
        let mut statement = connection.prepare(query).map_err(database)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(database)?;
        for row in rows {
            let row = row.map_err(database)?;
            digest.update(
                u64::try_from(row.len())
                    .map_err(|_| corrupt())?
                    .to_be_bytes(),
            );
            digest.update(row.as_bytes());
        }
    }
    Ok(digest.finalize().into())
}

const DIGEST_QUERIES: &[(&str, &str)] = &[
    (
        "frontiers",
        "SELECT quote(key_kind)||'|'||quote(key_a)||'|'||ifnull(quote(key_b),'NULL')||'|'||quote(fact_id) FROM authority_frontiers ORDER BY key_kind,key_a,key_b,fact_id",
    ),
    (
        "support",
        "SELECT quote(key_kind)||'|'||quote(key_a)||'|'||ifnull(quote(key_b),'NULL')||'|'||quote(fact_id) FROM authority_support ORDER BY key_kind,key_a,key_b,fact_id",
    ),
    (
        "installations",
        "SELECT quote(installation_id)||'|'||quote(root_fact)||'|'||quote(signing_key)||'|'||quote(encryption_key)||'|'||ifnull(quote(label),'NULL') FROM authority_installations ORDER BY installation_id",
    ),
    (
        "mailboxes",
        "SELECT quote(owner_id)||'|'||quote(mailbox_id)||'|'||quote(create_fact)||'|'||quote(mailbox_kind)||'|'||ifnull(quote(label),'NULL') FROM authority_mailboxes ORDER BY owner_id,mailbox_id",
    ),
    (
        "routes",
        "SELECT quote(owner_id)||'|'||quote(peer_id)||'|'||quote(route_state) FROM authority_peer_routes ORDER BY owner_id,peer_id",
    ),
    (
        "route-facts",
        "SELECT quote(owner_id)||'|'||quote(peer_id)||'|'||quote(fact_id)||'|'||quote(relation)||'|'||ifnull(quote(reason),'NULL') FROM authority_peer_route_facts ORDER BY owner_id,peer_id,fact_id,relation",
    ),
    (
        "route-candidates",
        "SELECT quote(owner_id)||'|'||quote(peer_id)||'|'||quote(fact_id)||'|'||quote(candidate_installation)||'|'||quote(candidate_signing_key)||'|'||quote(encryption_key)||'|'||ifnull(quote(label),'NULL') FROM authority_peer_route_candidates ORDER BY owner_id,peer_id,fact_id",
    ),
    (
        "route-relays",
        "SELECT quote(owner_id)||'|'||quote(peer_id)||'|'||quote(fact_id)||'|'||quote(position)||'|'||quote(scheme)||'|'||quote(value) FROM authority_peer_route_relays ORDER BY owner_id,peer_id,fact_id,position",
    ),
    (
        "capabilities",
        "SELECT quote(grant_id)||'|'||quote(mailbox_owner)||'|'||quote(mailbox_id)||'|'||quote(grantee_installation)||'|'||quote(grantee_signing_key)||'|'||quote(active) FROM authority_capabilities ORDER BY grant_id",
    ),
    (
        "capability-facts",
        "SELECT quote(grant_id)||'|'||quote(fact_id)||'|'||quote(relation) FROM authority_capability_facts ORDER BY grant_id,fact_id,relation",
    ),
    (
        "accounts",
        "SELECT quote(account_id)||'|'||quote(root_fact)||'|'||quote(creator_installation)||'|'||quote(creator_signing_key)||'|'||ifnull(quote(label),'NULL') FROM authority_accounts ORDER BY account_id",
    ),
    (
        "memberships",
        "SELECT quote(account_id)||'|'||quote(device_id)||'|'||quote(membership_state) FROM authority_memberships ORDER BY account_id,device_id",
    ),
    (
        "membership-facts",
        "SELECT quote(account_id)||'|'||quote(device_id)||'|'||quote(fact_id)||'|'||quote(relation) FROM authority_membership_facts ORDER BY account_id,device_id,fact_id,relation",
    ),
    (
        "membership-grants",
        "SELECT quote(account_id)||'|'||quote(device_id)||'|'||quote(grant_id)||'|'||quote(grant_fact)||'|'||quote(granted_installation)||'|'||quote(granted_signing_key)||'|'||ifnull(quote(label),'NULL') FROM authority_membership_grants ORDER BY account_id,device_id,grant_id",
    ),
    (
        "membership-relays",
        "SELECT quote(account_id)||'|'||quote(device_id)||'|'||quote(grant_id)||'|'||quote(position)||'|'||quote(scheme)||'|'||quote(value) FROM authority_membership_grant_relays ORDER BY account_id,device_id,grant_id,position",
    ),
    (
        "selections",
        "SELECT quote(installation_id)||'|'||ifnull(quote(active_account),'NULL') FROM authority_account_selections ORDER BY installation_id",
    ),
    (
        "selection-candidates",
        "SELECT quote(installation_id)||'|'||quote(account_id) FROM authority_account_selection_candidates ORDER BY installation_id,account_id",
    ),
];

fn database(_: rusqlite::Error) -> StoreError {
    StoreError::new(StoreErrorClass::DatabaseUnavailable)
}

fn corrupt() -> StoreError {
    StoreError::new(StoreErrorClass::RebuildableStateCorrupt)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn every_authority_projection_variant_round_trips_relationally() {
        let expected = complete_fixture();
        let connection = fixture_connection(&expected);

        assert_eq!(load(&connection).expect("authority rows load"), expected);
    }

    #[test]
    fn every_authority_table_family_fails_closed_on_valid_looking_corruption() {
        for mutation in [
            "UPDATE authority_state SET row_count = 64000001",
            "UPDATE authority_frontiers SET key_kind = 5 WHERE key_kind = 1",
            "UPDATE authority_support SET fact_id = (SELECT fact_id FROM canonical_facts ORDER BY fact_id DESC LIMIT 1) WHERE key_kind = 1",
            "UPDATE authority_installations SET label = 'changed'",
            "UPDATE authority_mailboxes SET mailbox_kind = 1",
            "UPDATE authority_peer_routes SET route_state = 1",
            "UPDATE authority_peer_route_facts SET reason = 'changed' WHERE relation = 2",
            "UPDATE authority_peer_route_candidates SET encryption_key = zeroblob(32)",
            "UPDATE authority_peer_route_relays SET value = 'changed'",
            "UPDATE authority_capabilities SET active = 1",
            "UPDATE authority_capability_facts SET relation = 2 WHERE relation = 1",
            "UPDATE authority_accounts SET label = 'changed'",
            "UPDATE authority_memberships SET membership_state = 1",
            "UPDATE authority_membership_facts SET fact_id = (SELECT fact_id FROM canonical_facts ORDER BY fact_id DESC LIMIT 1) WHERE relation = 1",
            "UPDATE authority_membership_grants SET label = 'changed'",
            "UPDATE authority_membership_grant_relays SET value = 'changed'",
            "UPDATE authority_account_selections SET active_account = zeroblob(32)",
            "UPDATE authority_account_selection_candidates SET account_id = zeroblob(32)",
        ] {
            let expected = complete_fixture();
            let connection = fixture_connection(&expected);
            connection
                .execute(mutation, [])
                .expect("valid-looking corruption writes");
            let error = load(&connection).expect_err("authority corruption rejects");
            assert_eq!(error.class(), StoreErrorClass::RebuildableStateCorrupt);
        }
    }

    #[test]
    fn authority_scalar_codecs_are_closed() {
        for kind in [MailboxKind::Human, MailboxKind::Agent] {
            assert_eq!(decode_mailbox_kind(encode_mailbox_kind(kind)), Some(kind));
        }
        for state in [
            PeerRouteState::Routable,
            PeerRouteState::Blocked,
            PeerRouteState::Conflicted,
        ] {
            assert_eq!(decode_route_state(encode_route_state(state)), Some(state));
        }
        for state in [
            MembershipState::Pending,
            MembershipState::Active,
            MembershipState::Revoked,
        ] {
            assert_eq!(
                decode_membership_state(encode_membership_state(state)),
                Some(state)
            );
        }
        for scheme in [
            ResourceScheme::GitRepository,
            ResourceScheme::WorkingTree,
            ResourceScheme::Container,
            ResourceScheme::Opaque,
        ] {
            assert_eq!(decode_scheme(encode_scheme(scheme)), Some(scheme));
        }
        assert_eq!(decode_mailbox_kind(3), None);
        assert_eq!(decode_route_state(4), None);
        assert_eq!(decode_membership_state(4), None);
        assert_eq!(decode_scheme(5), None);
        assert!(decode_aggregate_key(1, [1; 32], Some([2; 32])).is_none());
        assert!(decode_projection_key(6, [1; 32], None).is_none());
    }

    #[allow(clippy::too_many_lines)]
    fn complete_fixture() -> AuthorityProjectionSnapshot {
        let installation = InstallationId::from_bytes([0x11; 32]);
        let peer = InstallationId::from_bytes([0x22; 32]);
        let mailbox = MailboxAddress::new(installation, MailboxId::from_bytes([0x33; 32]));
        let grant = GrantId::from_bytes([0x44; 32]);
        let account = AccountId::from_bytes([0x55; 32]);
        let device = InstallationId::from_bytes([0x66; 32]);
        let route_relay = relays(ResourceScheme::Opaque, "wss://route.example");
        let grant_relay = relays(ResourceScheme::GitRepository, "https://account.example");
        let keys = [
            AuthorityProjectionKey::Installation(installation),
            AuthorityProjectionKey::Mailbox(mailbox),
            AuthorityProjectionKey::PeerRoute {
                owner: installation,
                peer,
            },
            AuthorityProjectionKey::MailboxCapability(grant),
            AuthorityProjectionKey::Account(account),
            AuthorityProjectionKey::Membership { account, device },
            AuthorityProjectionKey::AccountSelection(installation),
        ];
        let projections = BTreeMap::from([
            (
                keys[0],
                AuthorityProjection::Installation(InstallationView {
                    root_fact: id(1),
                    signing_key: SigningPublicKey::from_bytes([0x71; 32]),
                    encryption_key: EncryptionPublicKey::from_bytes([0x72; 32]),
                    label: Some(ShortText::new("installation").expect("label validates")),
                }),
            ),
            (
                keys[1],
                AuthorityProjection::Mailbox(MailboxView {
                    create_fact: id(2),
                    kind: MailboxKind::Agent,
                    label: Some(ShortText::new("mailbox").expect("label validates")),
                }),
            ),
            (
                keys[2],
                AuthorityProjection::PeerRoute(
                    PeerRouteView::from_parts(
                        PeerRouteState::Blocked,
                        BTreeSet::from([id(4), id(5)]),
                        BTreeMap::from([(
                            id(4),
                            PeerRouteCandidate {
                                peer: InstallationAddress::new(
                                    peer,
                                    SigningPublicKey::from_bytes([0x73; 32]),
                                ),
                                encryption_key: EncryptionPublicKey::from_bytes([0x74; 32]),
                                label: Some(ShortText::new("peer").expect("label validates")),
                                relay_hints: route_relay,
                            },
                        )]),
                        BTreeMap::from([(
                            id(5),
                            ErrorCode::new("blocked").expect("reason validates"),
                        )]),
                    )
                    .expect("route view is coherent"),
                ),
            ),
            (
                keys[3],
                AuthorityProjection::MailboxCapability(
                    CapabilityView::from_parts(
                        mailbox,
                        InstallationAddress::new(peer, SigningPublicKey::from_bytes([0x75; 32])),
                        false,
                        BTreeSet::from([id(6)]),
                        BTreeSet::from([id(7)]),
                    )
                    .expect("capability view is coherent"),
                ),
            ),
            (
                keys[4],
                AuthorityProjection::Account {
                    root_fact: id(8),
                    creator: InstallationAddress::new(
                        installation,
                        SigningPublicKey::from_bytes([0x76; 32]),
                    ),
                    label: Some(ShortText::new("account").expect("label validates")),
                },
            ),
            (
                keys[5],
                AuthorityProjection::Membership(
                    MembershipView::from_parts(
                        MembershipState::Active,
                        BTreeSet::from([id(11)]),
                        BTreeMap::from([(
                            GrantId::from_bytes([0x77; 32]),
                            DeviceGrantView {
                                grant_fact: id(10),
                                device: InstallationAddress::new(
                                    device,
                                    SigningPublicKey::from_bytes([0x78; 32]),
                                ),
                                label: Some(ShortText::new("device").expect("label validates")),
                                relay_hints: grant_relay,
                            },
                        )]),
                        BTreeSet::from([id(11)]),
                        BTreeSet::from([id(12)]),
                        BTreeSet::from([id(11)]),
                    )
                    .expect("membership view is coherent"),
                ),
            ),
            (
                keys[6],
                AuthorityProjection::AccountSelection {
                    candidates: BTreeSet::from([account]),
                    active: Some(account),
                },
            ),
        ]);
        let frontiers = BTreeMap::from([
            (
                AuthorityAggregateKey::Installation(installation),
                BTreeSet::from([id(1)]),
            ),
            (
                AuthorityAggregateKey::Mailbox(mailbox),
                BTreeSet::from([id(2)]),
            ),
            (
                AuthorityAggregateKey::PeerRoute {
                    owner: installation,
                    peer,
                },
                BTreeSet::from([id(3)]),
            ),
            (
                AuthorityAggregateKey::MailboxCapability(grant),
                BTreeSet::from([id(6)]),
            ),
            (
                AuthorityAggregateKey::Account(account),
                BTreeSet::from([id(8)]),
            ),
            (
                AuthorityAggregateKey::Membership { account, device },
                BTreeSet::from([id(9)]),
            ),
            (
                AuthorityAggregateKey::AccountSelection(installation),
                BTreeSet::from([id(13)]),
            ),
        ]);
        let support = keys
            .into_iter()
            .enumerate()
            .map(|(index, key)| {
                (
                    key,
                    BTreeSet::from([id(u8::try_from(index + 14).expect("fixture id fits"))]),
                )
            })
            .collect();
        AuthorityProjectionSnapshot {
            frontiers,
            projections,
            support,
        }
    }

    fn relays(scheme: ResourceScheme, value: &str) -> RelayHints {
        RelayHints::new([ResourceLocator::new(
            scheme,
            BoundedText::new(value).expect("relay validates"),
        )])
        .expect("relay list validates")
    }

    fn id(value: u8) -> FactId {
        FactId::from_bytes([value; 32])
    }

    fn fixture_connection(expected: &AuthorityProjectionSnapshot) -> Connection {
        let mut connection = Connection::open_in_memory().expect("memory database opens");
        connection
            .execute_batch(super::super::SCHEMA)
            .expect("schema creates");
        for value in 1_u8..=24 {
            connection
                .execute(
                    "INSERT INTO canonical_facts(fact_id, event_bytes, namespace, family) \
                     VALUES (?1, ?2, 1, 1)",
                    params![id(value).as_bytes().as_slice(), vec![value]],
                )
                .expect("canonical support inserts");
        }
        let transaction = connection.transaction().expect("transaction starts");
        insert(&transaction, expected).expect("authority rows insert");
        transaction.commit().expect("authority rows commit");
        connection
    }
}
