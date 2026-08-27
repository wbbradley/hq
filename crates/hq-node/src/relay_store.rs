//! Node-only mapping between relay consumer records and storage-owned records.

use std::num::NonZeroU64;

use hq_relay::{
    AttemptDisposition, CatchupCursor, DesiredRelayPolicy, DurableEnvelope, FailureClass,
    OutboundIntent, OutboxKey, PreparedEnvelope, PreparedEnvelopeMetadata, PreparedOutbound,
    QuarantineEvidence, RelayAttempt, RelayPolicy, RelayPortError, RelayStateMutation,
    RelayStatePort, RelayStateSnapshot, RelayUrl, StagedInput,
};
use hq_store::{
    RelayStateHandle, Store, StoreError, StoreErrorClass, StoredAttemptDisposition,
    StoredCatchupCursor, StoredDesiredRelayPolicy, StoredInboundClaim, StoredPreparedOutbound,
    StoredQuarantineEvidence, StoredRelayAttempt, StoredRelayPolicy, StoredRelayPolicyChange,
    StoredRelayStateMutation, StoredRelayStateSnapshot, StoredStagedInput,
};

/// Relay durable-state capability backed by the sole store actor.
#[derive(Clone, Debug)]
pub struct RelayStoreAdapter {
    store: RelayStateHandle,
}

impl RelayStoreAdapter {
    /// Creates an owned relay capability without exposing SQLite or worker shutdown ownership.
    pub fn new(store: &Store) -> Self {
        Self {
            store: store.relay_state_handle(),
        }
    }
}

impl RelayStatePort for RelayStoreAdapter {
    fn load_state(&self, limit: usize) -> Result<RelayStateSnapshot, RelayPortError> {
        self.store
            .load(limit)
            .map_err(map_store_error)
            .and_then(map_snapshot)
    }

    fn apply(&self, mutation: RelayStateMutation) -> Result<(), RelayPortError> {
        self.store
            .apply(map_mutation(mutation))
            .map_err(map_store_error)
    }
}

fn map_mutation(mutation: RelayStateMutation) -> StoredRelayStateMutation {
    match mutation {
        RelayStateMutation::Configure(change) => {
            StoredRelayStateMutation::Configure(StoredRelayPolicyChange {
                operation_id: change.operation_id,
                request_digest: change.request_digest,
                desired: store_desired(change.desired),
            })
        }
        RelayStateMutation::Prepare(prepared) => {
            let metadata = prepared.envelope.metadata;
            StoredRelayStateMutation::Prepare(StoredPreparedOutbound {
                fact_id: prepared.key.fact_id,
                recipient: prepared.key.recipient,
                wrapper_id: metadata.wrapper_id,
                one_use_public_key: metadata.one_use_public_key,
                recipient_public_key: metadata.recipient_public_key,
                canonical_event_id: metadata.canonical_event_id,
                canonical_sha256: metadata.canonical_sha256,
                wrapper_sha256: metadata.wrapper_sha256,
                seal_created_at: metadata.seal_created_at,
                gift_wrap_created_at: metadata.gift_wrap_created_at,
                exact_wire: prepared.envelope.exact_wire,
            })
        }
        RelayStateMutation::Attempt(attempt) => {
            StoredRelayStateMutation::Attempt(StoredRelayAttempt {
                url: attempt.url.into_string(),
                wrapper_id: attempt.wrapper_id,
                attempts: attempt.attempts,
                disposition: store_disposition(attempt.disposition),
                last_attempt_millis: attempt.last_attempt_millis,
                retry_at_millis: attempt.retry_at_millis,
            })
        }
        RelayStateMutation::Cursor(cursor) => {
            StoredRelayStateMutation::Cursor(StoredCatchupCursor {
                url: cursor.url.into_string(),
                generation: cursor.generation.get(),
                oldest_created_at: cursor.oldest_created_at,
                oldest_wrapper_id: cursor.oldest_wrapper_id,
                exhausted: cursor.exhausted,
            })
        }
        RelayStateMutation::ClaimInbound {
            claim,
            remove_staged,
        } => StoredRelayStateMutation::ClaimInbound {
            claim: StoredInboundClaim {
                wrapper_id: claim.wrapper_id,
                origin_installation_id: claim.logical_id.origin_installation_id,
                canonical_event_id: claim.logical_id.canonical_event_id,
                canonical_sha256: claim.canonical_sha256,
                received_at_millis: claim.received_at_millis,
            },
            remove_staged,
        },
        RelayStateMutation::Stage(input) => StoredRelayStateMutation::Stage(StoredStagedInput {
            wrapper_sha256: input.wrapper_sha256,
            exact_outer: input.exact_outer,
            first_received_millis: input.first_received_millis,
            attempts: input.attempts,
            retry_at_millis: input.retry_at_millis,
        }),
        RelayStateMutation::Quarantine {
            evidence,
            remove_staged,
        } => StoredRelayStateMutation::Quarantine {
            evidence: StoredQuarantineEvidence {
                wrapper_sha256: evidence.wrapper_sha256,
                wrapper_id: evidence.wrapper_id,
                failure_code: encode_failure(evidence.failure),
                received_at_millis: evidence.received_at_millis,
                byte_len: evidence.byte_len,
                raw_sample: evidence.raw_sample,
            },
            remove_staged,
        },
    }
}

