use std::{fmt, marker::PhantomData, num::NonZeroU64};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Visitor};

pub(super) const SHORT_TEXT_BYTES: usize = 128;
pub(super) const CONTENT_TEXT_BYTES: usize = 16_384;
pub(super) const LOCATOR_TEXT_BYTES: usize = 4_096;
pub(super) const PROVIDER_TEXT_BYTES: usize = 64;
pub(super) const SESSION_TEXT_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct Hex32(pub(super) [u8; 32]);

impl Serialize for Hex32 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(&HexDisplay(&self.0))
    }
}

impl<'de> Deserialize<'de> for Hex32 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(HexVisitor)
    }
}

struct HexVisitor;

impl Visitor<'_> for HexVisitor {
    type Value = Hex32;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("exactly 64 lowercase hexadecimal characters")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if value.len() != 64 {
            return Err(E::invalid_length(value.len(), &self));
        }
        let mut decoded = [0_u8; 32];
        for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
            let high = decode_nibble(pair[0])
                .ok_or_else(|| E::invalid_value(serde::de::Unexpected::Str(value), &self))?;
            let low = decode_nibble(pair[1])
                .ok_or_else(|| E::invalid_value(serde::de::Unexpected::Str(value), &self))?;
            decoded[index] = (high << 4) | low;
        }
        Ok(Hex32(decoded))
    }
}

const fn decode_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

struct HexDisplay<'a>(&'a [u8]);

impl fmt::Display for HexDisplay<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        for byte in self.0 {
            formatter.write_str(
                std::str::from_utf8(&[
                    DIGITS[usize::from(byte >> 4)],
                    DIGITS[usize::from(byte & 0x0f)],
                ])
                .map_err(|_| fmt::Error)?,
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Text<const MAXIMUM: usize>(pub(super) String);

impl<const MAXIMUM: usize> Serialize for Text<MAXIMUM> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de, const MAXIMUM: usize> Deserialize<'de> for Text<MAXIMUM> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_string(TextVisitor::<MAXIMUM>(PhantomData))
    }
}

struct TextVisitor<const MAXIMUM: usize>(PhantomData<()>);

impl<const MAXIMUM: usize> Visitor<'_> for TextVisitor<MAXIMUM> {
    type Value = Text<MAXIMUM>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "nonempty UTF-8 text of at most {MAXIMUM} bytes")
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_str(&value).map(|_| Text(value))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if value.is_empty() {
            return Err(E::invalid_length(0, &self));
        }
        if value.len() > MAXIMUM {
            return Err(E::invalid_length(value.len(), &self));
        }
        Ok(Text(value.to_owned()))
    }
}

