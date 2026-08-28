//! Explicit relational codecs for rebuildable project projections.

use std::collections::{BTreeMap, BTreeSet};

use hq_domain::{
    AgentId, AssignmentBinding, AssignmentId, AssignmentIntent, BoundedText, CommandDigest,
    CommandId, ContentText, DispatchId, ErrorCode, FactId, InstallationId, MailboxAddress,
    MailboxId, MessageContent, MessageId, MessagePurpose, OperationCorrelation, OperationId,
    PresentationKind, ProjectId, ProjectResource, ProviderId, ProviderSessionId,
    RemoteCommandResult, ResourceHealth, ResourceId, ResourceLocator, ResourceScheme,
    RuntimeObservation, ShortText, ThreadId,
};
use hq_reducer::{
    ProjectAggregateKey, ProjectAssignmentPhase, ProjectAssignmentView, ProjectDispatchView,
    ProjectInputView, ProjectLifecycle, ProjectOutputStatus, ProjectOutputView, ProjectProjection,
    ProjectProjectionKey, ProjectView, RemoteCommandStage, RemoteCommandView,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};

use crate::{ProjectProjectionSnapshot, StoreError, StoreErrorClass};

const MAXIMUM_PROJECT_ROWS: i64 = 64_000_000;
const ZERO: [u8; 32] = [0; 32];
const TABLES: [&str; 17] = [
    "project_aggregate_keys",
    "project_frontiers",
    "project_projection_keys",
    "project_support",
    "project_projects",
    "project_fork_participants",
    "project_resources",
    "project_active_claims",
    "project_claim_conflicts",
    "project_assignments",
    "project_assignment_support",
    "project_inputs",
    "project_dispatches",
    "project_outputs",
    "project_output_facts",
    "project_commands",
    "project_command_support",
];

pub(super) fn clear(transaction: &Transaction<'_>) -> Result<(), StoreError> {
    transaction
        .execute_batch(
            "DELETE FROM project_state;
             DELETE FROM project_command_support;
             DELETE FROM project_commands;
             DELETE FROM project_output_facts;
             DELETE FROM project_outputs;
             DELETE FROM project_dispatches;
             DELETE FROM project_inputs;
             DELETE FROM project_assignment_support;
             DELETE FROM project_assignments;
             DELETE FROM project_claim_conflicts;
             DELETE FROM project_active_claims;
             DELETE FROM project_resources;
             DELETE FROM project_fork_participants;
             DELETE FROM project_projects;
             DELETE FROM project_support;
             DELETE FROM project_projection_keys;
             DELETE FROM project_frontiers;
             DELETE FROM project_aggregate_keys;",
        )
        .map_err(database)
}

pub(super) fn insert(
    transaction: &Transaction<'_>,
    snapshot: &ProjectProjectionSnapshot,
) -> Result<(), StoreError> {
    if snapshot.projections().keys().ne(snapshot.support().keys()) {
        return Err(corrupt());
    }
    for (key, facts) in snapshot.frontiers() {
        let parts = aggregate_parts(key);
        let digest = key_digest(&parts);
        insert_aggregate_key(transaction, digest, &parts)?;
        insert_facts(transaction, "project_frontiers", digest, facts)?;
    }
    for (key, projection) in snapshot.projections() {
        let (kind, id) = projection_parts(key);
        let digest = projection_digest(kind, id);
        transaction
            .execute(
                "INSERT INTO project_projection_keys(key_digest, key_kind, key_id) \
                 VALUES (?1, ?2, ?3)",
                params![digest.as_slice(), kind, id.as_slice()],
            )
            .map_err(database)?;
        insert_projection(transaction, digest, key, projection)?;
        insert_facts(
            transaction,
            "project_support",
            digest,
            snapshot.support().get(key).ok_or_else(corrupt)?,
        )?;
    }
    let counts = Counts::read(transaction)?;
    validate_counts(snapshot, counts)?;
    let digest = row_digest(transaction)?;
    transaction
        .execute(
            "INSERT INTO project_state(singleton, aggregate_key_count, frontier_count, \
                 projection_key_count, projection_count, support_count, row_count, row_digest) \
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                counts.aggregate_key_count,
                counts.frontier_count,
                counts.projection_key_count,
                counts.projection_count,
                counts.support_count,
                counts.row_count,
                digest.as_slice(),
            ],
        )
        .map_err(database)?;
    Ok(())
}

pub(super) fn load(connection: &Connection) -> Result<ProjectProjectionSnapshot, StoreError> {
    let Some(state) = load_state(connection)? else {
        return if Counts::read(connection)?.row_count == 0 {
            Err(StoreError::new(StoreErrorClass::NotRepaired))
        } else {
            Err(corrupt())
        };
    };
    state.counts.validate()?;
    if Counts::read(connection)? != state.counts || row_digest(connection)? != state.digest {
        return Err(corrupt());
    }
    let frontiers = load_frontiers(connection)?;
    let mut statement = connection
        .prepare(
            "SELECT key_digest, key_kind, key_id FROM project_projection_keys \
             ORDER BY key_digest",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(database)?
        .map(|row| row.map_err(database))
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    let mut projections = BTreeMap::new();
    let mut support = BTreeMap::new();
    for (stored_digest, kind, stored_id) in rows {
        let digest = fixed(stored_digest)?;
        let id = fixed(stored_id)?;
        if projection_digest(kind, id) != digest {
            return Err(corrupt());
        }
        let key = decode_projection_key(kind, id)?;
        let projection = load_projection(connection, digest, &key)?;
        if projections.insert(key.clone(), projection).is_some()
            || support
                .insert(key, load_facts(connection, "project_support", digest)?)
                .is_some()
        {
            return Err(corrupt());
        }
    }
    let snapshot = ProjectProjectionSnapshot::new(frontiers, projections, support);
    validate_counts(&snapshot, state.counts)?;
    Ok(snapshot)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AggregateParts {
    kind: i64,
    a: [u8; 32],
    b: [u8; 32],
    scheme: i64,
    locator: String,
}

fn aggregate_parts(key: &ProjectAggregateKey) -> AggregateParts {
    match key {
        ProjectAggregateKey::Project(id) => simple_parts(1, *id.as_bytes()),
        ProjectAggregateKey::Resource { home, locator } => AggregateParts {
            kind: 2,
            a: *home.as_bytes(),
            b: ZERO,
            scheme: encode_scheme(locator.scheme()),
            locator: locator.value().to_owned(),
        },
        ProjectAggregateKey::AgentAssignment { home, agent } => AggregateParts {
            kind: 3,
            a: *home.as_bytes(),
            b: *agent.as_bytes(),
            scheme: 0,
            locator: String::new(),
        },
        ProjectAggregateKey::Input(id) => simple_parts(4, *id.as_bytes()),
        ProjectAggregateKey::Dispatch(id) => simple_parts(5, *id.as_bytes()),
        ProjectAggregateKey::Output(id) => simple_parts(6, *id.as_bytes()),
        ProjectAggregateKey::Command(id) => simple_parts(7, *id.as_bytes()),
    }
}

fn simple_parts(kind: i64, a: [u8; 32]) -> AggregateParts {
    AggregateParts {
        kind,
        a,
        b: ZERO,
        scheme: 0,
        locator: String::new(),
    }
}

fn decode_aggregate(parts: AggregateParts) -> Result<ProjectAggregateKey, StoreError> {
    match (
        parts.kind,
        parts.b == ZERO,
        parts.scheme,
        parts.locator.is_empty(),
    ) {
        (1, true, 0, true) => Ok(ProjectAggregateKey::Project(ProjectId::from_bytes(parts.a))),
        (2, true, scheme, false) => Ok(ProjectAggregateKey::Resource {
            home: InstallationId::from_bytes(parts.a),
            locator: decode_locator(scheme, parts.locator)?,
        }),
        (3, false, 0, true) => Ok(ProjectAggregateKey::AgentAssignment {
            home: InstallationId::from_bytes(parts.a),
            agent: AgentId::from_bytes(parts.b),
        }),
        (4, true, 0, true) => Ok(ProjectAggregateKey::Input(MessageId::from_bytes(parts.a))),
        (5, true, 0, true) => Ok(ProjectAggregateKey::Dispatch(DispatchId::from_bytes(
            parts.a,
        ))),
        (6, true, 0, true) => Ok(ProjectAggregateKey::Output(MessageId::from_bytes(parts.a))),
        (7, true, 0, true) => Ok(ProjectAggregateKey::Command(CommandId::from_bytes(parts.a))),
        _ => Err(corrupt()),
    }
}

fn projection_parts(key: &ProjectProjectionKey) -> (i64, [u8; 32]) {
    match key {
        ProjectProjectionKey::Project(id) => (1, *id.as_bytes()),
        ProjectProjectionKey::Input(id) => (2, *id.as_bytes()),
        ProjectProjectionKey::Dispatch(id) => (3, *id.as_bytes()),
        ProjectProjectionKey::Output(id) => (4, *id.as_bytes()),
        ProjectProjectionKey::Command(id) => (5, *id.as_bytes()),
    }
}

fn decode_projection_key(kind: i64, id: [u8; 32]) -> Result<ProjectProjectionKey, StoreError> {
    match kind {
        1 => Ok(ProjectProjectionKey::Project(ProjectId::from_bytes(id))),
        2 => Ok(ProjectProjectionKey::Input(MessageId::from_bytes(id))),
        3 => Ok(ProjectProjectionKey::Dispatch(DispatchId::from_bytes(id))),
        4 => Ok(ProjectProjectionKey::Output(MessageId::from_bytes(id))),
        5 => Ok(ProjectProjectionKey::Command(CommandId::from_bytes(id))),
        _ => Err(corrupt()),
    }
}

fn key_digest(parts: &AggregateParts) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"hq-store/project-aggregate/v1");
    digest.update(parts.kind.to_be_bytes());
    digest.update(parts.a);
    digest.update(parts.b);
    digest.update(parts.scheme.to_be_bytes());
    put_text(&mut digest, &parts.locator);
    digest.finalize().into()
}

