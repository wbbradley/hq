use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::value::RawValue;

use super::model::{
    AgentNameClaimedDto, AgentRetiredDto, AnswerGivenDto, AuthorityDto, BodyDto, ContentDto,
    HarnessActivityRecordedDto, Hex32, HumanAccountCreatedDto, HumanAccountSelectedDto,
    HumanDeviceAcceptedDto, HumanDeviceGrantedDto, HumanDeviceRevokedDto, InstallationDeclaredDto,
    LocatorDto, MailboxAccessGrantedDto, MailboxAccessRevokedDto, MailboxActionObservedDto,
    MailboxContextRecordedDto, MailboxCreatedDto, MailboxSessionBoundDto, MessageDto,
    MessagePurposeDto, MessageRejectedDto, MessageTargetDto, Milliseconds, NamespaceDto, ParentDto,
    PeerRouteBlockedDto, PeerRouteSetDto, ProjectAssignmentBlockedDto,
    ProjectAssignmentConfiguringDto, ProjectAssignmentEndedDto, ProjectAssignmentRunnableDto,
    ProjectClosedDto, ProjectCreatedDto, ProjectInputAcceptedDto, ProjectInputDispatchedDto,
    ProjectMetadataUpdatedDto, ProjectOutputRecordedDto, ProjectResourceAddedDto,
    ProjectResourceHealthObservedDto, ProjectResourceRemovedDto, ProjectResourceReplacedDto,
    ProjectResourceTargetDto, ProjectTargetDto, ProtocolDto, ProviderSessionRenamedDto,
    ProviderSessionSelectedDto, RemoteProjectCommandOutcomeDto, RemoteProjectCommandReceiptDto,
    RemoteProjectCommandRequestedDto, RoleDto, ScopeDto, ThreadCancelledDto,
};
use crate::{FailureClass, ProtocolError, ProtocolNamespace};

const MAX_PARENT_REFS: usize = 64;
const MAX_AUTHORITY_REFS: usize = 8;
const MAX_RELAY_HINTS: usize = 8;
const MAX_RESOURCE_ITEMS: usize = 64;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawContentDto {
    #[serde(rename = "p")]
    protocol: ProtocolDto,
    #[serde(rename = "v")]
    version: u64,
    #[serde(rename = "f")]
    family: u64,
    author: Hex32,
    time: Milliseconds,
    scope: ScopeDto,
    parents: Vec<ParentDto>,
    #[serde(rename = "auth")]
    authorities: Vec<AuthorityDto>,
    body: Box<RawValue>,
}

pub(super) fn decode_content(
    bytes: &[u8],
    expected_namespace: ProtocolNamespace,
    expected_version: u64,
    expected_family: u64,
    verified_public_key: [u8; 32],
) -> Result<ContentDto, ProtocolError> {
    let raw: RawContentDto = serde_json::from_slice(bytes).map_err(|_| malformed())?;
    decode_raw_content(
        raw,
        expected_namespace,
        expected_version,
        Some(expected_family),
        verified_public_key,
    )
}

pub(super) fn decode_unsigned_content(bytes: &[u8]) -> Result<ContentDto, ProtocolError> {
    let raw: RawContentDto = serde_json::from_slice(bytes).map_err(|_| malformed())?;
    let namespace = match raw.protocol {
        ProtocolDto::Canonical => ProtocolNamespace::Canonical,
        ProtocolDto::Control => ProtocolNamespace::Control,
    };
    let verified_public_key = if raw.family == 1 {
        serde_json::from_str::<InstallationDeclaredDto>(raw.body.get())
            .map_err(|_| malformed())?
            .signing
            .0
    } else {
        [0; 32]
    };
    decode_raw_content(raw, namespace, 1, None, verified_public_key)
}

fn decode_raw_content(
    raw: RawContentDto,
    expected_namespace: ProtocolNamespace,
    expected_version: u64,
    expected_family: Option<u64>,
    verified_public_key: [u8; 32],
) -> Result<ContentDto, ProtocolError> {
    let namespace = match raw.protocol {
        ProtocolDto::Canonical => ProtocolNamespace::Canonical,
        ProtocolDto::Control => ProtocolNamespace::Control,
    };
    if namespace != expected_namespace {
        return Err(ProtocolError::new(FailureClass::NamespaceConfusion));
    }
    if raw.version != expected_version || expected_family.is_some_and(|family| family != raw.family)
    {
        return Err(malformed());
    }
    let body = decode_body(raw.family, &raw.body)?;
    let content = ContentDto {
        protocol: raw.protocol,
        version: raw.version,
        family: raw.family,
        author: raw.author,
        time: raw.time,
        scope: raw.scope,
        parents: raw.parents,
        authorities: raw.authorities,
        body,
    };
    validate_content(&content, verified_public_key)?;
    Ok(content)
}

