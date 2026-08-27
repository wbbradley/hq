//! Two-store convergence through the concrete node relay composition.

#![allow(clippy::expect_used, clippy::panic)]

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    num::NonZeroUsize,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use hq_application::{
    ConfigureRelays, EffectOutcome, EffectRequest, PublishWake, RelayAccess, RelayAuthentication,
    RelayConfiguration,
};
use hq_domain::{
    AuthorityReference, AuthorityRole, BoundedSet, BoundedText, BoundedVec, CausalReferences,
    CommandDigest, EncryptionPublicKey, FactId, FactScope, InstallationAddress, InstallationId,
    MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, MailboxId, OperationId, ResourceLocator,
    ResourceScheme, SemanticPayload, Timestamp,
};
use hq_node::{CancellationToken, NodeComponent, RelayNodeComponent, RelayNodeConfig};
use hq_protocol::{Bip340Signer, CanonicalEventPlan, VerifiedSemanticFact};
use hq_reducer::AuthorityPolicy;
use hq_relay::{
    EnvelopeCodec, RelayConnection, RelayConnector, RelayFrame, RelayManagerConfig, RelayPortError,
    RelayReceive, RelaySessionConfig, RelayUrl,
};
use hq_store::Store;
use serde_json::Value;

const A: [u8; 32] = [0x11; 32];
const B: [u8; 32] = [0x22; 32];
#[test]
#[allow(clippy::too_many_lines)]
fn concrete_two_replica_composition_converges_despite_order_outage_restart_and_response_loss() {
    let directories = [TestDirectory::new(), TestDirectory::new()];
    let stores = [open_store(&directories[0]), open_store(&directories[1])];
    let policies = [policy(A), policy(B)];
    let relay = RelayUrl::new("ws://relay.test".to_owned()).expect("URL validates");
    let hub = Arc::new(RetainedHub::new(true, true, 1));

    let root = root_fact();
    let root_id = root.fact().id();
    let peer_root = peer_root_fact();
    let peer_root_id = peer_root.fact().id();
    let account = account_fact(root_id);
    let account_id = account.fact().id();
    let grant = grant_fact(account_id);
    let grant_id = grant.fact().id();
    let acceptance = acceptance_fact(grant_id);
    let route = route_fact(root_id, &relay, [], 5, 1);
    let route_id = route.fact().id();
    let peer_route = peer_route_fact(peer_root_id, &relay);
    for (store, policy) in stores.iter().zip(policies) {
        for fact in [
            root_fact(),
            peer_root_fact(),
            account_fact(root_id),
            grant_fact(account_id),
            acceptance_fact(grant_id),
            route_fact(root_id, &relay, [], 5, 1),
            peer_route_fact(peer_root_id, &relay),
        ] {
            store
                .ingest_verified(fact, policy)
                .expect("foundation fact ingests");
        }
    }
    drop((
        root, peer_root, account, grant, acceptance, route, peer_route,
    ));

    let first = project_fact(root_id, account_id, 0x61, 4);
    let second = project_fact(root_id, account_id, 0x62, 7);
    let expected = BTreeSet::from([first.fact().id(), second.fact().id()]);
    stores[0]
        .ingest_verified(first, policies[0])
        .expect("first project ingests");
    stores[0]
        .ingest_verified(second, policies[0])
        .expect("second project ingests");

    let mut sender = component(&stores[0], A, policies[0], 1, Arc::clone(&hub));
    sender
        .start(CancellationToken::new())
        .expect("sender starts");
    configure(&sender, &relay, 1);
    sender
        .publish_wake(hq_domain::Revision::new(1))
        .expect("sender wakes");
    wait_for(|| hub.retained_count() == 3);
    assert_eq!(
        hub.retained_count(),
        3,
        "response loss never regenerates a wrapper"
    );

    hub.restart();
    sender
        .publish_wake(hq_domain::Revision::new(2))
        .expect("restart wake succeeds");

    let mut receiver = component(&stores[1], B, policies[1], 2, Arc::clone(&hub));
    receiver
        .start(CancellationToken::new())
        .expect("receiver starts");
    configure(&receiver, &relay, 2);
    wait_for(|| corpus_ids(&stores[1]).is_superset(&expected));

    assert_converged(&stores[0], &stores[1], policies);
    assert_ne!(
        stores[0].load_relay_state(64).expect("sender relay state"),
        stores[1]
            .load_relay_state(64)
            .expect("receiver relay state"),
        "transport-local observations may differ"
    );

    receiver.stop_intake().expect("receiver intake stops");
    receiver.drain().expect("receiver drains for restart");

    let before_membership_traffic = hub.retained_count();
    let revoke = device_revoke_fact(account_id, grant_id, acceptance_fact(grant_id).fact().id());
    let revoke_id = revoke.fact().id();
    stores[0]
        .ingest_verified(revoke, policies[0])
        .expect("device revoke ingests");
    let regrant = device_regrant_fact(account_id, revoke_id);
    let regrant_fact_id = regrant.fact().id();
    stores[0]
        .ingest_verified(regrant, policies[0])
        .expect("device regrant ingests");
    sender
        .publish_wake(hq_domain::Revision::new(3))
        .expect("membership wake succeeds");
    wait_for(|| hub.retained_count() == before_membership_traffic + 2);

    let mut receiver = component(&stores[1], B, policies[1], 2, Arc::clone(&hub));
    receiver
        .start(CancellationToken::new())
        .expect("receiver restarts");
    configure(&receiver, &relay, 2);
    wait_for(|| {
        let ids = corpus_ids(&stores[1]);
        ids.contains(&revoke_id) && ids.contains(&regrant_fact_id)
    });
    let reacceptance = device_reacceptance_fact(regrant_fact_id);
    let reacceptance_id = reacceptance.fact().id();
    stores[1]
        .ingest_verified(reacceptance, policies[1])
        .expect("device reacceptance ingests");
    assert!(
        stores[1]
            .load_outbox_intents(64)
            .expect("receiver outbox loads")
            .iter()
            .any(|intent| intent.fact_id() == reacceptance_id),
        "reacceptance is addressed back to the creator"
    );
    receiver
        .publish_wake(hq_domain::Revision::new(3))
        .expect("reacceptance wake succeeds");
    wait_for_relay_fact(
        &stores[0],
        &stores[1],
        &hub,
        reacceptance_id,
        "reacceptance reaches creator",
    );
    assert_converged(&stores[0], &stores[1], policies);

    let block = route_blocked_fact(root_id, 8);
    let block_id = block.fact().id();
    for (store, policy) in stores.iter().zip(policies) {
        store
            .ingest_verified(route_blocked_fact(root_id, 8), policy)
            .expect("route block ingests");
    }
    let retained_while_blocked = hub.retained_count();
    let third = project_fact(root_id, account_id, 0x63, 9);
    let third_id = third.fact().id();
    stores[0]
        .ingest_verified(third, policies[0])
        .expect("third project queues");
    sender
        .publish_wake(hq_domain::Revision::new(4))
        .expect("blocked route wake succeeds");
    std::thread::sleep(Duration::from_millis(40));
    assert_eq!(
        hub.retained_count(),
        retained_while_blocked,
        "blocked route cannot publish"
    );

    for (store, policy) in stores.iter().zip(policies) {
        store
            .ingest_verified(
                route_fact(root_id, &relay, [route_id, block_id], 10, 3),
                policy,
            )
            .expect("full-frontier route restore ingests");
    }
    sender
        .publish_wake(hq_domain::Revision::new(5))
        .expect("regrant wakes sender");
    wait_for(|| corpus_ids(&stores[1]).contains(&third_id));
    assert_converged(&stores[0], &stores[1], policies);

    receiver.stop_intake().expect("receiver intake stops");
    sender.stop_intake().expect("sender intake stops");
    receiver.drain().expect("receiver drains");
    sender.drain().expect("sender drains");
}

