//! Provider/session and operation correlation values.

use crate::{BoundedText, OperationId, ValidatedValueError};

/// Maximum provider namespace length in UTF-8 bytes.
pub const PROVIDER_ID_MAX_BYTES: usize = 64;
/// Maximum provider session identity length in UTF-8 bytes.
pub const PROVIDER_SESSION_ID_MAX_BYTES: usize = 256;

/// Provider namespace, independent of any specific adapter vocabulary.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProviderId(BoundedText<PROVIDER_ID_MAX_BYTES>);

impl ProviderId {
    /// Validates a provider namespace.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidatedValueError> {
        BoundedText::new(value).map(Self)
    }

    /// Borrows the provider namespace.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// Session identity scoped by a provider namespace.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProviderSessionId(BoundedText<PROVIDER_SESSION_ID_MAX_BYTES>);

impl ProviderSessionId {
    /// Validates a provider session identity.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidatedValueError> {
        BoundedText::new(value).map(Self)
    }

    /// Borrows the provider session identity.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// Typed correlation for one provider operation.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct OperationCorrelation {
    provider: ProviderId,
    session: ProviderSessionId,
    operation: OperationId,
}

impl OperationCorrelation {
    /// Creates correlation from already validated components.
    pub const fn new(
        provider: ProviderId,
        session: ProviderSessionId,
        operation: OperationId,
    ) -> Self {
        Self {
            provider,
            session,
            operation,
        }
    }

    /// Returns the provider namespace.
    pub const fn provider(&self) -> &ProviderId {
        &self.provider
    }

    /// Returns the provider-scoped session identity.
    pub const fn session(&self) -> &ProviderSessionId {
        &self.session
    }

    /// Returns the operation identity.
    pub const fn operation(&self) -> OperationId {
        self.operation
    }
}
