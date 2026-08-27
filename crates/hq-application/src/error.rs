//! Stable application and value failure classifications.

use std::{error::Error, fmt};

/// Transport-independent class of an application boundary failure.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ApplicationErrorClass {
    /// The request is malformed or outside a documented bound.
    InvalidInput,
    /// A stable identity was reused for different input or current state conflicts.
    Conflict,
    /// The caller lacks required historical authority.
    Unauthorized,
    /// Required causal or external state is not currently available.
    Unresolved,
    /// The requested item does not exist.
    NotFound,
    /// A bounded intake cannot currently accept more work.
    Capacity,
    /// An adapter or owned worker cannot currently serve the operation.
    Unavailable,
    /// Durable or adapter state failed strict validation.
    CorruptState,
    /// An internal semantic invariant was violated.
    InvariantViolation,
}

/// Stable machine-facing application failure code.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ApplicationErrorCode {
    /// A request exceeded a documented bound or failed structural validation.
    InvalidRequest,
    /// One command identity was reused with a different request digest.
    CommandIdentityConflict,
    /// One immutable semantic identity collided with unequal retained state.
    StateIdentityConflict,
    /// Historical authorization rejected the operation.
    AuthorityRejected,
    /// Required causal information is absent or unusable.
    CausalStateUnavailable,
    /// A requested semantic item does not exist.
    ItemNotFound,
    /// A bounded mailbox or queue has no available capacity.
    IntakeFull,
    /// An adapter or its owned worker is unavailable.
    AdapterUnavailable,
    /// Retained state failed strict decoding or verification.
    StateCorrupt,
    /// Pure application or reducer invariants failed.
    InvariantViolation,
}

impl ApplicationErrorCode {
    /// Returns the one stable class associated with this code.
    pub const fn class(self) -> ApplicationErrorClass {
        match self {
            Self::InvalidRequest => ApplicationErrorClass::InvalidInput,
            Self::CommandIdentityConflict | Self::StateIdentityConflict => {
                ApplicationErrorClass::Conflict
            }
            Self::AuthorityRejected => ApplicationErrorClass::Unauthorized,
            Self::CausalStateUnavailable => ApplicationErrorClass::Unresolved,
            Self::ItemNotFound => ApplicationErrorClass::NotFound,
            Self::IntakeFull => ApplicationErrorClass::Capacity,
            Self::AdapterUnavailable => ApplicationErrorClass::Unavailable,
            Self::StateCorrupt => ApplicationErrorClass::CorruptState,
            Self::InvariantViolation => ApplicationErrorClass::InvariantViolation,
        }
    }

    /// Returns the stable snake-case code used by later protocol adapters.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::CommandIdentityConflict => "command_identity_conflict",
            Self::StateIdentityConflict => "state_identity_conflict",
            Self::AuthorityRejected => "authority_rejected",
            Self::CausalStateUnavailable => "causal_state_unavailable",
            Self::ItemNotFound => "item_not_found",
            Self::IntakeFull => "intake_full",
            Self::AdapterUnavailable => "adapter_unavailable",
            Self::StateCorrupt => "state_corrupt",
            Self::InvariantViolation => "invariant_violation",
        }
    }
}

/// Redacted typed application failure independent of adapter and transport details.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApplicationError {
    code: ApplicationErrorCode,
}

impl ApplicationError {
    /// Constructs a typed failure from its closed stable code.
    pub const fn new(code: ApplicationErrorCode) -> Self {
        Self { code }
    }

    /// Returns the stable error class.
    pub const fn class(self) -> ApplicationErrorClass {
        self.code.class()
    }

    /// Returns the stable machine-facing code.
    pub const fn code(self) -> ApplicationErrorCode {
        self.code
    }
}

impl fmt::Display for ApplicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code.as_str())
    }
}

impl Error for ApplicationError {}

/// Failure to construct a bounded application-owned value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplicationValueError {
    /// A collection that requires at least one item was empty.
    Empty,
    /// A collection exceeded its inclusive item limit.
    TooManyItems {
        /// Inclusive item limit.
        maximum: usize,
        /// Actual item count.
        actual: usize,
    },
    /// Versioned application bytes were malformed, unknown, or non-canonical.
    InvalidEncoding,
}

impl fmt::Display for ApplicationValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("application value must not be empty"),
            Self::TooManyItems { maximum, actual } => {
                write!(
                    formatter,
                    "application value has {actual} items; maximum is {maximum}"
                )
            }
            Self::InvalidEncoding => formatter.write_str("application encoding is invalid"),
        }
    }
}

impl Error for ApplicationValueError {}