fn component(
    store: &Store,
    installation: [u8; 32],
    authority_policy: AuthorityPolicy,
    secret: u8,
    hub: Arc<RetainedHub>,
) -> RelayNodeComponent {
    let session = RelaySessionConfig {
        receive_wait: Duration::from_millis(2),
        retained_page_items: 4,
        retry_initial: Duration::from_millis(5),
        retry_max: Duration::from_millis(20),
        ..RelaySessionConfig::default()
    };
    let config = RelayNodeConfig {
        manager: RelayManagerConfig {
            session,
            policy_page_items: 8,
            max_sessions: 4,
            periodic_poll: Duration::from_millis(5),
        },
    };
    RelayNodeComponent::new(
        config,
        store,
        EnvelopeCodec::from_secret_bytes(secret_bytes(secret)).expect("codec constructs"),
        InstallationId::from_bytes(installation),
        authority_policy,
        hub,
    )
}

fn configure(component: &RelayNodeComponent, relay: &RelayUrl, identity: u8) {
    let endpoint = ResourceLocator::new(
        ResourceScheme::Opaque,
        BoundedText::new(relay.as_str()).expect("endpoint is bounded"),
    );
    let request = EffectRequest::new(
        OperationId::from_bytes([identity; 32]),
        CommandDigest::from_bytes([identity.wrapping_add(10); 32]),
        Timestamp::from_unix_millis(0),
        RelayConfiguration {
            endpoint,
            access: RelayAccess::ReadWrite,
            authentication: RelayAuthentication::Disabled,
        },
    );
    assert_eq!(
        component.configure_relay(&request),
        Ok(EffectOutcome::Accepted(()))
    );
}

