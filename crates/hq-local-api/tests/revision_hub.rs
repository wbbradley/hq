//! Bounded revision-fanout and subscription lifecycle contracts.

#![allow(clippy::expect_used)]

use std::{
    collections::BTreeSet,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
    thread,
};

use hq_application::{ObserveRevisions, SubscriptionRequest, SubscriptionTopic};
use hq_domain::{OperationId, Revision};
use hq_local_api::{FanoutDisposition, RevisionHub};

fn id(byte: u8) -> OperationId {
    OperationId::from_bytes([byte; 32])
}

fn request(byte: u8, topics: impl IntoIterator<Item = SubscriptionTopic>) -> SubscriptionRequest {
    SubscriptionRequest::new(id(byte), topics).expect("bounded subscription")
}

#[test]
fn pending_commits_are_hidden_until_activation_then_coalesced_once() {
    let hub = RevisionHub::new(2).expect("positive capacity");
    hub.register_subscription(&request(1, [SubscriptionTopic::Conversation]))
        .expect("register pending");

    assert_eq!(
        hub.publish(Revision::new(7), [SubscriptionTopic::Conversation], false),
        FanoutDisposition::Scheduled { subscribers: 1 }
    );
    assert!(hub.take(id(1)).expect("known subscription").is_none());

    assert_eq!(
        hub.publish(Revision::new(9), [SubscriptionTopic::Project], true),
        FanoutDisposition::Coalesced { subscribers: 1 }
    );
    hub.activate_subscription(id(1)).expect("activate pending");

    let notice = hub
        .take(id(1))
        .expect("known subscription")
        .expect("one pending notice");
    assert_eq!(notice.revision(), Revision::new(9));
    assert_eq!(
        notice.topics(),
        &BTreeSet::from([SubscriptionTopic::Conversation, SubscriptionTopic::Project,])
    );
    assert!(notice.full_snapshot());
    assert!(hub.take(id(1)).expect("known subscription").is_none());
}

#[test]
fn unrelated_topics_are_filtered_and_slow_readers_never_gain_a_queue() {
    let hub = RevisionHub::new(1).expect("positive capacity");
    hub.register_subscription(&request(1, [SubscriptionTopic::Agent]))
        .expect("register");
    hub.activate_subscription(id(1)).expect("activate");

    assert_eq!(
        hub.publish(Revision::new(1), [SubscriptionTopic::Conversation], false),
        FanoutDisposition::Ignored
    );
    for revision in 2..=10_000 {
        let _ = hub.publish(Revision::new(revision), [SubscriptionTopic::Agent], false);
    }

    let notice = hub
        .take(id(1))
        .expect("known subscription")
        .expect("one coalesced notice");
    assert_eq!(notice.revision(), Revision::new(10_000));
    assert_eq!(notice.topics(), &BTreeSet::from([SubscriptionTopic::Agent]));
    assert!(hub.take(id(1)).expect("known subscription").is_none());
}

#[test]
fn capacity_duplicate_activation_and_cancellation_are_closed() {
    let hub = RevisionHub::new(1).expect("positive capacity");
    let first = request(1, [SubscriptionTopic::All]);
    hub.register_subscription(&first).expect("first fits");
    assert!(hub.register_subscription(&first).is_err());
    assert!(
        hub.register_subscription(&request(2, [SubscriptionTopic::All]))
            .is_err()
    );
    hub.cancel_subscription(id(1)).expect("cancel pending");
    hub.cancel_subscription(id(1)).expect("idempotent cancel");
    assert!(hub.activate_subscription(id(1)).is_err());
    hub.register_subscription(&request(2, [SubscriptionTopic::All]))
        .expect("capacity released");
}

