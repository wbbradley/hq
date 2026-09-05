//! Bounded protocol-prefix dispatch after cryptographic verification.

use std::fmt;

use crate::{
    CryptographicallyVerifiedEvent, FailureClass, ProtocolError,
    json::{JsonCursor, validate_content_json},
};

const MAX_DISCRIMINATOR_BYTES: usize = 128;

/// Independently versioned signed-content namespace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolNamespace {
    /// Immutable canonical fact records.
    Canonical,
    /// Signed remote-control audit records.
    Control,
}

/// Reason a cryptographically verified record cannot be interpreted by this build.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedReason {
    /// Protocol discriminator is not implemented.
    Protocol,
    /// Namespace is known but version is not implemented.
    Version,
    /// Namespace and version are known but family is not implemented.
    Family,
}

/// Result of bounded dispatch over cryptographically verified content.
#[derive(Debug)]
pub enum DispatchOutcome {
    /// Namespace, version, and family are implemented; full DTO verification remains next.
    Supported(SupportedContentBytes),
    /// Signed content is well-formed but not implemented by this build.
    Unsupported(VerifiedUnsupportedRecord),
}

/// Verified event whose namespace, version, and family are supported.
pub struct SupportedContentBytes {
    event: CryptographicallyVerifiedEvent,
    namespace: ProtocolNamespace,
    version: u64,
    family: u64,
}

impl SupportedContentBytes {
    /// Returns the owning content namespace.
    pub const fn namespace(&self) -> ProtocolNamespace {
        self.namespace
    }

    /// Returns the independently scoped protocol version.
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Returns the numeric family code.
    pub const fn family(&self) -> u64 {
        self.family
    }

    /// Returns exact cryptographically verified content bytes.
    pub fn content_bytes(&self) -> &[u8] {
        self.event.content_bytes()
    }

    /// Returns the verified event evidence for later DTO conversion and storage.
    pub const fn verified_event(&self) -> &CryptographicallyVerifiedEvent {
        &self.event
    }
}

impl fmt::Debug for SupportedContentBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SupportedContentBytes")
            .field("namespace", &self.namespace)
            .field("version", &self.version)
            .field("family", &self.family)
            .field("event", &self.event)
            .finish()
    }
}

/// Cryptographically verified but unsupported signed record.
pub struct VerifiedUnsupportedRecord {
    event: CryptographicallyVerifiedEvent,
    reason: UnsupportedReason,
    discriminator: String,
    version: u64,
    family: u64,
}

impl VerifiedUnsupportedRecord {
    /// Returns the closed unsupported classification.
    pub const fn reason(&self) -> UnsupportedReason {
        self.reason
    }

    /// Returns the bounded recognized discriminator.
    pub fn discriminator(&self) -> &str {
        &self.discriminator
    }

    /// Returns the signed version candidate.
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Returns the signed family candidate.
    pub const fn family(&self) -> u64 {
        self.family
    }

    /// Returns exact retained content bytes without exposing a supported DTO.
    pub fn content_bytes(&self) -> &[u8] {
        self.event.content_bytes()
    }

    /// Returns exact verified event evidence for bounded unsupported retention.
    pub const fn verified_event(&self) -> &CryptographicallyVerifiedEvent {
        &self.event
    }
}

impl fmt::Debug for VerifiedUnsupportedRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedUnsupportedRecord")
            .field("reason", &self.reason)
            .field("discriminator", &self.discriminator)
            .field("version", &self.version)
            .field("family", &self.family)
            .field("event", &self.event)
            .finish()
    }
}