fn map_snapshot(stored: StoredRelayStateSnapshot) -> Result<RelayStateSnapshot, RelayPortError> {
    Ok(RelayStateSnapshot {
        policies: stored
            .policies
            .into_iter()
            .map(map_policy)
            .collect::<Result<_, _>>()?,
        outbound: stored
            .outbound
            .into_iter()
            .map(|intent| OutboundIntent {
                key: OutboxKey {
                    fact_id: intent.fact_id(),
                    recipient: intent.recipient(),
                },
                exact_canonical_bytes: intent.exact_canonical_bytes().to_vec(),
                revision: intent.revision(),
            })
            .collect(),
        prepared: stored
            .prepared
            .into_iter()
            .map(map_prepared)
            .collect::<Result<_, _>>()?,
        attempts: stored
            .attempts
            .into_iter()
            .map(map_attempt)
            .collect::<Result<_, _>>()?,
        cursors: stored
            .cursors
            .into_iter()
            .map(map_cursor)
            .collect::<Result<_, _>>()?,
        staged: stored.staged.into_iter().map(map_staged).collect(),
        quarantine: stored
            .quarantine
            .into_iter()
            .map(map_quarantine)
            .collect::<Result<_, _>>()?,
    })
}

fn store_desired(policy: DesiredRelayPolicy) -> StoredDesiredRelayPolicy {
    StoredDesiredRelayPolicy {
        url: policy.url.into_string(),
        access: policy.access,
        authentication: policy.authentication,
        enabled: policy.enabled,
    }
}

fn map_policy(policy: StoredRelayPolicy) -> Result<RelayPolicy, RelayPortError> {
    Ok(RelayPolicy {
        url: relay_url(policy.url)?,
        access: policy.access,
        authentication: policy.authentication,
        enabled: policy.enabled,
        generation: NonZeroU64::new(policy.generation).ok_or(RelayPortError::Corrupt)?,
    })
}

fn map_prepared(stored: StoredPreparedOutbound) -> Result<PreparedOutbound, RelayPortError> {
    let key = OutboxKey {
        fact_id: stored.fact_id,
        recipient: stored.recipient,
    };
    let envelope = DurableEnvelope {
        metadata: PreparedEnvelopeMetadata {
            wrapper_id: stored.wrapper_id,
            one_use_public_key: stored.one_use_public_key,
            recipient_public_key: stored.recipient_public_key,
            canonical_event_id: stored.canonical_event_id,
            canonical_sha256: stored.canonical_sha256,
            wrapper_sha256: stored.wrapper_sha256,
            seal_created_at: stored.seal_created_at,
            gift_wrap_created_at: stored.gift_wrap_created_at,
            byte_len: stored.exact_wire.len(),
        },
        exact_wire: stored.exact_wire,
    };
    let envelope = PreparedEnvelope::restore(envelope)
        .map_err(|_| RelayPortError::Corrupt)?
        .into_durable();
    Ok(PreparedOutbound { key, envelope })
}

fn map_attempt(stored: StoredRelayAttempt) -> Result<RelayAttempt, RelayPortError> {
    Ok(RelayAttempt {
        url: relay_url(stored.url)?,
        wrapper_id: stored.wrapper_id,
        attempts: stored.attempts,
        disposition: map_disposition(stored.disposition),
        last_attempt_millis: stored.last_attempt_millis,
        retry_at_millis: stored.retry_at_millis,
    })
}

fn map_cursor(stored: StoredCatchupCursor) -> Result<CatchupCursor, RelayPortError> {
    Ok(CatchupCursor {
        url: relay_url(stored.url)?,
        generation: NonZeroU64::new(stored.generation).ok_or(RelayPortError::Corrupt)?,
        oldest_created_at: stored.oldest_created_at,
        oldest_wrapper_id: stored.oldest_wrapper_id,
        exhausted: stored.exhausted,
    })
}

