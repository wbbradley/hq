//! Concrete node composition for durable relay synchronization.

use std::{
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hq_application::{
    ApplicationError, ApplicationErrorCode, ConfigureRelays, EffectOutcome, EffectRequest,
    PublishWake, RelayConfiguration, SynchronizationRequest, WakeDisposition,
};
use hq_domain::{InstallationId, ResourceLocator, ResourceScheme, Revision};
use hq_reducer::{AuthorityPolicy, AuthorityProjection, AuthorityProjectionKey, PeerRouteState};
use hq_relay::{
    CanonicalIngest, DesiredRelayPolicy, EnvelopeCodec, OutboxKey, RelayClock, RelayConnector,
    RelayManager, RelayManagerConfig, RelayPolicyChange, RelayPortError, RelayStateMutation,
    RelayStatePort, RelayUrl, ResolvedRoute, RouteResolver, StableRelayJitter,
};
use hq_store::{ReplicationHandle, Store};

use crate::{
    CancellationToken, ComponentDrain, ComponentError, NodeComponent, RelayStoreAdapter,
    relay_store::map_store_error,
};

/// Concrete relay manager configuration at the node composition root.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RelayNodeConfig {
    /// Deterministic manager/session bounds and retry policy.
    pub manager: RelayManagerConfig,
}

/// Node lifecycle and application adapter around the sole concrete relay manager.
pub struct RelayNodeComponent {
    config: RelayNodeConfig,
    dependencies: hq_relay::RelaySessionDependencies,
    state: Arc<dyn RelayStatePort>,
    manager: Mutex<Option<RelayManager>>,
    accepting: AtomicBool,
}

impl RelayNodeComponent {
    /// Composes store, route, ingest, envelope, time, and connection capabilities without taking
    /// store or installation-identity shutdown ownership.
    pub fn new(
        config: RelayNodeConfig,
        store: &Store,
        envelope: EnvelopeCodec,
        local_installation: InstallationId,
        authority_policy: AuthorityPolicy,
        connector: Arc<dyn RelayConnector>,
    ) -> Self {
        let replication = store.replication_handle();
        let state: Arc<dyn RelayStatePort> = Arc::new(RelayStoreAdapter::new(store));
        let dependencies = hq_relay::RelaySessionDependencies {
            state: Arc::clone(&state),
            routes: Arc::new(VerifiedRouteResolver {
                store: replication.clone(),
                local_installation,
            }),
            ingest: Arc::new(StoreCanonicalIngest {
                store: replication,
                authority_policy,
            }),
            envelopes: Arc::new(envelope),
            clock: Arc::new(SystemRelayClock::new()),
            connector,
            jitter: Arc::new(StableRelayJitter),
        };
        Self {
            config,
            dependencies,
            state,
            manager: Mutex::new(None),
            accepting: AtomicBool::new(false),
        }
    }

    fn wake(&self) -> Result<(), RelayPortError> {
        self.manager
            .lock()
            .map_err(|_| RelayPortError::Unavailable)?
            .as_ref()
            .ok_or(RelayPortError::Unavailable)?
            .wake()
    }

    fn shutdown_manager(&mut self) -> Result<(), ComponentError> {
        let manager = self
            .manager
            .get_mut()
            .map_err(|_| ComponentError::unavailable())?
            .take();
        manager.map_or(Ok(()), |manager| {
            manager
                .shutdown()
                .map(|_| ())
                .map_err(|_| ComponentError::unavailable())
        })
    }

    fn ensure_accepting(&self) -> Result<(), ApplicationError> {
        if self.accepting.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(ApplicationError::new(
                ApplicationErrorCode::AdapterUnavailable,
            ))
        }
    }
}

