//! Private structural-index replacement and row codecs within the database adapter.

use std::collections::{BTreeMap, BTreeSet};

use hq_domain::{FactId, InstallationId, MailboxId};
use hq_reducer::AuthorityPolicy;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::{
    AuthorityProjectionSnapshot, IndexedConflict, IndexedDecision, ReductionDomain,
    ReductionIndexSnapshot, StoreError, StoreErrorClass,
    snapshot::{
        decode_domain, decode_reason, decode_role, decode_status, encode_domain, encode_reason,
        encode_role, encode_status, reason_belongs_to_domain,
    },
};

const MAXIMUM_INDEX_ROWS: i64 = 64_000_000;

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum RepairFailpoint {
    Never,
    AfterClear,
    AfterVertices,
    AfterReverseDependencies,
    AfterDecisions,
    AfterDependencyOrder,
    AfterPresentationOrder,
    AfterConflicts,
    AfterState,
    AfterAuthorityInsert,
    AfterAuthorityVerification,
    AfterVerification,
}

pub(crate) fn replace(
    connection: &mut Connection,
    expected: &ReductionIndexSnapshot,
    expected_authority: &AuthorityProjectionSnapshot,
) -> Result<(ReductionIndexSnapshot, AuthorityProjectionSnapshot), StoreError> {
    replace_at(
        connection,
        expected,
        expected_authority,
        RepairFailpoint::Never,
    )
}

#[cfg(test)]
pub(crate) fn replace_with_failpoint(
    connection: &mut Connection,
    expected: &ReductionIndexSnapshot,
    expected_authority: &AuthorityProjectionSnapshot,
    failpoint: RepairFailpoint,
) -> Result<(ReductionIndexSnapshot, AuthorityProjectionSnapshot), StoreError> {
    replace_at(connection, expected, expected_authority, failpoint)
}

fn replace_at(
    connection: &mut Connection,
    expected: &ReductionIndexSnapshot,
    expected_authority: &AuthorityProjectionSnapshot,
    failpoint: RepairFailpoint,
) -> Result<(ReductionIndexSnapshot, AuthorityProjectionSnapshot), StoreError> {
    let transaction = connection.transaction().map_err(database)?;
    clear_rebuildable(&transaction)?;
    super::authority::clear(&transaction)?;
    fail_at(failpoint, RepairFailpoint::AfterClear)?;
    insert_index(&transaction, expected, failpoint)?;
    super::authority::insert(&transaction, expected_authority)?;
    fail_at(failpoint, RepairFailpoint::AfterAuthorityInsert)?;
    let persisted = load_from_connection(&transaction)?;
    if persisted != *expected {
        return Err(corrupt());
    }
    let persisted_authority = super::authority::load(&transaction)?;
    if persisted_authority != *expected_authority {
        return Err(corrupt());
    }
    fail_at(failpoint, RepairFailpoint::AfterAuthorityVerification)?;
    fail_at(failpoint, RepairFailpoint::AfterVerification)?;
    transaction.commit().map_err(database)?;
    Ok((persisted, persisted_authority))
}

pub(crate) fn load(connection: &Connection) -> Result<ReductionIndexSnapshot, StoreError> {
    load_from_connection(connection)
}

fn clear_rebuildable(transaction: &Transaction<'_>) -> Result<(), StoreError> {
    transaction
        .execute_batch(
            "DELETE FROM reduction_state;
             DELETE FROM reduction_conflict_participants;
             DELETE FROM reduction_conflicts;
             DELETE FROM reduction_presentation_order;
             DELETE FROM reduction_dependency_order;
             DELETE FROM reduction_decision_participants;
             DELETE FROM reduction_failed_authorities;
             DELETE FROM reduction_unusable_dependencies;
             DELETE FROM reduction_missing_dependencies;
             DELETE FROM reduction_decisions;
             DELETE FROM reduction_reverse_dependencies;
             DELETE FROM reduction_vertices;",
        )
        .map_err(database)
}

