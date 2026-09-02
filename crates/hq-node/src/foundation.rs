//! RAII ownership of the state lock, identity, runtime namespace, and bounded store.

use std::{error::Error, fmt, num::NonZeroUsize, sync::Arc};

use hq_application::{
    CommitFacts, FactMutation, FactPlan, MutationAttempt, MutationDecision, MutationOutcome,
    QueryDomain,
};
use hq_domain::{
    BoundedSet, CausalReferences, CommandDigest, CommandId, EncryptionPublicKey, FactScope,
    MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, SemanticPayload, SigningPublicKey, Timestamp,
};
use hq_protocol::Bip340Signer;
use hq_reducer::{AuthorityPolicy, AuthorityProjection, AuthorityProjectionKey};
use hq_relay::RelayConnector;
use hq_store::{RevisionInvalidations, Store, StoreErrorClass, StoreGateway};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::{
    AcceptedLocalStream, ReadinessRecord, RuntimeArtifactError,
    local_transport::{BoundLocalListener, LocalTransportOwner},
};
use crate::{
    IdentityErrorClass, InstallationIdentity, LocalConfiguration, NodeAdmission, NodeLifecycle,
    NodeLifecycleError, NodeTransitionOutcome, RelayNodeComponent, RelayNodeConfig,
    RuntimeDirectoryOwner, RuntimePathErrorClass, RuntimePaths, StartupCause, StartupComponent,
    StartupDiagnostic, StateDirectoryOwner, StatePaths,
};

/// Explicit inputs for opening the node foundation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeFoundationConfig {
    state: StatePaths,
    runtime: RuntimePaths,
    store_capacity: NonZeroUsize,
}

impl NodeFoundationConfig {
    /// Constructs one explicit, deterministic foundation configuration.
    pub const fn new(
        state: StatePaths,
        runtime: RuntimePaths,
        store_capacity: NonZeroUsize,
    ) -> Self {
        Self {
            state,
            runtime,
            store_capacity,
        }
    }
}

/// Redacted node startup failure with actionable structured context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeStartupError {
    diagnostic: StartupDiagnostic,
}

impl NodeStartupError {
    /// Returns the stable structured diagnostic.
    pub const fn diagnostic(&self) -> &StartupDiagnostic {
        &self.diagnostic
    }
}

impl fmt::Display for NodeStartupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "node startup failed in {:?}: {:?}; action: {:?}",
            self.diagnostic.component(),
            self.diagnostic.cause(),
            self.diagnostic.action()
        )
    }
}

impl Error for NodeStartupError {}

/// Checked shutdown failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeShutdownError {
    /// The lifecycle was already in an invalid transition state.
    Lifecycle,
    /// The store worker did not acknowledge and join cleanly.
    Store,
    /// Listener or readiness cleanup detected failure or path substitution.
    Runtime,
}

impl fmt::Display for NodeShutdownError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Lifecycle => "node shutdown lifecycle is invalid",
            Self::Store => "node store did not shut down cleanly",
            Self::Runtime => "node runtime artifacts did not shut down cleanly",
        })
    }
}

impl Error for NodeShutdownError {}

/// Readiness failure separated from startup ownership and lifecycle-order failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NodeReadinessError {
    /// The store could not provide its authoritative serialized revision.
    Store(NodeStartupError),
    /// Readiness was acknowledged outside the `Starting` phase.
    Lifecycle(NodeLifecycleError),
}

impl fmt::Display for NodeReadinessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Lifecycle(error) => error.fmt(formatter),
        }
    }
}

impl Error for NodeReadinessError {}

/// Sole owner of node foundations required before component startup.
pub struct NodeFoundation {
    lifecycle: NodeLifecycle,
    store: Option<Store>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    local_transport: LocalTransportOwner,
    runtime: RuntimeDirectoryOwner,
    configuration: LocalConfiguration,
    identity: InstallationIdentity,
    state: StateDirectoryOwner,
}

