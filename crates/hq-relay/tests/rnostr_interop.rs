//! Explicitly opted-in interoperability against the pinned controlled rnostr relay.

#![allow(clippy::expect_used, clippy::panic)]

use std::{
    env,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hq_domain::{
    BoundedSet, CausalReferences, EncryptionPublicKey, FactScope, InstallationId,
    MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, SemanticPayload, ShortText, SigningPublicKey,
    Timestamp,
};
use hq_protocol::{Bip340Signer, CanonicalEventPlan, VerifiedSemanticFact};
use hq_relay::{
    EnvelopeCodec, RelayConnection, RelayConnector, RelayEnvelopePort, RelayFrame, RelayReceive,
    RelayUrl, SystemRandom, WebSocketRelayConfig, WebSocketRelayConnector,
};

const FIRST_INSTALLATION: [u8; 32] = [0x11; 32];

#[test]
#[ignore = "requires HQ_RUN_CONTROLLED_RELAY_SMOKE=1 and a controlled rnostr endpoint"]
fn controlled_rnostr_auth_publish_retained_and_reconnect() {
    assert_eq!(
        env::var("HQ_RUN_CONTROLLED_RELAY_SMOKE").as_deref(),
        Ok("1"),
        "preflight: set HQ_RUN_CONTROLLED_RELAY_SMOKE=1 and use scripts/rust-relay-smoke.sh"
    );
    let endpoint = env::var("HQ_CONTROLLED_RELAY_URL").expect(
        "preflight: HQ_CONTROLLED_RELAY_URL must name the controlled ws:// or wss:// relay",
    );
    let url = RelayUrl::new(endpoint).expect("preflight: controlled relay URL must be valid");
    let connector = WebSocketRelayConnector::new(WebSocketRelayConfig {
        connect_timeout: Duration::from_secs(5),
        write_timeout: Duration::from_secs(5),
        ..WebSocketRelayConfig::default()
    })
    .expect("bounded connector config validates");
    let sender = EnvelopeCodec::from_secret_bytes(secret(1)).expect("sender codec constructs");
    let receiver = EnvelopeCodec::from_secret_bytes(secret(2)).expect("receiver codec constructs");

    let canonical = canonical_fact();
    let mut random = SystemRandom;
    let wrapper = sender
        .prepare(
            &canonical,
            receiver.public_key(),
            unix_seconds(),
            &mut random,
        )
        .expect("gift wrap prepares");
    let wrapper_id = wrapper.metadata.wrapper_id;
    let exact_wrapper = wrapper.exact_wire().to_vec();

    let mut publisher = connector.connect(&url).expect("publisher connects");
    authenticate(&mut *publisher, &sender, &url);
    publisher
        .send(RelayFrame::Event(exact_wrapper.clone()))
        .expect("exact wrapper publishes");
    wait_for_ack(&mut *publisher, wrapper_id, "publish");
    publisher.close().expect("publisher closes");

    let subscription = "hq-controlled-retained";
    let mut reader = connector.connect(&url).expect("reader connects");
    authenticate(&mut *reader, &receiver, &url);
    assert_retained(
        &mut *reader,
        subscription,
        &exact_wrapper,
        &receiver,
        &canonical,
    );
    reader.close().expect("reader closes");

    let mut reconnected = connector.connect(&url).expect("reader reconnects");
    authenticate(&mut *reconnected, &receiver, &url);
    assert_retained(
        &mut *reconnected,
        subscription,
        &exact_wrapper,
        &receiver,
        &canonical,
    );
    reconnected.close().expect("reconnected reader closes");
}

fn authenticate(connection: &mut dyn RelayConnection, codec: &EnvelopeCodec, url: &RelayUrl) {
    let challenge = wait_for_frame(connection, |frame| match frame {
        RelayFrame::Auth(challenge) => Some(challenge),
        _ => None,
    });
    let authentication = RelayEnvelopePort::authenticate(codec, url, &challenge, unix_seconds())
        .expect("NIP-42 response signs");
    let exact = String::from_utf8(authentication.exact_event).expect("auth event is UTF-8 JSON");
    connection
        .send(RelayFrame::Auth(exact))
        .expect("NIP-42 response sends");
    wait_for_ack(connection, authentication.event_id, "authentication");
}

fn wait_for_ack(connection: &mut dyn RelayConnection, expected: [u8; 32], operation: &str) {
    let (event_id, accepted, message) = wait_for_frame(connection, |frame| match frame {
        RelayFrame::Ok {
            event_id,
            accepted,
            message,
        } => Some((event_id, accepted, message)),
        _ => None,
    });
    assert_eq!(event_id, expected, "{operation} acknowledgement identity");
    assert!(
        accepted,
        "{operation} rejected by controlled relay: {message}"
    );
}

fn request_retained(connection: &mut dyn RelayConnection, subscription: &str, recipient: [u8; 32]) {
    connection
        .send(RelayFrame::Request {
            subscription: subscription.to_owned(),
            filter: format!(
                "{{\"kinds\":[1059],\"#p\":[\"{}\"],\"limit\":16}}",
                hex(recipient)
            ),
        })
        .expect("retained request sends");
}

fn assert_retained(
    connection: &mut dyn RelayConnection,
    subscription_prefix: &str,
    expected_wrapper: &[u8],
    receiver: &EnvelopeCodec,
    canonical: &VerifiedSemanticFact,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut observed = false;
    let mut attempt = 0_u32;
    let mut subscription = format!("{subscription_prefix}-{attempt}");
    request_retained(connection, &subscription, receiver.public_key());
    loop {
        let frame = receive_frame(connection, deadline);
        match frame {
            RelayFrame::SubscriptionEvent {
                subscription: candidate,
                exact_event,
            } if candidate == subscription => {
                if exact_event == expected_wrapper {
                    let opened = receiver.open(&exact_event).expect("retained wrapper opens");
                    assert_eq!(
                        opened.canonical_event.as_ref(),
                        canonical.verified_event().exact_event_bytes(),
                        "retained wrapper preserves exact canonical bytes"
                    );
                    observed = true;
                }
            }
            RelayFrame::EndOfStoredEvents(candidate) if candidate == subscription => {
                if observed {
                    connection
                        .send(RelayFrame::Close(subscription.clone()))
                        .expect("completed retained subscription closes");
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "controlled relay never made the acknowledged wrapper query-visible"
                );
                connection
                    .send(RelayFrame::Close(subscription.clone()))
                    .expect("empty retained subscription closes before retry");
                std::thread::sleep(Duration::from_millis(25));
                attempt = attempt.checked_add(1).expect("retry count remains bounded");
                subscription = format!("{subscription_prefix}-{attempt}");
                request_retained(connection, &subscription, receiver.public_key());
            }
            RelayFrame::Closed {
                subscription: candidate,
                message,
            } if candidate == subscription => {
                panic!("controlled relay closed subscription: {message}")
            }
            _ => {}
        }
    }
}

fn wait_for_frame<T>(
    connection: &mut dyn RelayConnection,
    mut select: impl FnMut(RelayFrame) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(value) = select(receive_frame(connection, deadline)) {
            return value;
        }
    }
}