fn projection_digest(kind: i64, id: [u8; 32]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"hq-store/project-projection/v1");
    digest.update(kind.to_be_bytes());
    digest.update(id);
    digest.finalize().into()
}

fn insert_aggregate_key(
    transaction: &Transaction<'_>,
    digest: [u8; 32],
    parts: &AggregateParts,
) -> Result<(), StoreError> {
    transaction
        .execute(
            "INSERT INTO project_aggregate_keys( \
                 key_digest, key_kind, key_a, key_b, locator_scheme, locator_value \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                digest.as_slice(),
                parts.kind,
                parts.a.as_slice(),
                parts.b.as_slice(),
                parts.scheme,
                parts.locator,
            ],
        )
        .map_err(database)?;
    Ok(())
}

fn load_frontiers(
    connection: &Connection,
) -> Result<BTreeMap<ProjectAggregateKey, BTreeSet<FactId>>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT key_digest, key_kind, key_a, key_b, locator_scheme, locator_value \
             FROM project_aggregate_keys ORDER BY key_digest",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                AggregateParts {
                    kind: row.get(1)?,
                    a: fixed_sql(row.get(2)?)?,
                    b: fixed_sql(row.get(3)?)?,
                    scheme: row.get(4)?,
                    locator: row.get(5)?,
                },
            ))
        })
        .map_err(database)?
        .map(|row| row.map_err(database))
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    let mut frontiers = BTreeMap::new();
    for (stored_digest, parts) in rows {
        let digest = fixed(stored_digest)?;
        if key_digest(&parts) != digest
            || frontiers
                .insert(
                    decode_aggregate(parts)?,
                    load_facts(connection, "project_frontiers", digest)?,
                )
                .is_some()
        {
            return Err(corrupt());
        }
    }
    Ok(frontiers)
}

fn insert_facts(
    transaction: &Transaction<'_>,
    table: &str,
    digest: [u8; 32],
    facts: &BTreeSet<FactId>,
) -> Result<(), StoreError> {
    let query = match table {
        "project_frontiers" => "INSERT INTO project_frontiers(key_digest, fact_id) VALUES (?1, ?2)",
        "project_support" => "INSERT INTO project_support(key_digest, fact_id) VALUES (?1, ?2)",
        "project_fork_participants" => {
            "INSERT INTO project_fork_participants(key_digest, fact_id) VALUES (?1, ?2)"
        }
        "project_assignment_support" => {
            "INSERT INTO project_assignment_support(key_digest, fact_id) VALUES (?1, ?2)"
        }
        "project_output_facts" => {
            "INSERT INTO project_output_facts(key_digest, fact_id) VALUES (?1, ?2)"
        }
        "project_command_support" => {
            "INSERT INTO project_command_support(key_digest, fact_id) VALUES (?1, ?2)"
        }
        _ => return Err(corrupt()),
    };
    for fact in facts {
        transaction
            .execute(
                query,
                params![digest.as_slice(), fact.as_bytes().as_slice()],
            )
            .map_err(database)?;
    }
    Ok(())
}

fn load_facts(
    connection: &Connection,
    table: &str,
    digest: [u8; 32],
) -> Result<BTreeSet<FactId>, StoreError> {
    let query = match table {
        "project_frontiers" => {
            "SELECT fact_id FROM project_frontiers WHERE key_digest = ?1 ORDER BY fact_id"
        }
        "project_support" => {
            "SELECT fact_id FROM project_support WHERE key_digest = ?1 ORDER BY fact_id"
        }
        "project_fork_participants" => {
            "SELECT fact_id FROM project_fork_participants \
             WHERE key_digest = ?1 ORDER BY fact_id"
        }
        "project_assignment_support" => {
            "SELECT fact_id FROM project_assignment_support \
             WHERE key_digest = ?1 ORDER BY fact_id"
        }
        "project_output_facts" => {
            "SELECT fact_id FROM project_output_facts \
             WHERE key_digest = ?1 ORDER BY fact_id"
        }
        "project_command_support" => {
            "SELECT fact_id FROM project_command_support \
             WHERE key_digest = ?1 ORDER BY fact_id"
        }
        _ => return Err(corrupt()),
    };
    let mut statement = connection.prepare(query).map_err(database)?;
    let rows = statement
        .query_map([digest.as_slice()], |row| row.get::<_, Vec<u8>>(0))
        .map_err(database)?;
    let mut facts = BTreeSet::new();
    for row in rows {
        if !facts.insert(FactId::from_bytes(fixed(row.map_err(database)?)?)) {
            return Err(corrupt());
        }
    }
    Ok(facts)
}

fn insert_projection(
    transaction: &Transaction<'_>,
    digest: [u8; 32],
    key: &ProjectProjectionKey,
    projection: &ProjectProjection,
) -> Result<(), StoreError> {
    match (key, projection) {
        (ProjectProjectionKey::Project(id), ProjectProjection::Project(view)) => {
            insert_project(transaction, digest, *id, view)
        }
        (ProjectProjectionKey::Input(id), ProjectProjection::Input(view)) => {
            insert_input(transaction, digest, *id, view)
        }
        (ProjectProjectionKey::Dispatch(id), ProjectProjection::Dispatch(view)) => {
            insert_dispatch(transaction, digest, *id, view)
        }
        (ProjectProjectionKey::Output(id), ProjectProjection::Output(view)) => {
            insert_output(transaction, digest, *id, view)
        }
        (ProjectProjectionKey::Command(id), ProjectProjection::Command(view)) => {
            insert_command(transaction, digest, *id, view)
        }
        _ => Err(corrupt()),
    }
}

fn load_projection(
    connection: &Connection,
    digest: [u8; 32],
    key: &ProjectProjectionKey,
) -> Result<ProjectProjection, StoreError> {
    match key {
        ProjectProjectionKey::Project(id) => load_project(connection, digest, *id)
            .map(|value| ProjectProjection::Project(Box::new(value))),
        ProjectProjectionKey::Input(id) => load_input(connection, digest, *id)
            .map(|value| ProjectProjection::Input(Box::new(value))),
        ProjectProjectionKey::Dispatch(id) => load_dispatch(connection, digest, *id)
            .map(|value| ProjectProjection::Dispatch(Box::new(value))),
        ProjectProjectionKey::Output(id) => load_output(connection, digest, *id)
            .map(|value| ProjectProjection::Output(Box::new(value))),
        ProjectProjectionKey::Command(id) => load_command(connection, digest, *id)
            .map(|value| ProjectProjection::Command(Box::new(value))),
    }
}

