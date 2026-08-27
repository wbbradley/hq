//! Durable relay synchronization state contracts.

#![allow(clippy::expect_used)]

use hq_application::{RelayAccess, RelayAuthentication};
use hq_domain::{CommandDigest, OperationId};
use hq_store::{
    MAX_RELAY_QUARANTINE_ITEMS, MAX_RELAY_QUARANTINE_SAMPLE_BYTES, MAX_RELAY_STAGING_BYTES,
    MAX_RELAY_STAGING_ITEMS, MAX_RELAY_STATE_QUERY_ITEMS, MAX_RELAY_WRAPPER_BYTES, Store,
    StoreErrorClass, StoredAttemptDisposition, StoredCatchupCursor, StoredDesiredRelayPolicy,
    StoredInboundClaim, StoredPreparedOutbound, StoredQuarantineEvidence, StoredRelayAttempt,
    StoredRelayAttemptFailure, StoredRelayPolicyChange, StoredRelayStateMutation,
    StoredRelayStateQuery, StoredStagedInput,
};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

mod support;

use support::{
    TestDirectory, authority_policy, open_store, verified_account, verified_device_acceptance,
    verified_device_grant, verified_fact, verified_project,
};

const RELAY: &str = "wss://relay.example";

#[test]
fn relay_state_keyset_pages_reach_rows_after_the_first_bound() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    for (index, relay) in ["wss://a.example", "wss://b.example", "wss://c.example"]
        .into_iter()
        .enumerate()
    {
        let mut change = policy_change(
            u8::try_from(index + 1).expect("small operation index"),
            u8::try_from(index + 1).expect("small digest index"),
            true,
            RelayAccess::Read,
        );
        change.desired.url = relay.to_owned();
        store
            .apply_relay_state(StoredRelayStateMutation::Configure(change))
            .expect("policy commits");
    }

    let mut query = StoredRelayStateQuery::first(1);
    let mut urls = Vec::new();
    loop {
        let page = store
            .load_relay_state_page(query)
            .expect("bounded page loads");
        urls.extend(page.state.policies.into_iter().map(|policy| policy.url));
        let Some(next) = page.next else {
            break;
        };
        assert!(page.state.outbound.is_empty());
        query = next;
    }
    assert_eq!(
        urls,
        vec![
            "wss://a.example".to_owned(),
            "wss://b.example".to_owned(),
            "wss://c.example".to_owned(),
        ]
    );
}

#[test]
fn policy_operations_allocate_generations_and_replay_exactly_across_restart() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let mut invalid_url = policy_change(9, 9, true, RelayAccess::ReadWrite);
    invalid_url.desired.url = "ws:///missing-authority".to_owned();
    assert_eq!(
        store
            .apply_relay_state(StoredRelayStateMutation::Configure(invalid_url))
            .expect_err("invalid durable URL rejects")
            .class(),
        StoreErrorClass::InvalidOperationalRequest
    );
    let first = policy_change(1, 1, true, RelayAccess::ReadWrite);
    store
        .apply_relay_state(StoredRelayStateMutation::Configure(first.clone()))
        .expect("first policy commits");
    store
        .apply_relay_state(StoredRelayStateMutation::Configure(first.clone()))
        .expect("equal operation replay is a no-op");
    let mut collision = first;
    collision.request_digest = CommandDigest::from_bytes([9; 32]);
    assert_eq!(
        store
            .apply_relay_state(StoredRelayStateMutation::Configure(collision))
            .expect_err("unequal operation reuse conflicts")
            .class(),
        StoreErrorClass::RelayStateConflict
    );
    store
        .apply_relay_state(StoredRelayStateMutation::Configure(policy_change(
            2,
            2,
            true,
            RelayAccess::ReadWrite,
        )))
        .expect("equal desired state under a new operation reuses generation");
    assert_eq!(
        store.load_relay_state(1).expect("state loads").policies[0].generation,
        1
    );
    store
        .apply_relay_state(StoredRelayStateMutation::Configure(policy_change(
            3,
            3,
            false,
            RelayAccess::Read,
        )))
        .expect("changed desired policy advances generation");
    let policy = &store
        .load_relay_state(1)
        .expect("changed state loads")
        .policies[0];
    assert_eq!(policy.url, RELAY);
    assert_eq!(policy.access, RelayAccess::Read);
    assert!(!policy.enabled);
    assert_eq!(policy.generation, 2);
    store.close().expect("store closes");

    let reopened = open_store(&database);
    assert_eq!(
        reopened
            .load_relay_state(1)
            .expect("policy survives restart")
            .policies[0]
            .generation,
        2
    );
    for invalid in [0, MAX_RELAY_STATE_QUERY_ITEMS + 1] {
        assert_eq!(
            reopened
                .load_relay_state(invalid)
                .expect_err("unbounded state query rejects")
                .class(),
            StoreErrorClass::InvalidOperationalRequest
        );
    }
}