pub(super) type ShortText = Text<SHORT_TEXT_BYTES>;
pub(super) type ContentText = Text<CONTENT_TEXT_BYTES>;
pub(super) type LocatorText = Text<LOCATOR_TEXT_BYTES>;
pub(super) type ProviderText = Text<PROVIDER_TEXT_BYTES>;
pub(super) type SessionText = Text<SESSION_TEXT_BYTES>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(super) struct RequiredOption<T>(pub(super) Option<T>);

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<RequiredOption<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(RequiredOption)
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub(super) enum ProtocolDto {
    #[serde(rename = "hq/canonical")]
    Canonical,
    #[serde(rename = "hq/control")]
    Control,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub(super) enum NamespaceDto {
    #[serde(rename = "c")]
    Canonical,
    #[serde(rename = "r")]
    Control,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum RoleDto {
    AccountCreator,
    AccountMembership,
    ActiveHuman,
    Assignment,
    DeviceGrant,
    Dispatch,
    LocalInstallation,
    MailboxGrant,
    MailboxOwner,
    OutputBinding,
    PreviousState,
    ProjectHome,
    Request,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub(super) enum LocalTag {
    #[serde(rename = "local")]
    Local,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub(super) enum PeerTag {
    #[serde(rename = "peer")]
    Peer,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub(super) enum AccountTag {
    #[serde(rename = "account")]
    Account,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub(super) enum ControlTag {
    #[serde(rename = "control")]
    Control,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum ScopeDto {
    Local((LocalTag, Hex32)),
    Peer((PeerTag, Hex32, Hex32)),
    Account((AccountTag, Hex32)),
    Control((ControlTag, Hex32, Hex32)),
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub(super) struct ParentDto(pub(super) NamespaceDto, pub(super) Hex32);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub(super) struct AuthorityDto(
    pub(super) RoleDto,
    pub(super) NamespaceDto,
    pub(super) Hex32,
);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Milliseconds(pub(super) u64);

impl Serialize for Milliseconds {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(self.0)
    }
}

impl<'de> Deserialize<'de> for Milliseconds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        if value > i64::MAX as u64 {
            return Err(serde::de::Error::custom(
                "millisecond time exceeds i64::MAX",
            ));
        }
        Ok(Self(value))
    }
}

macro_rules! object {
    ($name:ident { $($(#[$attribute:meta])* $field:ident : $type:ty),+ $(,)? }) => {
        #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub(super) struct $name {
            $($(#[$attribute])* pub(super) $field: $type),+
        }
    };
}

object!(InstallationAddressDto {
    installation: Hex32,
    signing: Hex32,
});
object!(MailboxAddressDto {
    installation: Hex32,
    mailbox: Hex32,
});

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum LocatorSchemeDto {
    #[serde(rename = "git")]
    Git,
    Worktree,
    Container,
    Opaque,
}

object!(LocatorDto {
    scheme: LocatorSchemeDto,
    value: LocatorText,
});
object!(ContextDto {
    directory: LocatorDto,
    #[serde(deserialize_with = "deserialize_required_option")]
    repository: RequiredOption<LocatorDto>,
    #[serde(deserialize_with = "deserialize_required_option")]
    worktree: RequiredOption<LocatorDto>,
    #[serde(deserialize_with = "deserialize_required_option")]
    branch: RequiredOption<ShortText>,
});
object!(OperationDto {
    provider: ProviderText,
    session: SessionText,
    id: Hex32,
});

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum MessagePurposeDto {
    Question,
    Asynchronous,
    ProjectOutput,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum PresentationDto {
    Message,
    FinalAnswer,
    Status,
}

object!(MessageDto {
    id: Hex32,
    sender: MailboxAddressDto,
    #[serde(deserialize_with = "deserialize_required_option")]
    recipient: RequiredOption<MailboxAddressDto>,
    body: ContentText,
    purpose: MessagePurposeDto,
    presentation: PresentationDto,
    #[serde(deserialize_with = "deserialize_required_option")]
    correlation: RequiredOption<OperationDto>,
    #[serde(deserialize_with = "deserialize_required_option")]
    project: RequiredOption<Hex32>,
});

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ResourceHealthDto {
    Unknown,
    Healthy,
    Degraded,
    Unavailable,
}

object!(ResourceDto {
    id: Hex32,
    display: LocatorDto,
    canonical: LocatorDto,
    health: ResourceHealthDto,
});
object!(BindingDto {
    assignment: Hex32,
    agent: Hex32,
    provider: ProviderText,
    session: SessionText,
});
object!(ProjectActivityAttributionDto {
    project: Hex32,
    dispatch: Hex32,
    binding: BindingDto,
    thread: Hex32,
});

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ActivityKindDto {
    Status,
    Progress,
    Plan,
    Diff,
    CompletedItem,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ActivitySimpleStateDto {
    Snapshot,
    Running,
    Succeeded,
    Interrupted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) enum FailedStateTag {
    #[serde(rename = "failed")]
    Failed,
}

object!(ActivitySimpleStatusDto {
    state: ActivitySimpleStateDto,
});
object!(FailedStatusDto {
    state: FailedStateTag,
    code: ShortText,
});

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum ActivityStatusDto {
    Simple(ActivitySimpleStatusDto),
    Failed(FailedStatusDto),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) enum SucceededStateTag {
    #[serde(rename = "succeeded")]
    Succeeded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) enum UncertainStateTag {
    #[serde(rename = "uncertain")]
    Uncertain,
}

object!(SucceededRuntimeDto {
    state: SucceededStateTag,
});
object!(FailedRuntimeDto {
    state: FailedStateTag,
    code: ShortText,
});
object!(UncertainRuntimeDto {
    state: UncertainStateTag,
    code: ShortText,
});

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum RuntimeDto {
    Succeeded(SucceededRuntimeDto),
    Failed(FailedRuntimeDto),
    Uncertain(UncertainRuntimeDto),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum MailboxKindDto {
    Human,
    Agent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum InitialStateDto {
    Open,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) enum CommittedStateTag {
    #[serde(rename = "committed")]
    Committed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) enum RejectedStateTag {
    #[serde(rename = "rejected")]
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) enum ExternalStateWarningKindDto {
    #[serde(rename = "worktree-may-exist")]
    WorktreeMayExist,
}

object!(ExternalStateWarningDto {
    kind: ExternalStateWarningKindDto,
    destination: LocatorDto,
    branch: ShortText,
});

object!(CommittedResultDto {
    state: CommittedStateTag,
    head: Hex32,
});
object!(RejectedResultDto {
    state: RejectedStateTag,
    code: ShortText,
    #[serde(deserialize_with = "deserialize_required_option")]
    external_state_warning: RequiredOption<ExternalStateWarningDto>,
});

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum RemoteResultDto {
    Committed(CommittedResultDto),
    Rejected(RejectedResultDto),
}

object!(InstallationDeclaredDto {
    installation: Hex32,
    signing: Hex32,
    encryption: Hex32,
    #[serde(deserialize_with = "deserialize_required_option")]
    label: RequiredOption<ShortText>,
});
object!(MailboxCreatedDto {
    mailbox: Hex32,
    kind: MailboxKindDto,
    #[serde(deserialize_with = "deserialize_required_option")]
    label: RequiredOption<ShortText>,
});
object!(MailboxSessionBoundDto {
    mailbox: Hex32,
    provider: ProviderText,
    session: SessionText,
});
object!(MailboxContextRecordedDto {
    mailbox: Hex32,
    context: ContextDto,
});
object!(PeerRouteSetDto {
    peer: InstallationAddressDto,
    encryption: Hex32,
    #[serde(deserialize_with = "deserialize_required_option")]
    label: RequiredOption<ShortText>,
    relays: Vec<LocatorDto>,
});
object!(PeerRouteBlockedDto {
    peer: Hex32,
    reason: ShortText,
});
object!(MailboxAccessGrantedDto {
    grant: Hex32,
    mailbox: MailboxAddressDto,
    grantee: InstallationAddressDto,
});
object!(MailboxAccessRevokedDto {
    grant: Hex32,
    mailbox: MailboxAddressDto,
    grantee: Hex32,
});
object!(MailboxActionObservedDto {
    grant: Hex32,
    action: Hex32,
});
object!(HumanAccountCreatedDto {
    account: Hex32,
    creator: InstallationAddressDto,
    #[serde(deserialize_with = "deserialize_required_option")]
    label: RequiredOption<ShortText>,
});
object!(HumanAccountSelectedDto { account: Hex32 });
object!(HumanDeviceGrantedDto {
    account: Hex32,
    grant: Hex32,
    device: InstallationAddressDto,
    #[serde(deserialize_with = "deserialize_required_option")]
    label: RequiredOption<ShortText>,
    relays: Vec<LocatorDto>,
});
object!(HumanDeviceAcceptedDto {
    account: Hex32,
    grant: Hex32,
    device: InstallationAddressDto,
});
object!(HumanDeviceRevokedDto {
    account: Hex32,
    grant: Hex32,
    device: Hex32,
});
object!(AnswerGivenDto {
    thread: Hex32,
    message: MessageDto,
});
object!(AsynchronousMessageSentDto {
    #[serde(deserialize_with = "deserialize_required_option")]
    thread: RequiredOption<Hex32>,
    message: MessageDto,
});
object!(ThreadCancelledDto {
    thread: Hex32,
    #[serde(deserialize_with = "deserialize_required_option")]
    reason: RequiredOption<ContentText>,
});
object!(MessageTargetDto { message: Hex32 });
object!(MessageRejectedDto {
    message: Hex32,
    reason: ShortText,
});
object!(HarnessActivityRecordedDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<ProjectActivityAttributionDto>,
    source: MailboxAddressDto,
    operation: OperationDto,
    #[serde(deserialize_with = "deserialize_required_option")]
    item: RequiredOption<ShortText>,
    kind: ActivityKindDto,
    logical_key: ShortText,
    runtime: ShortText,
    sequence: NonZeroU64,
    occurred_at: Milliseconds,
    status: ActivityStatusDto,
    content: ContentText,
    truncated: bool,
});
object!(AgentNameClaimedDto {
    agent: Hex32,
    mailbox: Hex32,
    name: ShortText,
});
object!(AgentRetiredDto {
    agent: Hex32,
    mailbox: Hex32,
});
object!(ProviderSessionSelectedDto {
    agent: Hex32,
    mailbox: Hex32,
    provider: ProviderText,
    session: SessionText,
    context: ContextDto,
});
object!(ProviderSessionRenamedDto {
    agent: Hex32,
    provider: ProviderText,
    session: SessionText,
    #[serde(deserialize_with = "deserialize_required_option")]
    display: RequiredOption<ShortText>,
});
object!(ProjectCreatedDto {
    project: Hex32,
    mailbox: Hex32,
    home: Hex32,
    name: ShortText,
    #[serde(deserialize_with = "deserialize_required_option")]
    brief: RequiredOption<ContentText>,
    #[serde(deserialize_with = "deserialize_required_option")]
    predecessor: RequiredOption<Hex32>,
    resources: Vec<ResourceDto>,
    #[serde(deserialize_with = "deserialize_required_option")]
    primary: RequiredOption<Hex32>,
    state: InitialStateDto,
});
object!(ProjectTargetDto { project: Hex32 });
object!(ProjectClosedDto {
    project: Hex32,
    forced: bool,
    #[serde(deserialize_with = "deserialize_required_option")]
    runtime: RequiredOption<RuntimeDto>,
});
object!(ProjectMetadataUpdatedDto {
    project: Hex32,
    name: ShortText,
    #[serde(deserialize_with = "deserialize_required_option")]
    brief: RequiredOption<ContentText>,
});
object!(ProjectResourceAddedDto {
    project: Hex32,
    resource: ResourceDto,
    primary: bool,
});
object!(ProjectResourceRemovedDto {
    project: Hex32,
    resource: Hex32,
    force: bool,
});
object!(ProjectResourceReplacedDto {
    project: Hex32,
    old_resource: Hex32,
    resource: ResourceDto,
});
object!(ProjectResourceTargetDto {
    project: Hex32,
    resource: Hex32,
});
object!(ProjectResourceHealthObservedDto {
    project: Hex32,
    resource: Hex32,
    health: ResourceHealthDto,
    #[serde(deserialize_with = "deserialize_required_option")]
    details: RequiredOption<ContentText>,
    checked_at: Milliseconds,
});
object!(ProjectAssignmentConfiguringDto {
    project: Hex32,
    assignment: Hex32,
    agent: Hex32,
    provider: ProviderText,
});
object!(ProjectAssignmentRunnableDto {
    project: Hex32,
    binding: BindingDto,
    thread: Hex32,
    launch_directory: LocatorDto,
    activation: OperationDto,
});
object!(ProjectAssignmentBlockedDto {
    project: Hex32,
    assignment: Hex32,
    cause: ShortText,
});
object!(ProjectAssignmentEndedDto {
    project: Hex32,
    assignment: Hex32,
    forced: bool,
    #[serde(deserialize_with = "deserialize_required_option")]
    runtime: RequiredOption<RuntimeDto>,
});
object!(ProjectInputAcceptedDto {
    project: Hex32,
    message: Hex32,
    input_fact: Hex32,
    sequence: NonZeroU64,
});
object!(ProjectInputDispatchedDto {
    project: Hex32,
    message: Hex32,
    sequence: NonZeroU64,
    dispatch: Hex32,
    binding: BindingDto,
    thread: Hex32,
});
object!(ProjectOutputRecordedDto {
    project: Hex32,
    output: Hex32,
    dispatch: Hex32,
    binding: BindingDto,
    thread: Hex32,
    message: MessageDto,
});
object!(RemoteProjectCommandRequestedDto {
    command: Hex32,
    digest: Hex32,
    project: Hex32,
    target_home: Hex32,
    #[serde(deserialize_with = "deserialize_required_option")]
    expected_head: RequiredOption<Hex32>,
    operation: OperationDto,
    body: ContentText,
});
object!(RemoteProjectCommandReceiptDto {
    command: Hex32,
    digest: Hex32,
    project: Hex32,
    #[serde(deserialize_with = "deserialize_required_option")]
    received_head: RequiredOption<Hex32>,
    received_at: Milliseconds,
});
object!(RemoteProjectCommandOutcomeDto {
    command: Hex32,
    digest: Hex32,
    project: Hex32,
    result: RemoteResultDto,
    #[serde(deserialize_with = "deserialize_required_option")]
    runtime: RequiredOption<RuntimeDto>,
});

#[derive(Clone, Debug, Eq, PartialEq)]
// The temporary DTO catalog keeps direct owned variants so exact decode/re-encode does not add a
// second allocation for every body; the largest bounded variant is well below one kilobyte.
#[allow(clippy::large_enum_variant)]
pub(super) enum BodyDto {
    InstallationDeclared(InstallationDeclaredDto),
    MailboxCreated(MailboxCreatedDto),
    MailboxSessionBound(MailboxSessionBoundDto),
    MailboxContextRecorded(MailboxContextRecordedDto),
    PeerRouteSet(PeerRouteSetDto),
    PeerRouteBlocked(PeerRouteBlockedDto),
    MailboxAccessGranted(MailboxAccessGrantedDto),
    MailboxAccessRevoked(MailboxAccessRevokedDto),
    MailboxActionObserved(MailboxActionObservedDto),
    HumanAccountCreated(HumanAccountCreatedDto),
    HumanAccountSelected(HumanAccountSelectedDto),
    HumanDeviceGranted(HumanDeviceGrantedDto),
    HumanDeviceAccepted(HumanDeviceAcceptedDto),
    HumanDeviceRevoked(HumanDeviceRevokedDto),
    QuestionAsked(MessageDto),
    AsynchronousMessageSent(AsynchronousMessageSentDto),
    AnswerGiven(AnswerGivenDto),
    ThreadCancelled(ThreadCancelledDto),
    MessageArchived(MessageTargetDto),
    MessageRestored(MessageTargetDto),
    MessageRejected(MessageRejectedDto),
    HarnessActivityRecorded(HarnessActivityRecordedDto),
    AgentNameClaimed(AgentNameClaimedDto),
    AgentRetired(AgentRetiredDto),
    ProviderSessionSelected(ProviderSessionSelectedDto),
    ProviderSessionRenamed(ProviderSessionRenamedDto),
    ProjectCreated(ProjectCreatedDto),
    ProjectOpened(ProjectTargetDto),
    ProjectClosingStarted(ProjectTargetDto),
    ProjectClosed(ProjectClosedDto),
    ProjectArchived(ProjectTargetDto),
    ProjectUnarchived(ProjectTargetDto),
    ProjectMetadataUpdated(ProjectMetadataUpdatedDto),
    ProjectResourceAdded(ProjectResourceAddedDto),
    ProjectResourceRemoved(ProjectResourceRemovedDto),
    ProjectResourceReplaced(ProjectResourceReplacedDto),
    ProjectPrimaryResourceChanged(ProjectResourceTargetDto),
    ProjectResourceHealthObserved(ProjectResourceHealthObservedDto),
    ProjectAssignmentConfiguring(ProjectAssignmentConfiguringDto),
    ProjectAssignmentRunnable(ProjectAssignmentRunnableDto),
    ProjectAssignmentBlocked(ProjectAssignmentBlockedDto),
    ProjectAssignmentEnded(ProjectAssignmentEndedDto),
    ProjectInputAccepted(ProjectInputAcceptedDto),
    ProjectInputDispatched(ProjectInputDispatchedDto),
    ProjectOutputRecorded(ProjectOutputRecordedDto),
    RemoteProjectCommandRequested(RemoteProjectCommandRequestedDto),
    RemoteProjectCommandReceipt(RemoteProjectCommandReceiptDto),
    RemoteProjectCommandOutcome(RemoteProjectCommandOutcomeDto),
}

impl BodyDto {
    pub(super) const fn family(&self) -> u64 {
        match self {
            Self::InstallationDeclared(_) => 1,
            Self::MailboxCreated(_) => 2,
            Self::MailboxSessionBound(_) => 3,
            Self::MailboxContextRecorded(_) => 4,
            Self::PeerRouteSet(_) => 5,
            Self::PeerRouteBlocked(_) => 6,
            Self::MailboxAccessGranted(_) => 7,
            Self::MailboxAccessRevoked(_) => 8,
            Self::MailboxActionObserved(_) => 9,
            Self::HumanAccountCreated(_) => 10,
            Self::HumanAccountSelected(_) => 11,
            Self::HumanDeviceGranted(_) => 12,
            Self::HumanDeviceAccepted(_) => 13,
            Self::HumanDeviceRevoked(_) => 14,
            Self::QuestionAsked(_) => 15,
            Self::AsynchronousMessageSent(_) => 16,
            Self::AnswerGiven(_) => 17,
            Self::ThreadCancelled(_) => 18,
            Self::MessageArchived(_) => 19,
            Self::MessageRestored(_) => 20,
            Self::MessageRejected(_) => 21,
            Self::HarnessActivityRecorded(_) => 22,
            Self::AgentNameClaimed(_) => 23,
            Self::AgentRetired(_) => 24,
            Self::ProviderSessionSelected(_) => 25,
            Self::ProviderSessionRenamed(_) => 26,
            Self::ProjectCreated(_) => 27,
            Self::ProjectOpened(_) => 28,
            Self::ProjectClosingStarted(_) => 29,
            Self::ProjectClosed(_) => 30,
            Self::ProjectArchived(_) => 31,
            Self::ProjectUnarchived(_) => 32,
            Self::ProjectMetadataUpdated(_) => 33,
            Self::ProjectResourceAdded(_) => 34,
            Self::ProjectResourceRemoved(_) => 35,
            Self::ProjectResourceReplaced(_) => 36,
            Self::ProjectPrimaryResourceChanged(_) => 37,
            Self::ProjectResourceHealthObserved(_) => 38,
            Self::ProjectAssignmentConfiguring(_) => 39,
            Self::ProjectAssignmentRunnable(_) => 40,
            Self::ProjectAssignmentBlocked(_) => 41,
            Self::ProjectAssignmentEnded(_) => 42,
            Self::ProjectInputAccepted(_) => 43,
            Self::ProjectInputDispatched(_) => 44,
            Self::ProjectOutputRecorded(_) => 45,
            Self::RemoteProjectCommandRequested(_) => 46,
            Self::RemoteProjectCommandReceipt(_) => 47,
            Self::RemoteProjectCommandOutcome(_) => 48,
        }
    }
}

impl Serialize for BodyDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        macro_rules! serialize_body {
            ($($variant:ident),+ $(,)?) => {
                match self {
                    $(Self::$variant(body) => body.serialize(serializer)),+
                }
            };
        }
        serialize_body!(
            InstallationDeclared,
            MailboxCreated,
            MailboxSessionBound,
            MailboxContextRecorded,
            PeerRouteSet,
            PeerRouteBlocked,
            MailboxAccessGranted,
            MailboxAccessRevoked,
            MailboxActionObserved,
            HumanAccountCreated,
            HumanAccountSelected,
            HumanDeviceGranted,
            HumanDeviceAccepted,
            HumanDeviceRevoked,
            QuestionAsked,
            AsynchronousMessageSent,
            AnswerGiven,
            ThreadCancelled,
            MessageArchived,
            MessageRestored,
            MessageRejected,
            HarnessActivityRecorded,
            AgentNameClaimed,
            AgentRetired,
            ProviderSessionSelected,
            ProviderSessionRenamed,
            ProjectCreated,
            ProjectOpened,
            ProjectClosingStarted,
            ProjectClosed,
            ProjectArchived,
            ProjectUnarchived,
            ProjectMetadataUpdated,
            ProjectResourceAdded,
            ProjectResourceRemoved,
            ProjectResourceReplaced,
            ProjectPrimaryResourceChanged,
            ProjectResourceHealthObserved,
            ProjectAssignmentConfiguring,
            ProjectAssignmentRunnable,
            ProjectAssignmentBlocked,
            ProjectAssignmentEnded,
            ProjectInputAccepted,
            ProjectInputDispatched,
            ProjectOutputRecorded,
            RemoteProjectCommandRequested,
            RemoteProjectCommandReceipt,
            RemoteProjectCommandOutcome,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ContentDto {
    pub(super) protocol: ProtocolDto,
    pub(super) version: u64,
    pub(super) family: u64,
    pub(super) author: Hex32,
    pub(super) time: Milliseconds,
    pub(super) scope: ScopeDto,
    pub(super) parents: Vec<ParentDto>,
    pub(super) authorities: Vec<AuthorityDto>,
    pub(super) body: BodyDto,
}

impl Serialize for ContentDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("ContentDto", 9)?;
        state.serialize_field("p", &self.protocol)?;
        state.serialize_field("v", &self.version)?;
        state.serialize_field("f", &self.family)?;
        state.serialize_field("author", &self.author)?;
        state.serialize_field("time", &self.time)?;
        state.serialize_field("scope", &self.scope)?;
        state.serialize_field("parents", &self.parents)?;
        state.serialize_field("auth", &self.authorities)?;
        state.serialize_field("body", &self.body)?;
        state.end()
    }
}