impl CryptographicallyVerifiedEvent {
    /// Dispatches a verified event without constructing a payload DTO or semantic fact.
    pub fn dispatch(self) -> Result<DispatchOutcome, ProtocolError> {
        let prefix = parse_prefix(self.content_bytes())?;
        if prefix.authored_millis > i64::MAX as u64
            || prefix.authored_millis / 1000 != self.created_at()
        {
            return Err(ProtocolError::new(FailureClass::AuthoredTimeMismatch));
        }
        match classify(&prefix)? {
            Classification::Supported(namespace) => {
                Ok(DispatchOutcome::Supported(SupportedContentBytes {
                    event: self,
                    namespace,
                    version: prefix.version,
                    family: prefix.family,
                }))
            }
            Classification::Unsupported(reason) => {
                Ok(DispatchOutcome::Unsupported(VerifiedUnsupportedRecord {
                    event: self,
                    reason,
                    discriminator: prefix.discriminator,
                    version: prefix.version,
                    family: prefix.family,
                }))
            }
        }
    }
}

struct Prefix {
    discriminator: String,
    version: u64,
    family: u64,
    authored_millis: u64,
}

enum Classification {
    Supported(ProtocolNamespace),
    Unsupported(UnsupportedReason),
}

fn parse_prefix(content: &[u8]) -> Result<Prefix, ProtocolError> {
    validate_content_json(content)?;
    if content.starts_with(b"{\"schema\":") || content.starts_with(b"{\"type\":") {
        return Err(ProtocolError::new(FailureClass::LegacySchema));
    }
    let mut cursor = JsonCursor::content(content);
    cursor.expect(b"{\"p\":", FailureClass::ContentMalformed)?;
    let discriminator = cursor.parse_string(MAX_DISCRIMINATOR_BYTES)?;
    cursor.expect(b",\"v\":", FailureClass::ContentMalformed)?;
    let version = cursor.parse_u64()?;
    cursor.expect(b",\"f\":", FailureClass::ContentMalformed)?;
    let family = cursor.parse_u64()?;
    cursor.expect(b",\"author\":", FailureClass::ContentMalformed)?;
    let author = cursor.parse_string(128)?;
    if author.len() != 64 || !author.iter().all(u8::is_ascii_hexdigit) {
        return Err(ProtocolError::new(FailureClass::ContentMalformed));
    }
    if author.iter().any(u8::is_ascii_uppercase) {
        return Err(ProtocolError::new(FailureClass::ContentNonCanonical));
    }
    cursor.expect(b",\"time\":", FailureClass::ContentMalformed)?;
    let authored_millis = cursor.parse_u64()?;
    if cursor.peek() != Some(b',') || family == 0 {
        return Err(ProtocolError::new(FailureClass::ContentMalformed));
    }
    let discriminator = String::from_utf8(discriminator)
        .map_err(|_| ProtocolError::new(FailureClass::ContentMalformed))?;
    Ok(Prefix {
        discriminator,
        version,
        family,
        authored_millis,
    })
}

fn classify(prefix: &Prefix) -> Result<Classification, ProtocolError> {
    let namespace = match prefix.discriminator.as_str() {
        "hq/canonical" => ProtocolNamespace::Canonical,
        "hq/control" => ProtocolNamespace::Control,
        _ => return Ok(Classification::Unsupported(UnsupportedReason::Protocol)),
    };
    if prefix.version != 1 {
        return Ok(Classification::Unsupported(UnsupportedReason::Version));
    }
    match namespace {
        ProtocolNamespace::Canonical
            if (1..=45).contains(&prefix.family) || prefix.family == 49 =>
        {
            Ok(Classification::Supported(namespace))
        }
        ProtocolNamespace::Control if (46..=48).contains(&prefix.family) => {
            Ok(Classification::Supported(namespace))
        }
        ProtocolNamespace::Canonical if (46..=48).contains(&prefix.family) => {
            Err(ProtocolError::new(FailureClass::NamespaceConfusion))
        }
        ProtocolNamespace::Control if (1..=45).contains(&prefix.family) || prefix.family == 49 => {
            Err(ProtocolError::new(FailureClass::NamespaceConfusion))
        }
        _ => Ok(Classification::Unsupported(UnsupportedReason::Family)),
    }
}