fn assert_converged(left: &Store, right: &Store, policies: [AuthorityPolicy; 2]) {
    assert_eq!(corpus_ids(left), corpus_ids(right));
    for policy in policies {
        let left = left.complete_snapshot(policy).expect("left reduces");
        let right = right.complete_snapshot(policy).expect("right reduces");
        assert_eq!(left.normalized_index(), right.normalized_index());
        assert_eq!(
            left.authority_projection_snapshot(),
            right.authority_projection_snapshot()
        );
        assert_eq!(
            left.conversation_projection_snapshot(),
            right.conversation_projection_snapshot()
        );
        assert_eq!(
            left.agent_projection_snapshot(),
            right.agent_projection_snapshot()
        );
        assert_eq!(
            left.project_projection_snapshot(),
            right.project_projection_snapshot()
        );
    }
}

fn corpus_ids(store: &Store) -> BTreeSet<FactId> {
    store
        .load_corpus()
        .expect("corpus loads")
        .iter()
        .map(|fact| fact.fact().id())
        .collect()
}

fn root_fact() -> VerifiedSemanticFact {
    let signing = signer(1).public_key();
    author(
        1,
        A,
        0,
        FactScope::InstallationPrivate(InstallationId::from_bytes(A)),
        [],
        [],
        SemanticPayload::InstallationDeclared {
            installation_id: InstallationId::from_bytes(A),
            signing_key: hq_domain::SigningPublicKey::from_bytes(signing),
            encryption_key: EncryptionPublicKey::from_bytes(signing),
            label: Some(hq_domain::ShortText::new("alpha").expect("label validates")),
        },
        1,
    )
}

fn peer_root_fact() -> VerifiedSemanticFact {
    let signing = signer(2).public_key();
    author(
        2,
        B,
        0,
        FactScope::InstallationPrivate(InstallationId::from_bytes(B)),
        [],
        [],
        SemanticPayload::InstallationDeclared {
            installation_id: InstallationId::from_bytes(B),
            signing_key: hq_domain::SigningPublicKey::from_bytes(signing),
            encryption_key: EncryptionPublicKey::from_bytes(signing),
            label: Some(hq_domain::ShortText::new("beta").expect("label validates")),
        },
        2,
    )
}

