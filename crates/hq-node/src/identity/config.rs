//! Typed unsigned local relay and provider defaults.

use std::{
    collections::BTreeSet,
    path::{Component, Path},
};

use hq_domain::ProviderId;
use hq_relay::RelayUrl;
use serde::{Deserialize, Serialize};

use super::{IdentityError, IdentityErrorClass};

const MAX_RELAYS: usize = 16;
const MAX_THEME_SELECTION_BYTES: usize = 1_024;
const MAX_CODEX_MODEL_BYTES: usize = 256;
pub(super) const MAX_CONFIGURATION_BYTES: u64 = 65_536;

/// One bounded built-in/user theme name or explicit absolute theme file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemeSelection(String);

impl ThemeSelection {
    /// Validates one persisted theme selector without reading the filesystem.
    pub fn new(value: String) -> Result<Self, IdentityError> {
        let path = Path::new(&value);
        if value.is_empty()
            || value.len() > MAX_THEME_SELECTION_BYTES
            || value.chars().any(char::is_control)
            || (path.is_absolute()
                && path
                    .components()
                    .any(|component| component == Component::ParentDir))
            || (!path.is_absolute() && !valid_theme_name(&value))
        {
            return Err(IdentityError::new(IdentityErrorClass::ConfigurationInvalid));
        }
        Ok(Self(value))
    }

    /// Returns the exact validated selector.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Reports whether the selector names an explicit absolute file.
    pub fn is_absolute_path(&self) -> bool {
        Path::new(&self.0).is_absolute()
    }
}

fn valid_theme_name(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Versioned unsigned installation-local defaults.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalConfiguration {
    /// Relay endpoints in canonical order.
    pub relays: Vec<RelayUrl>,
    /// Optional provider default.
    pub default_provider: Option<ProviderId>,
    /// Optional startup theme name or absolute file.
    pub theme: Option<ThemeSelection>,
    /// Provider-private Codex defaults.
    pub codex: LocalCodexConfiguration,
}

/// Installation-local Codex defaults.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalCodexConfiguration {
    /// Disable Codex approvals and sandboxing for managed sessions.
    pub yolo: bool,
    /// Optional exact Codex model selector; absence delegates to Codex.
    pub model: Option<String>,
}

impl LocalCodexConfiguration {
    /// Validates and owns Codex-specific defaults.
    pub fn new(yolo: bool, model: Option<String>) -> Result<Self, IdentityError> {
        if model.as_ref().is_some_and(|model| {
            model.is_empty()
                || model.len() > MAX_CODEX_MODEL_BYTES
                || model.chars().any(char::is_control)
        }) {
            return Err(IdentityError::new(IdentityErrorClass::ConfigurationInvalid));
        }
        Ok(Self { yolo, model })
    }
}

impl LocalConfiguration {
    /// Validates, sorts, and owns local defaults.
    pub fn new(
        relays: impl IntoIterator<Item = RelayUrl>,
        default_provider: Option<ProviderId>,
    ) -> Result<Self, IdentityError> {
        Self::from_parts(
            relays,
            default_provider,
            None,
            LocalCodexConfiguration::default(),
        )
    }

    /// Validates and owns every installation-local default.
    pub fn from_parts(
        relays: impl IntoIterator<Item = RelayUrl>,
        default_provider: Option<ProviderId>,
        theme: Option<ThemeSelection>,
        codex: LocalCodexConfiguration,
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
            theme,
            codex,
        })
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodexConfigurationDto {
    yolo: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
}

impl CodexConfigurationDto {
    const fn is_default(&self) -> bool {
        !self.yolo && self.model.is_none()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigurationDto {
    version: u64,
    relays: Vec<String>,
    default_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    theme: Option<String>,
    #[serde(default, skip_serializing_if = "CodexConfigurationDto::is_default")]
    codex: CodexConfigurationDto,
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
        theme: value.theme.as_ref().map(|theme| theme.as_str().to_owned()),
        codex: CodexConfigurationDto {
            yolo: value.codex.yolo,
            model: value.codex.model.clone(),
        },
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
    let theme = dto.theme.map(ThemeSelection::new).transpose()?;
    let configuration = LocalConfiguration::from_parts(
        relays,
        provider,
        theme,
        LocalCodexConfiguration::new(dto.codex.yolo, dto.codex.model)?,
    )?;
    if encode(&configuration)?.as_slice() != bytes {
        return Err(IdentityError::new(
            IdentityErrorClass::ConfigurationMalformed,
        ));
    }
    Ok(configuration)
}