fn decode_body(family: u64, raw: &RawValue) -> Result<BodyDto, ProtocolError> {
    macro_rules! body {
        ($type:ty, $variant:ident) => {
            serde_json::from_str::<$type>(raw.get())
                .map(BodyDto::$variant)
                .map_err(|_| malformed())
        };
    }
    match family {
        1 => body!(InstallationDeclaredDto, InstallationDeclared),
        2 => body!(MailboxCreatedDto, MailboxCreated),
        3 => body!(MailboxSessionBoundDto, MailboxSessionBound),
        4 => body!(MailboxContextRecordedDto, MailboxContextRecorded),
        5 => body!(PeerRouteSetDto, PeerRouteSet),
        6 => body!(PeerRouteBlockedDto, PeerRouteBlocked),
        7 => body!(MailboxAccessGrantedDto, MailboxAccessGranted),
        8 => body!(MailboxAccessRevokedDto, MailboxAccessRevoked),
        9 => body!(MailboxActionObservedDto, MailboxActionObserved),
        10 => body!(HumanAccountCreatedDto, HumanAccountCreated),
        11 => body!(HumanAccountSelectedDto, HumanAccountSelected),
        12 => body!(HumanDeviceGrantedDto, HumanDeviceGranted),
        13 => body!(HumanDeviceAcceptedDto, HumanDeviceAccepted),
        14 => body!(HumanDeviceRevokedDto, HumanDeviceRevoked),
        15 => body!(MessageDto, QuestionAsked),
        16 => body!(MessageDto, AsynchronousMessageSent),
        17 => body!(AnswerGivenDto, AnswerGiven),
        18 => body!(ThreadCancelledDto, ThreadCancelled),
        19 => body!(MessageTargetDto, MessageArchived),
        20 => body!(MessageTargetDto, MessageRestored),
        21 => body!(MessageRejectedDto, MessageRejected),
        22 => body!(HarnessActivityRecordedDto, HarnessActivityRecorded),
        23 => body!(AgentNameClaimedDto, AgentNameClaimed),
        24 => body!(AgentRetiredDto, AgentRetired),
        25 => body!(ProviderSessionSelectedDto, ProviderSessionSelected),
        26 => body!(ProviderSessionRenamedDto, ProviderSessionRenamed),
        27 => body!(ProjectCreatedDto, ProjectCreated),
        28 => body!(ProjectTargetDto, ProjectOpened),
        29 => body!(ProjectTargetDto, ProjectClosingStarted),
        30 => body!(ProjectClosedDto, ProjectClosed),
        31 => body!(ProjectTargetDto, ProjectArchived),
        32 => body!(ProjectTargetDto, ProjectUnarchived),
        33 => body!(ProjectMetadataUpdatedDto, ProjectMetadataUpdated),
        34 => body!(ProjectResourceAddedDto, ProjectResourceAdded),
        35 => body!(ProjectResourceRemovedDto, ProjectResourceRemoved),
        36 => body!(ProjectResourceReplacedDto, ProjectResourceReplaced),
        37 => body!(ProjectResourceTargetDto, ProjectPrimaryResourceChanged),
        38 => body!(
            ProjectResourceHealthObservedDto,
            ProjectResourceHealthObserved
        ),
        39 => body!(
            ProjectAssignmentConfiguringDto,
            ProjectAssignmentConfiguring
        ),
        40 => body!(ProjectAssignmentRunnableDto, ProjectAssignmentRunnable),
        41 => body!(ProjectAssignmentBlockedDto, ProjectAssignmentBlocked),
        42 => body!(ProjectAssignmentEndedDto, ProjectAssignmentEnded),
        43 => body!(ProjectInputAcceptedDto, ProjectInputAccepted),
        44 => body!(ProjectInputDispatchedDto, ProjectInputDispatched),
        45 => body!(ProjectOutputRecordedDto, ProjectOutputRecorded),
        46 => body!(
            RemoteProjectCommandRequestedDto,
            RemoteProjectCommandRequested
        ),
        47 => body!(RemoteProjectCommandReceiptDto, RemoteProjectCommandReceipt),
        48 => body!(RemoteProjectCommandOutcomeDto, RemoteProjectCommandOutcome),
        _ => Err(malformed()),
    }
}