impl NodeFoundation {
    /// Opens every foundation in dependency order and unwinds automatically on any failure.
    pub fn open(config: NodeFoundationConfig) -> Result<Self, NodeStartupError> {
        let state_root = config.state.root().to_path_buf();
        let runtime_root = config.runtime.root().to_path_buf();
        let diagnostic = |component, cause| NodeStartupError {
            diagnostic: StartupDiagnostic::new(
                component,
                cause,
                state_root.clone(),
                runtime_root.clone(),
            ),
        };

        let state = StateDirectoryOwner::acquire(config.state.clone()).map_err(|error| {
            diagnostic(
                StartupComponent::StateOwnership,
                identity_cause(error.class()),
            )
        })?;
        let identity = state.load_identity().map_err(|error| {
            diagnostic(StartupComponent::Identity, identity_cause(error.class()))
        })?;
        let configuration = state.load_configuration().map_err(|error| {
            diagnostic(
                StartupComponent::Configuration,
                identity_cause(error.class()),
            )
        })?;
        let runtime = RuntimeDirectoryOwner::prepare(config.runtime)
            .map_err(|error| diagnostic(StartupComponent::Runtime, runtime_cause(error.class())))?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let local_transport = LocalTransportOwner::new(runtime.paths().clone());
        let store = Store::open(state.paths().database_file(), config.store_capacity)
            .map_err(|error| diagnostic(StartupComponent::Store, store_cause(error.class())))?;

        Ok(Self {
            lifecycle: NodeLifecycle::new(),
            store: Some(store),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            local_transport,
            runtime,
            configuration,
            identity,
            state,
        })
    }

    /// Returns the pure lifecycle owner.
    pub const fn lifecycle(&self) -> &NodeLifecycle {
        &self.lifecycle
    }

    /// Returns safe identity metadata without exposing signer secret bytes.
    pub fn public_identity(&self) -> crate::PublicIdentity {
        self.identity.public_identity()
    }

    /// Returns installation-local unsigned defaults.
    pub const fn configuration(&self) -> &LocalConfiguration {
        &self.configuration
    }

    /// Returns the validated runtime layout.
    pub const fn runtime_paths(&self) -> &RuntimePaths {
        self.runtime.paths()
    }

    /// Borrows the sole bounded store owner for later composition.
    pub const fn store(&self) -> Option<&Store> {
        self.store.as_ref()
    }

    /// Composes relay synchronization while the foundation retains store and identity ownership.
    pub(crate) fn compose_relay(
        &self,
        config: RelayNodeConfig,
        authority_policy: AuthorityPolicy,
        connector: Arc<dyn RelayConnector>,
        trace: crate::BoundaryTrace,
    ) -> Result<RelayNodeComponent, NodeStartupError> {
        let store = self.store.as_ref().ok_or_else(|| NodeStartupError {
            diagnostic: StartupDiagnostic::new(
                StartupComponent::Store,
                StartupCause::Unavailable,
                self.state.paths().root().to_path_buf(),
                self.runtime.paths().root().to_path_buf(),
            ),
        })?;
        let envelope = self
            .identity
            .envelope_codec()
            .map_err(|error| NodeStartupError {
                diagnostic: StartupDiagnostic::new(
                    StartupComponent::Identity,
                    identity_cause(error.class()),
                    self.state.paths().root().to_path_buf(),
                    self.runtime.paths().root().to_path_buf(),
                ),
            })?;
        Ok(RelayNodeComponent::new_with_trace(
            config,
            store,
            envelope,
            self.identity.public_identity().installation_id,
            authority_policy,
            connector,
            trace,
        ))
    }

    pub(crate) fn signer_handle(&self) -> std::sync::Arc<Bip340Signer> {
        self.identity.signer_handle()
    }