fn receive_frame(connection: &mut dyn RelayConnection, deadline: Instant) -> RelayFrame {
    let remaining = deadline.saturating_duration_since(Instant::now());
    assert!(!remaining.is_zero(), "controlled relay response timed out");
    match connection
        .receive(remaining.min(Duration::from_secs(2)))
        .expect("controlled relay receive succeeds")
    {
        RelayReceive::Frame(frame) => frame,
        RelayReceive::TimedOut => {
            assert!(
                Instant::now() < deadline,
                "controlled relay response timed out"
            );
            receive_frame(connection, deadline)
        }
        RelayReceive::Closed => panic!("controlled relay closed the connection"),
    }
}

fn canonical_fact() -> VerifiedSemanticFact {
    let signer = Bip340Signer::from_secret_bytes(secret(1)).expect("canonical signer constructs");
    let public_key = signer.public_key();
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        BoundedSet::new([]).expect("empty parents validate"),
        [],
    )
    .expect("empty authorities validate");
    CanonicalEventPlan::new(
        InstallationId::from_bytes(FIRST_INSTALLATION),
        Timestamp::from_unix_millis(
            i64::try_from(unix_seconds().saturating_mul(1_000)).unwrap_or(i64::MAX),
        ),
        FactScope::InstallationPrivate(InstallationId::from_bytes(FIRST_INSTALLATION)),
        causal,
        SemanticPayload::InstallationDeclared {
            installation_id: InstallationId::from_bytes(FIRST_INSTALLATION),
            signing_key: SigningPublicKey::from_bytes(public_key),
            encryption_key: EncryptionPublicKey::from_bytes(public_key),
            label: Some(ShortText::new("controlled-rnostr-smoke").expect("label validates")),
        },
    )
    .sign(&signer, [0x42; 32])
    .expect("canonical smoke fact signs")
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

fn secret(value: u8) -> [u8; 32] {
    let mut secret = [0_u8; 32];
    secret[31] = value;
    secret
}

fn hex(bytes: [u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}