#[allow(clippy::too_many_lines)]
fn insert_index(
    transaction: &Transaction<'_>,
    index: &ReductionIndexSnapshot,
    failpoint: RepairFailpoint,
) -> Result<(), StoreError> {
    for vertex in index.reverse_dependencies.keys() {
        transaction
            .execute(
                "INSERT INTO reduction_vertices(fact_id) VALUES (?1)",
                [vertex.as_bytes().as_slice()],
            )
            .map_err(database)?;
    }
    fail_at(failpoint, RepairFailpoint::AfterVertices)?;
    for (parent, children) in &index.reverse_dependencies {
        for child in children {
            transaction
                .execute(
                    "INSERT INTO reduction_reverse_dependencies(parent_id, child_id) \
                     VALUES (?1, ?2)",
                    params![parent.as_bytes().as_slice(), child.as_bytes().as_slice()],
                )
                .map_err(database)?;
        }
    }
    fail_at(failpoint, RepairFailpoint::AfterReverseDependencies)?;
    for ((domain, fact_id), decision) in &index.decisions {
        let (reason_code, reason_parameter) =
            decision.reason.as_ref().map_or((0, 0), encode_reason);
        transaction
            .execute(
                "INSERT INTO reduction_decisions( \
                     domain, fact_id, status, reason_code, reason_parameter \
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    encode_domain(*domain),
                    fact_id.as_bytes().as_slice(),
                    encode_status(decision.status),
                    reason_code,
                    reason_parameter
                ],
            )
            .map_err(database)?;
        for dependency in &decision.missing_dependencies {
            transaction
                .execute(
                    "INSERT INTO reduction_missing_dependencies(domain, fact_id, dependency_id) \
                     VALUES (?1, ?2, ?3)",
                    params![
                        encode_domain(*domain),
                        fact_id.as_bytes().as_slice(),
                        dependency.as_bytes().as_slice()
                    ],
                )
                .map_err(database)?;
        }
        for (dependency, status) in &decision.unusable_dependencies {
            transaction
                .execute(
                    "INSERT INTO reduction_unusable_dependencies( \
                         domain, fact_id, dependency_id, dependency_status \
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        encode_domain(*domain),
                        fact_id.as_bytes().as_slice(),
                        dependency.as_bytes().as_slice(),
                        encode_status(*status)
                    ],
                )
                .map_err(database)?;
        }
        for role in &decision.failed_authorities {
            transaction
                .execute(
                    "INSERT INTO reduction_failed_authorities(domain, fact_id, authority_role) \
                     VALUES (?1, ?2, ?3)",
                    params![
                        encode_domain(*domain),
                        fact_id.as_bytes().as_slice(),
                        encode_role(*role)
                    ],
                )
                .map_err(database)?;
        }
        for participant in &decision.conflict_participants {
            transaction
                .execute(
                    "INSERT INTO reduction_decision_participants( \
                         domain, fact_id, participant_id \
                     ) VALUES (?1, ?2, ?3)",
                    params![
                        encode_domain(*domain),
                        fact_id.as_bytes().as_slice(),
                        participant.as_bytes().as_slice()
                    ],
                )
                .map_err(database)?;
        }
    }
    fail_at(failpoint, RepairFailpoint::AfterDecisions)?;
    insert_orders(
        transaction,
        "reduction_dependency_order",
        &index.dependency_order,
    )?;
    fail_at(failpoint, RepairFailpoint::AfterDependencyOrder)?;
    insert_orders(
        transaction,
        "reduction_presentation_order",
        &index.presentation_order,
    )?;
    fail_at(failpoint, RepairFailpoint::AfterPresentationOrder)?;
    for (domain, conflicts) in &index.conflicts {
        for (ordinal, conflict) in conflicts.iter().enumerate() {
            let ordinal = i64::try_from(ordinal).map_err(|_| corrupt())?;
            let (reason_code, reason_parameter) = encode_reason(&conflict.reason);
            transaction
                .execute(
                    "INSERT INTO reduction_conflicts( \
                         domain, ordinal, reason_code, reason_parameter \
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        encode_domain(*domain),
                        ordinal,
                        reason_code,
                        reason_parameter
                    ],
                )
                .map_err(database)?;
            for participant in &conflict.participants {
                transaction
                    .execute(
                        "INSERT INTO reduction_conflict_participants( \
                             domain, ordinal, participant_id \
                         ) VALUES (?1, ?2, ?3)",
                        params![
                            encode_domain(*domain),
                            ordinal,
                            participant.as_bytes().as_slice()
                        ],
                    )
                    .map_err(database)?;
            }
        }
    }
    fail_at(failpoint, RepairFailpoint::AfterConflicts)?;
    let counts = Counts::from_index(index)?;
    transaction
        .execute(
            "INSERT INTO reduction_state( \
                 singleton, policy_installation, policy_human_mailbox, fact_count, vertex_count, reverse_count, \
                 decision_count, missing_count, unusable_count, failed_authority_count, \
                 decision_participant_count, dependency_order_count, presentation_order_count, \
                 conflict_count, conflict_participant_count, index_digest \
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                index.policy.local_installation().as_bytes().as_slice(),
                index.policy.local_human_mailbox().as_bytes().as_slice(),
                counts.fact_count,
                counts.vertex_count,
                counts.reverse_count,
                counts.decision_count,
                counts.missing_count,
                counts.unusable_count,
                counts.failed_authority_count,
                counts.decision_participant_count,
                counts.dependency_order_count,
                counts.presentation_order_count,
                counts.conflict_count,
                counts.conflict_participant_count,
                index.digest().as_slice()
            ],
        )
        .map_err(database)?;
    fail_at(failpoint, RepairFailpoint::AfterState)?;
    Ok(())
}

