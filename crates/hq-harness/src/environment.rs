//! Memory-only copied provider launch environment with redacted diagnostics.

use std::fmt;

use crate::{HarnessError, HarnessErrorClass};

/// Maximum copied environment entries in one launch template.
pub const MAX_HARNESS_ENVIRONMENT_ENTRIES: usize = 512;
/// Maximum UTF-8 bytes in one environment name.
pub const MAX_HARNESS_ENVIRONMENT_NAME_BYTES: usize = 256;
/// Maximum bytes in one environment value.
pub const MAX_HARNESS_ENVIRONMENT_VALUE_BYTES: usize = 32_768;
/// Maximum aggregate bytes across copied names and values.
pub const MAX_HARNESS_ENVIRONMENT_BYTES: usize = 1_048_576;

struct SecretEnvironmentEntry {
    name: String,
    value: Vec<u8>,
}

impl Drop for SecretEnvironmentEntry {
    fn drop(&mut self) {
        self.value.fill(0);
    }
}

/// Opaque memory-only launch environment whose values never appear in `Debug`.
#[derive(Default)]
pub struct HarnessEnvironment {
    entries: Vec<SecretEnvironmentEntry>,
}

impl HarnessEnvironment {
    /// Copies and validates a complete environment at the control boundary.
    pub fn copy_from<'a>(
        entries: impl IntoIterator<Item = (&'a str, &'a [u8])>,
    ) -> Result<Self, HarnessError> {
        let mut copied = Vec::new();
        let mut total = 0_usize;
        for (name, value) in entries {
            if copied.len() == MAX_HARNESS_ENVIRONMENT_ENTRIES
                || name.is_empty()
                || name.len() > MAX_HARNESS_ENVIRONMENT_NAME_BYTES
                || name.as_bytes().contains(&0)
                || name.as_bytes().contains(&b'=')
                || value.len() > MAX_HARNESS_ENVIRONMENT_VALUE_BYTES
                || value.contains(&0)
            {
                return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
            }
            total = total
                .checked_add(name.len())
                .and_then(|size| size.checked_add(value.len()))
                .ok_or_else(|| HarnessError::new(HarnessErrorClass::InvalidInput))?;
            if total > MAX_HARNESS_ENVIRONMENT_BYTES {
                return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
            }
            copied.push(SecretEnvironmentEntry {
                name: name.to_owned(),
                value: value.to_vec(),
            });
        }
        copied.sort_by(|left, right| left.name.cmp(&right.name));
        if copied.windows(2).any(|pair| pair[0].name == pair[1].name) {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        }
        Ok(Self { entries: copied })
    }

    /// Visits copied names and values without transferring or retaining ownership.
    pub fn visit(&self, mut visitor: impl FnMut(&str, &[u8])) {
        for entry in &self.entries {
            visitor(&entry.name, &entry.value);
        }
    }

    /// Creates another independently owned redacted copy for a retained launch template.
    pub fn try_copy(&self) -> Result<Self, HarnessError> {
        Self::copy_from(
            self.entries
                .iter()
                .map(|entry| (entry.name.as_str(), entry.value.as_slice())),
        )
    }

    /// Returns the number of copied entries without exposing values.
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Reports whether the copied environment is empty.
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl fmt::Debug for HarnessEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HarnessEnvironment")
            .field("entry_count", &self.entries.len())
            .finish_non_exhaustive()
    }
}