fn account_fact(root: FactId) -> VerifiedSemanticFact {
    let signing = hq_domain::SigningPublicKey::from_bytes(signer(1).public_key());
    author(
        1,
        A,
        3_000,
        FactScope::InstallationPrivate(InstallationId::from_bytes(A)),
        [root],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            root,
        )],
        SemanticPayload::HumanAccountCreated {
            account_id: hq_domain::AccountId::from_bytes([0x55; 32]),
            creator: InstallationAddress::new(InstallationId::from_bytes(A), signing),
            label: Some(hq_domain::ShortText::new("primary").expect("label validates")),
        },
        12,
    )
}

fn grant_fact(account: FactId) -> VerifiedSemanticFact {
    let peer = InstallationAddress::new(
        InstallationId::from_bytes(B),
        hq_domain::SigningPublicKey::from_bytes(signer(2).public_key()),
    );
    author(
        1,
        A,
        5_000,
        FactScope::AccountAddressed(hq_domain::AccountId::from_bytes([0x55; 32])),
        [account],
        [AuthorityReference::new(
            AuthorityRole::AccountCreator,
            account,
        )],
        SemanticPayload::HumanDeviceGranted {
            account_id: hq_domain::AccountId::from_bytes([0x55; 32]),
            grant_id: hq_domain::GrantId::from_bytes([0x88; 32]),
            device: peer,
            label: Some(hq_domain::ShortText::new("beta").expect("label validates")),
            relay_hints: BoundedVec::new([]).expect("empty hints validate"),
        },
        14,
    )
}

fn acceptance_fact(grant: FactId) -> VerifiedSemanticFact {
    let peer = InstallationAddress::new(
        InstallationId::from_bytes(B),
        hq_domain::SigningPublicKey::from_bytes(signer(2).public_key()),
    );
    author(
        2,
        B,
        6_000,
        FactScope::AccountAddressed(hq_domain::AccountId::from_bytes([0x55; 32])),
        [grant],
        [AuthorityReference::new(AuthorityRole::DeviceGrant, grant)],
        SemanticPayload::HumanDeviceAccepted {
            account_id: hq_domain::AccountId::from_bytes([0x55; 32]),
            grant_id: hq_domain::GrantId::from_bytes([0x88; 32]),
            device: peer,
        },
        15,
    )
}

fn device_revoke_fact(account: FactId, grant: FactId, acceptance: FactId) -> VerifiedSemanticFact {
    author(
        1,
        A,
        8_000,
        FactScope::AccountAddressed(hq_domain::AccountId::from_bytes([0x55; 32])),
        [account, grant, acceptance],
        [
            AuthorityReference::new(AuthorityRole::AccountCreator, account),
            AuthorityReference::new(AuthorityRole::DeviceGrant, grant),
        ],
        SemanticPayload::HumanDeviceRevoked {
            account_id: hq_domain::AccountId::from_bytes([0x55; 32]),
            grant_id: hq_domain::GrantId::from_bytes([0x88; 32]),
            device_id: InstallationId::from_bytes(B),
        },
        21,
    )
}

fn device_regrant_fact(account: FactId, revoke: FactId) -> VerifiedSemanticFact {
    let peer = InstallationAddress::new(
        InstallationId::from_bytes(B),
        hq_domain::SigningPublicKey::from_bytes(signer(2).public_key()),
    );
    author(
        1,
        A,
        9_000,
        FactScope::AccountAddressed(hq_domain::AccountId::from_bytes([0x55; 32])),
        [account, revoke],
        [AuthorityReference::new(
            AuthorityRole::AccountCreator,
            account,
        )],
        SemanticPayload::HumanDeviceGranted {
            account_id: hq_domain::AccountId::from_bytes([0x55; 32]),
            grant_id: hq_domain::GrantId::from_bytes([0x99; 32]),
            device: peer,
            label: Some(hq_domain::ShortText::new("beta-restored").expect("label validates")),
            relay_hints: BoundedVec::new([]).expect("empty hints validate"),
        },
        22,
    )
}

