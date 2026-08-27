//! Pure domain values and policies.

/// Stable identity used by the in-memory workspace skeleton.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FactId(u64);

impl FactId {
    /// Creates an in-memory fact identity.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the numeric skeleton representation.
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// A verified semantic fact used by the in-memory workspace skeleton.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fact {
    id: FactId,
    payload: String,
}

impl Fact {
    /// Creates a fact after an outer boundary has validated its input.
    pub fn new(id: FactId, payload: impl Into<String>) -> Self {
        Self {
            id,
            payload: payload.into(),
        }
    }

    /// Returns the fact identity.
    pub const fn id(&self) -> FactId {
        self.id
    }

    /// Returns the semantic payload used by the skeleton.
    pub fn payload(&self) -> &str {
        &self.payload
    }
}
