//! Supervisor boundary contracts for bounded buffering and secret launch state.

#![allow(clippy::expect_used)]

use std::num::{NonZeroU64, NonZeroUsize};

use hq_domain::{
    ActivityKind, ActivityStatus, CommandDigest, ContentText, MessageId, OperationId, ShortText,
};
use hq_harness::{
    HarnessActivity, HarnessBufferPush, HarnessBufferedEvent, HarnessEnvironment,
    HarnessEventBuffer, HarnessOutput, HarnessOutputKind, HarnessSnapshotKey,
};

#[test]
fn full_buffer_replaces_an_existing_snapshot_at_the_tail_without_crossing_durable_work() {
    let mut buffer = HarnessEventBuffer::new(NonZeroUsize::new(3).expect("capacity is nonzero"));
    let key = snapshot_key("plan");
    assert_eq!(
        buffer.push(durable(1, "first")),
        Ok(HarnessBufferPush::Accepted)
    );
    assert_eq!(
        buffer.push(snapshot(key.clone(), 1, "old plan")),
        Ok(HarnessBufferPush::Accepted)
    );
    assert_eq!(
        buffer.push(durable(2, "second")),
        Ok(HarnessBufferPush::Accepted)
    );

    assert_eq!(
        buffer.push(snapshot(key, 2, "new plan")),
        Ok(HarnessBufferPush::Replaced)
    );
    let rejected = buffer
        .push(snapshot(snapshot_key("other"), 1, "other"))
        .expect_err("a new key backpressures at capacity");
    assert!(matches!(*rejected, HarnessBufferedEvent::Snapshot { .. }));

    assert_eq!(buffer.pop(), Some(durable(1, "first")));
    assert_eq!(buffer.pop(), Some(durable(2, "second")));
    assert_eq!(
        buffer.pop(),
        Some(snapshot(snapshot_key("plan"), 2, "new plan"))
    );
    assert!(buffer.is_empty());
}

#[test]
fn copied_environment_is_independent_bounded_and_redacted() {
    let mut source = b"super-secret-token".to_vec();
    let environment = HarnessEnvironment::copy_from([("HQ_TEST_TOKEN", source.as_slice())])
        .expect("bounded environment copies");
    source.fill(b'x');

    let mut observed = Vec::new();
    environment.visit(|name, value| observed.push((name.to_owned(), value.to_vec())));
    assert_eq!(
        observed,
        vec![("HQ_TEST_TOKEN".to_owned(), b"super-secret-token".to_vec())]
    );
    let debug = format!("{environment:?}");
    assert!(debug.contains("entry_count"));
    assert!(!debug.contains("super-secret-token"));
}

fn durable(identity: u8, body: &str) -> HarnessBufferedEvent {
    HarnessBufferedEvent::Output {
        event_id: MessageId::from_bytes([identity; 32]),
        digest: CommandDigest::from_bytes([identity.saturating_add(20); 32]),
        output: HarnessOutput {
            output_id: MessageId::from_bytes([identity.saturating_add(40); 32]),
            operation_id: OperationId::from_bytes([7; 32]),
            kind: HarnessOutputKind::Update,
            status: ActivityStatus::Running,
            body: ContentText::new(body).expect("body validates"),
        },
    }
}

fn snapshot(key: HarnessSnapshotKey, sequence: u64, body: &str) -> HarnessBufferedEvent {
    HarnessBufferedEvent::Snapshot {
        event_id: MessageId::from_bytes([u8::try_from(sequence).expect("small sequence"); 32]),
        digest: CommandDigest::from_bytes(
            [u8::try_from(sequence)
                .expect("small sequence")
                .saturating_add(30); 32],
        ),
        key,
        activity: HarnessActivity {
            operation_id: OperationId::from_bytes([7; 32]),
            item: None,
            kind: ActivityKind::Plan,
            logical_key: ShortText::new("plan").expect("key validates"),
            runtime: ShortText::new("scripted").expect("runtime validates"),
            sequence: NonZeroU64::new(sequence).expect("sequence is positive"),
            status: ActivityStatus::Running,
            content: ContentText::new(body).expect("content validates"),
            truncated: false,
            completed: None,
        },
    }
}

fn snapshot_key(value: &str) -> HarnessSnapshotKey {
    HarnessSnapshotKey {
        operation_id: OperationId::from_bytes([7; 32]),
        item: None,
        logical_key: ShortText::new(value).expect("key validates"),
    }
}
