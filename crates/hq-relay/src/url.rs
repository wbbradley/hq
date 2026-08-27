//! Validated exact relay URL identity.

use std::{error::Error, fmt};

/// Maximum exact relay URL length.
pub const MAX_RELAY_URL_BYTES: usize = 2_048;

/// Invalid relay URL shape.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RelayUrlError;

impl fmt::Display for RelayUrlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("relay URL is invalid")
    }
}

impl Error for RelayUrlError {}

/// Validated exact `ws` or `wss` relay URL.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RelayUrl(String);

impl RelayUrl {
    /// Validates one exact relay URL without network I/O or lossy normalization.
    pub fn new(value: String) -> Result<Self, RelayUrlError> {
        let suffix = value
            .strip_prefix("ws://")
            .or_else(|| value.strip_prefix("wss://"));
        if suffix.is_none_or(|suffix| !valid_authority(suffix))
            || value.len() > MAX_RELAY_URL_BYTES
            || !value.is_ascii()
            || value
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        {
            return Err(RelayUrlError);
        }
        Ok(Self(value))
    }

    /// Borrows the exact validated spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the exact validated spelling.
    pub fn into_string(self) -> String {
        self.0
    }
}

fn valid_authority(suffix: &str) -> bool {
    if suffix.contains('#') {
        return false;
    }
    let authority = suffix
        .split_once(['/', '?'])
        .map_or(suffix, |(authority, _)| authority);
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    if let Some(bracketed) = authority.strip_prefix('[') {
        let Some((host, remainder)) = bracketed.split_once(']') else {
            return false;
        };
        return !host.is_empty()
            && (remainder.is_empty() || remainder.strip_prefix(':').is_some_and(valid_port));
    }
    if authority.contains(['[', ']']) {
        return false;
    }
    authority.rsplit_once(':').map_or_else(
        || !authority.is_empty(),
        |(host, port)| !host.is_empty() && !host.contains(':') && valid_port(port),
    )
}

fn valid_port(port: &str) -> bool {
    !port.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_url_accepts_exact_bounds_and_rejects_ambiguous_input() {
        assert!(RelayUrl::new("ws://r".to_owned()).is_ok());
        assert!(RelayUrl::new("ws://[::1]:7447/path?query=1".to_owned()).is_ok());
        assert!(RelayUrl::new(format!("wss://{}", "r".repeat(MAX_RELAY_URL_BYTES - 6))).is_ok());
        for invalid in [
            "",
            "https://relay",
            "ws://",
            "ws:///path",
            "ws://?query",
            "ws://relay:",
            "ws://relay:65536",
            "ws://[::1",
            "ws://relay#fragment",
            "ws://user@relay",
            "WS://relay",
            "ws://relay x",
            "ws://relay\n",
        ] {
            assert!(RelayUrl::new(invalid.to_owned()).is_err());
        }
        assert!(RelayUrl::new(format!("wss://{}", "r".repeat(MAX_RELAY_URL_BYTES - 5))).is_err());
    }
}