fn insert_project(
    transaction: &Transaction<'_>,
    digest: [u8; 32],
    project_id: ProjectId,
    view: &ProjectView,
) -> Result<(), StoreError> {
    let (predecessor_present, predecessor) =
        encode_id_option(view.predecessor.map(|id| *id.as_bytes()));
    let (brief_present, brief) = encode_text_option(view.brief.as_ref().map(ContentText::as_str));
    let (primary_present, primary) = encode_id_option(view.primary.map(|id| *id.as_bytes()));
    validate_project(project_id, view)?;
    transaction
        .execute(
            "INSERT INTO project_projects( \
                 key_digest, root_id, head_id, home_id, mailbox_installation, mailbox_id, \
                 predecessor_present, predecessor_id, name, brief_present, brief, \
                 primary_present, primary_id, lifecycle, archived, claimable, \
                 assignment_present, input_sequence \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, \
                       ?14, ?15, ?16, ?17, ?18)",
            params![
                digest.as_slice(),
                view.root.as_bytes().as_slice(),
                view.head.as_bytes().as_slice(),
                view.home.as_bytes().as_slice(),
                view.mailbox.installation_id().as_bytes().as_slice(),
                view.mailbox.mailbox_id().as_bytes().as_slice(),
                predecessor_present,
                predecessor.as_slice(),
                view.name.as_str(),
                brief_present,
                brief,
                primary_present,
                primary.as_slice(),
                encode_lifecycle(view.lifecycle),
                i64::from(view.archived),
                i64::from(view.claimable),
                i64::from(view.assignment.is_some()),
                view.input_sequence.to_be_bytes().as_slice(),
            ],
        )
        .map_err(database)?;
    insert_facts(
        transaction,
        "project_fork_participants",
        digest,
        &view.fork_participants,
    )?;
    for (resource_id, resource) in &view.resources {
        if resource_id != &resource.resource_id {
            return Err(corrupt());
        }
        transaction
            .execute(
                "INSERT INTO project_resources( \
                     key_digest, resource_id, display_scheme, display_value, \
                     canonical_scheme, canonical_value, health \
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    digest.as_slice(),
                    resource_id.as_bytes().as_slice(),
                    encode_scheme(resource.display_locator.scheme()),
                    resource.display_locator.value(),
                    encode_scheme(resource.canonical_locator.scheme()),
                    resource.canonical_locator.value(),
                    encode_health(resource.health),
                ],
            )
            .map_err(database)?;
    }
    for resource in &view.active_claims {
        transaction
            .execute(
                "INSERT INTO project_active_claims(key_digest, resource_id) VALUES (?1, ?2)",
                params![digest.as_slice(), resource.as_bytes().as_slice()],
            )
            .map_err(database)?;
    }
    for (resource, projects) in &view.claim_conflicts {
        for project in projects {
            transaction
                .execute(
                    "INSERT INTO project_claim_conflicts(key_digest, resource_id, project_id) \
                     VALUES (?1, ?2, ?3)",
                    params![
                        digest.as_slice(),
                        resource.as_bytes().as_slice(),
                        project.as_bytes().as_slice(),
                    ],
                )
                .map_err(database)?;
        }
    }
    if let Some(assignment) = &view.assignment {
        insert_assignment(transaction, digest, assignment)?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn load_project(
    connection: &Connection,
    digest: [u8; 32],
    project_id: ProjectId,
) -> Result<ProjectView, StoreError> {
    type Row = (
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        i64,
        Vec<u8>,
        String,
        i64,
        String,
        i64,
        Vec<u8>,
        i64,
        i64,
        i64,
        i64,
        Vec<u8>,
    );
    let row: Row = connection
        .query_row(
            "SELECT root_id, head_id, home_id, mailbox_installation, mailbox_id, \
                    predecessor_present, predecessor_id, name, brief_present, brief, \
                    primary_present, primary_id, lifecycle, archived, claimable, \
                    assignment_present, input_sequence \
             FROM project_projects WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                    row.get(13)?,
                    row.get(14)?,
                    row.get(15)?,
                    row.get(16)?,
                ))
            },
        )
        .map_err(database)?;
    let home = InstallationId::from_bytes(fixed(row.2)?);
    let mailbox = MailboxAddress::new(
        InstallationId::from_bytes(fixed(row.3)?),
        MailboxId::from_bytes(fixed(row.4)?),
    );
    let resources = load_resources(connection, digest)?;
    let active_claims = load_ids(
        connection,
        "project_active_claims",
        digest,
        ResourceId::from_bytes,
    )?;
    let claim_conflicts = load_claim_conflicts(connection, digest)?;
    let assignment = match row.15 {
        0 => {
            if count_for_key(connection, "project_assignments", digest)? != 0
                || count_for_key(connection, "project_assignment_support", digest)? != 0
            {
                return Err(corrupt());
            }
            None
        }
        1 => Some(load_assignment(connection, digest)?),
        _ => return Err(corrupt()),
    };
    let view = ProjectView {
        root: FactId::from_bytes(fixed(row.0)?),
        head: FactId::from_bytes(fixed(row.1)?),
        fork_participants: load_facts(connection, "project_fork_participants", digest)?,
        home,
        mailbox,
        predecessor: decode_id_option(row.5, fixed(row.6)?, ProjectId::from_bytes)?,
        name: ShortText::new(row.7).map_err(|_| corrupt())?,
        brief: decode_text_option(row.8, row.9, ContentText::new)?,
        resources,
        primary: decode_id_option(row.10, fixed(row.11)?, ResourceId::from_bytes)?,
        lifecycle: decode_lifecycle(row.12).ok_or_else(corrupt)?,
        archived: decode_bool(row.13)?,
        active_claims,
        claim_conflicts,
        claimable: decode_bool(row.14)?,
        assignment,
        input_sequence: decode_u64(row.16)?,
    };
    validate_project(project_id, &view)?;
    Ok(view)
}

fn validate_project(project_id: ProjectId, view: &ProjectView) -> Result<(), StoreError> {
    let resource_ids = view.resources.keys().copied().collect::<BTreeSet<_>>();
    if view.mailbox.installation_id() != view.home
        || view.primary.is_some_and(|id| !resource_ids.contains(&id))
        || !view.active_claims.is_subset(&resource_ids)
        || !view
            .claim_conflicts
            .keys()
            .all(|id| resource_ids.contains(id))
        || view.claim_conflicts.values().any(BTreeSet::is_empty)
        || view
            .claim_conflicts
            .values()
            .any(|projects| projects.contains(&project_id))
        || view.claimable != view.claim_conflicts.is_empty()
        || view.archived && view.lifecycle != ProjectLifecycle::Closed
        || view.lifecycle == ProjectLifecycle::Closed && view.assignment.is_some()
    {
        return Err(corrupt());
    }
    let expected_active = if view.claimable
        && matches!(
            view.lifecycle,
            ProjectLifecycle::Open | ProjectLifecycle::Closing
        ) {
        resource_ids
    } else {
        BTreeSet::new()
    };
    if view.active_claims != expected_active {
        return Err(corrupt());
    }
    if let Some(assignment) = &view.assignment {
        let expected_runnable = !assignment.cardinality_conflicted
            && view.claimable
            && view.lifecycle == ProjectLifecycle::Open
            && matches!(assignment.phase, ProjectAssignmentPhase::Runnable { .. });
        if assignment.runnable != expected_runnable {
            return Err(corrupt());
        }
    }
    Ok(())
}

fn load_resources(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<BTreeMap<ResourceId, ProjectResource>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT resource_id, display_scheme, display_value, canonical_scheme, \
                    canonical_value, health FROM project_resources \
             WHERE key_digest = ?1 ORDER BY resource_id",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([digest.as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(database)?;
    let mut resources = BTreeMap::new();
    for row in rows {
        let (id, display_scheme, display_value, canonical_scheme, canonical_value, health) =
            row.map_err(database)?;
        let id = ResourceId::from_bytes(fixed(id)?);
        if resources
            .insert(
                id,
                ProjectResource {
                    resource_id: id,
                    display_locator: decode_locator(display_scheme, display_value)?,
                    canonical_locator: decode_locator(canonical_scheme, canonical_value)?,
                    health: decode_health(health).ok_or_else(corrupt)?,
                },
            )
            .is_some()
        {
            return Err(corrupt());
        }
    }
    Ok(resources)
}

fn load_claim_conflicts(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<BTreeMap<ResourceId, BTreeSet<ProjectId>>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT resource_id, project_id FROM project_claim_conflicts \
             WHERE key_digest = ?1 ORDER BY resource_id, project_id",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([digest.as_slice()], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(database)?;
    let mut conflicts = BTreeMap::<ResourceId, BTreeSet<ProjectId>>::new();
    for row in rows {
        let (resource, project) = row.map_err(database)?;
        if !conflicts
            .entry(ResourceId::from_bytes(fixed(resource)?))
            .or_default()
            .insert(ProjectId::from_bytes(fixed(project)?))
        {
            return Err(corrupt());
        }
    }
    Ok(conflicts)
}

fn insert_assignment(
    transaction: &Transaction<'_>,
    digest: [u8; 32],
    view: &ProjectAssignmentView,
) -> Result<(), StoreError> {
    let (phase, thread, scheme, launch, error) = match &view.phase {
        ProjectAssignmentPhase::Configuring => (1, ZERO, 0, "", ""),
        ProjectAssignmentPhase::Runnable {
            thread_id,
            launch_directory,
        } => (
            2,
            *thread_id.as_bytes(),
            encode_scheme(launch_directory.scheme()),
            launch_directory.value(),
            "",
        ),
        ProjectAssignmentPhase::Blocked(error) => (3, ZERO, 0, "", error.as_str()),
    };
    let session = view
        .binding
        .as_ref()
        .map_or("", |binding| binding.session.as_str());
    if view.binding.as_ref().is_some_and(|binding| {
        binding.assignment_id != view.intent.assignment_id
            || binding.agent_id != view.intent.agent_id
            || binding.provider != view.intent.provider
    }) || (matches!(view.phase, ProjectAssignmentPhase::Runnable { .. })
        && view.binding.is_none())
    {
        return Err(corrupt());
    }
    transaction
        .execute(
            "INSERT INTO project_assignments( \
                 key_digest, assignment_id, agent_id, provider, session, phase, thread_id, \
                 launch_scheme, launch_value, error_code, cardinality_conflicted, runnable \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                digest.as_slice(),
                view.intent.assignment_id.as_bytes().as_slice(),
                view.intent.agent_id.as_bytes().as_slice(),
                view.intent.provider.as_str(),
                session,
                phase,
                thread.as_slice(),
                scheme,
                launch,
                error,
                i64::from(view.cardinality_conflicted),
                i64::from(view.runnable),
            ],
        )
        .map_err(database)?;
    insert_facts(
        transaction,
        "project_assignment_support",
        digest,
        &view.support,
    )
}

fn load_assignment(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<ProjectAssignmentView, StoreError> {
    let row = connection
        .query_row(
            "SELECT assignment_id, agent_id, provider, session, phase, thread_id, \
                    launch_scheme, launch_value, error_code, cardinality_conflicted, runnable \
             FROM project_assignments WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Vec<u8>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                ))
            },
        )
        .map_err(database)?;
    let thread = fixed(row.5)?;
    let phase = match (
        row.4,
        thread == ZERO,
        row.6,
        row.7.is_empty(),
        row.8.is_empty(),
    ) {
        (1, true, 0, true, true) => ProjectAssignmentPhase::Configuring,
        (2, false, scheme, false, true) => ProjectAssignmentPhase::Runnable {
            thread_id: ThreadId::from_bytes(thread),
            launch_directory: decode_locator(scheme, row.7)?,
        },
        (3, true, 0, true, false) => {
            ProjectAssignmentPhase::Blocked(ErrorCode::new(row.8).map_err(|_| corrupt())?)
        }
        _ => return Err(corrupt()),
    };
    let intent = AssignmentIntent {
        assignment_id: AssignmentId::from_bytes(fixed(row.0)?),
        agent_id: AgentId::from_bytes(fixed(row.1)?),
        provider: ProviderId::new(row.2.clone()).map_err(|_| corrupt())?,
    };
    let binding = if row.3.is_empty() {
        None
    } else {
        Some(AssignmentBinding {
            assignment_id: intent.assignment_id,
            agent_id: intent.agent_id,
            provider: intent.provider.clone(),
            session: ProviderSessionId::new(row.3).map_err(|_| corrupt())?,
        })
    };
    if matches!(phase, ProjectAssignmentPhase::Runnable { .. }) && binding.is_none() {
        return Err(corrupt());
    }
    Ok(ProjectAssignmentView {
        intent,
        binding,
        phase,
        cardinality_conflicted: decode_bool(row.9)?,
        runnable: decode_bool(row.10)?,
        support: load_facts(connection, "project_assignment_support", digest)?,
    })
}