    /// Reconciles the one canonical root fact for the exclusively owned installation.
    pub(crate) fn bootstrap_installation(
        &self,
        policy: AuthorityPolicy,
    ) -> Result<(), NodeStartupError> {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| self.store_diagnostic(StartupCause::Unavailable))?;
        let revision = store
            .current_revision()
            .map_err(|error| self.store_diagnostic(store_cause(error.class())))?;
        let public = self.public_identity();
        let gateway = StoreGateway::new(store, policy, self.signer_handle());
        if revision.value() > 0 {
            return verify_installation_projection(&gateway, &public)
                .map_err(|cause| self.store_diagnostic(cause));
        }

        let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
            BoundedSet::new([]).map_err(|_| self.store_diagnostic(StartupCause::Malformed))?,
            [],
        )
        .map_err(|_| self.store_diagnostic(StartupCause::Malformed))?;
        let mut auxiliary_randomness = [0_u8; 32];
        getrandom::fill(&mut auxiliary_randomness)
            .map_err(|_| self.store_diagnostic(StartupCause::Unavailable))?;
        let installation = public.installation_id;
        let signing_key = SigningPublicKey::from_bytes(public.signing_public_key);
        let plan = FactPlan::new(
            installation,
            Timestamp::from_unix_millis(0),
            FactScope::InstallationPrivate(installation),
            causal,
            SemanticPayload::InstallationDeclared {
                installation_id: installation,
                signing_key,
                encryption_key: EncryptionPublicKey::from_bytes(public.signing_public_key),
                label: None,
            },
            auxiliary_randomness,
        );
        let attempt = gateway
            .commit_facts(FactMutation::new(
                CommandId::from_bytes(*installation.as_bytes()),
                CommandDigest::from_bytes(public.signing_public_key),
                move |_| MutationDecision::commit(plan),
            ))
            .map_err(|error| self.store_diagnostic(application_cause(error.class())))?;
        if !matches!(
            attempt,
            MutationAttempt::Completed(ref receipt)
                if matches!(receipt.outcome(), MutationOutcome::Committed)
        ) {
            return Err(self.store_diagnostic(StartupCause::Unavailable));
        }
        verify_installation_projection(&gateway, &public)
            .map_err(|cause| self.store_diagnostic(cause))
    }

    /// Binds and retains the private nonblocking local listener while state ownership is live.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn bind_local_listener(&mut self) -> Result<(), RuntimeArtifactError> {
        self.local_transport.bind()
    }

    /// Accepts one waiting connection and validates same-user kernel credentials.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn accept_local(&self) -> Result<AcceptedLocalStream, RuntimeArtifactError> {
        self.local_transport.accept()
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(crate) fn take_local_listener(
        &mut self,
    ) -> Result<BoundLocalListener, RuntimeArtifactError> {
        self.local_transport.take_listener()
    }

    /// Creates an independent coalesced post-commit observer for runtime ownership.
    pub(crate) fn subscribe_store_invalidations(&self) -> Option<RevisionInvalidations> {
        self.store.as_ref().map(Store::subscribe_invalidations)
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(crate) fn publish_readiness(
        &mut self,
        record: &ReadinessRecord,
    ) -> Result<(), RuntimeArtifactError> {
        self.local_transport.publish(record)
    }

    /// Acknowledges store-backed readiness at the serialized current revision.
    pub fn mark_ready(&mut self) -> Result<NodeTransitionOutcome, NodeReadinessError> {
        let Some(store) = self.store() else {
            return Err(NodeReadinessError::Store(self.lifecycle_error()));
        };
        let revision = store.current_revision().map_err(|error| {
            NodeReadinessError::Store(NodeStartupError {
                diagnostic: StartupDiagnostic::new(
                    StartupComponent::Store,
                    store_cause(error.class()),
                    self.state.paths().root().to_path_buf(),
                    self.runtime_paths().root().to_path_buf(),
                ),
            })
        })?;
        self.lifecycle
            .mark_ready(revision.value())
            .map_err(NodeReadinessError::Lifecycle)
    }

    /// Evaluates whether current lifecycle policy accepts one operation family.
    pub fn admits(&self, admission: NodeAdmission) -> bool {
        self.lifecycle.admits(admission)
    }

    /// Closes side-effecting intake before ordered component drain.
    pub fn begin_drain(&mut self) -> Result<NodeTransitionOutcome, NodeLifecycleError> {
        self.lifecycle.begin_drain()
    }

    /// Closes intake while retaining explicit clean-restart intent.
    pub fn begin_restart(&mut self) -> Result<NodeTransitionOutcome, NodeLifecycleError> {
        self.lifecycle.begin_restart()
    }

    /// Closes the store before releasing runtime, identity, and state ownership.
    pub fn shutdown(mut self) -> Result<(), NodeShutdownError> {
        let lifecycle = self
            .lifecycle
            .begin_drain()
            .map(|_| ())
            .map_err(|_| NodeShutdownError::Lifecycle);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let runtime = self
            .local_transport
            .cleanup()
            .map_err(|_| NodeShutdownError::Runtime);
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let runtime = Ok(());
        let store = self.close_store();
        let stopped = self
            .lifecycle
            .acknowledge_stopped()
            .map(|_| ())
            .map_err(|_| NodeShutdownError::Lifecycle);
        lifecycle.and(runtime).and(store).and(stopped)
    }

    fn close_store(&mut self) -> Result<(), NodeShutdownError> {
        if let Some(store) = self.store.take() {
            store.close().map_err(|_| NodeShutdownError::Store)?;
        }
        Ok(())
    }

    fn lifecycle_error(&self) -> NodeStartupError {
        NodeStartupError {
            diagnostic: StartupDiagnostic::new(
                StartupComponent::Store,
                StartupCause::Unavailable,
                self.state.paths().root().to_path_buf(),
                self.runtime_paths().root().to_path_buf(),
            ),
        }
    }

    fn store_diagnostic(&self, cause: StartupCause) -> NodeStartupError {
        NodeStartupError {
            diagnostic: StartupDiagnostic::new(
                StartupComponent::Store,
                cause,
                self.state.paths().root().to_path_buf(),
                self.runtime.paths().root().to_path_buf(),
            ),
        }
    }
}