fn device_reacceptance_fact(grant: FactId) -> VerifiedSemanticFact {
    let peer = InstallationAddress::new(
        InstallationId::from_bytes(B),
        hq_domain::SigningPublicKey::from_bytes(signer(2).public_key()),
    );
    author(
        2,
        B,
        10_000,
        FactScope::AccountAddressed(hq_domain::AccountId::from_bytes([0x55; 32])),
        [grant],
        [AuthorityReference::new(AuthorityRole::DeviceGrant, grant)],
        SemanticPayload::HumanDeviceAccepted {
            account_id: hq_domain::AccountId::from_bytes([0x55; 32]),
            grant_id: hq_domain::GrantId::from_bytes([0x99; 32]),
            device: peer,
        },
        23,
    )
}

fn route_fact(
    root: FactId,
    relay: &RelayUrl,
    extra_parents: impl IntoIterator<Item = FactId>,
    millis: i64,
    auxiliary: u8,
) -> VerifiedSemanticFact {
    let peer_key = signer(2).public_key();
    let peer = InstallationAddress::new(
        InstallationId::from_bytes(B),
        hq_domain::SigningPublicKey::from_bytes(peer_key),
    );
    author(
        1,
        A,
        millis,
        FactScope::InstallationPrivate(InstallationId::from_bytes(A)),
        std::iter::once(root).chain(extra_parents),
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            root,
        )],
        SemanticPayload::PeerRouteSet {
            peer,
            encryption_key: EncryptionPublicKey::from_bytes(peer_key),
            label: None,
            relay_hints: BoundedVec::new([ResourceLocator::new(
                ResourceScheme::Opaque,
                BoundedText::new(relay.as_str()).expect("relay hint validates"),
            )])
            .expect("one hint validates"),
        },
        auxiliary,
    )
}

fn peer_route_fact(root: FactId, relay: &RelayUrl) -> VerifiedSemanticFact {
    let peer_key = signer(1).public_key();
    let peer = InstallationAddress::new(
        InstallationId::from_bytes(A),
        hq_domain::SigningPublicKey::from_bytes(peer_key),
    );
    author(
        2,
        B,
        5,
        FactScope::InstallationPrivate(InstallationId::from_bytes(B)),
        [root],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            root,
        )],
        SemanticPayload::PeerRouteSet {
            peer,
            encryption_key: EncryptionPublicKey::from_bytes(peer_key),
            label: None,
            relay_hints: BoundedVec::new([ResourceLocator::new(
                ResourceScheme::Opaque,
                BoundedText::new(relay.as_str()).expect("relay hint validates"),
            )])
            .expect("one hint validates"),
        },
        2,
    )
}

fn route_blocked_fact(root: FactId, auxiliary: u8) -> VerifiedSemanticFact {
    author(
        1,
        A,
        8,
        FactScope::InstallationPrivate(InstallationId::from_bytes(A)),
        [root],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            root,
        )],
        SemanticPayload::PeerRouteBlocked {
            peer_id: InstallationId::from_bytes(B),
            reason: hq_domain::ErrorCode::new("blocked").expect("reason validates"),
        },
        auxiliary,
    )
}