#[test]
fn valid_shaped_policy_corruption_fails_closed_on_load() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    store
        .apply_relay_state(StoredRelayStateMutation::Configure(policy_change(
            1,
            1,
            true,
            RelayAccess::Read,
        )))
        .expect("policy commits");
    store.close().expect("store closes");
    let connection = Connection::open(&database).expect("database opens for corruption fixture");
    connection
        .execute(
            "UPDATE relay_policies SET url = 'ws:///missing-authority'",
            [],
        )
        .expect("constraint-valid invalid URL stores");
    drop(connection);
    let reopened = open_store(&database);
    assert_eq!(
        reopened
            .load_relay_state(1)
            .expect_err("invalid stored URL rejects")
            .class(),
        StoreErrorClass::OperationalStateCorrupt
    );
}

#[test]
fn prepared_lineage_attempt_and_cursor_transitions_are_atomic_and_monotonic() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let prepared = create_outbox_and_prepared(&store, b"exact-wrapper".to_vec());
    store
        .apply_relay_state(StoredRelayStateMutation::Configure(policy_change(
            1,
            1,
            true,
            RelayAccess::ReadWrite,
        )))
        .expect("policy commits");
    store
        .apply_relay_state(StoredRelayStateMutation::Prepare(prepared.clone()))
        .expect("prepared bytes and uniqueness claims commit");
    store
        .apply_relay_state(StoredRelayStateMutation::Prepare(prepared.clone()))
        .expect("exact prepared replay is a no-op");
    let mut changed = prepared.clone();
    changed.one_use_public_key = [8; 32];
    assert_eq!(
        store
            .apply_relay_state(StoredRelayStateMutation::Prepare(changed))
            .expect_err("lineage cannot change after preparation")
            .class(),
        StoreErrorClass::IdentityCollision
    );

    let uncertain = StoredRelayAttempt {
        url: RELAY.to_owned(),
        wrapper_id: prepared.wrapper_id,
        attempts: 1,
        disposition: StoredAttemptDisposition::Uncertain,
        failure: None,
        last_attempt_millis: 10,
        retry_at_millis: Some(20),
    };
    store
        .apply_relay_state(StoredRelayStateMutation::Attempt(uncertain.clone()))
        .expect("uncertain attempt commits before response");
    assert_eq!(
        store
            .apply_relay_state(StoredRelayStateMutation::Attempt(StoredRelayAttempt {
                failure: Some(StoredRelayAttemptFailure::Permanent),
                ..uncertain.clone()
            }))
            .expect_err("non-rejected attempt cannot retain a rejection class")
            .class(),
        StoreErrorClass::InvalidOperationalRequest
    );
    let accepted = StoredRelayAttempt {
        disposition: StoredAttemptDisposition::Accepted,
        retry_at_millis: None,
        ..uncertain
    };
    store
        .apply_relay_state(StoredRelayStateMutation::Attempt(accepted.clone()))
        .expect("same-attempt acceptance recovers a lost response");
    let regression = StoredRelayAttempt {
        attempts: 2,
        disposition: StoredAttemptDisposition::Rejected,
        failure: Some(StoredRelayAttemptFailure::Permanent),
        last_attempt_millis: 11,
        retry_at_millis: Some(30),
        ..accepted.clone()
    };
    assert_eq!(
        store
            .apply_relay_state(StoredRelayStateMutation::Attempt(regression))
            .expect_err("accepted state is absorbing")
            .class(),
        StoreErrorClass::RelayStateConflict
    );

    assert_cursor_transitions_are_monotonic(&store);

    let state = store.load_relay_state(16).expect("relay state loads");
    assert_eq!(state.prepared, vec![prepared]);
    assert_eq!(state.attempts, vec![accepted]);
    assert!(state.cursors[0].exhausted);
}

