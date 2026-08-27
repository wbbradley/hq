//! Strict owned v1 DTO verification and deterministic encoding.

mod decode;
pub(crate) mod model;
mod semantic;

use std::fmt;

use decode::decode_content;
use model::ContentDto;

pub use semantic::VerifiedSemanticFact;

use crate::{
    CryptographicallyVerifiedEvent, FailureClass, MAX_CONTENT_BYTES, ProtocolError,
    ProtocolNamespace, SupportedContentBytes, json::validate_content_json,
};

/// Cryptographically verified content whose complete canonical v1 DTO is valid.
pub struct VerifiedSupportedRecord {
    supported: SupportedContentBytes,
    dto: ContentDto,
}

impl VerifiedSupportedRecord {
    /// Returns the independently versioned content namespace.
    pub const fn namespace(&self) -> ProtocolNamespace {
        self.supported.namespace()
    }

    /// Returns the exact supported v1 family number.
    pub const fn family(&self) -> u64 {
        self.supported.family()
    }

    /// Returns the exact retained cryptographically verified content bytes.
    pub fn content_bytes(&self) -> &[u8] {
        self.supported.content_bytes()
    }

    /// Returns the exact verified event evidence retained across DTO conversion.
    pub const fn verified_event(&self) -> &CryptographicallyVerifiedEvent {
        self.supported.verified_event()
    }

    /// Deterministically encodes the owned DTO in canonical v1 form.
    pub fn encode_content(&self) -> Result<String, ProtocolError> {
        let bytes = encode_dto(&self.dto)?;
        String::from_utf8(bytes).map_err(|_| ProtocolError::new(FailureClass::ContentMalformed))
    }

    /// Converts the complete verified DTO into its exhaustive semantic-domain representation.
    ///
    /// Prefix-only and verified-unsupported trust states deliberately cannot perform this
    /// transition:
    ///
    /// ```compile_fail
    /// use hq_protocol::SupportedContentBytes;
    ///
    /// fn bypass(prefix: SupportedContentBytes) {
    ///     let _ = prefix.into_semantic_fact();
    /// }
    /// ```
    ///
    /// ```compile_fail
    /// use hq_protocol::VerifiedUnsupportedRecord;
    ///
    /// fn bypass(unsupported: VerifiedUnsupportedRecord) {
    ///     let _ = unsupported.into_semantic_fact();
    /// }
    /// ```
    pub fn into_semantic_fact(self) -> Result<VerifiedSemanticFact, ProtocolError> {
        semantic::convert(self)
    }
}

impl fmt::Debug for VerifiedSupportedRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedSupportedRecord")
            .field("namespace", &self.namespace())
            .field("family", &self.family())
            .field("event", self.verified_event())
            .finish_non_exhaustive()
    }
}

impl SupportedContentBytes {
    /// Strictly decodes and canonically re-encodes the complete supported v1 DTO.
    pub fn decode_v1(self) -> Result<VerifiedSupportedRecord, ProtocolError> {
        validate_content_json(self.content_bytes())?;
        let dto = decode_content(
            self.content_bytes(),
            self.namespace(),
            self.version(),
            self.family(),
            self.verified_event().public_key(),
        )?;
        let canonical = encode_dto(&dto)?;
        if canonical.as_slice() != self.content_bytes() {
            return Err(ProtocolError::new(FailureClass::ContentNonCanonical));
        }
        Ok(VerifiedSupportedRecord {
            supported: self,
            dto,
        })
    }
}

fn encode_dto(dto: &ContentDto) -> Result<Vec<u8>, ProtocolError> {
    let encoded =
        serde_json::to_vec(dto).map_err(|_| ProtocolError::new(FailureClass::ContentMalformed))?;
    if encoded.len() > MAX_CONTENT_BYTES {
        return Err(ProtocolError::new(FailureClass::ContentTooLarge));
    }
    Ok(encoded)
}
