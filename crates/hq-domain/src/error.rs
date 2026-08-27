//! Structured domain failures independent of presentation text.

use crate::{BoundedText, ValidatedValueError};

/// Maximum stable domain error-code length in UTF-8 bytes.
pub const ERROR_CODE_MAX_BYTES: usize = 96;

/// Stable category suitable for application policy.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ErrorCategory {
    /// Input could not form a valid domain value or command.
    InvalidInput,
    /// Concurrent or repeated input conflicts with retained state.
    Conflict,
    /// Explicit historical authority is absent or unusable.
    Unauthorized,
    /// Required causal data has not arrived or is unusable.
    Unresolved,
    /// A requested aggregate or item does not exist.
    NotFound,
    /// Validated inputs exposed an internal invariant failure.
    InvariantViolation,
}

/// Stable machine-facing error code.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ErrorCode(BoundedText<ERROR_CODE_MAX_BYTES>);

impl ErrorCode {
    /// Validates a stable error code without parsing presentation text.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidatedValueError> {
        BoundedText::new(value).map(Self)
    }

    /// Borrows the stable error code.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// Typed domain failure without transport status or human prose.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainError {
    category: ErrorCategory,
    code: ErrorCode,
}

impl DomainError {
    /// Creates a domain failure from typed parts.
    pub const fn new(category: ErrorCategory, code: ErrorCode) -> Self {
        Self { category, code }
    }

    /// Returns the stable error category.
    pub const fn category(&self) -> ErrorCategory {
        self.category
    }

    /// Returns the stable machine-facing code.
    pub const fn code(&self) -> &ErrorCode {
        &self.code
    }
}