fn insert_input(
    transaction: &Transaction<'_>,
    digest: [u8; 32],
    message_id: MessageId,
    view: &ProjectInputView,
) -> Result<(), StoreError> {
    if view.message_id != message_id || view.sequence == 0 {
        return Err(corrupt());
    }
    transaction
        .execute(
            "INSERT INTO project_inputs( \
                 key_digest, project_id, message_id, input_fact_id, sequence, accepted_fact \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                digest.as_slice(),
                view.project_id.as_bytes().as_slice(),
                view.message_id.as_bytes().as_slice(),
                view.input_fact_id.as_bytes().as_slice(),
                view.sequence.to_be_bytes().as_slice(),
                view.accepted_fact.as_bytes().as_slice(),
            ],
        )
        .map_err(database)?;
    Ok(())
}

fn load_input(
    connection: &Connection,
    digest: [u8; 32],
    message_id: MessageId,
) -> Result<ProjectInputView, StoreError> {
    let row = connection
        .query_row(
            "SELECT project_id, message_id, input_fact_id, sequence, accepted_fact \
             FROM project_inputs WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            },
        )
        .map_err(database)?;
    let view = ProjectInputView {
        project_id: ProjectId::from_bytes(fixed(row.0)?),
        message_id: MessageId::from_bytes(fixed(row.1)?),
        input_fact_id: FactId::from_bytes(fixed(row.2)?),
        sequence: decode_u64(row.3)?,
        accepted_fact: FactId::from_bytes(fixed(row.4)?),
    };
    if view.message_id != message_id || view.sequence == 0 {
        return Err(corrupt());
    }
    Ok(view)
}

fn insert_dispatch(
    transaction: &Transaction<'_>,
    digest: [u8; 32],
    dispatch_id: DispatchId,
    view: &ProjectDispatchView,
) -> Result<(), StoreError> {
    if view.dispatch_id != dispatch_id || view.sequence == 0 {
        return Err(corrupt());
    }
    transaction
        .execute(
            "INSERT INTO project_dispatches( \
                 key_digest, dispatch_id, message_id, sequence, assignment_id, agent_id, \
                 provider, session, thread_id, fact_id, conflicted \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                digest.as_slice(),
                view.dispatch_id.as_bytes().as_slice(),
                view.message_id.as_bytes().as_slice(),
                view.sequence.to_be_bytes().as_slice(),
                view.binding.assignment_id.as_bytes().as_slice(),
                view.binding.agent_id.as_bytes().as_slice(),
                view.binding.provider.as_str(),
                view.binding.session.as_str(),
                view.thread_id.as_bytes().as_slice(),
                view.fact_id.as_bytes().as_slice(),
                i64::from(view.conflicted),
            ],
        )
        .map_err(database)?;
    Ok(())
}

fn load_dispatch(
    connection: &Connection,
    digest: [u8; 32],
    dispatch_id: DispatchId,
) -> Result<ProjectDispatchView, StoreError> {
    let row = connection
        .query_row(
            "SELECT dispatch_id, message_id, sequence, assignment_id, agent_id, provider, \
                    session, thread_id, fact_id, conflicted \
             FROM project_dispatches WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Vec<u8>>(7)?,
                    row.get::<_, Vec<u8>>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            },
        )
        .map_err(database)?;
    let view = ProjectDispatchView {
        dispatch_id: DispatchId::from_bytes(fixed(row.0)?),
        message_id: MessageId::from_bytes(fixed(row.1)?),
        sequence: decode_u64(row.2)?,
        binding: decode_binding(row.3, row.4, row.5, row.6)?,
        thread_id: ThreadId::from_bytes(fixed(row.7)?),
        fact_id: FactId::from_bytes(fixed(row.8)?),
        conflicted: decode_bool(row.9)?,
    };
    if view.dispatch_id != dispatch_id || view.sequence == 0 {
        return Err(corrupt());
    }
    Ok(view)
}

#[derive(Clone)]
struct MessageParts {
    message_id: [u8; 32],
    sender_installation: [u8; 32],
    sender_mailbox: [u8; 32],
    recipient_present: i64,
    recipient_installation: [u8; 32],
    recipient_mailbox: [u8; 32],
    body: String,
    purpose: i64,
    presentation: i64,
    correlation_present: i64,
    correlation_provider: String,
    correlation_session: String,
    correlation_id: [u8; 32],
    project_present: i64,
    project_id: [u8; 32],
}

impl MessageParts {
    fn from_message(message: &MessageContent) -> Self {
        let (recipient_present, recipient_installation, recipient_mailbox) =
            message.recipient.map_or((0, ZERO, ZERO), |address| {
                (
                    1,
                    *address.installation_id().as_bytes(),
                    *address.mailbox_id().as_bytes(),
                )
            });
        let (correlation_present, correlation_provider, correlation_session, correlation_id) =
            message.correlation.as_ref().map_or(
                (0, String::new(), String::new(), ZERO),
                |correlation| {
                    (
                        1,
                        correlation.provider().as_str().to_owned(),
                        correlation.session().as_str().to_owned(),
                        *correlation.operation().as_bytes(),
                    )
                },
            );
        let (project_present, project_id) =
            encode_id_option(message.project_id.map(|id| *id.as_bytes()));
        Self {
            message_id: *message.message_id.as_bytes(),
            sender_installation: *message.sender.installation_id().as_bytes(),
            sender_mailbox: *message.sender.mailbox_id().as_bytes(),
            recipient_present,
            recipient_installation,
            recipient_mailbox,
            body: message.body.as_str().to_owned(),
            purpose: encode_purpose(message.purpose),
            presentation: encode_presentation(message.presentation),
            correlation_present,
            correlation_provider,
            correlation_session,
            correlation_id,
            project_present,
            project_id,
        }
    }

    fn decode(self) -> Result<MessageContent, StoreError> {
        let recipient = decode_mailbox_option(
            self.recipient_present,
            self.recipient_installation,
            self.recipient_mailbox,
        )?;
        let correlation = match (
            self.correlation_present,
            self.correlation_provider.is_empty(),
            self.correlation_session.is_empty(),
            self.correlation_id == ZERO,
        ) {
            (0, true, true, true) => None,
            (1, false, false, _) => Some(OperationCorrelation::new(
                ProviderId::new(self.correlation_provider).map_err(|_| corrupt())?,
                ProviderSessionId::new(self.correlation_session).map_err(|_| corrupt())?,
                OperationId::from_bytes(self.correlation_id),
            )),
            _ => return Err(corrupt()),
        };
        Ok(MessageContent {
            message_id: MessageId::from_bytes(self.message_id),
            sender: MailboxAddress::new(
                InstallationId::from_bytes(self.sender_installation),
                MailboxId::from_bytes(self.sender_mailbox),
            ),
            recipient,
            body: ContentText::new(self.body).map_err(|_| corrupt())?,
            purpose: decode_purpose(self.purpose).ok_or_else(corrupt)?,
            presentation: decode_presentation(self.presentation).ok_or_else(corrupt)?,
            correlation,
            project_id: decode_id_option(
                self.project_present,
                self.project_id,
                ProjectId::from_bytes,
            )?,
        })
    }
}

#[allow(clippy::too_many_lines)]
fn insert_output(
    transaction: &Transaction<'_>,
    digest: [u8; 32],
    output_id: MessageId,
    view: &ProjectOutputView,
) -> Result<(), StoreError> {
    if !valid_output(output_id, view) {
        return Err(corrupt());
    }
    let message = MessageParts::from_message(&view.message);
    transaction
        .execute(
            "INSERT INTO project_outputs( \
                 key_digest, output_id, dispatch_id, assignment_id, agent_id, provider, session, \
                 thread_id, message_id, sender_installation, sender_mailbox, recipient_present, \
                 recipient_installation, recipient_mailbox, body, purpose, presentation, \
                 correlation_present, correlation_provider, correlation_session, correlation_id, \
                 project_present, project_id, status \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, \
                       ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
            params![
                digest.as_slice(),
                view.output_id.as_bytes().as_slice(),
                view.dispatch_id.as_bytes().as_slice(),
                view.binding.assignment_id.as_bytes().as_slice(),
                view.binding.agent_id.as_bytes().as_slice(),
                view.binding.provider.as_str(),
                view.binding.session.as_str(),
                view.thread_id.as_bytes().as_slice(),
                message.message_id.as_slice(),
                message.sender_installation.as_slice(),
                message.sender_mailbox.as_slice(),
                message.recipient_present,
                message.recipient_installation.as_slice(),
                message.recipient_mailbox.as_slice(),
                message.body,
                message.purpose,
                message.presentation,
                message.correlation_present,
                message.correlation_provider,
                message.correlation_session,
                message.correlation_id.as_slice(),
                message.project_present,
                message.project_id.as_slice(),
                encode_output_status(view.status),
            ],
        )
        .map_err(database)?;
    insert_facts(transaction, "project_output_facts", digest, &view.facts)
}