impl Drop for NodeFoundation {
    fn drop(&mut self) {
        let _ = self.lifecycle.begin_drain();
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let _ = self.local_transport.cleanup();
        let _ = self.close_store();
        let _ = self.lifecycle.acknowledge_stopped();
    }
}

impl fmt::Debug for NodeFoundation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NodeFoundation")
            .field("phase", &self.lifecycle.phase())
            .field("public_identity", &self.public_identity())
            .finish_non_exhaustive()
    }
}

const fn identity_cause(class: IdentityErrorClass) -> StartupCause {
    match class {
        IdentityErrorClass::PathUnavailable | IdentityErrorClass::InvalidPath => {
            StartupCause::InvalidPath
        }
        IdentityErrorClass::UnsafePermissions => StartupCause::UnsafePermissions,
        IdentityErrorClass::SymbolicLink => StartupCause::SymbolicLink,
        IdentityErrorClass::AlreadyOwned => StartupCause::AlreadyOwned,
        IdentityErrorClass::IdentityMissing => StartupCause::Missing,
        IdentityErrorClass::IdentityMalformed
        | IdentityErrorClass::ConfigurationMalformed
        | IdentityErrorClass::ConfigurationInvalid
        | IdentityErrorClass::IdentityExists
        | IdentityErrorClass::BackupMalformed
        | IdentityErrorClass::BackupAuthenticationFailed => StartupCause::Malformed,
        IdentityErrorClass::FileSystem
        | IdentityErrorClass::EntropyUnavailable
        | IdentityErrorClass::PasswordInvalid
        | IdentityErrorClass::BackupExists => StartupCause::Unavailable,
    }
}