fn assert_cursor_transitions_are_monotonic(store: &Store) {
    let cursor = StoredCatchupCursor {
        url: RELAY.to_owned(),
        generation: 1,
        scan_started_at_millis: 1_000,
        covered_through_millis: None,
        oldest_created_at: Some(100),
        oldest_wrapper_id: Some([2; 32]),
        exhausted: false,
    };
    store
        .apply_relay_state(StoredRelayStateMutation::Cursor(cursor.clone()))
        .expect("initial cursor commits");
    let older = StoredCatchupCursor {
        oldest_created_at: Some(90),
        oldest_wrapper_id: Some([3; 32]),
        ..cursor
    };
    store
        .apply_relay_state(StoredRelayStateMutation::Cursor(older.clone()))
        .expect("backward cursor advances");
    let newer = StoredCatchupCursor {
        oldest_created_at: Some(95),
        ..older.clone()
    };
    assert_eq!(
        store
            .apply_relay_state(StoredRelayStateMutation::Cursor(newer))
            .expect_err("cursor cannot regress toward the live edge")
            .class(),
        StoreErrorClass::RelayStateConflict
    );
    store
        .apply_relay_state(StoredRelayStateMutation::Cursor(StoredCatchupCursor {
            covered_through_millis: Some(1_000),
            exhausted: true,
            ..older.clone()
        }))
        .expect("same boundary may become exhausted");
    let refresh = StoredCatchupCursor {
        scan_started_at_millis: 2_000,
        covered_through_millis: Some(1_000),
        oldest_created_at: None,
        oldest_wrapper_id: None,
        exhausted: false,
        ..older
    };
    store
        .apply_relay_state(StoredRelayStateMutation::Cursor(refresh.clone()))
        .expect("completed coverage may start a newer overlap scan");
    store
        .apply_relay_state(StoredRelayStateMutation::Cursor(StoredCatchupCursor {
            covered_through_millis: Some(2_000),
            exhausted: true,
            ..refresh
        }))
        .expect("new overlap scan may become covered");
}

#[test]
fn rejection_classes_and_independent_attempt_pages_round_trip() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let prepared = create_outbox_and_prepared(&store, b"page-wrapper".to_vec());
    for (operation, url) in [(1, RELAY), (2, "wss://z.example")] {
        let mut policy = policy_change(operation, operation, true, RelayAccess::Write);
        policy.desired.url = url.to_owned();
        store
            .apply_relay_state(StoredRelayStateMutation::Configure(policy))
            .expect("relay policy commits");
    }
    store
        .apply_relay_state(StoredRelayStateMutation::Prepare(prepared.clone()))
        .expect("prepared lineage commits");
    let uncertain = StoredRelayAttempt {
        url: RELAY.to_owned(),
        wrapper_id: prepared.wrapper_id,
        attempts: 1,
        disposition: StoredAttemptDisposition::Uncertain,
        failure: None,
        last_attempt_millis: 10,
        retry_at_millis: Some(20),
    };
    let rejected = StoredRelayAttempt {
        url: "wss://z.example".to_owned(),
        wrapper_id: prepared.wrapper_id,
        attempts: 1,
        disposition: StoredAttemptDisposition::Rejected,
        failure: Some(StoredRelayAttemptFailure::RateLimited),
        last_attempt_millis: 12,
        retry_at_millis: Some(24),
    };
    for attempt in [uncertain.clone(), rejected.clone()] {
        store
            .apply_relay_state(StoredRelayStateMutation::Attempt(attempt))
            .expect("attempt commits");
    }

    let mut query = StoredRelayStateQuery::first(1);
    let mut paged_attempts = Vec::new();
    loop {
        let page = store
            .load_relay_state_page(query)
            .expect("independent keyset page loads");
        paged_attempts.extend(page.state.attempts);
        let Some(next) = page.next else {
            break;
        };
        query = next;
    }
    assert_eq!(paged_attempts, vec![uncertain, rejected]);
}