#[test]
fn concurrent_publish_cancel_and_poll_remain_bounded_and_deadlock_free() {
    let hub = Arc::new(RevisionHub::new(8).expect("positive capacity"));
    for byte in 1..=8 {
        hub.register_subscription(&request(byte, [SubscriptionTopic::All]))
            .expect("register");
        hub.activate_subscription(id(byte)).expect("activate");
    }

    let publisher = {
        let hub = Arc::clone(&hub);
        thread::spawn(move || {
            for revision in 1..=2_000 {
                let _ = hub.publish(
                    Revision::new(revision),
                    [SubscriptionTopic::Operations],
                    revision % 19 == 0,
                );
            }
        })
    };
    let consumer = {
        let hub = Arc::clone(&hub);
        thread::spawn(move || {
            for _ in 0..2_000 {
                for byte in 1..=4 {
                    let _ = hub.take(id(byte));
                }
            }
        })
    };
    let canceller = {
        let hub = Arc::clone(&hub);
        thread::spawn(move || {
            for byte in 5..=8 {
                hub.cancel_subscription(id(byte)).expect("cancel");
            }
        })
    };

    publisher.join().expect("publisher exits");
    consumer.join().expect("consumer exits");
    canceller.join().expect("canceller exits");
    assert_eq!(hub.len(), 4);
    assert!(hub.len() <= hub.capacity());
}

#[test]
fn wake_listener_retains_pre_wait_publication_and_coalesces_bursts() {
    let hub = RevisionHub::new(1).expect("positive capacity");
    let mut listener = hub.take_wake_listener().expect("listener transfers");
    hub.register_subscription(&request(1, [SubscriptionTopic::Conversation]))
        .expect("register");
    hub.activate_subscription(id(1)).expect("activate");

    let _ = hub.publish(Revision::new(1), [SubscriptionTopic::Conversation], false);
    let _ = hub.publish(Revision::new(2), [SubscriptionTopic::Conversation], false);

    assert!(listener.has_changed());
    assert!(!listener.has_changed());
    assert_eq!(
        hub.take(id(1)).expect("known").expect("notice").revision(),
        Revision::new(2)
    );
    let _ = hub.publish(Revision::new(3), [SubscriptionTopic::Conversation], false);
    assert!(listener.has_changed());
}

#[test]
fn activation_wakes_a_notice_published_while_registration_was_pending() {
    let hub = RevisionHub::new(1).expect("positive capacity");
    let mut listener = hub.take_wake_listener().expect("listener transfers");
    hub.register_subscription(&request(1, [SubscriptionTopic::Conversation]))
        .expect("register pending");

    let _ = hub.publish(Revision::new(1), [SubscriptionTopic::Conversation], false);
    assert!(!listener.has_changed());
    hub.activate_subscription(id(1)).expect("activate");
    assert!(listener.has_changed());
}

#[test]
fn wake_listener_ownership_is_exclusive_and_released_on_drop() {
    let hub = RevisionHub::new(1).expect("positive capacity");
    let listener = hub.take_wake_listener().expect("first listener transfers");
    assert!(hub.take_wake_listener().is_err());
    drop(listener);
    let _replacement = hub
        .take_wake_listener()
        .expect("listener ownership releases");
}

#[test]
fn publication_racing_waiter_registration_is_observed() {
    let hub = RevisionHub::new(1).expect("positive capacity");
    let mut listener = hub.take_wake_listener().expect("listener transfers");
    hub.register_subscription(&request(1, [SubscriptionTopic::All]))
        .expect("register");
    hub.activate_subscription(id(1)).expect("activate");
    let flag = Arc::new(WakeFlag(AtomicBool::new(false)));
    let waker = Waker::from(Arc::clone(&flag));
    let mut context = Context::from_waker(&waker);
    let mut changed = Box::pin(listener.changed());

    assert_eq!(
        Future::poll(Pin::as_mut(&mut changed), &mut context),
        Poll::Pending
    );
    let _ = hub.publish(Revision::new(1), [SubscriptionTopic::All], true);

    assert!(flag.0.load(Ordering::Acquire));
    assert_eq!(
        Future::poll(Pin::as_mut(&mut changed), &mut context),
        Poll::Ready(())
    );
}

struct WakeFlag(AtomicBool);

impl Wake for WakeFlag {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::Release);
    }
}