const fn runtime_cause(class: RuntimePathErrorClass) -> StartupCause {
    match class {
        RuntimePathErrorClass::InvalidPath | RuntimePathErrorClass::SocketPathTooLong => {
            StartupCause::InvalidPath
        }
        RuntimePathErrorClass::SymbolicLink => StartupCause::SymbolicLink,
        RuntimePathErrorClass::UnsafePermissions => StartupCause::UnsafePermissions,
        RuntimePathErrorClass::FileSystem => StartupCause::Unavailable,
    }
}

const fn store_cause(class: StoreErrorClass) -> StartupCause {
    match class {
        StoreErrorClass::InvalidPath => StartupCause::InvalidPath,
        StoreErrorClass::SymbolicLink => StartupCause::SymbolicLink,
        StoreErrorClass::UnsafePermissions => StartupCause::UnsafePermissions,
        StoreErrorClass::IncompatibleSchema => StartupCause::Incompatible,
        StoreErrorClass::CorruptDatabase
        | StoreErrorClass::InvalidEvidence
        | StoreErrorClass::IdentityCollision
        | StoreErrorClass::MutationConflict
        | StoreErrorClass::InvalidOperationalRequest
        | StoreErrorClass::RelayStateConflict
        | StoreErrorClass::HarnessStateConflict
        | StoreErrorClass::ProjectSagaConflict
        | StoreErrorClass::RevisionExhausted
        | StoreErrorClass::OperationalStateCorrupt
        | StoreErrorClass::ReductionFailed
        | StoreErrorClass::NotRepaired
        | StoreErrorClass::RebuildableStateCorrupt => StartupCause::Malformed,
        StoreErrorClass::FileSystem
        | StoreErrorClass::RelayStagingFull
        | StoreErrorClass::MailboxDraftsFull
        | StoreErrorClass::ActorClosed
        | StoreErrorClass::WorkerStopped
        | StoreErrorClass::DatabaseUnavailable => StartupCause::Unavailable,
    }
}

fn verify_installation_projection(
    gateway: &StoreGateway,
    public: &crate::PublicIdentity,
) -> Result<(), StartupCause> {
    let snapshot = gateway
        .authoritative_snapshot()
        .map_err(|error| application_cause(error.class()))?;
    match snapshot
        .domain()
        .authority()
        .projection(AuthorityProjectionKey::Installation(public.installation_id))
    {
        Some(AuthorityProjection::Installation(view))
            if view.signing_key == SigningPublicKey::from_bytes(public.signing_public_key)
                && view.encryption_key
                    == EncryptionPublicKey::from_bytes(public.signing_public_key) =>
        {
            Ok(())
        }
        Some(
            AuthorityProjection::Installation(_)
            | AuthorityProjection::Mailbox(_)
            | AuthorityProjection::PeerRoute(_)
            | AuthorityProjection::MailboxCapability(_)
            | AuthorityProjection::Account { .. }
            | AuthorityProjection::Membership(_)
            | AuthorityProjection::AccountSelection { .. },
        )
        | None => Err(StartupCause::Malformed),
    }
}

const fn application_cause(class: hq_application::ApplicationErrorClass) -> StartupCause {
    match class {
        hq_application::ApplicationErrorClass::Unavailable
        | hq_application::ApplicationErrorClass::Capacity => StartupCause::Unavailable,
        hq_application::ApplicationErrorClass::InvalidInput
        | hq_application::ApplicationErrorClass::Conflict
        | hq_application::ApplicationErrorClass::Unauthorized
        | hq_application::ApplicationErrorClass::Unresolved
        | hq_application::ApplicationErrorClass::NotFound
        | hq_application::ApplicationErrorClass::CorruptState
        | hq_application::ApplicationErrorClass::InvariantViolation => StartupCause::Malformed,
    }
}