#[allow(clippy::too_many_lines)]
fn load_output(
    connection: &Connection,
    digest: [u8; 32],
    output_id: MessageId,
) -> Result<ProjectOutputView, StoreError> {
    type Row = (
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        String,
        String,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        i64,
        Vec<u8>,
        Vec<u8>,
        String,
        i64,
        i64,
        i64,
        String,
        String,
        Vec<u8>,
        i64,
        Vec<u8>,
        i64,
    );
    let row: Row = connection
        .query_row(
            "SELECT output_id, dispatch_id, assignment_id, agent_id, provider, session, \
                    thread_id, message_id, sender_installation, sender_mailbox, recipient_present, \
                    recipient_installation, recipient_mailbox, body, purpose, presentation, \
                    correlation_present, correlation_provider, correlation_session, correlation_id, \
                    project_present, project_id, status \
             FROM project_outputs WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| {
                Ok((
                    row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                    row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?,
                    row.get(10)?, row.get(11)?, row.get(12)?, row.get(13)?, row.get(14)?,
                    row.get(15)?, row.get(16)?, row.get(17)?, row.get(18)?, row.get(19)?,
                    row.get(20)?, row.get(21)?, row.get(22)?,
                ))
            },
        )
        .map_err(database)?;
    let stored_output = MessageId::from_bytes(fixed(row.0)?);
    let message = MessageParts {
        message_id: fixed(row.7)?,
        sender_installation: fixed(row.8)?,
        sender_mailbox: fixed(row.9)?,
        recipient_present: row.10,
        recipient_installation: fixed(row.11)?,
        recipient_mailbox: fixed(row.12)?,
        body: row.13,
        purpose: row.14,
        presentation: row.15,
        correlation_present: row.16,
        correlation_provider: row.17,
        correlation_session: row.18,
        correlation_id: fixed(row.19)?,
        project_present: row.20,
        project_id: fixed(row.21)?,
    }
    .decode()?;
    if stored_output != output_id || message.message_id != output_id {
        return Err(corrupt());
    }
    let view = ProjectOutputView {
        output_id: stored_output,
        dispatch_id: DispatchId::from_bytes(fixed(row.1)?),
        binding: decode_binding(row.2, row.3, row.4, row.5)?,
        thread_id: ThreadId::from_bytes(fixed(row.6)?),
        message,
        status: decode_output_status(row.22).ok_or_else(corrupt)?,
        facts: load_facts(connection, "project_output_facts", digest)?,
    };
    if !valid_output(output_id, &view) {
        return Err(corrupt());
    }
    Ok(view)
}

fn valid_output(output_id: MessageId, view: &ProjectOutputView) -> bool {
    view.output_id == output_id
        && view.message.message_id == output_id
        && view.message.purpose == MessagePurpose::ProjectOutput
        && view.message.project_id.is_some()
        && !view.facts.is_empty()
}

fn insert_command(
    transaction: &Transaction<'_>,
    digest: [u8; 32],
    _command_id: CommandId,
    view: &RemoteCommandView,
) -> Result<(), StoreError> {
    if view.support.is_empty() {
        return Err(corrupt());
    }
    let mut received_head = ZERO;
    let mut result_kind = 0;
    let mut result_head = ZERO;
    let mut result_error = "";
    let mut runtime_kind = 0;
    let mut runtime_error = "";
    let stage = match &view.stage {
        RemoteCommandStage::Queued => 1,
        RemoteCommandStage::Received {
            received_head: head,
        } => {
            received_head = *head.as_bytes();
            2
        }
        RemoteCommandStage::Terminal { result, runtime } => {
            match result {
                RemoteCommandResult::Committed(head) => {
                    result_kind = 1;
                    result_head = *head.as_bytes();
                }
                RemoteCommandResult::Rejected(error) => {
                    result_kind = 2;
                    result_error = error.as_str();
                }
            }
            if let Some(runtime) = runtime {
                match runtime {
                    RuntimeObservation::Succeeded => runtime_kind = 1,
                    RuntimeObservation::Failed(error) => {
                        runtime_kind = 2;
                        runtime_error = error.as_str();
                    }
                    RuntimeObservation::Uncertain(error) => {
                        runtime_kind = 3;
                        runtime_error = error.as_str();
                    }
                }
            }
            3
        }
        RemoteCommandStage::Conflicted => 4,
    };
    transaction
        .execute(
            "INSERT INTO project_commands( \
                 key_digest, digest, project_id, expected_head, stage, received_head, \
                 result_kind, result_head, result_error, runtime_kind, runtime_error \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                digest.as_slice(),
                view.digest.as_bytes().as_slice(),
                view.project_id.as_bytes().as_slice(),
                view.expected_head.as_bytes().as_slice(),
                stage,
                received_head.as_slice(),
                result_kind,
                result_head.as_slice(),
                result_error,
                runtime_kind,
                runtime_error,
            ],
        )
        .map_err(database)?;
    insert_facts(
        transaction,
        "project_command_support",
        digest,
        &view.support,
    )
}

fn load_command(
    connection: &Connection,
    digest: [u8; 32],
    _command_id: CommandId,
) -> Result<RemoteCommandView, StoreError> {
    let row = connection
        .query_row(
            "SELECT digest, project_id, expected_head, stage, received_head, result_kind, \
                    result_head, result_error, runtime_kind, runtime_error \
             FROM project_commands WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Vec<u8>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, String>(9)?,
                ))
            },
        )
        .map_err(database)?;
    let received_head = fixed(row.4)?;
    let result_head = fixed(row.6)?;
    let stage = match row.3 {
        1 if received_head == ZERO
            && row.5 == 0
            && result_head == ZERO
            && row.7.is_empty()
            && row.8 == 0
            && row.9.is_empty() =>
        {
            RemoteCommandStage::Queued
        }
        2 if received_head != ZERO
            && row.5 == 0
            && result_head == ZERO
            && row.7.is_empty()
            && row.8 == 0
            && row.9.is_empty() =>
        {
            RemoteCommandStage::Received {
                received_head: FactId::from_bytes(received_head),
            }
        }
        3 if received_head == ZERO => RemoteCommandStage::Terminal {
            result: decode_result(row.5, result_head, row.7)?,
            runtime: decode_runtime(row.8, row.9)?,
        },
        4 if received_head == ZERO
            && row.5 == 0
            && result_head == ZERO
            && row.7.is_empty()
            && row.8 == 0
            && row.9.is_empty() =>
        {
            RemoteCommandStage::Conflicted
        }
        _ => return Err(corrupt()),
    };
    let view = RemoteCommandView {
        digest: CommandDigest::from_bytes(fixed(row.0)?),
        project_id: ProjectId::from_bytes(fixed(row.1)?),
        expected_head: FactId::from_bytes(fixed(row.2)?),
        stage,
        support: load_facts(connection, "project_command_support", digest)?,
    };
    if view.support.is_empty() {
        return Err(corrupt());
    }
    Ok(view)
}

fn decode_result(
    kind: i64,
    head: [u8; 32],
    error: String,
) -> Result<RemoteCommandResult, StoreError> {
    match (kind, head == ZERO, error.is_empty()) {
        (1, false, true) => Ok(RemoteCommandResult::Committed(FactId::from_bytes(head))),
        (2, true, false) => Ok(RemoteCommandResult::Rejected(
            ErrorCode::new(error).map_err(|_| corrupt())?,
        )),
        _ => Err(corrupt()),
    }
}

fn decode_runtime(kind: i64, error: String) -> Result<Option<RuntimeObservation>, StoreError> {
    match (kind, error.is_empty()) {
        (0, true) => Ok(None),
        (1, true) => Ok(Some(RuntimeObservation::Succeeded)),
        (2, false) => Ok(Some(RuntimeObservation::Failed(
            ErrorCode::new(error).map_err(|_| corrupt())?,
        ))),
        (3, false) => Ok(Some(RuntimeObservation::Uncertain(
            ErrorCode::new(error).map_err(|_| corrupt())?,
        ))),
        _ => Err(corrupt()),
    }
}

fn decode_binding(
    assignment: Vec<u8>,
    agent: Vec<u8>,
    provider: String,
    session: String,
) -> Result<AssignmentBinding, StoreError> {
    Ok(AssignmentBinding {
        assignment_id: AssignmentId::from_bytes(fixed(assignment)?),
        agent_id: AgentId::from_bytes(fixed(agent)?),
        provider: ProviderId::new(provider).map_err(|_| corrupt())?,
        session: ProviderSessionId::new(session).map_err(|_| corrupt())?,
    })
}

fn load_ids<T: Ord>(
    connection: &Connection,
    table: &str,
    digest: [u8; 32],
    decode: impl Fn([u8; 32]) -> T,
) -> Result<BTreeSet<T>, StoreError> {
    let query = match table {
        "project_active_claims" => {
            "SELECT resource_id FROM project_active_claims \
             WHERE key_digest = ?1 ORDER BY resource_id"
        }
        _ => return Err(corrupt()),
    };
    let mut statement = connection.prepare(query).map_err(database)?;
    let rows = statement
        .query_map([digest.as_slice()], |row| row.get::<_, Vec<u8>>(0))
        .map_err(database)?;
    let mut values = BTreeSet::new();
    for row in rows {
        if !values.insert(decode(fixed(row.map_err(database)?)?)) {
            return Err(corrupt());
        }
    }
    Ok(values)
}

fn count_for_key(
    connection: &Connection,
    table: &str,
    digest: [u8; 32],
) -> Result<i64, StoreError> {
    if !TABLES.contains(&table) {
        return Err(corrupt());
    }
    connection
        .query_row(
            &format!("SELECT count(*) FROM {table} WHERE key_digest = ?1"),
            [digest.as_slice()],
            |row| row.get(0),
        )
        .map_err(database)
}

const fn encode_lifecycle(value: ProjectLifecycle) -> i64 {
    match value {
        ProjectLifecycle::Open => 1,
        ProjectLifecycle::Closing => 2,
        ProjectLifecycle::Closed => 3,
    }
}

const fn decode_lifecycle(value: i64) -> Option<ProjectLifecycle> {
    match value {
        1 => Some(ProjectLifecycle::Open),
        2 => Some(ProjectLifecycle::Closing),
        3 => Some(ProjectLifecycle::Closed),
        _ => None,
    }
}

