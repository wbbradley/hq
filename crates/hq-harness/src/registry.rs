//! Provider registration and exact session-readiness validation.

use std::{collections::BTreeMap, sync::Arc};

use hq_domain::{ProviderId, ShortText};

use crate::{
    HarnessCapabilities, HarnessCapability, HarnessError, HarnessErrorClass, HarnessFactory,
    HarnessInstanceRequest, HarnessSessionRequest, OpenedHarnessSession,
};

struct RegisteredProvider {
    name: ShortText,
    capabilities: HarnessCapabilities,
    factory: Arc<dyn HarnessFactory>,
}

/// Passive metadata for one registered provider adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredProviderView {
    /// Stable provider namespace.
    pub provider: ProviderId,
    /// User-facing provider name.
    pub name: ShortText,
}

/// Provider-neutral factory registry owned by the node composition root.
#[derive(Default)]
pub struct HarnessRegistry {
    providers: BTreeMap<ProviderId, RegisteredProvider>,
}

impl HarnessRegistry {
    /// Constructs an empty registry.
    pub const fn new() -> Self {
        Self {
            providers: BTreeMap::new(),
        }
    }

    /// Registers one provider namespace after validating safe recovery capabilities.
    pub fn register(
        &mut self,
        provider_id: ProviderId,
        capabilities: HarnessCapabilities,
        factory: Arc<dyn HarnessFactory>,
    ) -> Result<(), HarnessError> {
        let name = ShortText::new(provider_id.as_str())
            .map_err(|_| HarnessError::new(HarnessErrorClass::InvalidInput))?;
        self.register_named(provider_id, name, capabilities, factory)
    }

    /// Registers one named provider for passive presentation and session control.
    pub fn register_named(
        &mut self,
        provider_id: ProviderId,
        name: ShortText,
        capabilities: HarnessCapabilities,
        factory: Arc<dyn HarnessFactory>,
    ) -> Result<(), HarnessError> {
        if !has_safe_submission_recovery(&capabilities) {
            return Err(HarnessError::new(HarnessErrorClass::UnsafeRecovery));
        }
        if self.providers.contains_key(&provider_id) {
            return Err(HarnessError::new(HarnessErrorClass::RegistrationConflict));
        }
        self.providers.insert(
            provider_id,
            RegisteredProvider {
                name,
                capabilities,
                factory,
            },
        );
        Ok(())
    }

    /// Returns stable passive metadata for every registered provider.
    pub fn provider_catalog(&self) -> Vec<RegisteredProviderView> {
        self.providers
            .iter()
            .map(|(provider, registration)| RegisteredProviderView {
                provider: provider.clone(),
                name: registration.name.clone(),
            })
            .collect()
    }

    /// Returns the declared capabilities for one registered provider.
    pub fn capabilities(&self, provider_id: &ProviderId) -> Option<&HarnessCapabilities> {
        self.providers
            .get(provider_id)
            .map(|provider| &provider.capabilities)
    }

    /// Creates one logical instance and validates exact new/resumed readiness.
    pub fn open_session(
        &self,
        provider_id: &ProviderId,
        instance_request: HarnessInstanceRequest,
        session_request: HarnessSessionRequest,
    ) -> Result<OpenedHarnessSession, HarnessError> {
        let provider = self
            .providers
            .get(provider_id)
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::ProviderNotRegistered))?;
        validate_session_capability(&provider.capabilities, &session_request)?;
        let requested_resume = match &session_request {
            HarnessSessionRequest::Start => None,
            HarnessSessionRequest::Resume { session_id } => Some(session_id.clone()),
        };
        let instance = provider.factory.create_instance(instance_request)?;
        let mut opened = instance.open_session(session_request)?;
        if requested_resume
            .as_ref()
            .is_some_and(|requested| requested != &opened.session_id)
        {
            opened
                .session
                .force_stop()
                .map_err(|_| HarnessError::new(HarnessErrorClass::CleanupFailed))?;
            return Err(HarnessError::new(
                HarnessErrorClass::SessionIdentityMismatch,
            ));
        }
        Ok(opened)
    }
}

fn validate_session_capability(
    capabilities: &HarnessCapabilities,
    request: &HarnessSessionRequest,
) -> Result<(), HarnessError> {
    let supported = match request {
        HarnessSessionRequest::Start => HarnessCapability::StartSessions,
        HarnessSessionRequest::Resume { .. } => HarnessCapability::ResumeSessions,
    };
    if capabilities.supported.contains(&supported) {
        Ok(())
    } else {
        Err(HarnessError::new(HarnessErrorClass::Unsupported))
    }
}

fn has_safe_submission_recovery(capabilities: &HarnessCapabilities) -> bool {
    capabilities
        .supported
        .contains(&HarnessCapability::StableSubmissionIdempotency)
        || capabilities
            .supported
            .contains(&HarnessCapability::SubmissionLookup)
}
