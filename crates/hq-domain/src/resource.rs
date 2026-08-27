//! Typed external-resource locators without filesystem access.

use crate::BoundedText;

/// Maximum canonical resource locator length in UTF-8 bytes.
pub const RESOURCE_LOCATOR_MAX_BYTES: usize = 4_096;

/// Semantic kind of external resource.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ResourceScheme {
    /// A canonical Git repository location.
    GitRepository,
    /// A canonical working-tree location.
    WorkingTree,
    /// A container or sandbox identity.
    Container,
    /// An adapter-defined scheme retained as opaque text.
    Opaque,
}

/// Validated, scheme-tagged locator spelling that performs no external observation.
///
/// Whether the value is human-selected or canonical is determined by the containing semantic
/// record. Keeping that distinction explicit prevents an adapter observation from silently
/// replacing durable resource identity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ResourceLocator {
    scheme: ResourceScheme,
    value: BoundedText<RESOURCE_LOCATOR_MAX_BYTES>,
}

impl ResourceLocator {
    /// Creates a locator from a typed scheme and validated value.
    pub const fn new(
        scheme: ResourceScheme,
        value: BoundedText<RESOURCE_LOCATOR_MAX_BYTES>,
    ) -> Self {
        Self { scheme, value }
    }

    /// Returns the semantic resource scheme.
    pub const fn scheme(&self) -> ResourceScheme {
        self.scheme
    }

    /// Borrows the opaque locator value.
    pub fn value(&self) -> &str {
        self.value.as_str()
    }
}