const fn encode_health(value: ResourceHealth) -> i64 {
    match value {
        ResourceHealth::Unknown => 1,
        ResourceHealth::Healthy => 2,
        ResourceHealth::Degraded => 3,
        ResourceHealth::Unavailable => 4,
    }
}

const fn decode_health(value: i64) -> Option<ResourceHealth> {
    match value {
        1 => Some(ResourceHealth::Unknown),
        2 => Some(ResourceHealth::Healthy),
        3 => Some(ResourceHealth::Degraded),
        4 => Some(ResourceHealth::Unavailable),
        _ => None,
    }
}

const fn encode_output_status(value: ProjectOutputStatus) -> i64 {
    match value {
        ProjectOutputStatus::Current => 1,
        ProjectOutputStatus::LateFromInactive => 2,
        ProjectOutputStatus::Conflicted => 3,
    }
}

const fn decode_output_status(value: i64) -> Option<ProjectOutputStatus> {
    match value {
        1 => Some(ProjectOutputStatus::Current),
        2 => Some(ProjectOutputStatus::LateFromInactive),
        3 => Some(ProjectOutputStatus::Conflicted),
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

const fn encode_purpose(value: MessagePurpose) -> i64 {
    match value {
        MessagePurpose::Question => 1,
        MessagePurpose::Asynchronous => 2,
        MessagePurpose::ProjectOutput => 3,
    }
}

const fn decode_purpose(value: i64) -> Option<MessagePurpose> {
    match value {
        1 => Some(MessagePurpose::Question),
        2 => Some(MessagePurpose::Asynchronous),
        3 => Some(MessagePurpose::ProjectOutput),
        _ => None,
    }
}

const fn encode_presentation(value: PresentationKind) -> i64 {
    match value {
        PresentationKind::Message => 1,
        PresentationKind::FinalAnswer => 2,
        PresentationKind::Status => 3,
    }
}

const fn decode_presentation(value: i64) -> Option<PresentationKind> {
    match value {
        1 => Some(PresentationKind::Message),
        2 => Some(PresentationKind::FinalAnswer),
        3 => Some(PresentationKind::Status),
        _ => None,
    }
}

fn decode_locator(scheme: i64, value: String) -> Result<ResourceLocator, StoreError> {
    Ok(ResourceLocator::new(
        decode_scheme(scheme).ok_or_else(corrupt)?,
        BoundedText::new(value).map_err(|_| corrupt())?,
    ))
}

fn encode_id_option(value: Option<[u8; 32]>) -> (i64, [u8; 32]) {
    value.map_or((0, ZERO), |value| (1, value))
}

fn decode_id_option<T>(
    present: i64,
    value: [u8; 32],
    decode: impl Fn([u8; 32]) -> T,
) -> Result<Option<T>, StoreError> {
    match (present, value == ZERO) {
        (0, true) => Ok(None),
        (1, _) => Ok(Some(decode(value))),
        _ => Err(corrupt()),
    }
}

fn encode_text_option(value: Option<&str>) -> (i64, &str) {
    value.map_or((0, ""), |value| (1, value))
}

fn decode_text_option<T>(
    present: i64,
    value: String,
    decode: impl Fn(String) -> Result<T, hq_domain::ValidatedValueError>,
) -> Result<Option<T>, StoreError> {
    match (present, value.is_empty()) {
        (0, true) => Ok(None),
        (1, false) => Ok(Some(decode(value).map_err(|_| corrupt())?)),
        _ => Err(corrupt()),
    }
}

fn decode_mailbox_option(
    present: i64,
    installation: [u8; 32],
    mailbox: [u8; 32],
) -> Result<Option<MailboxAddress>, StoreError> {
    match (present, installation == ZERO, mailbox == ZERO) {
        (0, true, true) => Ok(None),
        (1, _, _) => Ok(Some(MailboxAddress::new(
            InstallationId::from_bytes(installation),
            MailboxId::from_bytes(mailbox),
        ))),
        _ => Err(corrupt()),
    }
}

fn decode_bool(value: i64) -> Result<bool, StoreError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(corrupt()),
    }
}

fn decode_u64(value: Vec<u8>) -> Result<u64, StoreError> {
    Ok(u64::from_be_bytes(value.try_into().map_err(|_| corrupt())?))
}

fn fixed(value: Vec<u8>) -> Result<[u8; 32], StoreError> {
    value.try_into().map_err(|_| corrupt())
}

fn fixed_sql(value: Vec<u8>) -> rusqlite::Result<[u8; 32]> {
    value.try_into().map_err(|value: Vec<u8>| {
        rusqlite::Error::FromSqlConversionFailure(
            value.len(),
            rusqlite::types::Type::Blob,
            "expected 32-byte project identity".into(),
        )
    })
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Counts {
    aggregate_key_count: i64,
    frontier_count: i64,
    projection_key_count: i64,
    projection_count: i64,
    support_count: i64,
    row_count: i64,
}

impl Counts {
    fn read(connection: &Connection) -> Result<Self, StoreError> {
        let aggregate_key_count = count(connection, "project_aggregate_keys")?;
        let frontier_count = count(connection, "project_frontiers")?;
        let projection_key_count = count(connection, "project_projection_keys")?;
        let support_count = count(connection, "project_support")?;
        let projection_count = [
            "project_projects",
            "project_inputs",
            "project_dispatches",
            "project_outputs",
            "project_commands",
        ]
        .into_iter()
        .try_fold(0_i64, |total, table| {
            total
                .checked_add(count(connection, table)?)
                .ok_or_else(corrupt)
        })?;
        let row_count = TABLES.into_iter().try_fold(0_i64, |total, table| {
            total
                .checked_add(count(connection, table)?)
                .ok_or_else(corrupt)
        })?;
        let counts = Self {
            aggregate_key_count,
            frontier_count,
            projection_key_count,
            projection_count,
            support_count,
            row_count,
        };
        counts.validate()?;
        Ok(counts)
    }

    fn validate(self) -> Result<(), StoreError> {
        for value in [
            self.aggregate_key_count,
            self.frontier_count,
            self.projection_key_count,
            self.projection_count,
            self.support_count,
            self.row_count,
        ] {
            if !(0..=MAXIMUM_PROJECT_ROWS).contains(&value) {
                return Err(corrupt());
            }
        }
        Ok(())
    }
}

struct State {
    counts: Counts,
    digest: [u8; 32],
}

fn load_state(connection: &Connection) -> Result<Option<State>, StoreError> {
    let row = connection
        .query_row(
            "SELECT aggregate_key_count, frontier_count, projection_key_count, projection_count, \
                    support_count, row_count, row_digest FROM project_state WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get::<_, Vec<u8>>(6)?,
                ))
            },
        )
        .optional()
        .map_err(database)?;
    row.map(|row| {
        Ok(State {
            counts: Counts {
                aggregate_key_count: row.0,
                frontier_count: row.1,
                projection_key_count: row.2,
                projection_count: row.3,
                support_count: row.4,
                row_count: row.5,
            },
            digest: fixed(row.6)?,
        })
    })
    .transpose()
}

fn count(connection: &Connection, table: &str) -> Result<i64, StoreError> {
    if !TABLES.contains(&table) {
        return Err(corrupt());
    }
    connection
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .map_err(database)
}

fn row_digest(connection: &Connection) -> Result<[u8; 32], StoreError> {
    const QUERIES: [&str; 17] = [
        "SELECT * FROM project_aggregate_keys ORDER BY key_digest",
        "SELECT * FROM project_frontiers ORDER BY key_digest, fact_id",
        "SELECT * FROM project_projection_keys ORDER BY key_digest",
        "SELECT * FROM project_support ORDER BY key_digest, fact_id",
        "SELECT * FROM project_projects ORDER BY key_digest",
        "SELECT * FROM project_fork_participants ORDER BY key_digest, fact_id",
        "SELECT * FROM project_resources ORDER BY key_digest, resource_id",
        "SELECT * FROM project_active_claims ORDER BY key_digest, resource_id",
        "SELECT * FROM project_claim_conflicts ORDER BY key_digest, resource_id, project_id",
        "SELECT * FROM project_assignments ORDER BY key_digest",
        "SELECT * FROM project_assignment_support ORDER BY key_digest, fact_id",
        "SELECT * FROM project_inputs ORDER BY key_digest",
        "SELECT * FROM project_dispatches ORDER BY key_digest",
        "SELECT * FROM project_outputs ORDER BY key_digest",
        "SELECT * FROM project_output_facts ORDER BY key_digest, fact_id",
        "SELECT * FROM project_commands ORDER BY key_digest",
        "SELECT * FROM project_command_support ORDER BY key_digest, fact_id",
    ];
    let mut digest = Sha256::new();
    for (table, query) in TABLES.into_iter().zip(QUERIES) {
        put_text(&mut digest, table);
        let mut statement = connection.prepare(query).map_err(database)?;
        let columns = statement.column_count();
        let mut rows = statement.query([]).map_err(database)?;
        while let Some(row) = rows.next().map_err(database)? {
            digest.update(u64::try_from(columns).unwrap_or(u64::MAX).to_be_bytes());
            for index in 0..columns {
                put_value(&mut digest, row.get_ref(index).map_err(database)?);
            }
        }
    }
    Ok(digest.finalize().into())
}

fn put_text(digest: &mut Sha256, value: &str) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    digest.update(value.as_bytes());
}