fn validate_content(
    content: &ContentDto,
    verified_public_key: [u8; 32],
) -> Result<(), ProtocolError> {
    validate_scope(content)?;
    validate_references(content)?;
    validate_body(content, verified_public_key)
}

fn validate_scope(content: &ContentDto) -> Result<(), ProtocolError> {
    if !scope_is_allowed(content.family, &content.scope) {
        return Err(ProtocolError::new(FailureClass::NamespaceConfusion));
    }
    match (&content.protocol, &content.scope) {
        (ProtocolDto::Canonical, ScopeDto::Local((_, installation))) => {
            if installation != &content.author {
                return Err(ProtocolError::new(FailureClass::ScopeAuthorMismatch));
            }
        }
        (ProtocolDto::Canonical, ScopeDto::Peer(_) | ScopeDto::Account(_))
        | (ProtocolDto::Control, ScopeDto::Control(_)) => {}
        _ => return Err(ProtocolError::new(FailureClass::NamespaceConfusion)),
    }
    Ok(())
}

fn scope_is_allowed(family: u64, scope: &ScopeDto) -> bool {
    match family {
        1..=6 | 10..=11 | 23..=26 => matches!(scope, ScopeDto::Local(_)),
        7..=9 => matches!(scope, ScopeDto::Peer(_)),
        12..=14 | 27..=45 => matches!(scope, ScopeDto::Account(_)),
        15..=18 => matches!(
            scope,
            ScopeDto::Local(_) | ScopeDto::Peer(_) | ScopeDto::Account(_)
        ),
        19..=22 => matches!(scope, ScopeDto::Local(_) | ScopeDto::Account(_)),
        46..=48 => matches!(scope, ScopeDto::Control(_)),
        _ => false,
    }
}

fn validate_references(content: &ContentDto) -> Result<(), ProtocolError> {
    if content.parents.len() > MAX_PARENT_REFS || content.authorities.len() > MAX_AUTHORITY_REFS {
        return Err(ProtocolError::new(FailureClass::ContentTooManyItems));
    }
    if !strictly_sorted(&content.parents) {
        return Err(ProtocolError::new(FailureClass::ContentNonCanonical));
    }
    for parent in &content.parents {
        match (content.protocol, parent.0) {
            (ProtocolDto::Canonical, NamespaceDto::Canonical)
            | (ProtocolDto::Control, NamespaceDto::Canonical | NamespaceDto::Control) => {}
            _ => return Err(ProtocolError::new(FailureClass::NamespaceConfusion)),
        }
    }
    let mut roles = BTreeSet::new();
    for authority in &content.authorities {
        if !roles.insert(authority.0) {
            return Err(ProtocolError::new(FailureClass::DuplicateAuthorityRole));
        }
    }
    if !strictly_sorted(&content.authorities) {
        return Err(ProtocolError::new(FailureClass::ContentNonCanonical));
    }
    for authority in &content.authorities {
        if !role_is_allowed(content.family, authority.0) {
            return Err(malformed());
        }
        let expected_namespace = if authority.0 == RoleDto::Request {
            NamespaceDto::Control
        } else {
            NamespaceDto::Canonical
        };
        if authority.1 != expected_namespace
            || content.protocol == ProtocolDto::Canonical && authority.1 != NamespaceDto::Canonical
        {
            return Err(ProtocolError::new(FailureClass::NamespaceConfusion));
        }
        if !content
            .parents
            .contains(&ParentDto(authority.1, authority.2))
        {
            return Err(ProtocolError::new(FailureClass::AuthorityNotParent));
        }
    }
    Ok(())
}