fn map_staged(stored: StoredStagedInput) -> StagedInput {
    StagedInput {
        wrapper_sha256: stored.wrapper_sha256,
        exact_outer: stored.exact_outer,
        first_received_millis: stored.first_received_millis,
        attempts: stored.attempts,
        retry_at_millis: stored.retry_at_millis,
    }
}

fn map_quarantine(stored: StoredQuarantineEvidence) -> Result<QuarantineEvidence, RelayPortError> {
    Ok(QuarantineEvidence {
        wrapper_sha256: stored.wrapper_sha256,
        wrapper_id: stored.wrapper_id,
        failure: decode_failure(stored.failure_code)?,
        received_at_millis: stored.received_at_millis,
        byte_len: stored.byte_len,
        raw_sample: stored.raw_sample,
    })
}

const fn store_disposition(value: AttemptDisposition) -> StoredAttemptDisposition {
    match value {
        AttemptDisposition::Uncertain => StoredAttemptDisposition::Uncertain,
        AttemptDisposition::Rejected => StoredAttemptDisposition::Rejected,
        AttemptDisposition::Accepted => StoredAttemptDisposition::Accepted,
    }
}

const fn map_disposition(value: StoredAttemptDisposition) -> AttemptDisposition {
    match value {
        StoredAttemptDisposition::Uncertain => AttemptDisposition::Uncertain,
        StoredAttemptDisposition::Rejected => AttemptDisposition::Rejected,
        StoredAttemptDisposition::Accepted => AttemptDisposition::Accepted,
    }
}

const fn encode_failure(value: FailureClass) -> u16 {
    match value {
        FailureClass::Size => 1,
        FailureClass::MalformedJson => 2,
        FailureClass::EventIdentity => 3,
        FailureClass::Signature => 4,
        FailureClass::InvalidPublicKey => 5,
        FailureClass::LayerShape => 6,
        FailureClass::Recipient => 7,
        FailureClass::UnsupportedEncryption => 8,
        FailureClass::MalformedEncryption => 9,
        FailureClass::Mac => 10,
        FailureClass::Padding => 11,
        FailureClass::EnvelopeVersion => 12,
        FailureClass::Canonical => 13,
        FailureClass::IdentityAgreement => 14,
        FailureClass::OneUseKeyReuse => 15,
        FailureClass::AuthenticationInput => 16,
        FailureClass::Cryptography => 17,
    }
}

fn decode_failure(value: u16) -> Result<FailureClass, RelayPortError> {
    match value {
        1 => Ok(FailureClass::Size),
        2 => Ok(FailureClass::MalformedJson),
        3 => Ok(FailureClass::EventIdentity),
        4 => Ok(FailureClass::Signature),
        5 => Ok(FailureClass::InvalidPublicKey),
        6 => Ok(FailureClass::LayerShape),
        7 => Ok(FailureClass::Recipient),
        8 => Ok(FailureClass::UnsupportedEncryption),
        9 => Ok(FailureClass::MalformedEncryption),
        10 => Ok(FailureClass::Mac),
        11 => Ok(FailureClass::Padding),
        12 => Ok(FailureClass::EnvelopeVersion),
        13 => Ok(FailureClass::Canonical),
        14 => Ok(FailureClass::IdentityAgreement),
        15 => Ok(FailureClass::OneUseKeyReuse),
        16 => Ok(FailureClass::AuthenticationInput),
        17 => Ok(FailureClass::Cryptography),
        _ => Err(RelayPortError::Corrupt),
    }
}

fn relay_url(value: String) -> Result<RelayUrl, RelayPortError> {
    RelayUrl::new(value).map_err(|_| RelayPortError::Corrupt)
}

