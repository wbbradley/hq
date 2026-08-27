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

/// Validated, scheme-tagged locator that performs no external observation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ResourceLocator {
    scheme: ResourceScheme,
    value: BoundedText<RESOURCE_LOCATOR_MAX_BYTES>,
}

impl ResourceLocator {
    /// Creates a locator from a typed scheme and validated canonical value.
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

    /// Borrows the opaque canonical locator value.
    pub fn value(&self) -> &str {
        self.value.as_str()
    }
}
