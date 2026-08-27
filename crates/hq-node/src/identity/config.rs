//! Typed unsigned local relay and provider defaults.

use std::collections::BTreeSet;

use hq_domain::ProviderId;
use serde::{Deserialize, Serialize};

use super::{IdentityError, IdentityErrorClass};

const MAX_RELAYS: usize = 16;
const MAX_RELAY_BYTES: usize = 2_048;
pub(super) const MAX_CONFIGURATION_BYTES: u64 = 65_536;

/// Validated local WebSocket relay endpoint.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RelayEndpoint(String);

impl RelayEndpoint {
    /// Validates a `ws` or `wss` endpoint without performing network I/O.
    pub fn new(value: String) -> Result<Self, IdentityError> {
        let valid_scheme = value.starts_with("wss://") || value.starts_with("ws://");
        let suffix = value.split_once("://").map(|(_, suffix)| suffix);
        if !valid_scheme
            || suffix.is_none_or(str::is_empty)
            || value.len() > MAX_RELAY_BYTES
            || !value.is_ascii()
            || value
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        {
            return Err(IdentityError::new(IdentityErrorClass::ConfigurationInvalid));
        }
        Ok(Self(value))
    }

    /// Borrows the exact endpoint spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Versioned unsigned installation-local defaults.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalConfiguration {
    relays: Vec<RelayEndpoint>,
    default_provider: Option<ProviderId>,
}

impl LocalConfiguration {
    /// Validates, sorts, and owns local defaults.
    pub fn new(
        relays: impl IntoIterator<Item = RelayEndpoint>,
        default_provider: Option<ProviderId>,
    ) -> Result<Self, IdentityError> {
        let relays = relays.into_iter().collect::<Vec<_>>();
        if relays.len() > MAX_RELAYS {
            return Err(IdentityError::new(IdentityErrorClass::ConfigurationInvalid));
        }
        let unique = relays.iter().cloned().collect::<BTreeSet<_>>();
        if unique.len() != relays.len() {
            return Err(IdentityError::new(IdentityErrorClass::ConfigurationInvalid));
        }
        Ok(Self {
            relays: unique.into_iter().collect(),
            default_provider,
        })
    }

    /// Returns relay endpoints in canonical order.
    pub fn relays(&self) -> &[RelayEndpoint] {
        &self.relays
    }

    /// Returns the optional provider default.
    pub const fn default_provider(&self) -> Option<&ProviderId> {
        self.default_provider.as_ref()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigurationDto {
    version: u64,
    relays: Vec<String>,
    default_provider: Option<String>,
}

pub(super) fn encode(value: &LocalConfiguration) -> Result<Vec<u8>, IdentityError> {
    let dto = ConfigurationDto {
        version: 1,
        relays: value.relays.iter().map(|relay| relay.0.clone()).collect(),
        default_provider: value
            .default_provider
            .as_ref()
            .map(|provider| provider.as_str().to_owned()),
    };
    serde_json::to_vec(&dto)
        .map_err(|_| IdentityError::new(IdentityErrorClass::ConfigurationMalformed))
}

pub(super) fn decode(bytes: &[u8]) -> Result<LocalConfiguration, IdentityError> {
    let dto: ConfigurationDto = serde_json::from_slice(bytes)
        .map_err(|_| IdentityError::new(IdentityErrorClass::ConfigurationMalformed))?;
    if dto.version != 1 {
        return Err(IdentityError::new(
            IdentityErrorClass::ConfigurationMalformed,
        ));
    }
    let relays = dto
        .relays
        .into_iter()
        .map(RelayEndpoint::new)
        .collect::<Result<Vec<_>, _>>()?;
    let provider = dto
        .default_provider
        .map(ProviderId::new)
        .transpose()
        .map_err(|_| IdentityError::new(IdentityErrorClass::ConfigurationInvalid))?;
    let configuration = LocalConfiguration::new(relays, provider)?;
    if encode(&configuration)?.as_slice() != bytes {
        return Err(IdentityError::new(
            IdentityErrorClass::ConfigurationMalformed,
        ));
    }
    Ok(configuration)
}