#[test]
fn inbound_claims_and_fifo_staging_transition_atomically() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let claim = StoredInboundClaim {
        wrapper_id: [1; 32],
        origin_installation_id: [2; 32],
        canonical_event_id: [3; 32],
        canonical_sha256: [4; 32],
        received_at_millis: 5,
    };
    store
        .apply_relay_state(StoredRelayStateMutation::ClaimInbound {
            claim: claim.clone(),
            remove_staged: None,
        })
        .expect("dual identity claim commits");
    store
        .apply_relay_state(StoredRelayStateMutation::ClaimInbound {
            claim: StoredInboundClaim {
                received_at_millis: 99,
                ..claim.clone()
            },
            remove_staged: None,
        })
        .expect("equal transport identity ignores later observation time");
    store
        .apply_relay_state(StoredRelayStateMutation::ClaimInbound {
            claim: claim.clone(),
            remove_staged: None,
        })
        .expect("equal dual identity replay is a no-op");
    let mut outer_collision = claim.clone();
    outer_collision.canonical_sha256 = [9; 32];
    assert_eq!(
        store
            .apply_relay_state(StoredRelayStateMutation::ClaimInbound {
                claim: outer_collision,
                remove_staged: None,
            })
            .expect_err("outer identity cannot map to unequal evidence")
            .class(),
        StoreErrorClass::IdentityCollision
    );
    let logical_collision = StoredInboundClaim {
        wrapper_id: [8; 32],
        ..claim
    };
    assert_eq!(
        store
            .apply_relay_state(StoredRelayStateMutation::ClaimInbound {
                claim: logical_collision,
                remove_staged: None,
            })
            .expect_err("logical identity cannot map to a second outer identity")
            .class(),
        StoreErrorClass::IdentityCollision
    );

    let later = staged(b"later", 20, 0);
    let earlier = staged(b"earlier", 10, 0);
    store
        .apply_relay_state(StoredRelayStateMutation::Stage(later.clone()))
        .expect("later input stages");
    store
        .apply_relay_state(StoredRelayStateMutation::Stage(earlier.clone()))
        .expect("earlier input stages");
    assert_eq!(
        store.load_relay_state(16).expect("staging loads").staged,
        vec![earlier.clone(), later.clone()]
    );
    store
        .apply_relay_state(StoredRelayStateMutation::Stage(StoredStagedInput {
            attempts: 1,
            retry_at_millis: 30,
            ..earlier.clone()
        }))
        .expect("retry metadata advances without changing exact bytes");
    store
        .apply_relay_state(StoredRelayStateMutation::ClaimInbound {
            claim: StoredInboundClaim {
                wrapper_id: [10; 32],
                origin_installation_id: [11; 32],
                canonical_event_id: [12; 32],
                canonical_sha256: [13; 32],
                received_at_millis: 30,
            },
            remove_staged: Some(earlier.wrapper_sha256),
        })
        .expect("successful claim atomically removes staging");
    assert_eq!(
        store.load_relay_state(16).expect("staging reloads").staged,
        vec![later.clone()]
    );
}