fn project_fact(
    root: FactId,
    account: FactId,
    identity: u8,
    auxiliary: u8,
) -> VerifiedSemanticFact {
    let project_id = hq_domain::ProjectId::from_bytes([identity; 32]);
    let mailbox_id = MailboxId::from_bytes([identity.wrapping_add(1); 32]);
    let resource_id = hq_domain::ResourceId::from_bytes([identity.wrapping_add(2); 32]);
    let resource = hq_domain::ProjectResource {
        resource_id,
        locator: ResourceLocator::new(
            ResourceScheme::WorkingTree,
            BoundedText::new(format!("/workspace/{identity}")).expect("path validates"),
        ),
        health: hq_domain::ResourceHealth::Healthy,
    };
    author(
        1,
        A,
        i64::from(identity) * 1_000,
        FactScope::AccountAddressed(hq_domain::AccountId::from_bytes([0x55; 32])),
        [root, account],
        [
            AuthorityReference::new(AuthorityRole::AccountMembership, account),
            AuthorityReference::new(AuthorityRole::ActiveHuman, account),
            AuthorityReference::new(AuthorityRole::ProjectHome, root),
        ],
        SemanticPayload::ProjectCreated {
            project_id,
            mailbox_id,
            home: InstallationId::from_bytes(A),
            name: hq_domain::ShortText::new(format!("project-{identity}")).expect("name validates"),
            brief: Some(
                hq_domain::ContentText::new("replicated project").expect("brief validates"),
            ),
            predecessor: None,
            resources: BoundedVec::new([resource]).expect("resource validates"),
            primary: Some(resource_id),
            initial_state: hq_domain::InitialProjectState::Open,
        },
        auxiliary,
    )
}

#[allow(clippy::too_many_arguments)]
fn author(
    secret: u8,
    installation: [u8; 32],
    millis: i64,
    scope: FactScope,
    parents: impl IntoIterator<Item = FactId>,
    authorities: impl IntoIterator<Item = AuthorityReference>,
    payload: SemanticPayload,
    auxiliary: u8,
) -> VerifiedSemanticFact {
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        BoundedSet::new(parents).expect("parents validate"),
        authorities,
    )
    .expect("authorities validate");
    CanonicalEventPlan::new(
        InstallationId::from_bytes(installation),
        Timestamp::from_unix_millis(millis),
        scope,
        causal,
        payload,
    )
    .sign(&signer(secret), [auxiliary; 32])
    .expect("fact signs")
}

fn signer(value: u8) -> Bip340Signer {
    Bip340Signer::from_secret_bytes(secret_bytes(value)).expect("secret validates")
}

fn secret_bytes(value: u8) -> [u8; 32] {
    let mut secret = [0_u8; 32];
    secret[31] = value;
    secret
}

fn policy(installation: [u8; 32]) -> AuthorityPolicy {
    AuthorityPolicy::new(
        InstallationId::from_bytes(installation),
        MailboxId::from_bytes([0x33; 32]),
    )
}

fn open_store(directory: &TestDirectory) -> Store {
    Store::open(
        &directory.path,
        NonZeroUsize::new(32).expect("capacity is nonzero"),
    )
    .expect("store opens")
}

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        let path = std::env::temp_dir().join(format!(
            "hq-relay-convergence-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("test directory creates");
        Self {
            path: path.join("state").join("hq.sqlite3"),
        }
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }
}

#[derive(Clone)]
struct RetainedHub {
    state: Arc<Mutex<HubState>>,
    reverse: bool,
    duplicate: bool,
}

struct HubState {
    next_connection: usize,
    retained: Vec<Vec<u8>>,
    queues: BTreeMap<usize, VecDeque<RelayReceive>>,
    live: BTreeMap<usize, (String, String)>,
    drop_ok: usize,
}

impl RetainedHub {
    fn new(reverse: bool, duplicate: bool, drop_ok: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(HubState {
                next_connection: 0,
                retained: Vec::new(),
                queues: BTreeMap::new(),
                live: BTreeMap::new(),
                drop_ok,
            })),
            reverse,
            duplicate,
        }
    }

    fn retained_count(&self) -> usize {
        self.state.lock().expect("hub locks").retained.len()
    }

    fn restart(&self) {
        let mut state = self.state.lock().expect("hub locks");
        for queue in state.queues.values_mut() {
            queue.push_back(RelayReceive::Closed);
        }
        state.live.clear();
    }
}

