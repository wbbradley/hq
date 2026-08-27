//! Reusable non-empty text and bounded collection invariants.

use std::{collections::BTreeSet, error::Error, fmt};

/// Construction failures shared by validated domain values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidatedValueError {
    /// A value required at least one item or byte.
    Empty,
    /// A text value exceeded its UTF-8 byte budget.
    TooLong {
        /// Inclusive byte limit.
        maximum: usize,
        /// Actual UTF-8 byte count.
        actual: usize,
    },
    /// A collection exceeded its item budget.
    TooMany {
        /// Inclusive item limit.
        maximum: usize,
        /// Actual item count.
        actual: usize,
    },
    /// A set input repeated an item.
    Duplicate,
    /// An authority reference did not name a declared parent.
    AuthorityNotParent,
    /// More than one authority was supplied for the same semantic role.
    DuplicateAuthorityRole,
}

impl fmt::Display for ValidatedValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("value must not be empty"),
            Self::TooLong { maximum, actual } => {
                write!(formatter, "value has {actual} bytes; maximum is {maximum}")
            }
            Self::TooMany { maximum, actual } => {
                write!(
                    formatter,
                    "collection has {actual} items; maximum is {maximum}"
                )
            }
            Self::Duplicate => formatter.write_str("set input contains a duplicate"),
            Self::AuthorityNotParent => {
                formatter.write_str("authority reference is not a declared parent")
            }
            Self::DuplicateAuthorityRole => {
                formatter.write_str("authority role appears more than once")
            }
        }
    }
}

impl Error for ValidatedValueError {}

/// Owned, non-empty UTF-8 text with a maximum encoded byte length.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BoundedText<const MAXIMUM_BYTES: usize>(String);

impl<const MAXIMUM_BYTES: usize> BoundedText<MAXIMUM_BYTES> {
    /// Validates and owns text without applying hidden normalization.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidatedValueError> {
        let value = value.into();
        let actual = value.len();
        if actual == 0 {
            return Err(ValidatedValueError::Empty);
        }
        if actual > MAXIMUM_BYTES {
            return Err(ValidatedValueError::TooLong {
                maximum: MAXIMUM_BYTES,
                actual,
            });
        }
        Ok(Self(value))
    }

    /// Borrows the validated text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the owned validated text.
    pub fn into_string(self) -> String {
        self.0
    }
}

/// Owned collection with an inclusive maximum item count.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedVec<T, const MAXIMUM_ITEMS: usize>(Vec<T>);

impl<T, const MAXIMUM_ITEMS: usize> BoundedVec<T, MAXIMUM_ITEMS> {
    /// Collects items and rejects an oversized result.
    pub fn new(items: impl IntoIterator<Item = T>) -> Result<Self, ValidatedValueError> {
        let items = items.into_iter().collect::<Vec<_>>();
        let actual = items.len();
        if actual > MAXIMUM_ITEMS {
            return Err(ValidatedValueError::TooMany {
                maximum: MAXIMUM_ITEMS,
                actual,
            });
        }
        Ok(Self(items))
    }

    /// Borrows the validated items.
    pub fn as_slice(&self) -> &[T] {
        &self.0
    }

    /// Returns the owned validated items.
    pub fn into_vec(self) -> Vec<T> {
        self.0
    }
}

/// Owned, non-empty, duplicate-free ordered set with an item limit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NonEmptyBoundedSet<T, const MAXIMUM_ITEMS: usize>(BTreeSet<T>);

/// Owned, duplicate-free ordered set with an item limit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedSet<T, const MAXIMUM_ITEMS: usize>(BTreeSet<T>);

impl<T: Ord, const MAXIMUM_ITEMS: usize> BoundedSet<T, MAXIMUM_ITEMS> {
    /// Collects unique items and rejects duplicate or oversized input.
    pub fn new(items: impl IntoIterator<Item = T>) -> Result<Self, ValidatedValueError> {
        let items = items.into_iter().collect::<Vec<_>>();
        let actual = items.len();
        if actual > MAXIMUM_ITEMS {
            return Err(ValidatedValueError::TooMany {
                maximum: MAXIMUM_ITEMS,
                actual,
            });
        }
        let values = items.into_iter().collect::<BTreeSet<_>>();
        if values.len() != actual {
            return Err(ValidatedValueError::Duplicate);
        }
        Ok(Self(values))
    }

    /// Reports whether the set contains a value.
    pub fn contains(&self, value: &T) -> bool {
        self.0.contains(value)
    }

    /// Iterates in deterministic value order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &T> {
        self.0.iter()
    }
}

impl<T: Ord, const MAXIMUM_ITEMS: usize> NonEmptyBoundedSet<T, MAXIMUM_ITEMS> {
    /// Collects unique items and rejects empty, duplicate, or oversized input.
    pub fn new(items: impl IntoIterator<Item = T>) -> Result<Self, ValidatedValueError> {
        let items = items.into_iter().collect::<Vec<_>>();
        if items.is_empty() {
            return Err(ValidatedValueError::Empty);
        }
        BoundedSet::<_, MAXIMUM_ITEMS>::new(items).map(|set| Self(set.0))
    }

    /// Reports whether the set contains a value.
    pub fn contains(&self, value: &T) -> bool {
        self.0.contains(value)
    }

    /// Iterates in deterministic value order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &T> {
        self.0.iter()
    }
}