#[test]
fn quarantine_removes_staging_and_evicts_by_time_then_digest() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let staged = staged(b"permanent", 20, 0);
    store
        .apply_relay_state(StoredRelayStateMutation::Stage(staged.clone()))
        .expect("permanent input first stages");
    assert_eq!(
        store
            .apply_relay_state(StoredRelayStateMutation::Quarantine {
                evidence: StoredQuarantineEvidence {
                    wrapper_sha256: [0xff; 32],
                    wrapper_id: None,
                    failure_code: 1,
                    received_at_millis: 0,
                    byte_len: 1,
                    raw_sample: vec![0],
                },
                remove_staged: Some(staged.wrapper_sha256),
            })
            .expect_err("quarantine cannot remove unrelated staging")
            .class(),
        StoreErrorClass::InvalidOperationalRequest
    );
    for index in 0..=MAX_RELAY_QUARANTINE_ITEMS {
        let bytes = index.to_be_bytes();
        let digest = if index == 0 {
            staged.wrapper_sha256
        } else {
            Sha256::digest(bytes).into()
        };
        store
            .apply_relay_state(StoredRelayStateMutation::Quarantine {
                evidence: StoredQuarantineEvidence {
                    wrapper_sha256: digest,
                    wrapper_id: None,
                    failure_code: 1,
                    received_at_millis: u64::try_from(index).expect("index fits"),
                    byte_len: MAX_RELAY_QUARANTINE_SAMPLE_BYTES,
                    raw_sample: vec![
                        u8::try_from(index % 251).expect("modulo fits");
                        MAX_RELAY_QUARANTINE_SAMPLE_BYTES
                    ],
                },
                remove_staged: (index == 0).then_some(staged.wrapper_sha256),
            })
            .expect("bounded quarantine insert succeeds");
    }
    let state = store
        .load_relay_state(MAX_RELAY_STATE_QUERY_ITEMS)
        .expect("quarantine loads");
    assert!(state.staged.is_empty());
    let quarantine = state.quarantine;
    assert_eq!(quarantine.len(), MAX_RELAY_QUARANTINE_ITEMS);
    assert_eq!(quarantine[0].received_at_millis, 1);
    assert_eq!(
        quarantine
            .iter()
            .map(|row| row.raw_sample.len())
            .sum::<usize>(),
        MAX_RELAY_QUARANTINE_ITEMS * MAX_RELAY_QUARANTINE_SAMPLE_BYTES
    );
}

#[test]
fn staging_enforces_inclusive_item_and_total_byte_bounds_without_eviction() {
    let item_directory = TestDirectory::new();
    let item_store = open_store(&item_directory.database_path());
    for index in 0..MAX_RELAY_STAGING_ITEMS {
        let bytes = index.to_be_bytes().to_vec();
        item_store
            .apply_relay_state(StoredRelayStateMutation::Stage(staged_bytes(
                bytes,
                u64::try_from(index).expect("index fits"),
            )))
            .expect("inclusive item bound accepts");
    }
    assert_eq!(
        item_store
            .apply_relay_state(StoredRelayStateMutation::Stage(staged(
                b"overflow",
                9_999,
                0
            )))
            .expect_err("item overflow backpressures")
            .class(),
        StoreErrorClass::RelayStagingFull
    );

    let byte_directory = TestDirectory::new();
    let byte_store = open_store(&byte_directory.database_path());
    let rows = MAX_RELAY_STAGING_BYTES / MAX_RELAY_WRAPPER_BYTES;
    for index in 0..rows {
        let mut bytes = vec![0; MAX_RELAY_WRAPPER_BYTES];
        bytes[..8].copy_from_slice(&index.to_be_bytes());
        byte_store
            .apply_relay_state(StoredRelayStateMutation::Stage(staged_bytes(
                bytes,
                u64::try_from(index).expect("index fits"),
            )))
            .expect("inclusive byte bound accepts");
    }
    assert_eq!(rows * MAX_RELAY_WRAPPER_BYTES, MAX_RELAY_STAGING_BYTES);
    assert_eq!(
        byte_store
            .apply_relay_state(StoredRelayStateMutation::Stage(staged(b"x", 9_999, 0)))
            .expect_err("byte overflow backpressures")
            .class(),
        StoreErrorClass::RelayStagingFull
    );
}