const fn map_store_error(error: StoreError) -> RelayPortError {
    match error.class() {
        StoreErrorClass::InvalidOperationalRequest => RelayPortError::InvalidInput,
        StoreErrorClass::IdentityCollision
        | StoreErrorClass::MutationConflict
        | StoreErrorClass::RelayStateConflict => RelayPortError::Conflict,
        StoreErrorClass::CorruptDatabase
        | StoreErrorClass::InvalidEvidence
        | StoreErrorClass::OperationalStateCorrupt
        | StoreErrorClass::RebuildableStateCorrupt => RelayPortError::Corrupt,
        StoreErrorClass::RelayStagingFull => RelayPortError::Backpressure,
        StoreErrorClass::InvalidPath
        | StoreErrorClass::SymbolicLink
        | StoreErrorClass::UnsafePermissions
        | StoreErrorClass::FileSystem
        | StoreErrorClass::IncompatibleSchema
        | StoreErrorClass::RevisionExhausted
        | StoreErrorClass::ActorClosed
        | StoreErrorClass::WorkerStopped
        | StoreErrorClass::DatabaseUnavailable
        | StoreErrorClass::ReductionFailed
        | StoreErrorClass::NotRepaired => RelayPortError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{
        fs,
        num::NonZeroUsize,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use hq_application::{RelayAccess, RelayAuthentication};
    use hq_domain::{CommandDigest, FactId, InstallationId, OperationId};
    use hq_relay::RelayPolicyChange;

    use super::*;

    #[test]
    fn adapter_maps_public_policy_and_quarantine_records_without_leaking_store_types() {
        fn assert_owned_port<T: Send + Sync + 'static>() {}
        assert_owned_port::<RelayStoreAdapter>();
        let directory = TestDirectory::new();
        let store = Store::open(
            directory.path.join("state").join("hq.sqlite3"),
            NonZeroUsize::new(4).expect("capacity is positive"),
        )
        .expect("store opens");
        let adapter = RelayStoreAdapter::new(&store);
        let change = RelayStateMutation::Configure(RelayPolicyChange {
            operation_id: OperationId::from_bytes([1; 32]),
            request_digest: CommandDigest::from_bytes([2; 32]),
            desired: DesiredRelayPolicy {
                url: RelayUrl::new("wss://relay.example".to_owned()).expect("URL validates"),
                access: RelayAccess::ReadWrite,
                authentication: RelayAuthentication::Required,
                enabled: true,
            },
        });
        adapter
            .apply(change.clone())
            .expect("policy maps to storage");
        adapter.apply(change).expect("equal replay maps cleanly");
        let state = adapter.load_state(1).expect("storage maps back to relay");
        assert_eq!(state.policies[0].url.as_str(), "wss://relay.example");
        assert_eq!(state.policies[0].generation.get(), 1);
        assert_eq!(state.policies[0].access, RelayAccess::ReadWrite);

        adapter
            .apply(RelayStateMutation::Quarantine {
                evidence: QuarantineEvidence {
                    wrapper_sha256: [3; 32],
                    wrapper_id: Some([4; 32]),
                    failure: FailureClass::Mac,
                    received_at_millis: 5,
                    byte_len: 6,
                    raw_sample: vec![7; 6],
                },
                remove_staged: None,
            })
            .expect("quarantine maps to storage");
        assert_eq!(
            adapter
                .load_state(8)
                .expect("quarantine maps back")
                .quarantine[0]
                .failure,
            FailureClass::Mac
        );
        assert_eq!(adapter.load_state(0), Err(RelayPortError::InvalidInput));
        store.close().expect("store closes");
    }

    #[test]
    fn adapter_rejects_invalid_prepared_wire_and_unknown_failure_codes() {
        let stored = StoredPreparedOutbound {
            fact_id: FactId::from_bytes([1; 32]),
            recipient: InstallationId::from_bytes([2; 32]),
            wrapper_id: [3; 32],
            one_use_public_key: [4; 32],
            recipient_public_key: [5; 32],
            canonical_event_id: [1; 32],
            canonical_sha256: [6; 32],
            wrapper_sha256: [6; 32],
            seal_created_at: 7,
            gift_wrap_created_at: 8,
            exact_wire: b"not-an-event".to_vec(),
        };
        assert_eq!(map_prepared(stored), Err(RelayPortError::Corrupt));
        assert_eq!(decode_failure(0), Err(RelayPortError::Corrupt));
        assert_eq!(decode_failure(u16::MAX), Err(RelayPortError::Corrupt));
    }

    #[test]
    fn every_transport_failure_has_one_stable_storage_code() {
        let failures = [
            FailureClass::Size,
            FailureClass::MalformedJson,
            FailureClass::EventIdentity,
            FailureClass::Signature,
            FailureClass::InvalidPublicKey,
            FailureClass::LayerShape,
            FailureClass::Recipient,
            FailureClass::UnsupportedEncryption,
            FailureClass::MalformedEncryption,
            FailureClass::Mac,
            FailureClass::Padding,
            FailureClass::EnvelopeVersion,
            FailureClass::Canonical,
            FailureClass::IdentityAgreement,
            FailureClass::OneUseKeyReuse,
            FailureClass::AuthenticationInput,
            FailureClass::Cryptography,
        ];
        for failure in failures {
            assert_eq!(decode_failure(encode_failure(failure)), Ok(failure));
        }
    }

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "hq-relay-store-adapter-test-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("test directory creates");
            Self { path }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
