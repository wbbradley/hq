//! Redacted persistence failures.

use std::{error::Error, fmt};

/// Stable persistence failure classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreErrorClass {
    /// The database path cannot be safely used.
    InvalidPath,
    /// A state or database path is a symbolic link.
    SymbolicLink,
    /// Existing state is accessible beyond the owning operating-system user.
    UnsafePermissions,
    /// A filesystem operation failed.
    FileSystem,
    /// The database belongs to another schema or schema version.
    IncompatibleSchema,
    /// SQLite reported malformed or corrupt database content.
    CorruptDatabase,
    /// Stored signed evidence or its normalized indexes failed verification.
    InvalidEvidence,
    /// One immutable fact identity was reused with unequal evidence or indexes.
    IdentityCollision,
    /// One mutation identity was reused with a different request or retained result.
    MutationConflict,
    /// A bounded operational query or mutation request was invalid.
    InvalidOperationalRequest,
    /// A durable relay transition regressed or reused a stable identity unequally.
    RelayStateConflict,
    /// A durable harness transition regressed or reused a stable identity unequally.
    HarnessStateConflict,
    /// A durable project workflow regressed or reused a stable identity unequally.
    ProjectSagaConflict,
    /// Bounded relay staging cannot accept another exact wrapper.
    RelayStagingFull,
    /// Bounded installation-local mailbox drafts cannot accept another identity.
    MailboxDraftsFull,
    /// The monotonic durable revision reached its maximum representable value.
    RevisionExhausted,
    /// A durable receipt, revision, or outbox row failed strict decoding.
    OperationalStateCorrupt,
    /// The bounded store actor is no longer accepting requests.
    ActorClosed,
    /// The owning store worker stopped without a valid response.
    WorkerStopped,
    /// A database operation failed without establishing corruption.
    DatabaseUnavailable,
    /// Pure complete-batch reduction failed to produce a coherent report.
    ReductionFailed,
    /// No successful repair has materialized the structural index yet.
    NotRepaired,
    /// Rebuildable structural rows are partial, unknown, or inconsistent.
    RebuildableStateCorrupt,
}

/// A redacted persistence failure with a stable classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoreError {
    class: StoreErrorClass,
}

impl StoreError {
    pub(crate) const fn new(class: StoreErrorClass) -> Self {
        Self { class }
    }

    /// Returns the stable failure classification.
    pub const fn class(self) -> StoreErrorClass {
        self.class
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.class {
            StoreErrorClass::InvalidPath => "database path is invalid",
            StoreErrorClass::SymbolicLink => "database path contains a symbolic link",
            StoreErrorClass::UnsafePermissions => "database permissions are unsafe",
            StoreErrorClass::FileSystem => "database filesystem operation failed",
            StoreErrorClass::IncompatibleSchema => "database schema is incompatible",
            StoreErrorClass::CorruptDatabase => "database is corrupt",
            StoreErrorClass::InvalidEvidence => "stored fact evidence is invalid",
            StoreErrorClass::IdentityCollision => "immutable fact identity was reused",
            StoreErrorClass::MutationConflict => {
                "mutation identity was reused with different input"
            }
            StoreErrorClass::InvalidOperationalRequest => "operational store request is invalid",
            StoreErrorClass::RelayStateConflict => "relay state transition conflicts",
            StoreErrorClass::HarnessStateConflict => "harness state transition conflicts",
            StoreErrorClass::ProjectSagaConflict => "project saga transition conflicts",
            StoreErrorClass::RelayStagingFull => "relay staging is full",
            StoreErrorClass::MailboxDraftsFull => "mailbox draft capacity is full",
            StoreErrorClass::RevisionExhausted => "change revision is exhausted",
            StoreErrorClass::OperationalStateCorrupt => "durable operational state is corrupt",
            StoreErrorClass::ActorClosed => "store actor is closed",
            StoreErrorClass::WorkerStopped => "store worker stopped",
            StoreErrorClass::DatabaseUnavailable => "database operation failed",
            StoreErrorClass::ReductionFailed => "complete reduction failed",
            StoreErrorClass::NotRepaired => "rebuildable state has not been repaired",
            StoreErrorClass::RebuildableStateCorrupt => "rebuildable state is corrupt",
        })
    }
}

impl Error for StoreError {}
