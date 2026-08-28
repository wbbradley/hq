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
    MAX_RELAY_STATUS_POLICIES, PublishWake, RelayConfiguration, RelayPolicyStatus, RelayStatus,
    SynchronizationRequest, WakeDisposition,
};
use hq_domain::{
    BoundedText, InstallationId, RESOURCE_LOCATOR_MAX_BYTES, ResourceLocator, ResourceScheme,
    Revision,
};
use hq_reducer::{AuthorityPolicy, AuthorityProjection, AuthorityProjectionKey, PeerRouteState};
use hq_relay::{
    AttemptDisposition, CanonicalIngest, DesiredRelayPolicy, EnvelopeCodec, OutboxKey, RelayClock,
    RelayConnector, RelayManager, RelayManagerConfig, RelayPolicyChange, RelayPortError,
    RelayStateMutation, RelayStatePort, RelayStateQuery, RelayUrl, ResolvedRoute, RouteResolver,
    StableRelayJitter,
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
#[derive(Clone)]
pub struct RelayNodeComponent {
    inner: Arc<RelayNodeInner>,
}

struct RelayNodeInner {
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
            inner: Arc::new(RelayNodeInner {
                config,
                dependencies,
                state,
                manager: Mutex::new(None),
                accepting: AtomicBool::new(false),
            }),
        }
    }

    fn wake(&self) -> Result<(), RelayPortError> {
        self.inner
            .manager
            .lock()
            .map_err(|_| RelayPortError::Unavailable)?
            .as_ref()
            .ok_or(RelayPortError::Unavailable)?
            .wake()
    }

    fn shutdown_manager(&self) -> Result<(), ComponentError> {
        let manager = self
            .inner
            .manager
            .lock()
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
        if self.inner.accepting.load(Ordering::Acquire) {
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
            .field("config", &self.inner.config)
            .field("accepting", &self.inner.accepting.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl NodeComponent for RelayNodeComponent {
    fn start(&mut self, _cancellation: CancellationToken) -> Result<(), ComponentError> {
        let mut manager = self
            .inner
            .manager
            .lock()
            .map_err(|_| ComponentError::unavailable())?;
        if manager.is_some() {
            return Ok(());
        }
        let started = RelayManager::start(
            self.inner.config.manager.clone(),
            self.inner.dependencies.clone(),
        )
        .map_err(|_| ComponentError::unavailable())?;
        *manager = Some(started);
        self.inner.accepting.store(true, Ordering::Release);
        Ok(())
    }

    fn stop_intake(&mut self) -> Result<(), ComponentError> {
        self.inner.accepting.store(false, Ordering::Release);
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
        let relay = relay_url(&request.body.endpoint)?;
        let current = self
            .inner
            .state
            .load_page(RelayStateQuery::first(MAX_RELAY_STATUS_POLICIES + 1))
            .map_err(application_error)?;
        let already_configured = current
            .state
            .policies
            .iter()
            .any(|policy| policy.url == relay);
        if !already_configured && current.state.policies.len() >= MAX_RELAY_STATUS_POLICIES {
            return Ok(EffectOutcome::Rejected(relay_domain_error(
                hq_domain::ErrorCategory::InvalidInput,
                "relay_policy_limit",
            )?));
        }
        let enabled_others = current
            .state
            .policies
            .iter()
            .filter(|policy| policy.enabled && policy.url != relay)
            .count();
        if request.body.enabled && enabled_others >= self.inner.config.manager.max_sessions {
            return Ok(EffectOutcome::Rejected(relay_domain_error(
                hq_domain::ErrorCategory::InvalidInput,
                "relay_session_limit",
            )?));
        }
        self.inner
            .state
            .apply(RelayStateMutation::Configure(RelayPolicyChange {
                operation_id: request.operation_id,
                request_digest: request.request_digest,
                desired: DesiredRelayPolicy {
                    url: relay,
                    access: request.body.access,
                    authentication: request.body.authentication,
                    enabled: request.body.enabled,
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
        if let SynchronizationRequest::Relay(endpoint) = &request.body {
            let relay = relay_url(endpoint)?;
            let page = self
                .inner
                .state
                .load_page(RelayStateQuery::first(MAX_RELAY_STATUS_POLICIES))
                .map_err(application_error)?;
            let Some(policy) = page
                .state
                .policies
                .iter()
                .find(|policy| policy.url == relay)
            else {
                return Ok(EffectOutcome::Rejected(relay_domain_error(
                    hq_domain::ErrorCategory::NotFound,
                    "relay_policy_not_found",
                )?));
            };
            if !policy.enabled {
                return Ok(EffectOutcome::Rejected(relay_domain_error(
                    hq_domain::ErrorCategory::Unresolved,
                    "relay_policy_disabled",
                )?));
            }
        }
        self.wake().map_err(application_error)?;
        Ok(EffectOutcome::Accepted(()))
    }

    fn relay_status(&self) -> Result<RelayStatus, ApplicationError> {
        self.ensure_accepting()?;
        let page = self
            .inner
            .state
            .load_page(RelayStateQuery::first(MAX_RELAY_STATUS_POLICIES))
            .map_err(application_error)?;
        let policies = page
            .state
            .policies
            .into_iter()
            .map(|policy| {
                Ok(RelayPolicyStatus {
                    endpoint: ResourceLocator::new(
                        ResourceScheme::Opaque,
                        BoundedText::<RESOURCE_LOCATOR_MAX_BYTES>::new(policy.url.as_str())
                            .map_err(|_| {
                                ApplicationError::new(ApplicationErrorCode::InvariantViolation)
                            })?,
                    ),
                    access: policy.access,
                    authentication: policy.authentication,
                    enabled: policy.enabled,
                    generation: policy.generation.get(),
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?;
        Ok(RelayStatus {
            policies,
            queued: page.state.outbound.len(),
            prepared: page.state.prepared.len(),
            uncertain: page
                .state
                .attempts
                .iter()
                .filter(|attempt| attempt.disposition == AttemptDisposition::Uncertain)
                .count(),
            rejected: page
                .state
                .attempts
                .iter()
                .filter(|attempt| attempt.disposition == AttemptDisposition::Rejected)
                .count(),
            accepted: page
                .state
                .attempts
                .iter()
                .filter(|attempt| attempt.disposition == AttemptDisposition::Accepted)
                .count(),
            staged: page.state.staged.len(),
            quarantined: page.state.quarantine.len(),
            truncated: page.next.is_some(),
        })
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

fn relay_domain_error(
    category: hq_domain::ErrorCategory,
    code: &'static str,
) -> Result<hq_domain::DomainError, ApplicationError> {
    hq_domain::ErrorCode::new(code)
        .map(|code| hq_domain::DomainError::new(category, code))
        .map_err(|_| ApplicationError::new(ApplicationErrorCode::InvariantViolation))
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
