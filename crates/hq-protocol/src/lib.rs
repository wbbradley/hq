//! Strict protocol transitions into verified domain values.

use std::{error::Error, fmt};

use hq_domain::{BoundedText, Fact, FactId, SKELETON_PAYLOAD_MAX_BYTES};

/// A pre-serialization frame used only by the workspace walking skeleton.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InMemoryFrame {
    fact_id: u64,
    payload: String,
}

impl InMemoryFrame {
    /// Creates a frame at the untrusted side of the protocol boundary.
    pub fn new(fact_id: u64, payload: impl Into<String>) -> Self {
        Self {
            fact_id,
            payload: payload.into(),
        }
    }

    /// Validates the frame and converts it into a domain fact.
    pub fn decode(self) -> Result<Fact, DecodeError> {
        if self.payload.is_empty() {
            return Err(DecodeError::EmptyPayload);
        }

        let payload = BoundedText::<SKELETON_PAYLOAD_MAX_BYTES>::new(self.payload)
            .map_err(|_| DecodeError::PayloadTooLong)?;
        let mut id_bytes = [0; 32];
        id_bytes[24..].copy_from_slice(&self.fact_id.to_be_bytes());
        Ok(Fact::new(FactId::from_bytes(id_bytes), payload))
    }
}

/// Validation failures at the in-memory protocol boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    /// The frame contained no semantic payload.
    EmptyPayload,
    /// The frame exceeded the walking-skeleton payload limit.
    PayloadTooLong,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPayload => formatter.write_str("fact payload is empty"),
            Self::PayloadTooLong => formatter.write_str("fact payload is too long"),
        }
    }
}

impl Error for DecodeError {}