fn role_is_allowed(family: u64, role: RoleDto) -> bool {
    use RoleDto::{
        AccountCreator, AccountMembership, ActiveHuman, Assignment, DeviceGrant, Dispatch,
        LocalInstallation, MailboxGrant, MailboxOwner, OutputBinding, PreviousState, ProjectHome,
        Request,
    };
    match family {
        2..=6 | 10 | 23..=26 => role == LocalInstallation,
        7 => role == MailboxOwner,
        8..=9 => role == MailboxGrant,
        11 => matches!(role, LocalInstallation | AccountMembership),
        12 => role == AccountCreator,
        13 => role == DeviceGrant,
        14 => matches!(role, AccountCreator | DeviceGrant),
        15..=18 => matches!(role, LocalInstallation | MailboxGrant | AccountMembership),
        19..=22 => matches!(role, LocalInstallation | AccountMembership),
        27 => matches!(role, ProjectHome | AccountMembership | ActiveHuman),
        28..=37 | 39 => {
            matches!(
                role,
                PreviousState | ProjectHome | AccountMembership | ActiveHuman
            )
        }
        38 => matches!(
            role,
            PreviousState | ProjectHome | AccountMembership | ActiveHuman
        ),
        40..=41 => matches!(
            role,
            PreviousState | ProjectHome | AccountMembership | Assignment
        ),
        42 => matches!(
            role,
            PreviousState | ProjectHome | Assignment | AccountMembership | ActiveHuman
        ),
        43 => matches!(role, PreviousState | ProjectHome | AccountMembership),
        44 => matches!(
            role,
            PreviousState | ProjectHome | AccountMembership | Assignment | Dispatch
        ),
        45 => matches!(
            role,
            PreviousState | ProjectHome | AccountMembership | Dispatch | Assignment | OutputBinding
        ),
        46 => matches!(role, AccountMembership | ActiveHuman | ProjectHome),
        47..=48 => matches!(
            role,
            ProjectHome | Request | AccountMembership | ActiveHuman
        ),
        _ => false,
    }
}

fn validate_body(content: &ContentDto, verified_public_key: [u8; 32]) -> Result<(), ProtocolError> {
    match &content.body {
        BodyDto::InstallationDeclared(body) => {
            if body.installation != content.author
                || body.signing.0 != verified_public_key
                || !matches!(content.scope, ScopeDto::Local((_, installation)) if installation == body.installation)
            {
                return Err(ProtocolError::new(FailureClass::ScopeAuthorMismatch));
            }
        }
        BodyDto::PeerRouteSet(body) => validate_relays(&body.relays)?,
        BodyDto::HumanDeviceGranted(body) => validate_relays(&body.relays)?,
        BodyDto::QuestionAsked(message) => {
            if message.purpose != MessagePurposeDto::Question {
                return Err(malformed());
            }
        }
        BodyDto::AsynchronousMessageSent(message) => {
            if message.purpose != MessagePurposeDto::Asynchronous {
                return Err(malformed());
            }
        }
        BodyDto::AgentNameClaimed(body) => {
            if !valid_agent_name(&body.name.0) {
                return Err(malformed());
            }
        }
        BodyDto::ProjectCreated(body) => {
            if body.resources.len() > MAX_RESOURCE_ITEMS
                || has_duplicate_by(&body.resources, |resource| resource.id)
                || body.primary.0.is_some_and(|primary| {
                    !body.resources.iter().any(|resource| resource.id == primary)
                })
            {
                return Err(malformed());
            }
        }
        BodyDto::ProjectOutputRecorded(body) => {
            if body.message.id != body.output
                || body.message.purpose != MessagePurposeDto::ProjectOutput
                || body.message.project.0 != Some(body.project)
            {
                return Err(malformed());
            }
        }
        BodyDto::RemoteProjectCommandRequested(body) => {
            let ScopeDto::Control((_, _, target_home)) = content.scope else {
                return Err(ProtocolError::new(FailureClass::NamespaceConfusion));
            };
            if body.target_home != target_home {
                return Err(ProtocolError::new(FailureClass::ScopeAuthorMismatch));
            }
        }
        BodyDto::RemoteProjectCommandReceipt(_) | BodyDto::RemoteProjectCommandOutcome(_) => {
            let ScopeDto::Control((_, _, target_home)) = content.scope else {
                return Err(ProtocolError::new(FailureClass::NamespaceConfusion));
            };
            if content.author != target_home {
                return Err(ProtocolError::new(FailureClass::ScopeAuthorMismatch));
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_relays(relays: &[LocatorDto]) -> Result<(), ProtocolError> {
    if relays.len() > MAX_RELAY_HINTS || has_duplicates(relays) {
        return Err(malformed());
    }
    Ok(())
}

fn has_duplicates<T: PartialEq>(items: &[T]) -> bool {
    items
        .iter()
        .enumerate()
        .any(|(index, item)| items[..index].contains(item))
}

fn has_duplicate_by<T, K: Copy + PartialEq>(items: &[T], key: impl Fn(&T) -> K) -> bool {
    items
        .iter()
        .enumerate()
        .any(|(index, item)| items[..index].iter().any(|prior| key(prior) == key(item)))
}

fn strictly_sorted<T: Ord>(items: &[T]) -> bool {
    items.windows(2).all(|pair| pair[0] < pair[1])
}

fn valid_agent_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.first().is_some_and(u8::is_ascii_lowercase)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

const fn malformed() -> ProtocolError {
    ProtocolError::new(FailureClass::ContentMalformed)
}