impl RelayConnector for RetainedHub {
    fn connect(&self, _url: &RelayUrl) -> Result<Box<dyn RelayConnection>, RelayPortError> {
        let mut state = self.state.lock().map_err(|_| RelayPortError::Connection)?;
        let identity = state.next_connection;
        state.next_connection = state.next_connection.saturating_add(1);
        state.queues.insert(identity, VecDeque::new());
        Ok(Box::new(HubConnection {
            hub: self.clone(),
            identity,
            closed: false,
        }))
    }
}

struct HubConnection {
    hub: RetainedHub,
    identity: usize,
    closed: bool,
}

impl RelayConnection for HubConnection {
    fn send(&mut self, frame: RelayFrame) -> Result<(), RelayPortError> {
        let mut state = self
            .hub
            .state
            .lock()
            .map_err(|_| RelayPortError::Connection)?;
        match frame {
            RelayFrame::Event(exact) => {
                let wrapper_id = event_id(&exact)?;
                if !state.retained.iter().any(|stored| stored == &exact) {
                    state.retained.push(exact.clone());
                    let live = state.live.clone();
                    let recipient = event_recipient(&exact)?;
                    for (identity, (subscription, expected_recipient)) in live {
                        if expected_recipient == recipient
                            && let Some(queue) = state.queues.get_mut(&identity)
                        {
                            queue.push_back(RelayReceive::Frame(RelayFrame::SubscriptionEvent {
                                subscription,
                                exact_event: exact.clone(),
                            }));
                        }
                    }
                }
                if state.drop_ok > 0 {
                    state.drop_ok -= 1;
                } else if let Some(queue) = state.queues.get_mut(&self.identity) {
                    queue.push_back(RelayReceive::Frame(RelayFrame::Ok {
                        event_id: wrapper_id,
                        accepted: true,
                        message: String::new(),
                    }));
                }
            }
            RelayFrame::Request {
                subscription,
                filter,
            } if subscription.contains("live") => {
                let filter: Value =
                    serde_json::from_str(&filter).map_err(|_| RelayPortError::Connection)?;
                state.live.insert(
                    self.identity,
                    (subscription, filter_recipient(&filter)?.to_owned()),
                );
            }
            RelayFrame::Request {
                subscription,
                filter,
            } => {
                enqueue_retained(
                    &mut state,
                    self.identity,
                    &subscription,
                    &filter,
                    self.hub.reverse,
                    self.hub.duplicate,
                )?;
            }
            RelayFrame::Close(subscription) => {
                if state
                    .live
                    .get(&self.identity)
                    .is_some_and(|(active, _)| active == &subscription)
                {
                    state.live.remove(&self.identity);
                }
            }
            RelayFrame::Auth(_)
            | RelayFrame::Ok { .. }
            | RelayFrame::EndOfStoredEvents(_)
            | RelayFrame::Closed { .. }
            | RelayFrame::Notice(_)
            | RelayFrame::SubscriptionEvent { .. } => {}
        }
        Ok(())
    }

    fn receive(&mut self, _wait: Duration) -> Result<RelayReceive, RelayPortError> {
        self.hub
            .state
            .lock()
            .map_err(|_| RelayPortError::Connection)?
            .queues
            .get_mut(&self.identity)
            .and_then(VecDeque::pop_front)
            .map_or(Ok(RelayReceive::TimedOut), Ok)
    }

    fn close(&mut self) -> Result<(), RelayPortError> {
        if !self.closed {
            let mut state = self
                .hub
                .state
                .lock()
                .map_err(|_| RelayPortError::Connection)?;
            state.queues.remove(&self.identity);
            state.live.remove(&self.identity);
            self.closed = true;
        }
        Ok(())
    }
}