#[test]
fn relay_operational_state_survives_projection_repair_and_corruption_rejects() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let prepared = create_outbox_and_prepared(&store, vec![7; MAX_RELAY_WRAPPER_BYTES]);
    store
        .apply_relay_state(StoredRelayStateMutation::Prepare(prepared.clone()))
        .expect("maximum exact wrapper commits");
    let before = store.load_relay_state(16).expect("state loads");
    store
        .repair(authority_policy())
        .expect("projection repair succeeds");
    assert_eq!(
        store.load_relay_state(16).expect("state survives repair"),
        before
    );
    store.close().expect("store closes");

    let connection = Connection::open(&database).expect("database opens for corruption fixture");
    connection
        .execute(
            "UPDATE prepared_relay_outbox SET wrapper_sha256 = ?1 WHERE wrapper_id = ?2",
            rusqlite::params![[0xff_u8; 32].as_slice(), prepared.wrapper_id.as_slice()],
        )
        .expect("valid-shaped unequal digest stores");
    drop(connection);
    let reopened = open_store(&database);
    assert_eq!(
        reopened
            .load_relay_state(16)
            .expect_err("corrupt prepared metadata rejects")
            .class(),
        StoreErrorClass::OperationalStateCorrupt
    );
}

fn policy_change(
    operation: u8,
    digest: u8,
    enabled: bool,
    access: RelayAccess,
) -> StoredRelayPolicyChange {
    StoredRelayPolicyChange {
        operation_id: OperationId::from_bytes([operation; 32]),
        request_digest: CommandDigest::from_bytes([digest; 32]),
        desired: StoredDesiredRelayPolicy {
            url: RELAY.to_owned(),
            access,
            authentication: RelayAuthentication::Required,
            enabled,
        },
    }
}

fn create_outbox_and_prepared(store: &Store, exact_wire: Vec<u8>) -> StoredPreparedOutbound {
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    store
        .ingest_verified(root, authority_policy())
        .expect("root ingests");
    let account = verified_account(root_id);
    let account_id = account.verified_event().event_id();
    store
        .ingest_verified(account, authority_policy())
        .expect("account ingests");
    let grant = verified_device_grant(account_id);
    let grant_id = grant.verified_event().event_id();
    store
        .ingest_verified(grant, authority_policy())
        .expect("grant ingests");
    store
        .ingest_verified(verified_device_acceptance(grant_id), authority_policy())
        .expect("grant acceptance ingests");
    let project = verified_project(root_id, account_id);
    store
        .ingest_verified(project, authority_policy())
        .expect("remote project ingests");
    let intent = store
        .load_outbox_intents(1)
        .expect("outbox loads")
        .pop()
        .expect("outbox has one intent");
    let wrapper_sha256 = Sha256::digest(&exact_wire).into();
    StoredPreparedOutbound {
        fact_id: intent.fact_id(),
        recipient: intent.recipient(),
        wrapper_id: [5; 32],
        one_use_public_key: [6; 32],
        recipient_public_key: [7; 32],
        canonical_event_id: *intent.fact_id().as_bytes(),
        canonical_sha256: Sha256::digest(intent.exact_canonical_bytes()).into(),
        wrapper_sha256,
        seal_created_at: u64::MAX,
        gift_wrap_created_at: u64::MAX - 1,
        exact_wire,
    }
}

fn staged(bytes: &[u8], received_at: u64, attempts: u32) -> StoredStagedInput {
    let mut input = staged_bytes(bytes.to_vec(), received_at);
    input.attempts = attempts;
    input
}

fn staged_bytes(bytes: Vec<u8>, received_at: u64) -> StoredStagedInput {
    StoredStagedInput {
        wrapper_sha256: Sha256::digest(&bytes).into(),
        exact_outer: bytes,
        first_received_millis: received_at,
        attempts: 0,
        retry_at_millis: received_at,
    }
}
