//! Generic command, outcome, pagination, and view envelopes.

use crate::{BoundedText, CommandId, DomainError, Revision, Timestamp, ValidatedValueError};

/// Maximum opaque page cursor length in UTF-8 bytes.
pub const PAGE_CURSOR_MAX_BYTES: usize = 512;

/// Retryable command with explicit identity and caller-supplied time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Command<T> {
    id: CommandId,
    issued_at: Timestamp,
    body: T,
}

impl<T> Command<T> {
    /// Creates a command from typed, explicit inputs.
    pub const fn new(id: CommandId, issued_at: Timestamp, body: T) -> Self {
        Self {
            id,
            issued_at,
            body,
        }
    }

    /// Returns the stable command identity.
    pub const fn id(&self) -> CommandId {
        self.id
    }

    /// Returns the caller-supplied issue time.
    pub const fn issued_at(&self) -> Timestamp {
        self.issued_at
    }

    /// Borrows the command body.
    pub const fn body(&self) -> &T {
        &self.body
    }
}

/// Typed result of attempting a domain command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome<T> {
    /// The command committed and produced a value.
    Committed(T),
    /// The command was rejected by domain policy.
    Rejected(DomainError),
}

/// Opaque continuation cursor owned by an application query.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PageCursor(BoundedText<PAGE_CURSOR_MAX_BYTES>);

impl PageCursor {
    /// Validates an opaque cursor.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidatedValueError> {
        BoundedText::new(value).map(Self)
    }

    /// Borrows the opaque cursor.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// One bounded query result page and its optional continuation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Page<T> {
    items: Vec<T>,
    next_cursor: Option<PageCursor>,
}

impl<T> Page<T> {
    /// Creates a page after the owning query has applied its item limit.
    pub const fn new(items: Vec<T>, next_cursor: Option<PageCursor>) -> Self {
        Self { items, next_cursor }
    }

    /// Borrows the page items.
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Returns the continuation cursor, when more items exist.
    pub const fn next_cursor(&self) -> Option<&PageCursor> {
        self.next_cursor.as_ref()
    }
}

/// Rebuildable view paired with its monotonic local revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionedView<T> {
    revision: Revision,
    value: T,
}

impl<T> VersionedView<T> {
    /// Creates a versioned projection value.
    pub const fn new(revision: Revision, value: T) -> Self {
        Self { revision, value }
    }

    /// Returns the projection revision.
    pub const fn revision(&self) -> Revision {
        self.revision
    }

    /// Borrows the projected value.
    pub const fn value(&self) -> &T {
        &self.value
    }
}