impl fmt::Debug for RelayNodeComponent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayNodeComponent")
            .field("config", &self.config)
            .field("accepting", &self.accepting.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl NodeComponent for RelayNodeComponent {
    fn start(&mut self, _cancellation: CancellationToken) -> Result<(), ComponentError> {
        if self
            .manager
            .get_mut()
            .map_err(|_| ComponentError::unavailable())?
            .is_some()
        {
            return Ok(());
        }
        let manager = RelayManager::start(self.config.manager.clone(), self.dependencies.clone())
            .map_err(|_| ComponentError::unavailable())?;
        *self
            .manager
            .get_mut()
            .map_err(|_| ComponentError::unavailable())? = Some(manager);
        self.accepting.store(true, Ordering::Release);
        Ok(())
    }

    fn stop_intake(&mut self) -> Result<(), ComponentError> {
        self.accepting.store(false, Ordering::Release);
        Ok(())
    }

    fn drain(&mut self) -> Result<ComponentDrain, ComponentError> {
        self.shutdown_manager()?;
        Ok(ComponentDrain::Complete)
    }

    fn force_stop(&mut self) -> Result<(), ComponentError> {
        self.shutdown_manager()
    }
}

impl PublishWake for RelayNodeComponent {
    fn publish_wake(&self, _revision: Revision) -> Result<WakeDisposition, ApplicationError> {
        self.wake()
            .map(|()| WakeDisposition::Scheduled)
            .map_err(application_error)
    }
}

impl ConfigureRelays for RelayNodeComponent {
    fn configure_relay(
        &self,
        request: &EffectRequest<RelayConfiguration>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        self.ensure_accepting()?;
        let relay = relay_url(&request.body().endpoint)?;
        self.state
            .apply(RelayStateMutation::Configure(RelayPolicyChange {
                operation_id: request.operation_id(),
                request_digest: request.request_digest(),
                desired: DesiredRelayPolicy {
                    url: relay,
                    access: request.body().access,
                    authentication: request.body().authentication,
                    enabled: true,
                },
            }))
            .map_err(application_error)?;
        self.wake().map_err(application_error)?;
        Ok(EffectOutcome::Accepted(()))
    }

    fn synchronize(
        &self,
        request: &EffectRequest<SynchronizationRequest>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        self.ensure_accepting()?;
        if let SynchronizationRequest::Relay(endpoint) = request.body() {
            relay_url(endpoint)?;
        }
        self.wake().map_err(application_error)?;
        Ok(EffectOutcome::Accepted(()))
    }
}

struct VerifiedRouteResolver {
    store: ReplicationHandle,
    local_installation: InstallationId,
}

impl RouteResolver for VerifiedRouteResolver {
    fn resolve(&self, key: OutboxKey) -> Result<ResolvedRoute, RelayPortError> {
        let snapshot = self
            .store
            .load_authority_snapshot()
            .map_err(map_store_error)?;
        let projection = snapshot.projection(AuthorityProjectionKey::PeerRoute {
            owner: self.local_installation,
            peer: key.recipient,
        });
        let Some(AuthorityProjection::PeerRoute(route)) = projection else {
            return Err(RelayPortError::Unavailable);
        };
        if route.state() != PeerRouteState::Routable || route.frontier().len() != 1 {
            return Err(RelayPortError::Unavailable);
        }
        let route_fact = route
            .frontier()
            .first()
            .ok_or(RelayPortError::Unavailable)?;
        let candidate = route
            .routes
            .get(route_fact)
            .ok_or(RelayPortError::Corrupt)?;
        if candidate.peer.installation_id() != key.recipient {
            return Err(RelayPortError::Corrupt);
        }
        let relays = candidate
            .relay_hints
            .as_slice()
            .iter()
            .map(|hint| {
                if hint.scheme() != ResourceScheme::Opaque {
                    return Err(RelayPortError::Unavailable);
                }
                RelayUrl::new(hint.value().to_owned()).map_err(|_| RelayPortError::Unavailable)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if relays.is_empty() {
            return Err(RelayPortError::Unavailable);
        }
        Ok(ResolvedRoute {
            recipient_public_key: *candidate.encryption_key.as_bytes(),
            relays,
        })
    }
}

struct StoreCanonicalIngest {
    store: ReplicationHandle,
    authority_policy: AuthorityPolicy,
}

impl CanonicalIngest for StoreCanonicalIngest {
    fn ingest(&self, exact_canonical_bytes: Vec<u8>) -> Result<(), RelayPortError> {
        let fact = hq_protocol::decode_semantic_event(exact_canonical_bytes)
            .map_err(|_| RelayPortError::InvalidInput)?
            .ok_or(RelayPortError::InvalidInput)?;
        self.store
            .ingest_verified(fact, self.authority_policy)
            .map(|_| ())
            .map_err(map_store_error)
    }
}

struct SystemRelayClock {
    started: Instant,
}

impl SystemRelayClock {
    fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl RelayClock for SystemRelayClock {
    fn unix_millis(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    fn monotonic_millis(&self) -> u64 {
        self.started
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

fn relay_url(locator: &ResourceLocator) -> Result<RelayUrl, ApplicationError> {
    if locator.scheme() != ResourceScheme::Opaque {
        return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest));
    }
    RelayUrl::new(locator.value().to_owned())
        .map_err(|_| ApplicationError::new(ApplicationErrorCode::InvalidRequest))
}

const fn application_error(error: RelayPortError) -> ApplicationError {
    let code = match error {
        RelayPortError::InvalidInput => ApplicationErrorCode::InvalidRequest,
        RelayPortError::Conflict => ApplicationErrorCode::CommandIdentityConflict,
        RelayPortError::Corrupt => ApplicationErrorCode::StateCorrupt,
        RelayPortError::Unavailable | RelayPortError::Connection => {
            ApplicationErrorCode::AdapterUnavailable
        }
        RelayPortError::Backpressure => ApplicationErrorCode::IntakeFull,
    };
    ApplicationError::new(code)
}
