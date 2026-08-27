//! Explicit semantic time and revision values.

/// Signed Unix timestamp in milliseconds supplied by an outer boundary.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Timestamp(i64);

impl Timestamp {
    /// Constructs a timestamp without consulting an ambient clock.
    pub const fn from_unix_millis(value: i64) -> Self {
        Self(value)
    }

    /// Returns the signed Unix-millisecond value.
    pub const fn as_unix_millis(self) -> i64 {
        self.0
    }
}

/// Monotonic local projection revision supplied by durable coordination.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Revision(u64);

impl Revision {
    /// Creates an explicit revision.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the revision number.
    pub const fn value(self) -> u64 {
        self.0
    }
}