fn fail_at(actual: RepairFailpoint, expected: RepairFailpoint) -> Result<(), StoreError> {
    if actual == expected {
        Err(StoreError::new(StoreErrorClass::DatabaseUnavailable))
    } else {
        Ok(())
    }
}

fn insert_orders(
    transaction: &Transaction<'_>,
    table: &str,
    orders: &BTreeMap<ReductionDomain, Vec<FactId>>,
) -> Result<(), StoreError> {
    let sql = match table {
        "reduction_dependency_order" => {
            "INSERT INTO reduction_dependency_order(domain, position, fact_id) VALUES (?1, ?2, ?3)"
        }
        "reduction_presentation_order" => {
            "INSERT INTO reduction_presentation_order(domain, position, fact_id) VALUES (?1, ?2, ?3)"
        }
        _ => return Err(corrupt()),
    };
    for (domain, facts) in orders {
        for (position, fact_id) in facts.iter().enumerate() {
            transaction
                .execute(
                    sql,
                    params![
                        encode_domain(*domain),
                        i64::try_from(position).map_err(|_| corrupt())?,
                        fact_id.as_bytes().as_slice()
                    ],
                )
                .map_err(database)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn load_from_connection(connection: &Connection) -> Result<ReductionIndexSnapshot, StoreError> {
    let Some(state) = load_state(connection)? else {
        return if rebuildable_row_count(connection)? == 0 {
            Err(StoreError::new(StoreErrorClass::NotRepaired))
        } else {
            Err(corrupt())
        };
    };
    state.counts.validate()?;
    if Counts::from_database(connection)? != state.counts {
        return Err(corrupt());
    }
    let mut index = ReductionIndexSnapshot {
        policy: AuthorityPolicy::new(
            InstallationId::from_bytes(state.policy_installation),
            MailboxId::from_bytes(state.policy_human_mailbox),
        ),
        reverse_dependencies: load_reverse_dependencies(connection)?,
        decisions: load_decisions(connection)?,
        dependency_order: load_orders(connection, "reduction_dependency_order")?,
        presentation_order: load_orders(connection, "reduction_presentation_order")?,
        conflicts: load_conflicts(connection)?,
    };
    attach_missing(connection, &mut index)?;
    attach_unusable(connection, &mut index)?;
    attach_failed_authorities(connection, &mut index)?;
    attach_decision_participants(connection, &mut index)?;
    ensure_all_domains(&mut index);
    if Counts::from_index(&index)? != state.counts || index.digest() != state.digest {
        return Err(corrupt());
    }
    Ok(index)
}

fn load_reverse_dependencies(
    connection: &Connection,
) -> Result<BTreeMap<FactId, BTreeSet<FactId>>, StoreError> {
    let mut reverse = BTreeMap::new();
    let mut vertices = connection
        .prepare("SELECT fact_id FROM reduction_vertices ORDER BY fact_id")
        .map_err(database)?;
    let rows = vertices
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .map_err(database)?;
    for row in rows {
        reverse.insert(
            FactId::from_bytes(fixed(row.map_err(database)?)?),
            BTreeSet::new(),
        );
    }
    drop(vertices);
    let mut statement = connection
        .prepare(
            "SELECT parent_id, child_id FROM reduction_reverse_dependencies \
             ORDER BY parent_id, child_id",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(database)?;
    for row in rows {
        let (parent, child) = row.map_err(database)?;
        let parent = FactId::from_bytes(fixed(parent)?);
        let child = FactId::from_bytes(fixed(child)?);
        reverse.get_mut(&parent).ok_or_else(corrupt)?.insert(child);
    }
    Ok(reverse)
}

fn load_decisions(
    connection: &Connection,
) -> Result<BTreeMap<(ReductionDomain, FactId), IndexedDecision>, StoreError> {
    let mut decisions = BTreeMap::new();
    let mut statement = connection
        .prepare(
            "SELECT domain, fact_id, status, reason_code, reason_parameter \
             FROM reduction_decisions ORDER BY domain, fact_id",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(database)?;
    for row in rows {
        let (domain, fact_id, status, reason_code, reason_parameter) = row.map_err(database)?;
        let reason = match (reason_code, reason_parameter) {
            (0, 0) => None,
            (0, _) => return Err(corrupt()),
            _ => Some(decode_reason(reason_code, reason_parameter).ok_or_else(corrupt)?),
        };
        let domain = decode_domain(domain).ok_or_else(corrupt)?;
        if reason
            .as_ref()
            .is_some_and(|reason| !reason_belongs_to_domain(domain, reason))
        {
            return Err(corrupt());
        }
        let key = (domain, FactId::from_bytes(fixed(fact_id)?));
        if decisions
            .insert(
                key,
                IndexedDecision {
                    status: decode_status(status).ok_or_else(corrupt)?,
                    reason,
                    missing_dependencies: BTreeSet::new(),
                    unusable_dependencies: BTreeMap::new(),
                    failed_authorities: BTreeSet::new(),
                    conflict_participants: BTreeSet::new(),
                },
            )
            .is_some()
        {
            return Err(corrupt());
        }
    }
    Ok(decisions)
}

fn attach_missing(
    connection: &Connection,
    index: &mut ReductionIndexSnapshot,
) -> Result<(), StoreError> {
    let rows = triple_rows(
        connection,
        "SELECT domain, fact_id, dependency_id FROM reduction_missing_dependencies \
         ORDER BY domain, fact_id, dependency_id",
    )?;
    for (domain, fact, dependency) in rows {
        decision_mut(index, domain, fact)?
            .missing_dependencies
            .insert(dependency);
    }
    Ok(())
}

fn attach_unusable(
    connection: &Connection,
    index: &mut ReductionIndexSnapshot,
) -> Result<(), StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT domain, fact_id, dependency_id, dependency_status \
             FROM reduction_unusable_dependencies ORDER BY domain, fact_id, dependency_id",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(database)?;
    for row in rows {
        let (domain, fact, dependency, status) = row.map_err(database)?;
        decision_mut(
            index,
            decode_domain(domain).ok_or_else(corrupt)?,
            FactId::from_bytes(fixed(fact)?),
        )?
        .unusable_dependencies
        .insert(
            FactId::from_bytes(fixed(dependency)?),
            decode_status(status).ok_or_else(corrupt)?,
        );
    }
    Ok(())
}

fn attach_failed_authorities(
    connection: &Connection,
    index: &mut ReductionIndexSnapshot,
) -> Result<(), StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT domain, fact_id, authority_role FROM reduction_failed_authorities \
             ORDER BY domain, fact_id, authority_role",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(database)?;
    for row in rows {
        let (domain, fact, role) = row.map_err(database)?;
        decision_mut(
            index,
            decode_domain(domain).ok_or_else(corrupt)?,
            FactId::from_bytes(fixed(fact)?),
        )?
        .failed_authorities
        .insert(decode_role(role).ok_or_else(corrupt)?);
    }
    Ok(())
}

fn attach_decision_participants(
    connection: &Connection,
    index: &mut ReductionIndexSnapshot,
) -> Result<(), StoreError> {
    let rows = triple_rows(
        connection,
        "SELECT domain, fact_id, participant_id FROM reduction_decision_participants \
         ORDER BY domain, fact_id, participant_id",
    )?;
    for (domain, fact, participant) in rows {
        decision_mut(index, domain, fact)?
            .conflict_participants
            .insert(participant);
    }
    Ok(())
}

fn triple_rows(
    connection: &Connection,
    sql: &str,
) -> Result<Vec<(ReductionDomain, FactId, FactId)>, StoreError> {
    let mut statement = connection.prepare(sql).map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(database)?;
    rows.map(|row| {
        let (domain, fact, related) = row.map_err(database)?;
        Ok((
            decode_domain(domain).ok_or_else(corrupt)?,
            FactId::from_bytes(fixed(fact)?),
            FactId::from_bytes(fixed(related)?),
        ))
    })
    .collect()
}

fn decision_mut(
    index: &mut ReductionIndexSnapshot,
    domain: ReductionDomain,
    fact_id: FactId,
) -> Result<&mut IndexedDecision, StoreError> {
    index
        .decisions
        .get_mut(&(domain, fact_id))
        .ok_or_else(corrupt)
}

fn load_orders(
    connection: &Connection,
    table: &str,
) -> Result<BTreeMap<ReductionDomain, Vec<FactId>>, StoreError> {
    let sql = match table {
        "reduction_dependency_order" => {
            "SELECT domain, position, fact_id FROM reduction_dependency_order \
             ORDER BY domain, position"
        }
        "reduction_presentation_order" => {
            "SELECT domain, position, fact_id FROM reduction_presentation_order \
             ORDER BY domain, position"
        }
        _ => return Err(corrupt()),
    };
    let mut orders = BTreeMap::<ReductionDomain, Vec<FactId>>::new();
    let mut statement = connection.prepare(sql).map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(database)?;
    for row in rows {
        let (domain, position, fact_id) = row.map_err(database)?;
        let domain = decode_domain(domain).ok_or_else(corrupt)?;
        let expected_position =
            i64::try_from(orders.entry(domain).or_default().len()).map_err(|_| corrupt())?;
        if position != expected_position {
            return Err(corrupt());
        }
        orders
            .get_mut(&domain)
            .ok_or_else(corrupt)?
            .push(FactId::from_bytes(fixed(fact_id)?));
    }
    Ok(orders)
}

fn load_conflicts(
    connection: &Connection,
) -> Result<BTreeMap<ReductionDomain, Vec<IndexedConflict>>, StoreError> {
    let mut conflicts = BTreeMap::<ReductionDomain, Vec<IndexedConflict>>::new();
    let mut statement = connection
        .prepare(
            "SELECT domain, ordinal, reason_code, reason_parameter \
             FROM reduction_conflicts ORDER BY domain, ordinal",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(database)?;
    for row in rows {
        let (domain, ordinal, reason, parameter) = row.map_err(database)?;
        let domain = decode_domain(domain).ok_or_else(corrupt)?;
        let list = conflicts.entry(domain).or_default();
        if ordinal != i64::try_from(list.len()).map_err(|_| corrupt())? {
            return Err(corrupt());
        }
        let reason = decode_reason(reason, parameter).ok_or_else(corrupt)?;
        if !reason_belongs_to_domain(domain, &reason) {
            return Err(corrupt());
        }
        list.push(IndexedConflict {
            reason,
            participants: BTreeSet::new(),
        });
    }
    drop(statement);
    let mut members = connection
        .prepare(
            "SELECT domain, ordinal, participant_id FROM reduction_conflict_participants \
             ORDER BY domain, ordinal, participant_id",
        )
        .map_err(database)?;
    let rows = members
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(database)?;
    for row in rows {
        let (domain, ordinal, participant) = row.map_err(database)?;
        let domain = decode_domain(domain).ok_or_else(corrupt)?;
        let ordinal = usize::try_from(ordinal).map_err(|_| corrupt())?;
        conflicts
            .get_mut(&domain)
            .and_then(|values| values.get_mut(ordinal))
            .ok_or_else(corrupt)?
            .participants
            .insert(FactId::from_bytes(fixed(participant)?));
    }
    Ok(conflicts)
}

fn ensure_all_domains(index: &mut ReductionIndexSnapshot) {
    for domain in ReductionDomain::ALL {
        index.dependency_order.entry(domain).or_default();
        index.presentation_order.entry(domain).or_default();
        index.conflicts.entry(domain).or_default();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Counts {
    fact_count: i64,
    vertex_count: i64,
    reverse_count: i64,
    decision_count: i64,
    missing_count: i64,
    unusable_count: i64,
    failed_authority_count: i64,
    decision_participant_count: i64,
    dependency_order_count: i64,
    presentation_order_count: i64,
    conflict_count: i64,
    conflict_participant_count: i64,
}

impl Counts {
    fn from_database(connection: &Connection) -> Result<Self, StoreError> {
        let counts = connection
            .query_row(
                "SELECT \
                    (SELECT count(*) FROM reduction_decisions WHERE domain = 1), \
                    (SELECT count(*) FROM reduction_vertices), \
                    (SELECT count(*) FROM reduction_reverse_dependencies), \
                    (SELECT count(*) FROM reduction_decisions), \
                    (SELECT count(*) FROM reduction_missing_dependencies), \
                    (SELECT count(*) FROM reduction_unusable_dependencies), \
                    (SELECT count(*) FROM reduction_failed_authorities), \
                    (SELECT count(*) FROM reduction_decision_participants), \
                    (SELECT count(*) FROM reduction_dependency_order), \
                    (SELECT count(*) FROM reduction_presentation_order), \
                    (SELECT count(*) FROM reduction_conflicts), \
                    (SELECT count(*) FROM reduction_conflict_participants)",
                [],
                |row| {
                    Ok(Self {
                        fact_count: row.get(0)?,
                        vertex_count: row.get(1)?,
                        reverse_count: row.get(2)?,
                        decision_count: row.get(3)?,
                        missing_count: row.get(4)?,
                        unusable_count: row.get(5)?,
                        failed_authority_count: row.get(6)?,
                        decision_participant_count: row.get(7)?,
                        dependency_order_count: row.get(8)?,
                        presentation_order_count: row.get(9)?,
                        conflict_count: row.get(10)?,
                        conflict_participant_count: row.get(11)?,
                    })
                },
            )
            .map_err(database)?;
        counts.validate()?;
        Ok(counts)
    }

    fn from_index(index: &ReductionIndexSnapshot) -> Result<Self, StoreError> {
        let authority_facts = index
            .decisions
            .keys()
            .filter_map(|(domain, fact)| (*domain == ReductionDomain::Authority).then_some(*fact))
            .collect::<BTreeSet<_>>();
        for domain in ReductionDomain::ALL {
            let domain_facts = index
                .decisions
                .keys()
                .filter_map(|(candidate, fact)| (*candidate == domain).then_some(*fact))
                .collect::<BTreeSet<_>>();
            if domain_facts != authority_facts {
                return Err(corrupt());
            }
        }
        let expected_decisions = authority_facts
            .len()
            .checked_mul(ReductionDomain::ALL.len())
            .ok_or_else(corrupt)?;
        if expected_decisions != index.decisions.len()
            || !authority_facts
                .iter()
                .all(|fact| index.reverse_dependencies.contains_key(fact))
        {
            return Err(corrupt());
        }
        let value = Self {
            fact_count: i64::try_from(authority_facts.len()).map_err(|_| corrupt())?,
            vertex_count: i64::try_from(index.reverse_dependencies.len()).map_err(|_| corrupt())?,
            reverse_count: sum(index.reverse_dependencies.values().map(BTreeSet::len))?,
            decision_count: i64::try_from(index.decisions.len()).map_err(|_| corrupt())?,
            missing_count: sum(index
                .decisions
                .values()
                .map(|decision| decision.missing_dependencies.len()))?,
            unusable_count: sum(index
                .decisions
                .values()
                .map(|decision| decision.unusable_dependencies.len()))?,
            failed_authority_count: sum(index
                .decisions
                .values()
                .map(|decision| decision.failed_authorities.len()))?,
            decision_participant_count: sum(index
                .decisions
                .values()
                .map(|decision| decision.conflict_participants.len()))?,
            dependency_order_count: sum(index.dependency_order.values().map(Vec::len))?,
            presentation_order_count: sum(index.presentation_order.values().map(Vec::len))?,
            conflict_count: sum(index.conflicts.values().map(Vec::len))?,
            conflict_participant_count: sum(index.conflicts.values().flat_map(|conflicts| {
                conflicts.iter().map(|conflict| conflict.participants.len())
            }))?,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(self) -> Result<(), StoreError> {
        if [
            self.fact_count,
            self.vertex_count,
            self.reverse_count,
            self.decision_count,
            self.missing_count,
            self.unusable_count,
            self.failed_authority_count,
            self.decision_participant_count,
            self.dependency_order_count,
            self.presentation_order_count,
            self.conflict_count,
            self.conflict_participant_count,
        ]
        .into_iter()
        .all(|count| (0..=MAXIMUM_INDEX_ROWS).contains(&count))
        {
            Ok(())
        } else {
            Err(corrupt())
        }
    }
}

struct State {
    policy_installation: [u8; 32],
    policy_human_mailbox: [u8; 32],
    counts: Counts,
    digest: [u8; 32],
}

fn load_state(connection: &Connection) -> Result<Option<State>, StoreError> {
    connection
        .query_row(
            "SELECT policy_installation, policy_human_mailbox, fact_count, vertex_count, reverse_count, \
                    decision_count, missing_count, unusable_count, failed_authority_count, \
                    decision_participant_count, dependency_order_count, \
                    presentation_order_count, conflict_count, conflict_participant_count, \
                    index_digest \
             FROM reduction_state WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    Counts {
                        fact_count: row.get(2)?,
                        vertex_count: row.get(3)?,
                        reverse_count: row.get(4)?,
                        decision_count: row.get(5)?,
                        missing_count: row.get(6)?,
                        unusable_count: row.get(7)?,
                        failed_authority_count: row.get(8)?,
                        decision_participant_count: row.get(9)?,
                        dependency_order_count: row.get(10)?,
                        presentation_order_count: row.get(11)?,
                        conflict_count: row.get(12)?,
                        conflict_participant_count: row.get(13)?,
                    },
                    row.get::<_, Vec<u8>>(14)?,
                ))
            },
        )
        .optional()
        .map_err(database)?
        .map(|(installation, mailbox, counts, digest)| {
            Ok(State {
                policy_installation: fixed(installation)?,
                policy_human_mailbox: fixed(mailbox)?,
                counts,
                digest: fixed(digest)?,
            })
        })
        .transpose()
}

fn rebuildable_row_count(connection: &Connection) -> Result<i64, StoreError> {
    connection
        .query_row(
            "SELECT \
                (SELECT count(*) FROM reduction_vertices) +
                (SELECT count(*) FROM reduction_reverse_dependencies) +
                (SELECT count(*) FROM reduction_decisions) +
                (SELECT count(*) FROM reduction_missing_dependencies) +
                (SELECT count(*) FROM reduction_unusable_dependencies) +
                (SELECT count(*) FROM reduction_failed_authorities) +
                (SELECT count(*) FROM reduction_decision_participants) +
                (SELECT count(*) FROM reduction_dependency_order) +
                (SELECT count(*) FROM reduction_presentation_order) +
                (SELECT count(*) FROM reduction_conflicts) +
                (SELECT count(*) FROM reduction_conflict_participants)",
            [],
            |row| row.get(0),
        )
        .map_err(database)
}

fn sum(values: impl IntoIterator<Item = usize>) -> Result<i64, StoreError> {
    values.into_iter().try_fold(0_i64, |total, value| {
        total
            .checked_add(i64::try_from(value).map_err(|_| corrupt())?)
            .ok_or_else(corrupt)
    })
}

fn fixed(bytes: Vec<u8>) -> Result<[u8; 32], StoreError> {
    bytes.try_into().map_err(|_| corrupt())
}

fn database(_: rusqlite::Error) -> StoreError {
    StoreError::new(StoreErrorClass::DatabaseUnavailable)
}

fn corrupt() -> StoreError {
    StoreError::new(StoreErrorClass::RebuildableStateCorrupt)
}
