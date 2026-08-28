//! Typed unsigned local relay and provider defaults.

use std::collections::BTreeSet;

use hq_domain::ProviderId;
use hq_relay::RelayUrl;
use serde::{Deserialize, Serialize};

use super::{IdentityError, IdentityErrorClass};

const MAX_RELAYS: usize = 16;
pub(super) const MAX_CONFIGURATION_BYTES: u64 = 65_536;

/// Versioned unsigned installation-local defaults.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalConfiguration {
    /// Relay endpoints in canonical order.
    pub relays: Vec<RelayUrl>,
    /// Optional provider default.
    pub default_provider: Option<ProviderId>,
}

impl LocalConfiguration {
    /// Validates, sorts, and owns local defaults.
    pub fn new(
        relays: impl IntoIterator<Item = RelayUrl>,
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
        relays: value
            .relays
            .iter()
            .map(|relay| relay.as_str().to_owned())
            .collect(),
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
        .map(|value| {
            RelayUrl::new(value)
                .map_err(|_| IdentityError::new(IdentityErrorClass::ConfigurationInvalid))
        })
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