fn put_value(digest: &mut Sha256, value: rusqlite::types::ValueRef<'_>) {
    match value {
        rusqlite::types::ValueRef::Null => digest.update([0]),
        rusqlite::types::ValueRef::Integer(value) => {
            digest.update([1]);
            digest.update(value.to_be_bytes());
        }
        rusqlite::types::ValueRef::Real(value) => {
            digest.update([2]);
            digest.update(value.to_bits().to_be_bytes());
        }
        rusqlite::types::ValueRef::Text(value) => {
            digest.update([3]);
            digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
            digest.update(value);
        }
        rusqlite::types::ValueRef::Blob(value) => {
            digest.update([4]);
            digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
            digest.update(value);
        }
    }
}

fn validate_counts(snapshot: &ProjectProjectionSnapshot, counts: Counts) -> Result<(), StoreError> {
    counts.validate()?;
    if snapshot.projections().keys().ne(snapshot.support().keys())
        || counts.aggregate_key_count != length(std::iter::once(snapshot.frontiers().len()))?
        || counts.frontier_count != length(snapshot.frontiers().values().map(BTreeSet::len))?
        || counts.projection_key_count != length(std::iter::once(snapshot.projections().len()))?
        || counts.projection_count != length(std::iter::once(snapshot.projections().len()))?
        || counts.support_count != length(snapshot.support().values().map(BTreeSet::len))?
    {
        return Err(corrupt());
    }
    Ok(())
}