fn enqueue_retained(
    state: &mut HubState,
    identity: usize,
    subscription: &str,
    exact_filter: &str,
    reverse: bool,
    duplicate: bool,
) -> Result<(), RelayPortError> {
    let filter: Value =
        serde_json::from_str(exact_filter).map_err(|_| RelayPortError::Connection)?;
    let until = filter
        .get("until")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX);
    let limit = filter
        .get("limit")
        .and_then(Value::as_u64)
        .and_then(|limit| usize::try_from(limit).ok())
        .ok_or(RelayPortError::Connection)?;
    let recipient = filter_recipient(&filter)?;
    let mut retained = state
        .retained
        .iter()
        .filter(|exact| event_recipient(exact).is_ok_and(|candidate| candidate == recipient))
        .filter_map(|exact| {
            event_order(exact)
                .ok()
                .filter(|order| order.0 <= until)
                .map(|order| (order, exact.clone()))
        })
        .collect::<Vec<_>>();
    retained.sort_by_key(|value| std::cmp::Reverse(value.0));
    retained.truncate(limit);
    let mut retained = retained
        .into_iter()
        .map(|(_, exact)| exact)
        .collect::<Vec<_>>();
    if duplicate && retained.len() == limit {
        retained.extend(retained.first().cloned());
    }
    if reverse {
        retained.reverse();
    }
    let queue = state
        .queues
        .get_mut(&identity)
        .ok_or(RelayPortError::Connection)?;
    for exact_event in retained {
        queue.push_back(RelayReceive::Frame(RelayFrame::SubscriptionEvent {
            subscription: subscription.to_owned(),
            exact_event,
        }));
    }
    queue.push_back(RelayReceive::Frame(RelayFrame::EndOfStoredEvents(
        subscription.to_owned(),
    )));
    Ok(())
}

fn event_id(exact: &[u8]) -> Result<[u8; 32], RelayPortError> {
    let value: Value = serde_json::from_slice(exact).map_err(|_| RelayPortError::Connection)?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or(RelayPortError::Connection)?;
    if id.len() != 64 {
        return Err(RelayPortError::Connection);
    }
    let mut output = [0_u8; 32];
    for (index, pair) in id.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        output[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Ok(output)
}

fn event_order(exact: &[u8]) -> Result<(u64, [u8; 32]), RelayPortError> {
    let value: Value = serde_json::from_slice(exact).map_err(|_| RelayPortError::Connection)?;
    let created_at = value
        .get("created_at")
        .and_then(Value::as_u64)
        .ok_or(RelayPortError::Connection)?;
    Ok((created_at, event_id(exact)?))
}

fn filter_recipient(filter: &Value) -> Result<&str, RelayPortError> {
    filter
        .get("#p")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .ok_or(RelayPortError::Connection)
}

fn event_recipient(exact: &[u8]) -> Result<String, RelayPortError> {
    let value: Value = serde_json::from_slice(exact).map_err(|_| RelayPortError::Connection)?;
    value
        .get("tags")
        .and_then(Value::as_array)
        .and_then(|tags| {
            tags.iter().find_map(|tag| {
                let values = tag.as_array()?;
                (values.first()?.as_str()? == "p")
                    .then(|| values.get(1)?.as_str())
                    .flatten()
            })
        })
        .map(str::to_owned)
        .ok_or(RelayPortError::Connection)
}

const fn nibble(value: u8) -> Result<u8, RelayPortError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(RelayPortError::Connection),
    }
}

fn wait_for(condition: impl Fn() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("condition did not become true");
}

fn wait_for_relay_fact(
    store: &Store,
    source: &Store,
    hub: &RetainedHub,
    fact_id: FactId,
    context: &str,
) {
    for _ in 0..500 {
        if corpus_ids(store).contains(&fact_id) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let source_state = source
        .load_relay_state(64)
        .expect("source relay state loads");
    let target_state = store
        .load_relay_state(64)
        .expect("target relay state loads");
    panic!(
        "{context}: fact {fact_id:?} absent; retained={}, source attempts={:?}, source cursors={:?}, source quarantine={}, target quarantine={}",
        hub.retained_count(),
        source_state.attempts,
        source_state.cursors,
        source_state.quarantine.len(),
        target_state.quarantine.len(),
    );
}