fn length(values: impl IntoIterator<Item = usize>) -> Result<i64, StoreError> {
    values.into_iter().try_fold(0_i64, |total, value| {
        total
            .checked_add(i64::try_from(value).map_err(|_| corrupt())?)
            .ok_or_else(corrupt)
    })
}

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
    fn every_project_projection_variant_and_nested_state_round_trips() {
        let expected = exhaustive_snapshot();
        let connection = fixture_connection(&expected);
        assert_eq!(load(&connection).expect("project rows load"), expected);
    }

    #[test]
    fn project_scalar_codecs_are_closed_and_full_width() {
        for lifecycle in [
            ProjectLifecycle::Open,
            ProjectLifecycle::Closing,
            ProjectLifecycle::Closed,
        ] {
            assert_eq!(
                decode_lifecycle(encode_lifecycle(lifecycle)),
                Some(lifecycle)
            );
        }
        assert_eq!(decode_lifecycle(0), None);
        for purpose in [
            MessagePurpose::Question,
            MessagePurpose::Asynchronous,
            MessagePurpose::ProjectOutput,
        ] {
            assert_eq!(decode_purpose(encode_purpose(purpose)), Some(purpose));
        }
        assert_eq!(decode_health(0), None);
        assert_eq!(decode_output_status(4), None);
        assert_eq!(decode_purpose(0), None);
        assert_eq!(decode_presentation(4), None);
        assert_eq!(decode_u64(u64::MAX.to_be_bytes().to_vec()), Ok(u64::MAX));
        assert!(decode_u64(vec![0; 7]).is_err());
        assert!(decode_runtime(1, "unexpected".to_owned()).is_err());
        assert!(decode_result(2, ZERO, String::new()).is_err());
    }

    #[test]
    fn every_project_table_family_fails_closed_on_valid_looking_corruption() {
        let expected = exhaustive_snapshot();
        for mutation in [
            "UPDATE project_state SET row_count = row_count + 1",
            "UPDATE project_aggregate_keys SET key_a = zeroblob(32)",
            "UPDATE project_frontiers SET fact_id = zeroblob(32) WHERE (key_digest, fact_id) = (SELECT key_digest, fact_id FROM project_frontiers LIMIT 1)",
            "UPDATE project_projection_keys SET key_id = zeroblob(32)",
            "UPDATE project_support SET fact_id = zeroblob(32) WHERE (key_digest, fact_id) = (SELECT key_digest, fact_id FROM project_support LIMIT 1)",
            "UPDATE project_projects SET archived = CASE archived WHEN 1 THEN 0 ELSE 1 END",
            "UPDATE project_fork_participants SET fact_id = zeroblob(32) WHERE fact_id = (SELECT fact_id FROM project_fork_participants LIMIT 1)",
            "UPDATE project_resources SET canonical_value = canonical_value || '-changed'",
            "UPDATE project_active_claims SET resource_id = zeroblob(32) WHERE (key_digest, resource_id) = (SELECT key_digest, resource_id FROM project_active_claims LIMIT 1)",
            "UPDATE project_claim_conflicts SET project_id = zeroblob(32)",
            "UPDATE project_assignments SET runnable = CASE runnable WHEN 1 THEN 0 ELSE 1 END",
            "UPDATE project_assignment_support SET fact_id = zeroblob(32) WHERE (key_digest, fact_id) = (SELECT key_digest, fact_id FROM project_assignment_support LIMIT 1)",
            "UPDATE project_inputs SET sequence = x'0000000000000001'",
            "UPDATE project_dispatches SET conflicted = CASE conflicted WHEN 1 THEN 0 ELSE 1 END",
            "UPDATE project_outputs SET body = body || '-changed'",
            "UPDATE project_output_facts SET fact_id = zeroblob(32) WHERE (key_digest, fact_id) = (SELECT key_digest, fact_id FROM project_output_facts LIMIT 1)",
            "UPDATE project_commands SET expected_head = zeroblob(32)",
            "UPDATE project_command_support SET fact_id = zeroblob(32)",
        ] {
            let connection = fixture_connection(&expected);
            connection
                .execute_batch(mutation)
                .expect("constraint-valid mutation applies");
            assert_eq!(
                load(&connection)
                    .expect_err("changed project rows reject")
                    .class(),
                StoreErrorClass::RebuildableStateCorrupt,
                "mutation unexpectedly loaded: {mutation}",
            );
        }
    }

    #[allow(clippy::too_many_lines)]
    fn exhaustive_snapshot() -> ProjectProjectionSnapshot {
        let home = InstallationId::from_bytes([0x21; 32]);
        let project_one = ProjectId::from_bytes([0x31; 32]);
        let project_two = ProjectId::from_bytes([0x32; 32]);
        let project_three = ProjectId::from_bytes([0x33; 32]);
        let project_four = ProjectId::from_bytes([0x34; 32]);
        let resource_one = resource_with_locators(
            0x41,
            ResourceScheme::WorkingTree,
            "/selected/one",
            "/workspace/one",
            ResourceHealth::Healthy,
        );
        let resource_two = resource(
            0x42,
            ResourceScheme::GitRepository,
            "repo:two",
            ResourceHealth::Degraded,
        );
        let binding = assignment(0x51, 0x52, "provider-one", "session-one");
        let runnable = ProjectAssignmentView {
            intent: AssignmentIntent {
                assignment_id: binding.assignment_id,
                agent_id: binding.agent_id,
                provider: binding.provider.clone(),
            },
            binding: Some(binding.clone()),
            phase: ProjectAssignmentPhase::Runnable {
                thread_id: ThreadId::from_bytes([0x53; 32]),
                launch_directory: locator(ResourceScheme::WorkingTree, "/workspace/one"),
            },
            cardinality_conflicted: false,
            runnable: true,
            support: set([id(9), id(10)]),
        };
        let blocked_binding = assignment(0x54, 0x55, "provider-two", "session-two");
        let blocked = ProjectAssignmentView {
            intent: AssignmentIntent {
                assignment_id: blocked_binding.assignment_id,
                agent_id: blocked_binding.agent_id,
                provider: blocked_binding.provider.clone(),
            },
            binding: Some(blocked_binding),
            phase: ProjectAssignmentPhase::Blocked(error("blocked")),
            cardinality_conflicted: true,
            runnable: false,
            support: set([id(11)]),
        };
        let configuring_binding = assignment(0x56, 0x57, "provider-three", "session-three");
        let configuring = ProjectAssignmentView {
            intent: AssignmentIntent {
                assignment_id: configuring_binding.assignment_id,
                agent_id: configuring_binding.agent_id,
                provider: configuring_binding.provider,
            },
            binding: None,
            phase: ProjectAssignmentPhase::Configuring,
            cardinality_conflicted: false,
            runnable: false,
            support: set([id(12)]),
        };
        let project_keys = [
            ProjectProjectionKey::Project(project_one),
            ProjectProjectionKey::Project(project_two),
            ProjectProjectionKey::Project(project_three),
            ProjectProjectionKey::Project(project_four),
        ];
        let mut projections = BTreeMap::from([
            (
                project_keys[0].clone(),
                ProjectProjection::Project(Box::new(ProjectView {
                    root: id(1),
                    head: id(2),
                    fork_participants: set([id(3), id(4)]),
                    home,
                    mailbox: MailboxAddress::new(home, MailboxId::from_bytes([0x61; 32])),
                    predecessor: Some(ProjectId::from_bytes([0x30; 32])),
                    name: short("project-one"),
                    brief: Some(content("complete brief")),
                    resources: BTreeMap::from([
                        (resource_one.resource_id, resource_one.clone()),
                        (resource_two.resource_id, resource_two.clone()),
                    ]),
                    primary: Some(resource_one.resource_id),
                    lifecycle: ProjectLifecycle::Open,
                    archived: false,
                    active_claims: set([resource_one.resource_id, resource_two.resource_id]),
                    claim_conflicts: BTreeMap::new(),
                    claimable: true,
                    assignment: Some(runnable),
                    input_sequence: u64::MAX,
                })),
            ),
            (
                project_keys[1].clone(),
                ProjectProjection::Project(Box::new(ProjectView {
                    root: id(5),
                    head: id(6),
                    fork_participants: BTreeSet::new(),
                    home,
                    mailbox: MailboxAddress::new(home, MailboxId::from_bytes([0x62; 32])),
                    predecessor: None,
                    name: short("project-two"),
                    brief: None,
                    resources: BTreeMap::from([(resource_one.resource_id, resource_one.clone())]),
                    primary: Some(resource_one.resource_id),
                    lifecycle: ProjectLifecycle::Closing,
                    archived: false,
                    active_claims: BTreeSet::new(),
                    claim_conflicts: BTreeMap::from([(
                        resource_one.resource_id,
                        set([project_one]),
                    )]),
                    claimable: false,
                    assignment: Some(blocked),
                    input_sequence: 7,
                })),
            ),
            (
                project_keys[2].clone(),
                ProjectProjection::Project(Box::new(ProjectView {
                    root: id(7),
                    head: id(8),
                    fork_participants: BTreeSet::new(),
                    home,
                    mailbox: MailboxAddress::new(home, MailboxId::from_bytes([0x63; 32])),
                    predecessor: None,
                    name: short("project-three"),
                    brief: None,
                    resources: BTreeMap::new(),
                    primary: None,
                    lifecycle: ProjectLifecycle::Open,
                    archived: false,
                    active_claims: BTreeSet::new(),
                    claim_conflicts: BTreeMap::new(),
                    claimable: true,
                    assignment: Some(configuring),
                    input_sequence: 0,
                })),
            ),
            (
                project_keys[3].clone(),
                ProjectProjection::Project(Box::new(ProjectView {
                    root: id(25),
                    head: id(26),
                    fork_participants: BTreeSet::new(),
                    home,
                    mailbox: MailboxAddress::new(home, MailboxId::from_bytes([0x64; 32])),
                    predecessor: None,
                    name: short("project-four"),
                    brief: None,
                    resources: BTreeMap::new(),
                    primary: None,
                    lifecycle: ProjectLifecycle::Closed,
                    archived: true,
                    active_claims: BTreeSet::new(),
                    claim_conflicts: BTreeMap::new(),
                    claimable: true,
                    assignment: None,
                    input_sequence: 0,
                })),
            ),
        ]);
        let input_id = MessageId::from_bytes([0x71; 32]);
        let dispatch_id = DispatchId::from_bytes([0x72; 32]);
        projections.insert(
            ProjectProjectionKey::Input(input_id),
            ProjectProjection::Input(Box::new(ProjectInputView {
                project_id: project_one,
                message_id: input_id,
                input_fact_id: id(13),
                sequence: u64::MAX,
                accepted_fact: id(14),
            })),
        );
        projections.insert(
            ProjectProjectionKey::Dispatch(dispatch_id),
            ProjectProjection::Dispatch(Box::new(ProjectDispatchView {
                dispatch_id,
                message_id: input_id,
                sequence: u64::MAX,
                binding: binding.clone(),
                thread_id: ThreadId::from_bytes([0x73; 32]),
                fact_id: id(15),
                conflicted: true,
            })),
        );
        for (offset, status, recipient, correlation) in [
            (0_u8, ProjectOutputStatus::Current, true, true),
            (1, ProjectOutputStatus::LateFromInactive, false, false),
            (2, ProjectOutputStatus::Conflicted, true, false),
        ] {
            let output_id = MessageId::from_bytes([0x74 + offset; 32]);
            projections.insert(
                ProjectProjectionKey::Output(output_id),
                ProjectProjection::Output(Box::new(ProjectOutputView {
                    output_id,
                    dispatch_id,
                    binding: binding.clone(),
                    thread_id: ThreadId::from_bytes([0x73; 32]),
                    message: message(
                        output_id,
                        MessagePurpose::ProjectOutput,
                        recipient,
                        correlation,
                        true,
                        project_one,
                    ),
                    status,
                    facts: set([id(16 + offset), id(20 + offset)]),
                })),
            );
        }
        let command_stages = [
            RemoteCommandStage::Queued,
            RemoteCommandStage::Received {
                received_head: id(23),
            },
            RemoteCommandStage::Terminal {
                result: RemoteCommandResult::Committed(id(24)),
                runtime: None,
            },
            RemoteCommandStage::Terminal {
                result: RemoteCommandResult::Rejected(error("rejected")),
                runtime: Some(RuntimeObservation::Succeeded),
            },
            RemoteCommandStage::Terminal {
                result: RemoteCommandResult::Rejected(error("failed")),
                runtime: Some(RuntimeObservation::Failed(error("runtime-failed"))),
            },
            RemoteCommandStage::Terminal {
                result: RemoteCommandResult::Rejected(error("uncertain")),
                runtime: Some(RuntimeObservation::Uncertain(error("runtime-uncertain"))),
            },
            RemoteCommandStage::Conflicted,
        ];
        for (offset, stage) in command_stages.into_iter().enumerate() {
            let offset = u8::try_from(offset).expect("small offset");
            let command_id = CommandId::from_bytes([0x80 + offset; 32]);
            projections.insert(
                ProjectProjectionKey::Command(command_id),
                ProjectProjection::Command(Box::new(RemoteCommandView {
                    digest: CommandDigest::from_bytes([0x90 + offset; 32]),
                    project_id: project_one,
                    expected_head: id(30 + offset),
                    stage,
                    support: set([id(40 + offset)]),
                })),
            );
        }
        let support = projections
            .keys()
            .enumerate()
            .map(|(index, key)| {
                (
                    key.clone(),
                    set([id(60 + u8::try_from(index).expect("small support index"))]),
                )
            })
            .collect();
        let frontiers = BTreeMap::from([
            (
                ProjectAggregateKey::Project(project_one),
                set([id(1), id(2)]),
            ),
            (
                ProjectAggregateKey::Resource {
                    home,
                    locator: resource_one.canonical_locator.clone(),
                },
                set([id(3)]),
            ),
            (
                ProjectAggregateKey::AgentAssignment {
                    home,
                    agent: binding.agent_id,
                },
                set([id(4)]),
            ),
            (ProjectAggregateKey::Input(input_id), set([id(5)])),
            (ProjectAggregateKey::Dispatch(dispatch_id), set([id(6)])),
            (
                ProjectAggregateKey::Output(MessageId::from_bytes([0x74; 32])),
                set([id(7)]),
            ),
            (
                ProjectAggregateKey::Command(CommandId::from_bytes([0x80; 32])),
                set([id(8)]),
            ),
        ]);
        ProjectProjectionSnapshot::new(frontiers, projections, support)
    }

    fn fixture_connection(snapshot: &ProjectProjectionSnapshot) -> Connection {
        let mut connection = Connection::open_in_memory().expect("memory database opens");
        connection
            .execute_batch(super::super::SCHEMA)
            .expect("schema creates");
        for value in 0_u8..=100 {
            connection
                .execute(
                    "INSERT INTO canonical_facts(fact_id, event_bytes, namespace, family) \
                     VALUES (?1, x'00', 1, 1)",
                    [[value; 32].as_slice()],
                )
                .expect("canonical fixture fact inserts");
        }
        let transaction = connection.transaction().expect("transaction begins");
        insert(&transaction, snapshot).expect("project rows insert");
        transaction.commit().expect("transaction commits");
        connection
    }

    fn id(value: u8) -> FactId {
        FactId::from_bytes([value; 32])
    }

    fn set<T: Ord, const N: usize>(values: [T; N]) -> BTreeSet<T> {
        values.into_iter().collect()
    }

    fn short(value: &str) -> ShortText {
        ShortText::new(value).expect("short text validates")
    }

    fn content(value: &str) -> ContentText {
        ContentText::new(value).expect("content validates")
    }

    fn error(value: &str) -> ErrorCode {
        ErrorCode::new(value).expect("error validates")
    }

    fn locator(scheme: ResourceScheme, value: &str) -> ResourceLocator {
        ResourceLocator::new(scheme, BoundedText::new(value).expect("locator validates"))
    }

    fn resource(
        id: u8,
        scheme: ResourceScheme,
        value: &str,
        health: ResourceHealth,
    ) -> ProjectResource {
        ProjectResource {
            resource_id: ResourceId::from_bytes([id; 32]),
            display_locator: locator(scheme, value),
            canonical_locator: locator(scheme, value),
            health,
        }
    }

    fn resource_with_locators(
        id: u8,
        scheme: ResourceScheme,
        display: &str,
        canonical: &str,
        health: ResourceHealth,
    ) -> ProjectResource {
        ProjectResource {
            resource_id: ResourceId::from_bytes([id; 32]),
            display_locator: locator(scheme, display),
            canonical_locator: locator(scheme, canonical),
            health,
        }
    }

    fn assignment(id: u8, agent: u8, provider: &str, session: &str) -> AssignmentBinding {
        AssignmentBinding {
            assignment_id: AssignmentId::from_bytes([id; 32]),
            agent_id: AgentId::from_bytes([agent; 32]),
            provider: ProviderId::new(provider).expect("provider validates"),
            session: ProviderSessionId::new(session).expect("session validates"),
        }
    }

    fn message(
        id: MessageId,
        purpose: MessagePurpose,
        recipient: bool,
        correlation: bool,
        project: bool,
        project_id: ProjectId,
    ) -> MessageContent {
        MessageContent {
            message_id: id,
            sender: MailboxAddress::new(
                InstallationId::from_bytes([0xa1; 32]),
                MailboxId::from_bytes([0xa2; 32]),
            ),
            recipient: recipient.then(|| {
                MailboxAddress::new(
                    InstallationId::from_bytes([0xa3; 32]),
                    MailboxId::from_bytes([0xa4; 32]),
                )
            }),
            body: content("project output"),
            purpose,
            presentation: PresentationKind::FinalAnswer,
            correlation: correlation.then(|| {
                OperationCorrelation::new(
                    ProviderId::new("provider-one").expect("provider validates"),
                    ProviderSessionId::new("session-one").expect("session validates"),
                    OperationId::from_bytes([0xa5; 32]),
                )
            }),
            project_id: project.then_some(project_id),
        }
    }
}
