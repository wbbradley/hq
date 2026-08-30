//! Pure identity-aware TUI transition algebra.

use std::{
    collections::BTreeMap,
    num::NonZeroU64,
    path::{Component, Path, PathBuf},
    time::Duration,
};

const PERIODIC_REFRESH: Duration = Duration::from_secs(300);
const RETRY_DELAY: Duration = Duration::from_millis(250);
const DRAFT_AUTOSAVE_DELAY: Duration = Duration::from_millis(250);
const COMPLETION_NOTICE_DELAY: Duration = Duration::from_secs(4);
const MAX_DRAFT_BYTES: usize = 16 * 1024;
const MAX_AGENT_TEXT_BYTES: usize = 256;
const MAX_PROJECT_TEXT_BYTES: usize = 16 * 1024;
pub(crate) const WIDE_WIDTH: u16 = 96;

/// Stable identity attached to an asynchronous UI effect and its completion.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EffectId(NonZeroU64);

/// Current shell connection state presented by the UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiConnectionState {
    /// No connection attempt has started.
    Disconnected,
    /// An initial snapshot request is in flight.
    Connecting,
    /// A complete authoritative snapshot is available.
    Ready,
    /// The shell is recovering connectivity or a lost refresh.
    Reconnecting,
    /// The local endpoint has no compatible protocol version.
    Incompatible,
}

/// Current local human-account availability derived by the authoritative client mapper.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiHumanState {
    /// One uniquely selected active human account is available.
    Ready,
    /// A typed condition prevents safe human-account use.
    NeedsAttention(UiHumanIssue),
}

/// Exact reason the local human account cannot currently be used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiHumanIssue {
    /// This installation has no account selection or candidates.
    NoAccountSelected,
    /// One selection record has candidates but no unique active account.
    SelectionCandidates {
        /// Exact causal-maximal account candidates.
        candidates: Vec<[u8; 32]>,
        /// Complete selection frontier.
        frontier: Vec<[u8; 32]>,
    },
    /// More than one local selection projection was present.
    SelectionRecords {
        /// Complete conflicting local selection records.
        records: Vec<UiHumanSelectionEvidence>,
    },
    /// An account is selected but no local creator or membership authority exists.
    SelectedWithoutAuthority {
        /// Selected human account.
        account_id: [u8; 32],
        /// Complete selection frontier.
        selection_frontier: Vec<[u8; 32]>,
    },
    /// This installation has not completed the selected account invitation.
    MembershipPending(UiHumanMembershipEvidence),
    /// This installation was revoked from the selected account.
    MembershipRevoked(UiHumanMembershipEvidence),
    /// Local membership projections or active acceptances are not unique.
    MembershipAuthorityConflict {
        /// Complete matching local membership records.
        records: Vec<UiHumanMembershipEvidence>,
    },
}

/// Technical evidence for one local human-account selection projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiHumanSelectionEvidence {
    /// Exact causal-maximal account candidates.
    pub candidates: Vec<[u8; 32]>,
    /// Unique selected account claimed by this record.
    pub active: Option<[u8; 32]>,
    /// Complete selection frontier.
    pub frontier: Vec<[u8; 32]>,
}

/// Closed local membership classification used by human recovery presentation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiHumanMembershipStatus {
    /// A grant exists without a current exact acceptance.
    Pending,
    /// One or more current exact acceptances exist.
    Active,
    /// A current revoke removes local membership.
    Revoked,
    /// Evidence did not match the closed membership vocabulary.
    Conflicted,
}

/// Technical evidence for one local membership projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiHumanMembershipEvidence {
    /// Selected human account.
    pub account_id: [u8; 32],
    /// Closed projected membership status.
    pub status: UiHumanMembershipStatus,
    /// Complete membership frontier.
    pub frontier: Vec<[u8; 32]>,
    /// Exact active acceptance authorities.
    pub active_acceptances: Vec<[u8; 32]>,
}

/// Top-level semantic section selected by the user.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiSection {
    /// Open human mailbox work.
    Inbox,
    /// Human-authored sent work.
    Sent,
    /// Archived human mailbox work.
    Archived,
    /// Named agents and sessions.
    Agents,
    /// Projects and resources.
    Projects,
}

impl UiSection {
    pub(crate) const ALL: [Self; 5] = [
        Self::Inbox,
        Self::Sent,
        Self::Archived,
        Self::Agents,
        Self::Projects,
    ];

    fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    fn previous(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0);
        Self::ALL[(index + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0)
    }
}

/// Logical focus independent of terminal coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiFocus {
    /// Top-level section navigation.
    Navigation,
    /// Current section content.
    Content,
    /// Open conversation history.
    Conversation,
}

/// Page shown by the persistent contextual-help overlay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiHelpPage {
    /// Plain-language purpose, state, and available actions.
    Context,
    /// Stable identities and recovery evidence for the current context.
    Technical,
}

/// Shell-normalized terminal input understood by the pure model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiInput {
    /// Exit the UI.
    Quit,
    /// Open contextual help from any screen or dialog.
    Help,
    /// Reload the complete authoritative workspace.
    Refresh,
    /// Move focus forward.
    NextFocus,
    /// Move focus backward.
    PreviousFocus,
    /// Select the next top-level section.
    NextSection,
    /// Select the previous top-level section.
    PreviousSection,
    /// Select the next logical row.
    NextItem,
    /// Select the previous logical row.
    PreviousItem,
    /// Activate the selected row.
    Activate,
    /// Request the next reducer-ordered conversation page.
    LoadMore,
    /// Dismiss the current transient interaction.
    Escape,
    /// One printable Unicode scalar from the terminal.
    Character(char),
    /// One bounded pasted text fragment.
    Paste(String),
    /// Delete the preceding Unicode scalar while composing.
    Backspace,
    /// Move the insertion caret one Unicode scalar left.
    MoveCursorLeft,
    /// Move the insertion caret one Unicode scalar right.
    MoveCursorRight,
    /// Move the insertion caret to the beginning of the field.
    MoveCursorHome,
    /// Move the insertion caret to the end of the field.
    MoveCursorEnd,
    /// Delete the Unicode scalar under the insertion caret.
    Delete,
}

/// Passive terminal dimensions supplied by the shell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiSize {
    /// Terminal columns.
    pub width: u16,
    /// Terminal rows.
    pub height: u16,
}

/// Passive shell-normalized status for one summary row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiRowState {
    /// Actionable current work.
    Open,
    /// Work awaiting another actor or external effect.
    Waiting,
    /// Work retained outside the active view.
    Archived,
    /// Work whose current truth is incomplete or conflicted.
    Attention,
}

/// Passive semantic kind for one summary row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiRowKind {
    /// A conversation whose entries are available by bounded page query.
    Conversation,
    /// Inert diagnostic state that cannot be used as an action target.
    Diagnostic,
    /// A named-agent summary.
    Agent,
    /// A project summary.
    Project,
}

/// Passive shell-normalized summary row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiRow {
    /// Stable logical identity used to preserve selection across reloads.
    pub id: String,
    /// Primary bounded display text.
    pub title: String,
    /// Secondary bounded display text.
    pub detail: String,
    /// Typed presentation state.
    pub state: UiRowState,
    /// Typed semantic row kind; never inferred from display text.
    pub kind: UiRowKind,
}

/// Closed message state presented without interpreting prose.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiMessageState {
    /// The message remains open and actionable where its type permits.
    Open,
    /// The message is reversibly archived.
    Archived,
    /// The message was absorbing-rejected.
    Rejected,
}

/// Closed reducer-owned conversation entry family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiConversationEntryKind {
    /// Durable message presentation.
    Message,
    /// Non-actionable durable or coalesced activity presentation.
    Activity,
}

/// Closed activity status presented without parsing a display string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiActivityStatus {
    /// Informational snapshot without a lifecycle claim.
    Snapshot,
    /// Correlated work remains active.
    Running,
    /// Correlated work completed successfully.
    Succeeded,
    /// Correlated work failed with a stable reason code.
    Failed {
        /// Stable bounded failure reason.
        reason: String,
    },
    /// Correlated work was explicitly interrupted.
    Interrupted,
}

/// One typed namespaced technical disclosure section.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiTechnicalSection {
    /// Exact message routing identities.
    Routing {
        /// Full sender mailbox address.
        sender: String,
        /// Full recipient mailbox address when directly addressed.
        recipient: Option<String>,
    },
    /// Typed purpose, presentation, and optional operation correlation.
    Semantics {
        /// Stable purpose label from the protocol enum.
        purpose: String,
        /// Stable presentation label from the protocol enum.
        presentation: String,
        /// Provider namespace when correlated.
        provider: Option<String>,
        /// Provider session when correlated.
        session: Option<String>,
        /// Operation identity when correlated.
        operation: Option<String>,
        /// Project identity when associated.
        project: Option<String>,
    },
    /// Exact causal and delivery evidence identities.
    Evidence {
        /// Stable public message identity.
        message_id: String,
        /// Stable causal thread identity.
        thread_id: String,
        /// Causal-maximal reversible-state frontier.
        state_frontier: Vec<String>,
        /// Peer-authored children proving receipt.
        peer_received_by: Vec<String>,
        /// Normalized question root when present.
        root_fact: Option<String>,
        /// Normalized root public message when present.
        root_message: Option<String>,
        /// Whether the message is currently a ready answer.
        ready_answer: bool,
        /// Whether its question thread has a valid cancellation.
        thread_cancelled: bool,
    },
    /// Typed non-actionable activity metadata.
    Activity {
        /// Positive source sequence selected by the reducer.
        sequence: u64,
        /// Closed activity status.
        status: UiActivityStatus,
        /// Whether content was explicitly truncated at authoring.
        truncated: bool,
    },
}

/// Passive reducer-ordered conversation presentation entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiConversationEntry {
    /// Stable canonical fact identity used as the logical scroll anchor.
    pub id: String,
    /// Typed entry family.
    pub kind: UiConversationEntryKind,
    /// Bounded sanitized display content.
    pub content: String,
    /// Stable display source or status summary.
    pub summary: String,
    /// Typed message state; absent for non-actionable activity.
    pub message_state: Option<UiMessageState>,
    /// Typed canonical action target; absent for activity and diagnostic entries.
    pub message_target: Option<UiMessageTarget>,
    /// Namespaced technical sections, already bounded by the local protocol.
    pub technical: Vec<UiTechnicalSection>,
}

/// Canonical message identity and typed action capability selected by the node mapper.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiMessageTarget {
    /// Stable public message identity.
    pub message_id: [u8; 32],
    /// Whether this message's typed purpose permits a reply interaction.
    pub reply_allowed: bool,
}

/// Resolved direct-message target offered by the authoritative snapshot mapper.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiDirectTarget {
    /// Target installation identity.
    pub installation_id: [u8; 32],
    /// Target mailbox identity.
    pub mailbox_id: [u8; 32],
    /// Bounded resolved display label.
    pub label: String,
}

/// Passive neutral provider choice supplied by the running node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiProvider {
    /// Stable provider namespace retained for typed commands and technical details.
    pub provider: String,
    /// User-facing provider name.
    pub name: String,
    /// Whether the running node can start a new session with this provider.
    pub available: bool,
    /// Whether installation configuration names this provider as the preferred default.
    pub configured_default: bool,
}

/// Closed named-agent lifecycle supplied by the authoritative mapper.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiAgentLifecycle {
    /// One active, unconflicted permanent identity exists.
    Active,
    /// Candidate identity state is ambiguous or incomplete.
    Conflicted,
    /// Retirement is absorbing and the agent is historical only.
    Retired,
}

/// Passive installation-qualified agent mailbox.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiAgentMailbox {
    /// Owning installation identity.
    pub installation_id: [u8; 32],
    /// Mailbox identity.
    pub mailbox_id: [u8; 32],
}

/// Passive durable provider-session presentation for one named agent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiAgentSession {
    /// Neutral provider namespace.
    pub provider: String,
    /// Exact provider-scoped durable session.
    pub session: String,
    /// Unique immutable mailbox binding when resolved.
    pub mailbox: Option<UiAgentMailbox>,
    /// Whether immutable bindings conflict.
    pub conflicted: bool,
    /// Whether this is the resolved durable selection.
    pub selected: bool,
    /// Whether the display-name register is resolved.
    pub name_resolved: bool,
    /// Resolved display name, or `None` for an explicit clear.
    pub display_name: Option<String>,
}

/// User-facing phase of one current project assignment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiAgentAssignmentPhase {
    /// HQ is preparing the provider session and project conversation.
    SettingUp,
    /// The durable assignment is ready to accept project input.
    Ready,
    /// The assignment cannot currently accept project input.
    Blocked,
}

/// Exact project-assignment context retained for agent status and details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiAgentProjectAssignment {
    /// Stable project identity.
    pub project_id: [u8; 32],
    /// Bounded project display name.
    pub project_name: String,
    /// Immutable assignment epoch.
    pub assignment_id: [u8; 32],
    /// Selected provider namespace.
    pub provider: String,
    /// Acknowledged exact provider session, when present.
    pub session: Option<String>,
    /// User-facing assignment phase derived from typed projection fields.
    pub phase: UiAgentAssignmentPhase,
    /// Stable blocking evidence retained for technical details.
    pub blocked: Option<String>,
    /// Whether global project/agent cardinality is conflicted.
    pub cardinality_conflicted: bool,
}

/// Closed reason an agent requires user attention.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiAgentAttentionReason {
    /// Permanent identity claims are incomplete or conflicted.
    IdentityConflict,
    /// More than one project assignment or an explicit cardinality conflict exists.
    AssignmentConflict,
    /// The one current project assignment is explicitly blocked.
    AssignmentBlocked,
}

/// User-facing status for one named agent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiAgentStatus {
    /// The active agent is not currently assigned to project work.
    Unassigned,
    /// One unconflicted current project assignment exists.
    Assigned(UiAgentProjectAssignment),
    /// Identity or assignment state requires explicit inspection or recovery.
    NeedsAttention {
        /// Stable user-facing attention category.
        reason: UiAgentAttentionReason,
        /// Every current assignment retained as exact supporting context.
        assignments: Vec<UiAgentProjectAssignment>,
    },
    /// Retirement is absorbing and the agent is historical only.
    Retired,
}

/// Complete passive named-agent presentation used by search and inspection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiAgent {
    /// Stable durable agent identity.
    pub agent_id: [u8; 32],
    /// Candidate permanent names in stable order.
    pub names: Vec<String>,
    /// Candidate installation-qualified mailboxes in stable order.
    pub mailboxes: Vec<UiAgentMailbox>,
    /// Typed lifecycle.
    pub lifecycle: UiAgentLifecycle,
    /// Whether one durable provider session is selected without conflict.
    pub runnable: bool,
    /// Assignment-aware user-facing status.
    pub status: UiAgentStatus,
    /// Compatible durable provider sessions in stable order.
    pub sessions: Vec<UiAgentSession>,
}

/// Passive desired resource shown with one authoritative project.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiProjectResource {
    /// Stable resource identity.
    pub resource_id: [u8; 32],
    /// Human-readable locator value.
    pub display_path: String,
    /// Canonical resource locator value.
    pub canonical_path: String,
    /// Authoritative health classification.
    pub health: String,
    /// Whether this is the project's primary desired resource.
    pub primary: bool,
    /// Whether the project currently holds the active claim.
    pub active_claim: bool,
    /// Stable identities of projects with conflicting claims.
    pub conflicting_projects: Vec<[u8; 32]>,
}

/// Passive current project-assignment presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiProjectAssignment {
    /// Immutable assignment epoch.
    pub assignment_id: [u8; 32],
    /// Assigned durable named agent.
    pub agent_id: [u8; 32],
    /// Selected provider namespace.
    pub provider: String,
    /// Acknowledged exact provider session.
    pub session: Option<String>,
    /// Stable configuring, runnable, or blocked phase.
    pub phase: String,
    /// Runnable project thread when present.
    pub thread_id: Option<[u8; 32]>,
    /// Acknowledged runtime launch directory.
    pub launch_directory: Option<String>,
    /// Stable blocked cause when present.
    pub blocked: Option<String>,
    /// Whether project/agent cardinality is conflicted.
    pub cardinality_conflicted: bool,
    /// Whether the assignment is currently runnable.
    pub runnable: bool,
}

/// Passive exact historical provider-session/project-thread binding.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct UiProjectThread {
    /// Durable agent that owned the thread.
    pub agent_id: [u8; 32],
    /// Provider namespace.
    pub provider: String,
    /// Exact durable provider session.
    pub session: String,
    /// Immutable project-scoped thread.
    pub thread_id: [u8; 32],
}

/// Complete passive project presentation used by selection and details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiProject {
    /// Stable project identity.
    pub project_id: [u8; 32],
    /// Stable home installation identity.
    pub home: [u8; 32],
    /// Current authoritative project name.
    pub name: String,
    /// Current authoritative lifecycle classification.
    pub lifecycle: String,
    /// Whether the project is archived.
    pub archived: bool,
    /// Whether its desired resources can currently be claimed.
    pub claimable: bool,
    /// Current authoritative assignment, when present.
    pub assignment: Option<UiProjectAssignment>,
    /// Complete exact project-scoped historical threads.
    pub threads: Vec<UiProjectThread>,
    /// Exact project head used for optimistic commands.
    pub head: [u8; 32],
    /// Next durable input sequence.
    pub input_sequence: u64,
    /// Desired resources in stable order.
    pub resources: Vec<UiProjectResource>,
}

/// Exact project catalog, creation, or input command chosen by the pure model.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiProjectAction {
    PreviewCreateExisting {
        name: String,
        brief: Option<String>,
        path: String,
    },
    CreateExisting {
        name: String,
        brief: Option<String>,
        path: String,
    },
    CreateWorktree {
        name: String,
        brief: Option<String>,
        source: String,
        destination: String,
        branch: String,
        base: Option<String>,
    },
    SendInput {
        project_id: [u8; 32],
        content: String,
    },
    PreviewAddResource {
        project_id: [u8; 32],
        path: String,
        make_primary: bool,
    },
    AddResource {
        project_id: [u8; 32],
        path: String,
        make_primary: bool,
    },
    PreviewReplaceResource {
        project_id: [u8; 32],
        resource_id: [u8; 32],
        path: String,
    },
    ReplaceResource {
        project_id: [u8; 32],
        resource_id: [u8; 32],
        path: String,
    },
    RemoveResource {
        project_id: [u8; 32],
        resource_id: [u8; 32],
        force: bool,
    },
    SetPrimaryResource {
        project_id: [u8; 32],
        resource_id: [u8; 32],
    },
    CheckResources {
        project_id: [u8; 32],
        resource_id: Option<[u8; 32]>,
    },
    Activate {
        project_id: [u8; 32],
        agent_id: [u8; 32],
        provider: String,
        resume_session: Option<String>,
        resume_thread: Option<[u8; 32]>,
        launch_directory: String,
    },
    DispatchPending {
        project_id: [u8; 32],
    },
    Handoff {
        project_id: [u8; 32],
        agent_id: [u8; 32],
        provider: String,
        resume_session: Option<String>,
        thread_id: [u8; 32],
        launch_directory: String,
        force_takeover: bool,
    },
    Open {
        project_id: [u8; 32],
    },
    PreviewClose {
        project_id: [u8; 32],
    },
    Close {
        project_id: [u8; 32],
        force: bool,
    },
    SetArchived {
        project_id: [u8; 32],
        archived: bool,
    },
}

/// Passive domain-selected overlap for one proposed desired resource.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiProjectResourceConflict {
    /// Stable conflicting project identity.
    pub project_id: [u8; 32],
    /// Stable conflicting desired-resource identity.
    pub resource_id: [u8; 32],
    /// Conflicting display locator.
    pub display_path: String,
    /// Conflicting canonical locator.
    pub canonical_path: String,
    /// Exact equal, ancestor, or descendant relationship.
    pub relationship: String,
}

/// Passive fresh resource-inspection result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiProjectResourceCheck {
    /// Stable desired-resource identity.
    pub resource_id: [u8; 32],
    /// Stable accepted, rejected, uncertain, or response-lost status.
    pub status: String,
    /// Fresh health classification when accepted.
    pub health: Option<String>,
    /// Fresh release classification when accepted.
    pub release: Option<String>,
    /// Fresh canonical locator when observed.
    pub observed_canonical_path: Option<String>,
    /// Bounded inert adapter detail.
    pub details: Option<String>,
    /// Stable rejection category.
    pub error_category: Option<String>,
    /// Stable rejection code.
    pub error_code: Option<String>,
    /// Stable reconciliation identity when uncertain.
    pub reconciliation_id: Option<[u8; 32]>,
}

/// Typed project workflow outcome retained without parsing display text.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiProjectOutcome {
    Completed {
        project_head: Option<[u8; 32]>,
    },
    Running {
        stage: String,
    },
    Rejected {
        category: String,
        code: String,
    },
    Reconcilable {
        stage: String,
        category: String,
        code: String,
        warning: Option<UiProjectExternalWarning>,
    },
    InputSent {
        message_id: [u8; 32],
    },
    ResourcePreview {
        display_path: String,
        canonical_path: String,
        conflicts: Vec<UiProjectResourceConflict>,
    },
    ResourceChecks {
        checks: Vec<UiProjectResourceCheck>,
    },
}

/// Passive external Git state that HQ deliberately does not remove.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiProjectExternalWarning {
    /// Stable external-state warning kind.
    pub kind: String,
    /// Worktree destination retained outside HQ authority.
    pub destination: String,
    /// Git branch retained outside HQ authority.
    pub branch: String,
}

/// Passive completion evidence for one stable project workflow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiProjectResult {
    /// Exact action submitted by the model.
    pub action: UiProjectAction,
    /// Stable command identity used for retries.
    pub command_id: [u8; 32],
    /// Stable operation identity used for reconciliation.
    pub operation_id: [u8; 32],
    /// Project affected by the operation.
    pub project_id: [u8; 32],
    /// Stable succeeded, failed, or uncertain runtime observation.
    pub runtime_state: Option<String>,
    /// Stable runtime failure or uncertainty code.
    pub runtime_code: Option<String>,
    /// Typed authoritative outcome.
    pub outcome: UiProjectOutcome,
}

/// Editable field selected in a pure project form.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum UiProjectFormField {
    /// Project name.
    Name,
    /// Optional project brief.
    Brief,
    /// Existing working-tree path.
    Path,
    /// Source Git working tree.
    Source,
    /// New worktree destination.
    Destination,
    /// New worktree branch.
    Branch,
    /// Optional worktree base revision.
    Base,
    /// Project input content.
    Content,
    /// Provider namespace.
    Provider,
    /// Runtime launch directory.
    Directory,
    /// Whether a new resource becomes the project's primary resource.
    Primary,
    /// Stable named-agent selection.
    Agent,
    /// New-session or exact-resume mode.
    SessionMode,
    /// Exact project-scoped historical thread.
    Thread,
    /// Separate handoff confirmation.
    Confirmation,
    /// Separate force-takeover authorization.
    Force,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum UiFormField {
    Project(UiProjectFormField),
    AgentSearch,
    ProjectSearch,
    AgentName,
    SessionName,
    Message,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UiFormKind {
    AgentSearch,
    ProjectSearch,
    AgentCreate,
    AgentRename,
    ProjectCreateExisting,
    ProjectCreateWorktree,
    ProjectInput,
    ProjectAddResource,
    ProjectReplaceResource,
    ProjectActivate,
    ProjectHandoff,
    ProjectConfirmClose,
    MailboxCompose,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct UiFormState {
    active: Option<UiFormKind>,
    focused: Option<UiFormField>,
    cursors: BTreeMap<UiFormField, usize>,
    errors: BTreeMap<UiFormField, String>,
}

#[derive(Clone, Copy)]
enum TextEdit<'a> {
    Insert(&'a str),
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
}

/// Current project catalog, creation, input, or outcome interaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiProjectCreationChoice {
    /// Record an existing folder as the project's first owned resource.
    ExistingFolder,
    /// Create a separate Git branch and worktree as an advanced convenience.
    IsolatedWorktree,
}

/// High-level intent selected from the global `New...` launcher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiNewChoice {
    /// Start or continue agent work in a project.
    ProjectWork,
    /// Compose to one typed reachable recipient.
    DirectMessage,
    /// Write a durable note addressed only to the local human.
    PersonalNote,
}

impl UiNewChoice {
    const ALL: [Self; 3] = [Self::ProjectWork, Self::DirectMessage, Self::PersonalNote];

    fn cycle(self, forward: bool) -> Self {
        let current = Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % Self::ALL.len()
        } else {
            current.checked_sub(1).unwrap_or(Self::ALL.len() - 1)
        };
        Self::ALL[next]
    }
}

/// Current interaction in the global, intent-oriented `New...` workflow.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiNewModal {
    Launcher {
        selected: UiNewChoice,
    },
    ChooseProject {
        projects: Vec<UiProject>,
        selected: Option<[u8; 32]>,
        create_new: bool,
    },
    ChooseAgent {
        project: UiProject,
        agents: Vec<UiAgent>,
        selected: Option<[u8; 32]>,
        create_new: bool,
    },
    ChooseProvider {
        project: UiProject,
        agent: UiAgent,
        providers: Vec<UiProvider>,
        provider: String,
    },
    ReviewProject {
        project: UiProject,
        agent: UiAgent,
        provider: String,
        resumes_existing: bool,
        moves_project: bool,
        submitting: bool,
    },
    AgentUnavailable {
        project: UiProject,
        agent: UiAgent,
        competing_project_id: [u8; 32],
        competing_project: String,
    },
    ProjectUnavailable {
        project: UiProject,
        competing_project: Option<String>,
        reason: String,
    },
    Working {
        project: String,
        agent: String,
        stage: String,
    },
}

/// Current project catalog, creation, input, or outcome interaction.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiProjectModal {
    ChooseCreation {
        selected: UiProjectCreationChoice,
    },
    Search {
        query: String,
    },
    Details {
        project: UiProject,
        selected_resource: Option<[u8; 32]>,
    },
    CreateExisting {
        name: String,
        brief: String,
        path: String,
        field: UiProjectFormField,
        submitting: bool,
    },
    CreateWorktree {
        name: String,
        brief: String,
        source: String,
        destination: String,
        branch: String,
        base: String,
        field: UiProjectFormField,
        submitting: bool,
    },
    SendInput {
        project: UiProject,
        content: String,
        submitting: bool,
    },
    AddResource {
        project: UiProject,
        path: String,
        make_primary: bool,
        submitting: bool,
    },
    ReplaceResource {
        project: UiProject,
        resource_id: [u8; 32],
        path: String,
        submitting: bool,
    },
    ConfirmRemoveResource {
        project: UiProject,
        resource_id: [u8; 32],
        force: bool,
        submitting: bool,
    },
    ConfirmPrimaryResource {
        project: UiProject,
        resource_id: [u8; 32],
        submitting: bool,
    },
    Activate {
        project: UiProject,
        agents: Vec<UiAgent>,
        providers: Vec<UiProvider>,
        agent_id: Option<[u8; 32]>,
        thread: Option<UiProjectThread>,
        new_session: bool,
        provider: String,
        directory: String,
        field: UiProjectFormField,
        submitting: bool,
    },
    Handoff {
        project: UiProject,
        agents: Vec<UiAgent>,
        providers: Vec<UiProvider>,
        agent_id: Option<[u8; 32]>,
        thread: Option<UiProjectThread>,
        new_session: bool,
        provider: String,
        directory: String,
        field: UiProjectFormField,
        confirmed: bool,
        force_takeover: bool,
        submitting: bool,
    },
    ConfirmClose {
        project: UiProject,
        checks: Vec<UiProjectResourceCheck>,
        confirmed: bool,
        force: bool,
        submitting: bool,
    },
    ConfirmArchive {
        project: UiProject,
        archived: bool,
        submitting: bool,
    },
    Outcome {
        result: UiProjectResult,
    },
}

/// Exact named-agent administration command chosen by the pure model.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiAgentAction {
    Create {
        name: String,
    },
    RenameSession {
        agent_id: [u8; 32],
        provider: String,
        session: String,
        display_name: Option<String>,
    },
    Retire {
        agent_id: [u8; 32],
        force: bool,
    },
}

/// Exact provider-neutral managed-session command chosen by the pure model.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiManagedSessionAction {
    Start {
        agent_id: [u8; 32],
        provider: String,
    },
    Resume {
        agent_id: [u8; 32],
        provider: String,
        session: String,
    },
    Stop {
        agent_id: [u8; 32],
        provider: String,
    },
}

/// Typed actionable outcome of one stable managed-session operation.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiManagedSessionOutcome {
    Ready { session: String },
    Stopped,
    Rejected { category: String, code: String },
    Uncertain { reconciliation_id: [u8; 32] },
}

/// Passive completion evidence returned by the ordinary local client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiManagedSessionResult {
    /// Exact command whose stable operation completed.
    pub action: UiManagedSessionAction,
    /// Retry-safe operation identity allocated by the shared command workflow.
    pub operation_id: [u8; 32],
    /// Typed operation outcome; this is not inferred runtime presence.
    pub outcome: UiManagedSessionOutcome,
}

/// Current named-agent search, inspection, or administration interaction.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiAgentModal {
    Search {
        query: String,
    },
    Details {
        agent: UiAgent,
        selected_session: Option<(String, String)>,
    },
    Create {
        name: String,
        submitting: bool,
    },
    RenameSession {
        agent_id: [u8; 32],
        provider: String,
        session: String,
        display_name: String,
        submitting: bool,
    },
    ConfirmRetire {
        agent: UiAgent,
        force: bool,
        submitting: bool,
    },
    ManagedProvider {
        agent: UiAgent,
        providers: Vec<UiProvider>,
        selected: Option<String>,
    },
    ConfirmManagedSession {
        agent: UiAgent,
        action: UiManagedSessionAction,
    },
    ManagingSession {
        agent: UiAgent,
        action: UiManagedSessionAction,
    },
    ManagedSessionOutcome {
        agent: UiAgent,
        result: UiManagedSessionResult,
    },
}

/// Explicit semantic target retained with one installation-local draft.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiMailboxDraftTarget {
    /// Reply to one exact message.
    Reply { message_id: [u8; 32] },
    /// Send to one exact installation-qualified mailbox.
    Direct {
        installation_id: [u8; 32],
        mailbox_id: [u8; 32],
    },
    /// Send a note to the local human mailbox.
    SelfNote,
}

/// Complete passive local draft returned by the ordinary client boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiMailboxDraft {
    /// Stable draft identity.
    pub draft_id: [u8; 32],
    /// Exact semantic target.
    pub target: UiMailboxDraftTarget,
    /// Possibly-empty bounded composition text.
    pub content: String,
    /// Optimistic local draft version.
    pub version: u64,
}

/// Canonical mailbox command selected by the pure model.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiMailboxAction {
    /// Reply using the currently loaded draft.
    Reply { target_message: [u8; 32] },
    /// Send the currently loaded draft to an exact mailbox.
    Direct {
        recipient_installation: [u8; 32],
        recipient_mailbox: [u8; 32],
    },
    /// Submit the currently loaded self-note draft.
    SelfNote,
    /// Archive one exact message.
    Archive { target_message: [u8; 32] },
    /// Restore one exact message.
    Restore { target_message: [u8; 32] },
}

/// Current mailbox modal presentation borrowed by the renderer.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiMailboxModal {
    /// Select one resolved direct-message target by stable mailbox identity.
    SelectDirect {
        /// Current authoritative resolved candidates.
        targets: Vec<UiDirectTarget>,
        /// Stable selected mailbox identity.
        selected: Option<([u8; 32], [u8; 32])>,
    },
    /// An applicable draft is being loaded or created.
    LoadingDraft { target: UiMailboxDraftTarget },
    /// Edit one durable draft.
    Compose {
        /// Latest local draft state, including unsaved content.
        draft: UiMailboxDraft,
        /// Whether text differs from the last acknowledged version.
        dirty: bool,
        /// Whether submit is waiting for the latest autosave.
        submitting: bool,
        /// Whether cancellation is waiting for the latest autosave.
        closing: bool,
    },
    /// Confirm a reversible canonical state command.
    Confirm { action: UiMailboxAction },
}

/// Passive bounded page returned by the ordinary local API client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiConversationPage {
    /// Stable summary-row identity requested by the model.
    pub row_id: String,
    /// Reducer-ordered entries for this page.
    pub entries: Vec<UiConversationEntry>,
    /// Opaque continuation cursor when more reducer-ordered entries exist.
    pub next_cursor: Option<String>,
}

/// Passive accumulated conversation presentation held by the pure model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiConversation {
    /// Stable summary-row identity.
    pub row_id: String,
    /// Reducer-ordered entries loaded so far.
    pub entries: Vec<UiConversationEntry>,
    /// Opaque next-page cursor.
    pub next_cursor: Option<String>,
}

/// Passive complete UI snapshot produced from one authoritative local-API snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiSnapshot {
    /// Serialized authoritative revision.
    pub revision: u64,
    /// Current local human-account availability.
    pub human_state: UiHumanState,
    /// Reducer-ordered open human mailbox rows.
    pub inbox_rows: Vec<UiRow>,
    /// Reducer-ordered human-authored rows.
    pub sent_rows: Vec<UiRow>,
    /// Reducer-ordered archived human mailbox rows.
    pub archived_rows: Vec<UiRow>,
    /// Stable named-agent summary rows.
    pub agent_rows: Vec<UiRow>,
    /// Stable project summary rows.
    pub project_rows: Vec<UiRow>,
    /// Resolved named-agent mailboxes available for direct composition.
    pub direct_targets: Vec<UiDirectTarget>,
    /// Providers registered with the running installation in stable order.
    pub providers: Vec<UiProvider>,
    /// Complete named-agent records for cached navigation and detail views.
    pub agents: Vec<UiAgent>,
    /// Complete passive project catalog for cached navigation and detail views.
    pub projects: Vec<UiProject>,
}

impl UiSnapshot {
    /// Borrows the rows for one selected semantic section.
    pub fn rows(&self, section: UiSection) -> &[UiRow] {
        match section {
            UiSection::Inbox => &self.inbox_rows,
            UiSection::Sent => &self.sent_rows,
            UiSection::Archived => &self.archived_rows,
            UiSection::Agents => &self.agent_rows,
            UiSection::Projects => &self.project_rows,
        }
    }
}

/// Passive stable actionable failure shown without behavioral prose parsing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiFailure {
    /// Stable machine-readable failure code.
    pub code: String,
    /// Bounded safe operator action.
    pub action: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UiTransientHelp {
    OpenConversationMessage,
    SelectConversationMessage,
}

impl UiTransientHelp {
    const fn text(self) -> &'static str {
        match self {
            Self::OpenConversationMessage => {
                "open the conversation with Enter, then select a message to reply, archive, or restore"
            }
            Self::SelectConversationMessage => {
                "select a message; activity updates cannot be replied to, archived, or restored"
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UiCompletionNotice {
    AgentReady,
    AgentStopped,
    ProjectCreated,
    ProjectUpdated,
    InstructionsSent,
    ProjectWorkReady,
}

impl UiCompletionNotice {
    const fn text(self) -> &'static str {
        match self {
            Self::AgentReady => "Agent conversation ready",
            Self::AgentStopped => "Agent stopped; saved conversation kept",
            Self::ProjectCreated => "Project created",
            Self::ProjectUpdated => "Project updated",
            Self::InstructionsSent => "Instructions sent",
            Self::ProjectWorkReady => "Project work is ready",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UiProjectCompletionContinuation {
    Select,
    Details,
    ComposeInput,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum UiCompletionContext {
    Agent {
        agent_id: [u8; 32],
        selected_session: Option<(String, String)>,
    },
    Project {
        project_id: [u8; 32],
        continuation: UiProjectCompletionContinuation,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UiCompletionRefresh {
    Initial,
    Followup,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UiPendingCompletion {
    target: UiCompletionContext,
    refresh: UiCompletionRefresh,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UiGuidedSubmission {
    project_id: [u8; 32],
    agent_id: [u8; 32],
    provider: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum UiGuidedPending {
    ProjectCreation,
    ProjectSnapshot {
        project_id: [u8; 32],
    },
    AgentCreation {
        project_id: [u8; 32],
        expected_name: Option<String>,
    },
    Activation(UiGuidedSubmission),
}

/// Closed timer purpose owned by the shell effect executor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiTimerKind {
    /// Periodic full-snapshot repair.
    PeriodicRefresh,
    /// Bounded retry after a failed snapshot request.
    RetrySnapshot,
    /// Debounced local draft autosave.
    AutosaveDraft,
    /// Bounded routine-completion confirmation.
    DismissCompletion,
}

/// Closed event vocabulary accepted by the pure UI model.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiEvent {
    /// Start the model exactly once.
    Started,
    /// Normalized terminal input.
    Input(UiInput),
    /// Complete terminal resize.
    Resized(UiSize),
    /// One scheduled timer elapsed.
    TimerElapsed {
        /// Identity of the completed timer effect.
        effect_id: EffectId,
    },
    /// One authoritative snapshot request completed.
    SnapshotLoaded {
        /// Identity of the completed snapshot effect.
        effect_id: EffectId,
        /// Complete shell-normalized snapshot.
        snapshot: UiSnapshot,
    },
    /// One authoritative snapshot request failed.
    SnapshotFailed {
        /// Identity of the completed snapshot effect.
        effect_id: EffectId,
        /// Stable actionable failure.
        failure: UiFailure,
    },
    /// One reducer-ordered conversation page request completed.
    ConversationLoaded {
        /// Identity of the completed page effect.
        effect_id: EffectId,
        /// Complete passive page for the requested row.
        page: UiConversationPage,
    },
    /// One conversation page request failed.
    ConversationFailed {
        /// Identity of the completed page effect.
        effect_id: EffectId,
        /// Stable actionable failure.
        failure: UiFailure,
    },
    /// One applicable local draft was loaded or created.
    DraftLoaded {
        /// Identity of the completed open-draft effect.
        effect_id: EffectId,
        /// Complete current draft.
        draft: UiMailboxDraft,
    },
    /// One local draft autosave completed.
    DraftSaved {
        /// Identity of the completed save effect.
        effect_id: EffectId,
        /// Complete acknowledged draft.
        draft: UiMailboxDraft,
    },
    /// One local draft operation failed without losing editor text.
    DraftFailed {
        /// Identity of the failed draft effect.
        effect_id: EffectId,
        /// Stable actionable failure.
        failure: UiFailure,
        /// Current server draft on an optimistic conflict, when available.
        current: Option<UiMailboxDraft>,
    },
    /// One stable mailbox command committed canonically.
    MailboxCommandCommitted {
        /// Identity of the completed command effect.
        effect_id: EffectId,
        /// Durable transaction revision.
        revision: u64,
    },
    /// One stable mailbox command was rejected or could not be completed.
    MailboxCommandFailed {
        /// Identity of the failed command effect.
        effect_id: EffectId,
        /// Stable actionable failure.
        failure: UiFailure,
    },
    /// One stable named-agent administration command completed.
    AgentCommandCommitted {
        /// Identity of the completed command effect.
        effect_id: EffectId,
        /// Authoritative revision observed after completion.
        revision: u64,
    },
    /// One named-agent administration command was rejected or remained uncertain.
    AgentCommandFailed {
        /// Identity of the failed command effect.
        effect_id: EffectId,
        /// Stable actionable failure.
        failure: UiFailure,
    },
    /// One stable managed-session command reached a typed outcome.
    ManagedSessionCompleted {
        /// Identity of the completed model effect.
        effect_id: EffectId,
        /// Passive operation evidence returned by the ordinary client.
        result: UiManagedSessionResult,
    },
    /// One managed-session command could not reach the local API.
    ManagedSessionFailed {
        /// Identity of the failed model effect.
        effect_id: EffectId,
        /// Stable actionable client failure.
        failure: UiFailure,
    },
    /// One stable project command reached a typed outcome.
    ProjectCommandCompleted {
        /// Identity of the completed model effect.
        effect_id: EffectId,
        /// Passive operation evidence returned by the ordinary client.
        result: UiProjectResult,
    },
    /// One project command could not reach the local API.
    ProjectCommandFailed {
        /// Identity of the failed model effect.
        effect_id: EffectId,
        /// Stable actionable client failure.
        failure: UiFailure,
    },
    /// A revision-only wake marked the current snapshot stale.
    Invalidated {
        /// Greatest revision known to the shell.
        revision: u64,
    },
    /// The reconnecting client reported a generation-scoped state.
    ConnectionObserved {
        /// Monotonic shell connection generation.
        generation: u64,
        /// State observed for that generation.
        state: UiConnectionState,
    },
    /// The reconnecting client reported a stable generation-scoped failure.
    ClientFailed {
        /// Monotonic shell connection generation.
        generation: u64,
        /// Stable actionable client failure.
        failure: UiFailure,
    },
}

/// Closed side effects emitted by pure UI transitions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiEffect {
    /// Request one complete authoritative snapshot through the ordinary client.
    LoadSnapshot {
        /// Identity required on the completion event.
        id: EffectId,
    },
    /// Request one bounded reducer-ordered conversation page.
    LoadConversation {
        /// Identity required on the completion event.
        id: EffectId,
        /// Stable summary-row identity selected by the model.
        row_id: String,
        /// Opaque continuation cursor; absent for the first page.
        cursor: Option<String>,
    },
    /// Load one applicable draft by semantic target, creating it when absent.
    OpenDraft {
        /// Identity required on the completion event.
        id: EffectId,
        /// Exact semantic draft target.
        target: UiMailboxDraftTarget,
    },
    /// Persist one complete optimistic local draft replacement.
    SaveDraft {
        /// Identity required on the completion event.
        id: EffectId,
        /// Complete locally edited draft.
        draft: UiMailboxDraft,
    },
    /// Execute or reconcile one stable authoritative mailbox command.
    SubmitMailboxCommand {
        /// Identity required on the completion event.
        id: EffectId,
        /// Draft consumed only if the command commits.
        draft: Option<UiMailboxDraft>,
        /// Exact typed action selected by the model.
        action: UiMailboxAction,
    },
    /// Execute or reconcile one stable named-agent administration command.
    SubmitAgentCommand {
        /// Identity required on the completion event.
        id: EffectId,
        /// Exact typed command selected by the model.
        action: UiAgentAction,
    },
    /// Execute or reconcile one stable provider-neutral managed-session command.
    SubmitManagedSession {
        /// Identity required on the completion event.
        id: EffectId,
        /// Exact typed command selected by the model.
        action: UiManagedSessionAction,
    },
    /// Execute or reconcile one stable project command.
    SubmitProjectCommand {
        /// Identity required on the completion event.
        id: EffectId,
        /// Exact typed command selected by the model.
        action: UiProjectAction,
    },
    /// Schedule one bounded timer through the shell clock.
    ScheduleTimer {
        /// Identity required on the completion event.
        id: EffectId,
        /// Closed timer purpose.
        kind: UiTimerKind,
        /// Exact delay requested from the shell.
        after: Duration,
    },
    /// Coalescible request to render the latest borrowed model.
    RequestRedraw,
    /// Leave the terminal loop after restoration ownership unwinds.
    Exit,
}

/// Pure transition result with the complete next model and ordered effects.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiTransition {
    /// Complete next model.
    pub model: UiModel,
    /// Ordered effects for the shell executor.
    pub effects: Vec<UiEffect>,
}

/// Closed model transition failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiError {
    /// The one-time start event was repeated.
    AlreadyStarted,
    /// The process-local effect identity space was exhausted.
    EffectIdentityExhausted,
    /// A shell returned a page for a row other than the exact requested row.
    ConversationRowMismatch,
}

impl std::fmt::Display for UiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "TUI transition failed: {self:?}")
    }
}

impl std::error::Error for UiError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingSnapshot {
    id: EffectId,
    minimum_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingConversation {
    id: EffectId,
    row_id: String,
    cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingMailboxKind {
    OpenDraft,
    SaveDraft,
    SubmitCommand,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingMailbox {
    id: EffectId,
    kind: PendingMailboxKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UiSectionWorkspace {
    selected_row: Option<String>,
    conversation: Option<UiConversation>,
    conversation_anchor: Option<String>,
    technical_visible: bool,
    focus: UiFocus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingProject {
    id: EffectId,
    action: UiProjectAction,
}

/// Complete invariant-bearing TUI application state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiModel {
    viewport: UiSize,
    connection: UiConnectionState,
    connection_generation: u64,
    section: UiSection,
    focus: UiFocus,
    snapshot: Option<UiSnapshot>,
    selected_row: Option<String>,
    conversation: Option<UiConversation>,
    conversation_anchor: Option<String>,
    technical_visible: bool,
    mailbox_modal: Option<UiMailboxModal>,
    agent_modal: Option<UiAgentModal>,
    project_modal: Option<UiProjectModal>,
    new_modal: Option<UiNewModal>,
    agent_search: String,
    project_search: String,
    home_directory: Option<String>,
    form: UiFormState,
    help_page: Option<UiHelpPage>,
    required_revision: Option<u64>,
    pending_snapshot: Option<PendingSnapshot>,
    pending_conversation: Option<PendingConversation>,
    pending_mailbox: Option<PendingMailbox>,
    pending_agent: Option<EffectId>,
    pending_managed_session: Option<EffectId>,
    pending_project: Option<PendingProject>,
    section_workspaces: [Option<UiSectionWorkspace>; 5],
    periodic_timer: Option<EffectId>,
    retry_timer: Option<EffectId>,
    autosave_timer: Option<EffectId>,
    completion_timer: Option<EffectId>,
    next_effect_id: Option<NonZeroU64>,
    last_failure: Option<UiFailure>,
    transient_help: Option<UiTransientHelp>,
    completion_notice: Option<UiCompletionNotice>,
    completion_context: Option<UiPendingCompletion>,
    guided_pending: Option<UiGuidedPending>,
    started: bool,
    should_exit: bool,
}

impl UiModel {
    /// Constructs a disconnected model without performing any effects.
    pub const fn new(viewport: UiSize) -> Self {
        Self {
            viewport,
            connection: UiConnectionState::Disconnected,
            connection_generation: 0,
            section: UiSection::Inbox,
            focus: UiFocus::Navigation,
            snapshot: None,
            selected_row: None,
            conversation: None,
            conversation_anchor: None,
            technical_visible: false,
            mailbox_modal: None,
            agent_modal: None,
            project_modal: None,
            new_modal: None,
            agent_search: String::new(),
            project_search: String::new(),
            home_directory: None,
            form: UiFormState {
                active: None,
                focused: None,
                cursors: BTreeMap::new(),
                errors: BTreeMap::new(),
            },
            help_page: None,
            required_revision: None,
            pending_snapshot: None,
            pending_conversation: None,
            pending_mailbox: None,
            pending_agent: None,
            pending_managed_session: None,
            pending_project: None,
            section_workspaces: [None, None, None, None, None],
            periodic_timer: None,
            retry_timer: None,
            autosave_timer: None,
            completion_timer: None,
            next_effect_id: NonZeroU64::new(1),
            last_failure: None,
            transient_help: None,
            completion_notice: None,
            completion_context: None,
            guided_pending: None,
            started: false,
            should_exit: false,
        }
    }

    /// Supplies the current user's absolute home directory for explicit `~` path expansion.
    #[must_use]
    pub fn with_home_directory(mut self, home_directory: Option<String>) -> Self {
        self.home_directory = home_directory;
        self
    }

    pub(crate) fn project_field_cursor(&self, field: UiProjectFormField, value: &str) -> usize {
        self.form_cursor(UiFormField::Project(field), value)
    }

    pub(crate) fn project_field_error(&self, field: UiProjectFormField) -> Option<&str> {
        self.form_error(UiFormField::Project(field))
    }

    pub(crate) fn project_field_is_focused(&self, field: UiProjectFormField) -> bool {
        if self.form.active != self.active_form_kind() {
            return (matches!(self.project_modal, Some(UiProjectModal::AddResource { .. }))
                && field == UiProjectFormField::Path)
                || (matches!(
                    self.project_modal,
                    Some(UiProjectModal::ConfirmClose { .. })
                ) && field == UiProjectFormField::Confirmation);
        }
        self.form.focused == Some(UiFormField::Project(field))
    }

    pub(crate) fn normalized_path_preview(&self, value: &str) -> Result<String, &'static str> {
        normalize_path_input(value, self.home_directory.as_deref())
    }

    pub(crate) fn agent_field_cursor(&self, value: &str) -> usize {
        self.form_cursor(UiFormField::AgentName, value)
    }

    pub(crate) fn session_field_cursor(&self, value: &str) -> usize {
        self.form_cursor(UiFormField::SessionName, value)
    }

    pub(crate) fn message_field_cursor(&self, value: &str) -> usize {
        self.form_cursor(UiFormField::Message, value)
    }

    pub(crate) fn search_field_cursor(&self, value: &str, projects: bool) -> usize {
        self.form_cursor(
            if projects {
                UiFormField::ProjectSearch
            } else {
                UiFormField::AgentSearch
            },
            value,
        )
    }

    pub(crate) fn agent_field_error(&self) -> Option<&str> {
        self.form_error(UiFormField::AgentName)
    }

    pub(crate) fn message_field_error(&self) -> Option<&str> {
        self.form_error(UiFormField::Message)
    }

    fn form_cursor(&self, field: UiFormField, value: &str) -> usize {
        if self.form.active != self.active_form_kind() {
            return value.len();
        }
        let mut cursor = self
            .form
            .cursors
            .get(&field)
            .copied()
            .unwrap_or(value.len())
            .min(value.len());
        while !value.is_char_boundary(cursor) {
            cursor = cursor.saturating_sub(1);
        }
        cursor
    }

    fn form_error(&self, field: UiFormField) -> Option<&str> {
        (self.form.active == self.active_form_kind())
            .then(|| self.form.errors.get(&field).map(String::as_str))
            .flatten()
    }

    fn active_form_kind(&self) -> Option<UiFormKind> {
        match (&self.project_modal, &self.agent_modal, &self.mailbox_modal) {
            (Some(UiProjectModal::Search { .. }), _, _) => Some(UiFormKind::ProjectSearch),
            (Some(UiProjectModal::CreateExisting { .. }), _, _) => {
                Some(UiFormKind::ProjectCreateExisting)
            }
            (Some(UiProjectModal::CreateWorktree { .. }), _, _) => {
                Some(UiFormKind::ProjectCreateWorktree)
            }
            (Some(UiProjectModal::SendInput { .. }), _, _) => Some(UiFormKind::ProjectInput),
            (Some(UiProjectModal::AddResource { .. }), _, _) => {
                Some(UiFormKind::ProjectAddResource)
            }
            (Some(UiProjectModal::ReplaceResource { .. }), _, _) => {
                Some(UiFormKind::ProjectReplaceResource)
            }
            (Some(UiProjectModal::Activate { .. }), _, _) => Some(UiFormKind::ProjectActivate),
            (Some(UiProjectModal::Handoff { .. }), _, _) => Some(UiFormKind::ProjectHandoff),
            (Some(UiProjectModal::ConfirmClose { .. }), _, _) => {
                Some(UiFormKind::ProjectConfirmClose)
            }
            (_, Some(UiAgentModal::Search { .. }), _) => Some(UiFormKind::AgentSearch),
            (_, Some(UiAgentModal::Create { .. }), _) => Some(UiFormKind::AgentCreate),
            (_, Some(UiAgentModal::RenameSession { .. }), _) => Some(UiFormKind::AgentRename),
            (_, _, Some(UiMailboxModal::Compose { .. })) => Some(UiFormKind::MailboxCompose),
            _ => None,
        }
    }

    fn sync_form(&mut self) {
        let active = self.active_form_kind();
        if self.form.active != active {
            let focused = match active {
                Some(UiFormKind::ProjectAddResource) => {
                    Some(UiFormField::Project(UiProjectFormField::Path))
                }
                Some(UiFormKind::ProjectConfirmClose) => {
                    Some(UiFormField::Project(UiProjectFormField::Confirmation))
                }
                _ => None,
            };
            self.form = UiFormState {
                active,
                focused,
                ..UiFormState::default()
            };
        }
    }

    /// Returns the latest terminal dimensions.
    pub const fn viewport(&self) -> UiSize {
        self.viewport
    }

    /// Returns the currently presented connection state.
    pub const fn connection(&self) -> UiConnectionState {
        self.connection
    }

    /// Returns the selected semantic section.
    pub const fn section(&self) -> UiSection {
        self.section
    }

    /// Returns the current logical focus.
    pub const fn focus(&self) -> UiFocus {
        self.focus
    }

    /// Borrows the latest complete snapshot.
    pub const fn snapshot(&self) -> Option<&UiSnapshot> {
        self.snapshot.as_ref()
    }

    /// Borrows the latest rows for the selected semantic section.
    pub fn rows(&self) -> Option<&[UiRow]> {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.rows(self.section))
    }

    /// Returns current local human-account availability when a snapshot exists.
    pub fn human_state(&self) -> Option<&UiHumanState> {
        self.snapshot.as_ref().map(|snapshot| &snapshot.human_state)
    }

    /// Reports whether a fresh authoritative snapshot is loading behind retained content.
    pub const fn refreshing(&self) -> bool {
        self.snapshot.is_some() && self.pending_snapshot.is_some()
    }

    /// Returns the selected stable row identity.
    pub fn selected_row(&self) -> Option<&str> {
        self.selected_row.as_deref()
    }

    /// Borrows the selected row's current authoritative presentation data.
    pub fn selected_row_data(&self) -> Option<&UiRow> {
        let selected = self.selected_row.as_deref()?;
        self.rows()?.iter().find(|row| row.id == selected)
    }

    /// Returns the currently visible contextual-help page.
    pub const fn help_page(&self) -> Option<UiHelpPage> {
        self.help_page
    }

    /// Borrows the reducer-ordered conversation loaded for the selected row.
    pub const fn conversation(&self) -> Option<&UiConversation> {
        self.conversation.as_ref()
    }

    /// Returns the stable selected conversation-entry identity.
    pub fn conversation_anchor(&self) -> Option<&str> {
        self.conversation_anchor.as_deref()
    }

    /// Reports whether typed technical disclosure is expanded.
    pub const fn technical_visible(&self) -> bool {
        self.technical_visible
    }

    /// Borrows the current mailbox interaction, when a modal is open.
    pub const fn mailbox_modal(&self) -> Option<&UiMailboxModal> {
        self.mailbox_modal.as_ref()
    }

    /// Borrows the current named-agent interaction.
    pub const fn agent_modal(&self) -> Option<&UiAgentModal> {
        self.agent_modal.as_ref()
    }

    /// Borrows the current project interaction.
    pub const fn project_modal(&self) -> Option<&UiProjectModal> {
        self.project_modal.as_ref()
    }

    /// Borrows the current global `New...` interaction.
    pub const fn new_modal(&self) -> Option<&UiNewModal> {
        self.new_modal.as_ref()
    }

    /// Returns the retained project search query.
    pub fn project_search(&self) -> &str {
        &self.project_search
    }

    /// Returns the retained named-agent search query.
    pub fn agent_search(&self) -> &str {
        &self.agent_search
    }

    /// Returns the greatest revision required by coalesced invalidations.
    pub const fn required_revision(&self) -> Option<u64> {
        self.required_revision
    }

    /// Returns the current authoritative snapshot effect identity.
    pub const fn pending_snapshot(&self) -> Option<EffectId> {
        match self.pending_snapshot {
            Some(pending) => Some(pending.id),
            None => None,
        }
    }

    /// Returns the current conversation-page effect identity.
    pub const fn pending_conversation(&self) -> Option<EffectId> {
        match &self.pending_conversation {
            Some(pending) => Some(pending.id),
            None => None,
        }
    }

    /// Returns the current draft or mailbox-command effect identity.
    pub const fn pending_mailbox(&self) -> Option<EffectId> {
        match self.pending_mailbox {
            Some(pending) => Some(pending.id),
            None => None,
        }
    }

    /// Returns the current named-agent administration effect identity.
    pub const fn pending_agent(&self) -> Option<EffectId> {
        self.pending_agent
    }

    /// Returns the current stable managed-session effect identity.
    pub const fn pending_managed_session(&self) -> Option<EffectId> {
        self.pending_managed_session
    }

    /// Returns the current stable project command effect identity.
    pub const fn pending_project(&self) -> Option<EffectId> {
        match &self.pending_project {
            Some(pending) => Some(pending.id),
            None => None,
        }
    }

    /// Borrows the latest matching failure.
    pub const fn last_failure(&self) -> Option<&UiFailure> {
        self.last_failure.as_ref()
    }

    /// Returns transient prerequisite guidance produced by the latest input.
    pub fn transient_help(&self) -> Option<&'static str> {
        self.transient_help.map(UiTransientHelp::text)
    }

    /// Returns the current bounded routine-completion confirmation.
    pub fn completion_notice(&self) -> Option<&'static str> {
        self.completion_notice.map(UiCompletionNotice::text)
    }

    /// Reports whether the model requested loop exit.
    pub const fn should_exit(&self) -> bool {
        self.should_exit
    }

    fn allocate_effect(&mut self) -> Result<EffectId, UiError> {
        let current = self
            .next_effect_id
            .ok_or(UiError::EffectIdentityExhausted)?;
        self.next_effect_id = current.get().checked_add(1).and_then(NonZeroU64::new);
        Ok(EffectId(current))
    }

    fn request_snapshot(&mut self, effects: &mut Vec<UiEffect>) -> Result<(), UiError> {
        if self.pending_snapshot.is_some() {
            return Ok(());
        }
        let id = self.allocate_effect()?;
        let minimum_revision = self.required_revision.unwrap_or_else(|| {
            self.snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.revision)
        });
        self.pending_snapshot = Some(PendingSnapshot {
            id,
            minimum_revision,
        });
        effects.push(UiEffect::LoadSnapshot { id });
        Ok(())
    }

    fn request_conversation(
        &mut self,
        row_id: String,
        cursor: Option<String>,
        effects: &mut Vec<UiEffect>,
    ) -> Result<(), UiError> {
        if self.pending_conversation.is_some() {
            return Ok(());
        }
        let id = self.allocate_effect()?;
        self.pending_conversation = Some(PendingConversation {
            id,
            row_id: row_id.clone(),
            cursor: cursor.clone(),
        });
        effects.push(UiEffect::LoadConversation { id, row_id, cursor });
        Ok(())
    }

    fn open_draft(
        &mut self,
        target: UiMailboxDraftTarget,
        effects: &mut Vec<UiEffect>,
    ) -> Result<(), UiError> {
        if self.pending_mailbox.is_some() {
            return Ok(());
        }
        let id = self.allocate_effect()?;
        self.pending_mailbox = Some(PendingMailbox {
            id,
            kind: PendingMailboxKind::OpenDraft,
        });
        self.mailbox_modal = Some(UiMailboxModal::LoadingDraft {
            target: target.clone(),
        });
        effects.push(UiEffect::OpenDraft { id, target });
        Ok(())
    }

    fn save_draft(&mut self, effects: &mut Vec<UiEffect>) -> Result<(), UiError> {
        if self.pending_mailbox.is_some() {
            return Ok(());
        }
        let Some(UiMailboxModal::Compose {
            draft, dirty: true, ..
        }) = &self.mailbox_modal
        else {
            return Ok(());
        };
        let draft = draft.clone();
        let id = self.allocate_effect()?;
        self.pending_mailbox = Some(PendingMailbox {
            id,
            kind: PendingMailboxKind::SaveDraft,
        });
        effects.push(UiEffect::SaveDraft { id, draft });
        Ok(())
    }

    fn submit_mailbox(
        &mut self,
        draft: Option<UiMailboxDraft>,
        action: UiMailboxAction,
        effects: &mut Vec<UiEffect>,
    ) -> Result<(), UiError> {
        if self.pending_mailbox.is_some() {
            return Ok(());
        }
        let id = self.allocate_effect()?;
        self.pending_mailbox = Some(PendingMailbox {
            id,
            kind: PendingMailboxKind::SubmitCommand,
        });
        effects.push(UiEffect::SubmitMailboxCommand { id, draft, action });
        Ok(())
    }

    fn submit_agent(
        &mut self,
        action: UiAgentAction,
        effects: &mut Vec<UiEffect>,
    ) -> Result<(), UiError> {
        if self.pending_agent.is_some() {
            return Ok(());
        }
        if let (
            Some(UiGuidedPending::AgentCreation { expected_name, .. }),
            UiAgentAction::Create { name },
        ) = (&mut self.guided_pending, &action)
        {
            *expected_name = Some(name.clone());
        }
        let id = self.allocate_effect()?;
        self.pending_agent = Some(id);
        effects.push(UiEffect::SubmitAgentCommand { id, action });
        Ok(())
    }

    fn submit_managed_session(
        &mut self,
        action: UiManagedSessionAction,
        effects: &mut Vec<UiEffect>,
    ) -> Result<(), UiError> {
        if self.pending_managed_session.is_some() {
            return Ok(());
        }
        let id = self.allocate_effect()?;
        self.pending_managed_session = Some(id);
        effects.push(UiEffect::SubmitManagedSession { id, action });
        Ok(())
    }

    fn submit_project(
        &mut self,
        action: UiProjectAction,
        effects: &mut Vec<UiEffect>,
    ) -> Result<(), UiError> {
        if self.pending_project.is_some() {
            return Ok(());
        }
        let id = self.allocate_effect()?;
        self.pending_project = Some(PendingProject {
            id,
            action: action.clone(),
        });
        effects.push(UiEffect::SubmitProjectCommand { id, action });
        Ok(())
    }

    fn save_section_workspace(&mut self) {
        self.section_workspaces[self.section.index()] = Some(UiSectionWorkspace {
            selected_row: self.selected_row.clone(),
            conversation: self.conversation.clone(),
            conversation_anchor: self.conversation_anchor.clone(),
            technical_visible: self.technical_visible,
            focus: self.focus,
        });
    }

    fn restore_section_workspace(&mut self) {
        let workspace = self.section_workspaces[self.section.index()].clone();
        if let Some(workspace) = workspace {
            self.selected_row = workspace.selected_row;
            self.conversation = workspace.conversation;
            self.conversation_anchor = workspace.conversation_anchor;
            self.technical_visible = workspace.technical_visible;
            self.focus = workspace.focus;
        } else {
            self.selected_row = None;
            self.conversation = None;
            self.conversation_anchor = None;
            self.technical_visible = false;
            self.focus = UiFocus::Navigation;
        }
        self.pending_conversation = None;
    }

    fn change_section(&mut self, next: UiSection) {
        if self.section == next {
            return;
        }
        self.save_section_workspace();
        self.section = next;
        self.restore_section_workspace();
        self.reconcile_current_section();
    }

    fn schedule_timer(
        &mut self,
        kind: UiTimerKind,
        after: Duration,
        effects: &mut Vec<UiEffect>,
    ) -> Result<(), UiError> {
        let id = self.allocate_effect()?;
        match kind {
            UiTimerKind::PeriodicRefresh => self.periodic_timer = Some(id),
            UiTimerKind::RetrySnapshot => self.retry_timer = Some(id),
            UiTimerKind::AutosaveDraft => self.autosave_timer = Some(id),
            UiTimerKind::DismissCompletion => self.completion_timer = Some(id),
        }
        effects.push(UiEffect::ScheduleTimer { id, kind, after });
        Ok(())
    }

    fn move_row_selection(&mut self, forward: bool) -> bool {
        let Some(rows) = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.rows(self.section))
        else {
            return false;
        };
        if rows.is_empty() {
            return false;
        }
        let current = self
            .selected_row
            .as_deref()
            .and_then(|selected| rows.iter().position(|row| row.id == selected));
        let next = match (current, forward) {
            (Some(index), true) => (index + 1).min(rows.len() - 1),
            (Some(index), false) => index.saturating_sub(1),
            (None, _) => 0,
        };
        let selected = rows[next].id.clone();
        if self.selected_row.as_ref() == Some(&selected) {
            false
        } else {
            self.selected_row = Some(selected);
            self.close_conversation();
            true
        }
    }

    fn move_conversation_anchor(&mut self, forward: bool) -> bool {
        let Some(conversation) = &self.conversation else {
            return false;
        };
        if conversation.entries.is_empty() {
            return false;
        }
        let current = self.conversation_anchor.as_deref().and_then(|selected| {
            conversation
                .entries
                .iter()
                .position(|entry| entry.id == selected)
        });
        let next = match (current, forward) {
            (Some(index), true) => (index + 1).min(conversation.entries.len() - 1),
            (Some(index), false) => index.saturating_sub(1),
            (None, _) => 0,
        };
        let selected = conversation.entries[next].id.clone();
        if self.conversation_anchor.as_ref() == Some(&selected) {
            false
        } else {
            self.conversation_anchor = Some(selected);
            self.technical_visible = false;
            true
        }
    }

    fn selected_row_is_conversation(&self) -> bool {
        self.selected_row.as_ref().is_some_and(|selected| {
            self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot
                    .rows(self.section)
                    .iter()
                    .any(|row| &row.id == selected && row.kind == UiRowKind::Conversation)
            })
        })
    }

    fn close_conversation(&mut self) {
        self.conversation = None;
        self.conversation_anchor = None;
        self.technical_visible = false;
        self.pending_conversation = None;
        if self.focus == UiFocus::Conversation {
            self.focus = UiFocus::Content;
        }
    }

    fn apply_snapshot(&mut self, snapshot: UiSnapshot) {
        if let Some(UiMailboxModal::SelectDirect { selected, targets }) = &mut self.mailbox_modal {
            let keep = selected.filter(|(installation, mailbox)| {
                snapshot.direct_targets.iter().any(|target| {
                    target.installation_id == *installation && target.mailbox_id == *mailbox
                })
            });
            *selected = keep.or_else(|| {
                snapshot
                    .direct_targets
                    .first()
                    .map(|target| (target.installation_id, target.mailbox_id))
            });
            targets.clone_from(&snapshot.direct_targets);
        }
        refresh_agent_modal(self, &snapshot);
        refresh_project_modal(self, &snapshot);
        refresh_new_modal(self, &snapshot);
        self.snapshot = Some(snapshot);
        self.reconcile_current_section();
        select_agent_search_match(self, false);
        select_project_search_match(self, false);
    }

    fn reconcile_current_section(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            self.selected_row = None;
            self.close_conversation();
            return;
        };
        let rows = snapshot.rows(self.section);
        let keep = self.selected_row.as_ref().and_then(|selected| {
            rows.iter()
                .find(|row| &row.id == selected)
                .map(|row| row.id.clone())
        });
        self.selected_row = keep.or_else(|| rows.first().map(|row| row.id.clone()));
        let conversation_survives = self.conversation.as_ref().is_some_and(|conversation| {
            self.selected_row.as_ref() == Some(&conversation.row_id)
                && rows
                    .iter()
                    .any(|row| row.id == conversation.row_id && row.kind == UiRowKind::Conversation)
        });
        if !conversation_survives {
            self.close_conversation();
        }
    }
}

/// Applies one event without performing I/O or domain mutation.
pub fn update(mut model: UiModel, event: UiEvent) -> Result<UiTransition, UiError> {
    let mut effects = Vec::new();
    match event {
        UiEvent::Started => start(&mut model, &mut effects)?,
        UiEvent::Input(value) => apply_input(&mut model, &value, &mut effects)?,
        UiEvent::Resized(viewport) => {
            if model.viewport != viewport {
                model.viewport = viewport;
                effects.push(UiEffect::RequestRedraw);
            }
        }
        UiEvent::TimerElapsed { effect_id } => {
            timer_elapsed(&mut model, effect_id, &mut effects)?;
        }
        UiEvent::SnapshotLoaded {
            effect_id,
            snapshot,
        } => snapshot_loaded(&mut model, effect_id, snapshot, &mut effects)?,
        UiEvent::SnapshotFailed { effect_id, failure } => {
            snapshot_failed(&mut model, effect_id, failure, &mut effects)?;
        }
        UiEvent::ConversationLoaded { effect_id, page } => {
            conversation_loaded(&mut model, effect_id, page, &mut effects)?;
        }
        UiEvent::ConversationFailed { effect_id, failure } => {
            conversation_failed(&mut model, effect_id, failure, &mut effects);
        }
        UiEvent::DraftLoaded { effect_id, draft } => {
            draft_loaded(&mut model, effect_id, draft, &mut effects);
        }
        UiEvent::DraftSaved { effect_id, draft } => {
            draft_saved(&mut model, effect_id, &draft, &mut effects)?;
        }
        UiEvent::DraftFailed {
            effect_id,
            failure,
            current,
        } => draft_failed(&mut model, effect_id, failure, current, &mut effects),
        UiEvent::MailboxCommandCommitted {
            effect_id,
            revision,
        } => mailbox_command_committed(&mut model, effect_id, revision, &mut effects)?,
        UiEvent::MailboxCommandFailed { effect_id, failure } => {
            mailbox_command_failed(&mut model, effect_id, failure, &mut effects);
        }
        UiEvent::AgentCommandCommitted {
            effect_id,
            revision,
        } => agent_command_committed(&mut model, effect_id, revision, &mut effects)?,
        UiEvent::AgentCommandFailed { effect_id, failure } => {
            agent_command_failed(&mut model, effect_id, failure, &mut effects);
        }
        UiEvent::ManagedSessionCompleted { effect_id, result } => {
            managed_session_completed(&mut model, effect_id, result, &mut effects)?;
        }
        UiEvent::ManagedSessionFailed { effect_id, failure } => {
            managed_session_failed(&mut model, effect_id, failure, &mut effects);
        }
        UiEvent::ProjectCommandCompleted { effect_id, result } => {
            project_command_completed(&mut model, effect_id, result, &mut effects)?;
        }
        UiEvent::ProjectCommandFailed { effect_id, failure } => {
            project_command_failed(&mut model, effect_id, failure, &mut effects);
        }
        UiEvent::Invalidated { revision } => invalidated(&mut model, revision, &mut effects)?,
        UiEvent::ConnectionObserved { generation, state } => {
            connection_observed(&mut model, generation, state, &mut effects)?;
        }
        UiEvent::ClientFailed {
            generation,
            failure,
        } => client_failed(&mut model, generation, failure, &mut effects),
    }
    Ok(UiTransition { model, effects })
}

fn start(model: &mut UiModel, effects: &mut Vec<UiEffect>) -> Result<(), UiError> {
    if model.started {
        return Err(UiError::AlreadyStarted);
    }
    model.started = true;
    model.connection = UiConnectionState::Connecting;
    model.request_snapshot(effects)?;
    model.schedule_timer(UiTimerKind::PeriodicRefresh, PERIODIC_REFRESH, effects)?;
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn apply_input(
    model: &mut UiModel,
    input: &UiInput,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    model.sync_form();
    let normalized_input = normalize_vim_navigation(model, input);
    let input = &normalized_input;
    let dismissed_completion = model.completion_notice.take().is_some();
    if dismissed_completion {
        model.completion_timer = None;
    }
    if matches!(input, UiInput::Help) {
        model.help_page = match model.help_page {
            Some(_) => None,
            None => Some(UiHelpPage::Context),
        };
        effects.push(UiEffect::RequestRedraw);
        return Ok(());
    }
    if matches!(input, UiInput::Refresh) {
        model.request_snapshot(effects)?;
        effects.push(UiEffect::RequestRedraw);
        return Ok(());
    }
    if let Some(changed) = apply_open_modal_input(model, input, effects)? {
        if changed || dismissed_completion {
            effects.push(UiEffect::RequestRedraw);
        }
        return Ok(());
    }
    if model.help_page.is_some() {
        let changed = apply_help_input(model, input, effects);
        if changed {
            effects.push(UiEffect::RequestRedraw);
        }
        return Ok(());
    }
    let dismissed_transient_help = model.transient_help.take().is_some();
    let changed = match input {
        UiInput::Quit => {
            if model.should_exit {
                false
            } else {
                model.should_exit = true;
                effects.push(UiEffect::Exit);
                false
            }
        }
        UiInput::NextFocus => {
            model.focus = match model.focus {
                UiFocus::Navigation => UiFocus::Content,
                UiFocus::Content if model.conversation.is_some() => UiFocus::Conversation,
                UiFocus::Content | UiFocus::Conversation => UiFocus::Navigation,
            };
            true
        }
        UiInput::PreviousFocus => {
            model.focus = match model.focus {
                UiFocus::Navigation if model.conversation.is_some() => UiFocus::Conversation,
                UiFocus::Navigation | UiFocus::Conversation => UiFocus::Content,
                UiFocus::Content => UiFocus::Navigation,
            };
            true
        }
        UiInput::NextSection | UiInput::MoveCursorRight => {
            match (model.viewport.width >= WIDE_WIDTH, model.focus) {
                (true, UiFocus::Navigation) => {
                    model.focus = UiFocus::Content;
                    true
                }
                (false, _) => {
                    model.change_section(model.section.next());
                    true
                }
                _ => false,
            }
        }
        UiInput::PreviousSection | UiInput::MoveCursorLeft => {
            match (model.viewport.width >= WIDE_WIDTH, model.focus) {
                (true, UiFocus::Content | UiFocus::Conversation) => {
                    model.focus = UiFocus::Navigation;
                    true
                }
                (false, _) => {
                    model.change_section(model.section.previous());
                    true
                }
                _ => false,
            }
        }
        UiInput::NextItem => match model.focus {
            UiFocus::Conversation => model.move_conversation_anchor(true),
            UiFocus::Navigation if model.viewport.width >= WIDE_WIDTH => {
                model.change_section(model.section.next());
                true
            }
            UiFocus::Navigation | UiFocus::Content => model.move_row_selection(true),
        },
        UiInput::PreviousItem => match model.focus {
            UiFocus::Conversation => model.move_conversation_anchor(false),
            UiFocus::Navigation if model.viewport.width >= WIDE_WIDTH => {
                model.change_section(model.section.previous());
                true
            }
            UiFocus::Navigation | UiFocus::Content => model.move_row_selection(false),
        },
        UiInput::Activate => activate(model, effects)?,
        UiInput::LoadMore => load_more(model, effects)?,
        UiInput::Escape => escape(model),
        UiInput::Character('?') => {
            model.help_page = Some(UiHelpPage::Context);
            true
        }
        UiInput::Character(character) => mailbox_shortcut(model, *character, effects)?,
        UiInput::Paste(_)
        | UiInput::Help
        | UiInput::Refresh
        | UiInput::Backspace
        | UiInput::MoveCursorHome
        | UiInput::MoveCursorEnd
        | UiInput::Delete => false,
    };
    if changed || dismissed_transient_help || dismissed_completion {
        effects.push(UiEffect::RequestRedraw);
    }
    Ok(())
}

fn normalize_vim_navigation(model: &UiModel, input: &UiInput) -> UiInput {
    if text_input_is_active(model) {
        return input.clone();
    }
    match input {
        UiInput::Character('j') => UiInput::NextItem,
        UiInput::Character('k') => UiInput::PreviousItem,
        _ => input.clone(),
    }
}

fn text_input_is_active(model: &UiModel) -> bool {
    if model.new_modal.is_some() {
        return false;
    }
    if let Some(modal) = &model.project_modal {
        return match modal {
            UiProjectModal::Search { .. }
            | UiProjectModal::CreateExisting { .. }
            | UiProjectModal::CreateWorktree { .. }
            | UiProjectModal::SendInput { .. }
            | UiProjectModal::ReplaceResource { .. } => true,
            UiProjectModal::AddResource { .. } => {
                model.form.focused == Some(UiFormField::Project(UiProjectFormField::Path))
            }
            UiProjectModal::Activate { field, .. } | UiProjectModal::Handoff { field, .. } => {
                *field == UiProjectFormField::Directory
            }
            UiProjectModal::ChooseCreation { .. }
            | UiProjectModal::Details { .. }
            | UiProjectModal::ConfirmRemoveResource { .. }
            | UiProjectModal::ConfirmPrimaryResource { .. }
            | UiProjectModal::ConfirmClose { .. }
            | UiProjectModal::ConfirmArchive { .. }
            | UiProjectModal::Outcome { .. } => false,
        };
    }
    if matches!(
        model.agent_modal,
        Some(
            UiAgentModal::Search { .. }
                | UiAgentModal::Create { .. }
                | UiAgentModal::RenameSession { .. }
        )
    ) {
        return true;
    }
    matches!(model.mailbox_modal, Some(UiMailboxModal::Compose { .. }))
}

fn apply_open_modal_input(
    model: &mut UiModel,
    input: &UiInput,
    effects: &mut Vec<UiEffect>,
) -> Result<Option<bool>, UiError> {
    if model.new_modal.is_some() {
        return apply_new_modal_input(model, input.clone(), effects).map(Some);
    }
    if model.project_modal.is_some() {
        return apply_project_modal_input(model, input.clone(), effects).map(Some);
    }
    if model.agent_modal.is_some() {
        return apply_agent_modal_input(model, input.clone(), effects).map(Some);
    }
    if model.mailbox_modal.is_some() {
        return apply_modal_input(model, input.clone(), effects).map(Some);
    }
    Ok(None)
}

#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
fn apply_new_modal_input(
    model: &mut UiModel,
    input: UiInput,
    effects: &mut Vec<UiEffect>,
) -> Result<bool, UiError> {
    if matches!(input, UiInput::Quit) {
        model.should_exit = true;
        effects.push(UiEffect::Exit);
        return Ok(false);
    }
    let Some(interaction) = model.new_modal.clone() else {
        return Ok(false);
    };
    if matches!(input, UiInput::Escape) {
        model.new_modal = match interaction {
            UiNewModal::Launcher { .. } => None,
            UiNewModal::ChooseProject { .. } => Some(UiNewModal::Launcher {
                selected: UiNewChoice::ProjectWork,
            }),
            UiNewModal::ChooseAgent { .. } | UiNewModal::ProjectUnavailable { .. } => {
                Some(guided_project_picker(model))
            }
            UiNewModal::ChooseProvider { project, .. }
            | UiNewModal::AgentUnavailable { project, .. } => {
                Some(guided_agent_picker(model, project))
            }
            UiNewModal::ReviewProject {
                project,
                agent,
                provider,
                ..
            } => Some(guided_provider_picker(model, project, agent, provider)),
            UiNewModal::Working { .. } => return Ok(false),
        };
        model.last_failure = None;
        return Ok(true);
    }
    match interaction {
        UiNewModal::Launcher { selected } => match input {
            UiInput::NextItem | UiInput::PreviousItem => {
                model.new_modal = Some(UiNewModal::Launcher {
                    selected: selected.cycle(matches!(input, UiInput::NextItem)),
                });
                Ok(true)
            }
            UiInput::Activate => {
                model.new_modal = None;
                match selected {
                    UiNewChoice::ProjectWork => {
                        model.new_modal = Some(guided_project_picker(model));
                    }
                    UiNewChoice::DirectMessage => open_direct_target_picker(model),
                    UiNewChoice::PersonalNote => {
                        model.open_draft(UiMailboxDraftTarget::SelfNote, effects)?;
                    }
                }
                Ok(true)
            }
            _ => Ok(false),
        },
        UiNewModal::ChooseProject {
            projects,
            selected,
            create_new,
        } => match input {
            UiInput::NextItem | UiInput::PreviousItem => {
                let (selected, create_new) = cycle_identity_choice(
                    &projects,
                    selected,
                    create_new,
                    matches!(input, UiInput::NextItem),
                    |project| project.project_id,
                );
                model.new_modal = Some(UiNewModal::ChooseProject {
                    projects,
                    selected,
                    create_new,
                });
                Ok(true)
            }
            UiInput::Activate if create_new => {
                model.new_modal = None;
                model.guided_pending = Some(UiGuidedPending::ProjectCreation);
                model.project_modal = Some(UiProjectModal::ChooseCreation {
                    selected: UiProjectCreationChoice::ExistingFolder,
                });
                Ok(true)
            }
            UiInput::Activate => {
                let Some(project) = selected.and_then(|project_id| {
                    projects
                        .iter()
                        .find(|project| project.project_id == project_id)
                        .cloned()
                }) else {
                    return Ok(false);
                };
                open_guided_project(model, project);
                Ok(true)
            }
            _ => Ok(false),
        },
        UiNewModal::ChooseAgent {
            project,
            agents,
            selected,
            create_new,
        } => match input {
            UiInput::NextItem | UiInput::PreviousItem => {
                let (selected, create_new) = cycle_identity_choice(
                    &agents,
                    selected,
                    create_new,
                    matches!(input, UiInput::NextItem),
                    |agent| agent.agent_id,
                );
                model.new_modal = Some(UiNewModal::ChooseAgent {
                    project,
                    agents,
                    selected,
                    create_new,
                });
                Ok(true)
            }
            UiInput::Activate if create_new => {
                model.new_modal = None;
                model.guided_pending = Some(UiGuidedPending::AgentCreation {
                    project_id: project.project_id,
                    expected_name: None,
                });
                model.agent_modal = Some(UiAgentModal::Create {
                    name: String::new(),
                    submitting: false,
                });
                Ok(true)
            }
            UiInput::Activate => {
                let Some(agent) = selected.and_then(|agent_id| {
                    agents
                        .iter()
                        .find(|agent| agent.agent_id == agent_id)
                        .cloned()
                }) else {
                    return Ok(false);
                };
                open_guided_agent(model, project, agent);
                Ok(true)
            }
            _ => Ok(false),
        },
        UiNewModal::ChooseProvider {
            project,
            agent,
            providers,
            mut provider,
        } => match input {
            UiInput::NextItem | UiInput::PreviousItem => {
                if let Some(next) = cycle_provider_choice(
                    &providers,
                    Some(&provider),
                    matches!(input, UiInput::NextItem),
                ) {
                    provider = next;
                }
                model.new_modal = Some(UiNewModal::ChooseProvider {
                    project,
                    agent,
                    providers,
                    provider,
                });
                Ok(true)
            }
            UiInput::Activate => {
                let resumes_existing = guided_thread(&project, agent.agent_id).is_some()
                    || project.assignment.as_ref().is_some_and(|assignment| {
                        assignment.agent_id == agent.agent_id && assignment.runnable
                    });
                if !resumes_existing && !provider_is_available(&providers, &provider) {
                    model.last_failure = Some(UiFailure {
                        code: "guided_provider_unavailable".to_owned(),
                        action: "configure an agent service before starting project work"
                            .to_owned(),
                    });
                    return Ok(true);
                }
                let moves_project = project
                    .assignment
                    .as_ref()
                    .is_some_and(|assignment| assignment.agent_id != agent.agent_id);
                if moves_project {
                    model.new_modal = Some(UiNewModal::ReviewProject {
                        project,
                        agent,
                        provider,
                        resumes_existing,
                        moves_project,
                        submitting: false,
                    });
                } else {
                    submit_guided_project(model, project, &agent, provider, effects)?;
                }
                Ok(true)
            }
            _ => Ok(false),
        },
        UiNewModal::ReviewProject {
            project,
            agent,
            provider,
            submitting,
            ..
        } if matches!(input, UiInput::Activate) && !submitting => {
            submit_guided_project(model, project, &agent, provider, effects)?;
            Ok(true)
        }
        UiNewModal::AgentUnavailable {
            competing_project_id,
            ..
        } if matches!(input, UiInput::Activate) => {
            let competing = model.snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .projects
                    .iter()
                    .find(|project| project.project_id == competing_project_id)
                    .cloned()
            });
            if let Some(project) = competing {
                model.new_modal = None;
                model.change_section(UiSection::Projects);
                model.selected_row = Some(agent_hex(project.project_id));
                model.project_modal = Some(UiProjectModal::Details {
                    selected_resource: default_project_resource(&project),
                    project,
                });
            }
            Ok(true)
        }
        UiNewModal::ProjectUnavailable { project, .. } if matches!(input, UiInput::Activate) => {
            model.new_modal = None;
            model.change_section(UiSection::Projects);
            model.selected_row = Some(agent_hex(project.project_id));
            model.project_modal = Some(UiProjectModal::Details {
                selected_resource: default_project_resource(&project),
                project,
            });
            Ok(true)
        }
        UiNewModal::ReviewProject { .. }
        | UiNewModal::AgentUnavailable { .. }
        | UiNewModal::ProjectUnavailable { .. }
        | UiNewModal::Working { .. } => Ok(false),
    }
}

fn cycle_identity_choice<T>(
    values: &[T],
    selected: Option<[u8; 32]>,
    create_new: bool,
    forward: bool,
    identity: impl Fn(&T) -> [u8; 32],
) -> (Option<[u8; 32]>, bool) {
    let count = values.len() + 1;
    let current = if create_new {
        values.len()
    } else {
        selected
            .and_then(|selected| values.iter().position(|value| identity(value) == selected))
            .unwrap_or(0)
    };
    let next = if forward {
        (current + 1) % count
    } else {
        current.checked_sub(1).unwrap_or(count - 1)
    };
    if next == values.len() {
        (None, true)
    } else {
        (Some(identity(&values[next])), false)
    }
}

fn guided_project_picker(model: &UiModel) -> UiNewModal {
    let projects = model.snapshot.as_ref().map_or_else(Vec::new, |snapshot| {
        snapshot
            .projects
            .iter()
            .filter(|project| !project.archived)
            .cloned()
            .collect::<Vec<_>>()
    });
    UiNewModal::ChooseProject {
        selected: projects.first().map(|project| project.project_id),
        create_new: projects.is_empty(),
        projects,
    }
}

fn open_guided_project(model: &mut UiModel, project: UiProject) {
    if !project.claimable
        || project
            .resources
            .iter()
            .any(|resource| !resource.conflicting_projects.is_empty())
    {
        let competing_id = project
            .resources
            .iter()
            .flat_map(|resource| resource.conflicting_projects.iter())
            .next()
            .copied();
        let competing_project = competing_id.and_then(|project_id| {
            model.snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .projects
                    .iter()
                    .find(|candidate| candidate.project_id == project_id)
                    .map(|candidate| candidate.name.clone())
            })
        });
        model.new_modal = Some(UiNewModal::ProjectUnavailable {
            project,
            competing_project,
            reason: "folder ownership needs attention".to_owned(),
        });
        return;
    }
    if let Some(assignment) = &project.assignment
        && assignment.runnable
    {
        open_project_message_composer(model, project);
        return;
    }
    model.new_modal = Some(guided_agent_picker(model, project));
}

fn open_project_message_composer(model: &mut UiModel, project: UiProject) {
    model.guided_pending = None;
    model.new_modal = None;
    model.change_section(UiSection::Projects);
    model.selected_row = Some(agent_hex(project.project_id));
    model.project_modal = Some(UiProjectModal::SendInput {
        project,
        content: String::new(),
        submitting: false,
    });
}

fn guided_agent_picker(model: &UiModel, project: UiProject) -> UiNewModal {
    guided_agent_picker_from_snapshot(model.snapshot.as_ref(), project)
}

fn guided_agent_picker_from_snapshot(
    snapshot: Option<&UiSnapshot>,
    project: UiProject,
) -> UiNewModal {
    let mut agents = snapshot.map_or_else(Vec::new, |snapshot| {
        snapshot
            .agents
            .iter()
            .filter(|agent| {
                agent.lifecycle == UiAgentLifecycle::Active
                    && agent
                        .mailboxes
                        .iter()
                        .any(|mailbox| mailbox.installation_id == project.home)
            })
            .cloned()
            .collect::<Vec<_>>()
    });
    agents.sort_by_key(|agent| {
        (
            !matches!(agent.status, UiAgentStatus::Unassigned),
            agent.names.first().cloned().unwrap_or_default(),
            agent.agent_id,
        )
    });
    UiNewModal::ChooseAgent {
        selected: agents.first().map(|agent| agent.agent_id),
        create_new: agents.is_empty(),
        project,
        agents,
    }
}

fn open_guided_agent(model: &mut UiModel, project: UiProject, agent: UiAgent) {
    let competing = match &agent.status {
        UiAgentStatus::Assigned(assignment) if assignment.project_id != project.project_id => {
            Some((assignment.project_id, assignment.project_name.clone()))
        }
        UiAgentStatus::NeedsAttention { assignments, .. } => assignments
            .first()
            .map(|assignment| (assignment.project_id, assignment.project_name.clone())),
        UiAgentStatus::Unassigned | UiAgentStatus::Assigned(_) | UiAgentStatus::Retired => None,
    };
    if let Some((competing_project_id, competing_project)) = competing {
        model.new_modal = Some(UiNewModal::AgentUnavailable {
            project,
            agent,
            competing_project_id,
            competing_project,
        });
        return;
    }
    if project
        .assignment
        .as_ref()
        .is_some_and(|assignment| assignment.agent_id == agent.agent_id && assignment.runnable)
    {
        open_project_message_composer(model, project);
        return;
    }
    let provider = guided_thread(&project, agent.agent_id)
        .map(|thread| thread.provider.clone())
        .or_else(|| default_provider_choice_from_model(model))
        .unwrap_or_default();
    model.new_modal = Some(guided_provider_picker(model, project, agent, provider));
}

fn guided_provider_picker(
    model: &UiModel,
    project: UiProject,
    agent: UiAgent,
    provider: String,
) -> UiNewModal {
    let providers = model
        .snapshot
        .as_ref()
        .map_or_else(Vec::new, |snapshot| snapshot.providers.clone());
    UiNewModal::ChooseProvider {
        project,
        agent,
        providers,
        provider,
    }
}

fn default_provider_choice_from_model(model: &UiModel) -> Option<String> {
    model
        .snapshot
        .as_ref()
        .and_then(|snapshot| default_provider_choice(&snapshot.providers))
}

fn guided_thread(project: &UiProject, agent_id: [u8; 32]) -> Option<&UiProjectThread> {
    project
        .threads
        .iter()
        .find(|thread| thread.agent_id == agent_id)
}

fn open_direct_target_picker(model: &mut UiModel) {
    let targets = model
        .snapshot
        .as_ref()
        .map_or_else(Vec::new, |snapshot| snapshot.direct_targets.clone());
    let selected = targets
        .first()
        .map(|target| (target.installation_id, target.mailbox_id));
    model.mailbox_modal = Some(UiMailboxModal::SelectDirect { targets, selected });
}

fn submit_guided_project(
    model: &mut UiModel,
    project: UiProject,
    agent: &UiAgent,
    provider: String,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    let agent_name = agent
        .names
        .first()
        .cloned()
        .unwrap_or_else(|| "selected agent".to_owned());
    let historical = guided_thread(&project, agent.agent_id);
    let submission = UiGuidedSubmission {
        project_id: project.project_id,
        agent_id: agent.agent_id,
        provider: provider.clone(),
    };
    let directory = project
        .resources
        .iter()
        .find(|resource| resource.primary)
        .or_else(|| project.resources.first())
        .map(|resource| resource.display_path.clone())
        .unwrap_or_default();
    if directory.is_empty() {
        model.last_failure = Some(UiFailure {
            code: "guided_project_has_no_folder".to_owned(),
            action: "add a project folder before starting agent work".to_owned(),
        });
        return Ok(());
    }
    let action = if let Some(current) = project.assignment.as_ref() {
        let Some(thread_id) = current.thread_id else {
            model.last_failure = Some(UiFailure {
                code: "guided_handoff_not_ready".to_owned(),
                action: "inspect the current project setup before moving it to another agent"
                    .to_owned(),
            });
            return Ok(());
        };
        UiProjectAction::Handoff {
            project_id: project.project_id,
            agent_id: agent.agent_id,
            provider,
            resume_session: historical.map(|thread| thread.session.clone()),
            thread_id,
            launch_directory: directory,
            force_takeover: false,
        }
    } else {
        UiProjectAction::Activate {
            project_id: project.project_id,
            agent_id: agent.agent_id,
            provider,
            resume_session: historical.map(|thread| thread.session.clone()),
            resume_thread: historical.map(|thread| thread.thread_id),
            launch_directory: directory,
        }
    };
    model.guided_pending = Some(UiGuidedPending::Activation(submission));
    model.new_modal = Some(UiNewModal::Working {
        project: project.name,
        agent: agent_name,
        stage: "Preparing the project conversation…".to_owned(),
    });
    model.last_failure = None;
    model.submit_project(action, effects)
}

fn apply_help_input(model: &mut UiModel, input: &UiInput, effects: &mut Vec<UiEffect>) -> bool {
    match input {
        UiInput::Quit | UiInput::Character('q' | 'Q') => {
            if !model.should_exit {
                model.should_exit = true;
                effects.push(UiEffect::Exit);
            }
            false
        }
        UiInput::Escape | UiInput::Help | UiInput::Character('?') => {
            model.help_page = None;
            true
        }
        UiInput::Character('t' | 'T') => {
            model.help_page = Some(match model.help_page {
                Some(UiHelpPage::Context) => UiHelpPage::Technical,
                Some(UiHelpPage::Technical) | None => UiHelpPage::Context,
            });
            true
        }
        _ => false,
    }
}

#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
fn apply_modal_input(
    model: &mut UiModel,
    input: UiInput,
    effects: &mut Vec<UiEffect>,
) -> Result<bool, UiError> {
    if matches!(input, UiInput::Quit) {
        model.should_exit = true;
        effects.push(UiEffect::Exit);
        return Ok(false);
    }
    if matches!(input, UiInput::Escape) {
        if let Some(UiMailboxModal::Compose { draft, dirty, .. }) = model.mailbox_modal.clone() {
            if dirty {
                model.mailbox_modal = Some(UiMailboxModal::Compose {
                    draft: draft.clone(),
                    dirty: true,
                    submitting: false,
                    closing: true,
                });
                model.autosave_timer = None;
                if model.pending_mailbox.is_none() {
                    model.save_draft(effects)?;
                }
            } else {
                model.mailbox_modal = None;
            }
        } else {
            model.mailbox_modal = None;
        }
        return Ok(true);
    }

    match model.mailbox_modal.clone() {
        Some(UiMailboxModal::SelectDirect { targets, selected }) => match input {
            UiInput::NextItem | UiInput::PreviousItem => {
                if targets.is_empty() {
                    return Ok(false);
                }
                let current = selected.and_then(|identity| {
                    targets
                        .iter()
                        .position(|target| (target.installation_id, target.mailbox_id) == identity)
                });
                let next = match (current, matches!(input, UiInput::NextItem)) {
                    (Some(index), true) => (index + 1).min(targets.len() - 1),
                    (Some(index), false) => index.saturating_sub(1),
                    (None, _) => 0,
                };
                if let Some(UiMailboxModal::SelectDirect { selected, .. }) =
                    &mut model.mailbox_modal
                {
                    *selected = Some((targets[next].installation_id, targets[next].mailbox_id));
                }
                Ok(true)
            }
            UiInput::Activate => {
                let Some((installation_id, mailbox_id)) = selected else {
                    return Ok(false);
                };
                model.open_draft(
                    UiMailboxDraftTarget::Direct {
                        installation_id,
                        mailbox_id,
                    },
                    effects,
                )?;
                Ok(true)
            }
            _ => Ok(false),
        },
        Some(UiMailboxModal::LoadingDraft { .. }) | None => Ok(false),
        Some(UiMailboxModal::Compose {
            mut draft,
            dirty,
            submitting,
            closing,
        }) => match input {
            UiInput::Character(_)
            | UiInput::Paste(_)
            | UiInput::Backspace
            | UiInput::Delete
            | UiInput::MoveCursorLeft
            | UiInput::MoveCursorRight
            | UiInput::MoveCursorHome
            | UiInput::MoveCursorEnd
                if !submitting && !closing =>
            {
                if !edit_text_input(
                    &mut model.form,
                    UiFormField::Message,
                    &mut draft.content,
                    &input,
                    MAX_DRAFT_BYTES,
                ) {
                    return Ok(false);
                }
                update_composer(model, draft, true, false, effects)?;
                Ok(true)
            }
            UiInput::Activate if !submitting && !closing => {
                if draft.content.is_empty() {
                    model.form.errors.insert(
                        UiFormField::Message,
                        "Enter a message before sending".to_owned(),
                    );
                    model.last_failure = None;
                    return Ok(true);
                }
                if dirty {
                    model.mailbox_modal = Some(UiMailboxModal::Compose {
                        draft,
                        dirty: true,
                        submitting: true,
                        closing: false,
                    });
                    model.autosave_timer = None;
                    model.save_draft(effects)?;
                } else {
                    let action = draft_action(&draft.target);
                    model.mailbox_modal = Some(UiMailboxModal::Compose {
                        draft: draft.clone(),
                        dirty: false,
                        submitting: true,
                        closing: false,
                    });
                    model.submit_mailbox(Some(draft), action, effects)?;
                }
                Ok(true)
            }
            _ => Ok(false),
        },
        Some(UiMailboxModal::Confirm { action }) => {
            if matches!(input, UiInput::Activate) {
                model.submit_mailbox(None, action, effects)?;
                Ok(true)
            } else {
                Ok(false)
            }
        }
    }
}

fn update_composer(
    model: &mut UiModel,
    draft: UiMailboxDraft,
    dirty: bool,
    submitting: bool,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    model.mailbox_modal = Some(UiMailboxModal::Compose {
        draft,
        dirty,
        submitting,
        closing: false,
    });
    model.last_failure = None;
    if model.pending_mailbox.is_none() {
        model.schedule_timer(UiTimerKind::AutosaveDraft, DRAFT_AUTOSAVE_DELAY, effects)?;
    }
    Ok(())
}

#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
fn apply_project_modal_input(
    model: &mut UiModel,
    input: UiInput,
    effects: &mut Vec<UiEffect>,
) -> Result<bool, UiError> {
    if matches!(input, UiInput::Quit) {
        model.should_exit = true;
        effects.push(UiEffect::Exit);
        return Ok(false);
    }
    if matches!(input, UiInput::Escape) {
        if model.pending_project.is_none() {
            if let Some(UiProjectModal::Outcome {
                result:
                    UiProjectResult {
                        action: UiProjectAction::PreviewCreateExisting { name, brief, path },
                        outcome: UiProjectOutcome::ResourcePreview { .. },
                        ..
                    },
            }) = model.project_modal.clone()
            {
                model.project_modal = Some(UiProjectModal::CreateExisting {
                    name,
                    brief: brief.unwrap_or_default(),
                    path,
                    field: UiProjectFormField::Path,
                    submitting: false,
                });
                model.last_failure = None;
                return Ok(true);
            }
            if let Some(UiProjectModal::Search { query }) = &model.project_modal {
                model.project_search.clone_from(query);
            }
            if matches!(model.guided_pending, Some(UiGuidedPending::ProjectCreation)) {
                model.guided_pending = None;
                model.new_modal = Some(UiNewModal::Launcher {
                    selected: UiNewChoice::ProjectWork,
                });
            }
            model.project_modal = None;
            return Ok(true);
        }
        return Ok(false);
    }

    match model.project_modal.clone() {
        Some(UiProjectModal::ChooseCreation { mut selected }) => match input {
            UiInput::NextItem | UiInput::PreviousItem => {
                selected = match selected {
                    UiProjectCreationChoice::ExistingFolder => {
                        UiProjectCreationChoice::IsolatedWorktree
                    }
                    UiProjectCreationChoice::IsolatedWorktree => {
                        UiProjectCreationChoice::ExistingFolder
                    }
                };
                model.project_modal = Some(UiProjectModal::ChooseCreation { selected });
                Ok(true)
            }
            UiInput::Activate => {
                match selected {
                    UiProjectCreationChoice::ExistingFolder => open_existing_project_form(model),
                    UiProjectCreationChoice::IsolatedWorktree => open_worktree_project_form(model),
                }
                Ok(true)
            }
            _ => Ok(false),
        },
        Some(UiProjectModal::Search { mut query }) => match input {
            UiInput::Character(_)
            | UiInput::Paste(_)
            | UiInput::Backspace
            | UiInput::Delete
            | UiInput::MoveCursorLeft
            | UiInput::MoveCursorRight
            | UiInput::MoveCursorHome
            | UiInput::MoveCursorEnd => {
                let changed = edit_text_input(
                    &mut model.form,
                    UiFormField::ProjectSearch,
                    &mut query,
                    &input,
                    MAX_PROJECT_TEXT_BYTES,
                );
                if !changed {
                    return Ok(false);
                }
                update_project_search(model, query);
                Ok(true)
            }
            UiInput::NextItem | UiInput::PreviousItem => {
                model.project_search.clone_from(&query);
                select_project_search_match(model, matches!(input, UiInput::NextItem));
                Ok(true)
            }
            UiInput::Activate => {
                model.project_search = query;
                let Some(project) = selected_project(model).cloned() else {
                    return Ok(false);
                };
                let selected_resource = default_project_resource(&project);
                model.project_modal = Some(UiProjectModal::Details {
                    project,
                    selected_resource,
                });
                Ok(true)
            }
            _ => Ok(false),
        },
        Some(UiProjectModal::Details {
            project,
            selected_resource,
        }) => match input {
            UiInput::NextItem | UiInput::PreviousItem => {
                let selected_resource = move_project_resource(
                    &project,
                    selected_resource,
                    matches!(input, UiInput::NextItem),
                );
                model.project_modal = Some(UiProjectModal::Details {
                    project,
                    selected_resource,
                });
                Ok(true)
            }
            UiInput::Character(value) if value.eq_ignore_ascii_case(&'n') => {
                model.project_modal = Some(UiProjectModal::SendInput {
                    project,
                    content: String::new(),
                    submitting: false,
                });
                Ok(true)
            }
            UiInput::Character('a') => {
                model.project_modal = Some(UiProjectModal::AddResource {
                    project,
                    path: String::new(),
                    make_primary: false,
                    submitting: false,
                });
                Ok(true)
            }
            UiInput::Character('e') => {
                let Some(resource_id) = selected_resource else {
                    return Ok(false);
                };
                model.project_modal = Some(UiProjectModal::ReplaceResource {
                    project,
                    resource_id,
                    path: String::new(),
                    submitting: false,
                });
                Ok(true)
            }
            UiInput::Character('x') => {
                let Some(resource_id) = selected_resource else {
                    return Ok(false);
                };
                model.project_modal = Some(UiProjectModal::ConfirmRemoveResource {
                    project,
                    resource_id,
                    force: false,
                    submitting: false,
                });
                Ok(true)
            }
            UiInput::Character('p') => {
                let Some(resource_id) = selected_resource else {
                    return Ok(false);
                };
                model.project_modal = Some(UiProjectModal::ConfirmPrimaryResource {
                    project,
                    resource_id,
                    submitting: false,
                });
                Ok(true)
            }
            UiInput::Character(value @ ('r' | 'R')) => {
                let resource_id = (value == 'r').then_some(selected_resource).flatten();
                if value == 'r' && resource_id.is_none() {
                    return Ok(false);
                }
                model.submit_project(
                    UiProjectAction::CheckResources {
                        project_id: project.project_id,
                        resource_id,
                    },
                    effects,
                )?;
                Ok(true)
            }
            UiInput::Character('v') => {
                if project.assignment.is_some() {
                    model.last_failure = Some(UiFailure {
                        code: "project_already_assigned".to_owned(),
                        action: "use handoff for a project with a current assignment".to_owned(),
                    });
                    return Ok(true);
                }
                open_project_activation(model, project, false);
                Ok(true)
            }
            UiInput::Character('d') => {
                model.submit_project(
                    UiProjectAction::DispatchPending {
                        project_id: project.project_id,
                    },
                    effects,
                )?;
                Ok(true)
            }
            UiInput::Character('h') => {
                if project.assignment.is_none() {
                    model.last_failure = Some(UiFailure {
                        code: "project_unassigned".to_owned(),
                        action: "activate an agent before requesting a handoff".to_owned(),
                    });
                    return Ok(true);
                }
                open_project_activation(model, project, true);
                Ok(true)
            }
            UiInput::Character('o') if project.lifecycle == "closed" => {
                model.submit_project(
                    UiProjectAction::Open {
                        project_id: project.project_id,
                    },
                    effects,
                )?;
                Ok(true)
            }
            UiInput::Character('c') if project.lifecycle == "open" => {
                model.submit_project(
                    UiProjectAction::PreviewClose {
                        project_id: project.project_id,
                    },
                    effects,
                )?;
                Ok(true)
            }
            UiInput::Character('z') => {
                model.project_modal = Some(UiProjectModal::ConfirmArchive {
                    archived: !project.archived,
                    project,
                    submitting: false,
                });
                Ok(true)
            }
            _ => Ok(false),
        },
        Some(
            UiProjectModal::CreateExisting { submitting, .. }
            | UiProjectModal::CreateWorktree { submitting, .. }
            | UiProjectModal::SendInput { submitting, .. }
            | UiProjectModal::AddResource { submitting, .. }
            | UiProjectModal::ReplaceResource { submitting, .. },
        ) => {
            if submitting {
                return Ok(false);
            }
            match input {
                UiInput::NextFocus | UiInput::PreviousFocus => {
                    if matches!(
                        model.project_modal,
                        Some(UiProjectModal::AddResource { .. })
                    ) {
                        let path = UiFormField::Project(UiProjectFormField::Path);
                        let primary = UiFormField::Project(UiProjectFormField::Primary);
                        model.form.focused = Some(if model.form.focused == Some(path) {
                            primary
                        } else {
                            path
                        });
                    } else {
                        cycle_project_field(model, matches!(input, UiInput::NextFocus));
                    }
                    Ok(true)
                }
                UiInput::NextItem | UiInput::PreviousItem
                    if matches!(
                        model.project_modal,
                        Some(UiProjectModal::AddResource { .. })
                    ) && model.form.focused
                        == Some(UiFormField::Project(UiProjectFormField::Primary)) =>
                {
                    if let Some(UiProjectModal::AddResource { make_primary, .. }) =
                        &mut model.project_modal
                    {
                        *make_primary = !*make_primary;
                    }
                    Ok(true)
                }
                UiInput::Activate => submit_project_modal(model, effects),
                _ => Ok(edit_project_field(model, &input)),
            }
        }
        Some(UiProjectModal::ConfirmRemoveResource {
            project,
            resource_id,
            mut force,
            submitting,
        }) => match input {
            UiInput::Character(value) if value.eq_ignore_ascii_case(&'f') && !submitting => {
                force = !force;
                model.project_modal = Some(UiProjectModal::ConfirmRemoveResource {
                    project,
                    resource_id,
                    force,
                    submitting: false,
                });
                Ok(true)
            }
            UiInput::Activate if !submitting => {
                if project.assignment.is_some() && !force {
                    model.last_failure = Some(UiFailure {
                        code: "project_resource_remove_force_required".to_owned(),
                        action: "toggle force to authorize assigned-project removal".to_owned(),
                    });
                    return Ok(true);
                }
                model.project_modal = Some(UiProjectModal::ConfirmRemoveResource {
                    project: project.clone(),
                    resource_id,
                    force,
                    submitting: true,
                });
                model.submit_project(
                    UiProjectAction::RemoveResource {
                        project_id: project.project_id,
                        resource_id,
                        force,
                    },
                    effects,
                )?;
                Ok(true)
            }
            _ => Ok(false),
        },
        Some(UiProjectModal::ConfirmPrimaryResource {
            project,
            resource_id,
            submitting,
        }) if matches!(input, UiInput::Activate) && !submitting => {
            model.project_modal = Some(UiProjectModal::ConfirmPrimaryResource {
                project: project.clone(),
                resource_id,
                submitting: true,
            });
            model.submit_project(
                UiProjectAction::SetPrimaryResource {
                    project_id: project.project_id,
                    resource_id,
                },
                effects,
            )?;
            Ok(true)
        }
        Some(UiProjectModal::ConfirmClose {
            project,
            checks,
            mut confirmed,
            mut force,
            submitting,
        }) => match input {
            UiInput::NextFocus | UiInput::PreviousFocus if !submitting => {
                let confirmation = UiFormField::Project(UiProjectFormField::Confirmation);
                let force = UiFormField::Project(UiProjectFormField::Force);
                model.form.focused = Some(if model.form.focused == Some(confirmation) {
                    force
                } else {
                    confirmation
                });
                Ok(true)
            }
            UiInput::NextItem | UiInput::PreviousItem if !submitting => {
                if model.form.focused == Some(UiFormField::Project(UiProjectFormField::Force)) {
                    force = !force;
                } else {
                    confirmed = !confirmed;
                }
                model.project_modal = Some(UiProjectModal::ConfirmClose {
                    project,
                    checks,
                    confirmed,
                    force,
                    submitting: false,
                });
                let field = model.form.focused;
                if let Some(field) = field {
                    model.form.errors.remove(&field);
                }
                model.last_failure = None;
                Ok(true)
            }
            UiInput::Character('c') if !submitting => {
                confirmed = !confirmed;
                model.project_modal = Some(UiProjectModal::ConfirmClose {
                    project,
                    checks,
                    confirmed,
                    force,
                    submitting: false,
                });
                model
                    .form
                    .errors
                    .remove(&UiFormField::Project(UiProjectFormField::Confirmation));
                model.last_failure = None;
                Ok(true)
            }
            UiInput::Character('f') if !submitting => {
                force = !force;
                model.project_modal = Some(UiProjectModal::ConfirmClose {
                    project,
                    checks,
                    confirmed,
                    force,
                    submitting: false,
                });
                model
                    .form
                    .errors
                    .remove(&UiFormField::Project(UiProjectFormField::Force));
                model.last_failure = None;
                Ok(true)
            }
            UiInput::Activate if !submitting => {
                if !confirmed {
                    let field = UiFormField::Project(UiProjectFormField::Confirmation);
                    model.form.focused = Some(field);
                    model
                        .form
                        .errors
                        .insert(field, "Confirm that the project should close".to_owned());
                    model.last_failure = None;
                    return Ok(true);
                }
                let force_required = checks.iter().any(|check| {
                    check.status != "accepted"
                        || !matches!(check.release.as_deref(), Some("clean" | "not_applicable"))
                });
                if force_required && !force {
                    let field = UiFormField::Project(UiProjectFormField::Force);
                    model.form.focused = Some(field);
                    model.form.errors.insert(
                        field,
                        "Review the release evidence and authorize the override".to_owned(),
                    );
                    model.last_failure = None;
                    return Ok(true);
                }
                model.project_modal = Some(UiProjectModal::ConfirmClose {
                    project: project.clone(),
                    checks,
                    confirmed,
                    force,
                    submitting: true,
                });
                model.submit_project(
                    UiProjectAction::Close {
                        project_id: project.project_id,
                        force,
                    },
                    effects,
                )?;
                Ok(true)
            }
            _ => Ok(false),
        },
        Some(UiProjectModal::ConfirmArchive {
            project,
            archived,
            submitting,
        }) if matches!(input, UiInput::Activate) && !submitting => {
            model.project_modal = Some(UiProjectModal::ConfirmArchive {
                project: project.clone(),
                archived,
                submitting: true,
            });
            model.submit_project(
                UiProjectAction::SetArchived {
                    project_id: project.project_id,
                    archived,
                },
                effects,
            )?;
            Ok(true)
        }
        Some(
            UiProjectModal::Activate { submitting, .. }
            | UiProjectModal::Handoff { submitting, .. },
        ) => {
            if submitting {
                return Ok(false);
            }
            match input {
                UiInput::NextFocus | UiInput::PreviousFocus => {
                    cycle_activation_field(model, matches!(input, UiInput::NextFocus));
                    Ok(true)
                }
                UiInput::NextItem | UiInput::PreviousItem => {
                    adjust_activation_selection(model, matches!(input, UiInput::NextItem));
                    Ok(true)
                }
                UiInput::Activate => submit_project_modal(model, effects),
                _ => Ok(edit_project_field(model, &input)),
            }
        }
        Some(UiProjectModal::Outcome { result }) => {
            submit_project_preview(model, &result, &input, effects)
        }
        Some(
            UiProjectModal::ConfirmPrimaryResource { .. } | UiProjectModal::ConfirmArchive { .. },
        )
        | None => Ok(false),
    }
}

fn stop_guided_activation(model: &mut UiModel) -> bool {
    let Some(project_id) = (match &model.guided_pending {
        Some(UiGuidedPending::Activation(submission)) => Some(submission.project_id),
        _ => None,
    }) else {
        return false;
    };
    model.guided_pending = None;
    model.new_modal = None;
    if model.project_modal.is_none()
        && let Some(project) = model.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .projects
                .iter()
                .find(|project| project.project_id == project_id)
                .cloned()
        })
    {
        model.change_section(UiSection::Projects);
        model.selected_row = Some(agent_hex(project_id));
        model.project_modal = Some(UiProjectModal::Details {
            selected_resource: default_project_resource(&project),
            project,
        });
    }
    true
}

fn default_provider_choice(providers: &[UiProvider]) -> Option<String> {
    providers
        .iter()
        .find(|provider| provider.available && provider.configured_default)
        .or_else(|| providers.iter().find(|provider| provider.available))
        .map(|provider| provider.provider.clone())
}

fn provider_is_available(providers: &[UiProvider], selected: &str) -> bool {
    providers
        .iter()
        .any(|provider| provider.available && provider.provider == selected)
}

fn cycle_provider_choice(
    providers: &[UiProvider],
    selected: Option<&str>,
    forward: bool,
) -> Option<String> {
    let available = providers
        .iter()
        .filter(|provider| provider.available)
        .collect::<Vec<_>>();
    if available.is_empty() {
        return None;
    }
    let current = selected.and_then(|selected| {
        available
            .iter()
            .position(|provider| provider.provider == selected)
    });
    let next = match (current, forward) {
        (Some(index), true) => (index + 1) % available.len(),
        (Some(index), false) => index.checked_sub(1).unwrap_or(available.len() - 1),
        (None, _) => 0,
    };
    Some(available[next].provider.clone())
}

fn open_project_activation(model: &mut UiModel, project: UiProject, handoff: bool) {
    let providers = model
        .snapshot
        .as_ref()
        .map_or_else(Vec::new, |snapshot| snapshot.providers.clone());
    let agents = model
        .snapshot
        .as_ref()
        .map(|snapshot| {
            snapshot
                .agents
                .iter()
                .filter(|agent| {
                    agent.lifecycle == UiAgentLifecycle::Active
                        && agent
                            .mailboxes
                            .iter()
                            .any(|mailbox| mailbox.installation_id == project.home)
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let current_agent = project.assignment.as_ref().map(|value| value.agent_id);
    let agent_id = agents
        .iter()
        .find(|agent| !handoff || Some(agent.agent_id) != current_agent)
        .or_else(|| agents.first())
        .map(|agent| agent.agent_id);
    let thread = agent_id.and_then(|agent_id| {
        project
            .threads
            .iter()
            .find(|thread| thread.agent_id == agent_id)
            .cloned()
    });
    let provider = default_provider_choice(&providers).unwrap_or_default();
    let directory = project
        .resources
        .iter()
        .find(|resource| resource.primary)
        .or_else(|| project.resources.first())
        .map(|resource| resource.display_path.clone())
        .unwrap_or_default();
    model.project_modal = Some(if handoff {
        UiProjectModal::Handoff {
            project,
            agents,
            providers,
            agent_id,
            thread,
            new_session: true,
            provider,
            directory,
            field: UiProjectFormField::Agent,
            confirmed: false,
            force_takeover: false,
            submitting: false,
        }
    } else {
        UiProjectModal::Activate {
            project,
            agents,
            providers,
            agent_id,
            thread,
            new_session: true,
            provider,
            directory,
            field: UiProjectFormField::Agent,
            submitting: false,
        }
    });
    model.last_failure = None;
}

fn cycle_activation_field(model: &mut UiModel, forward: bool) {
    let is_handoff = matches!(&model.project_modal, Some(UiProjectModal::Handoff { .. }));
    let Some(
        UiProjectModal::Activate {
            field, new_session, ..
        }
        | UiProjectModal::Handoff {
            field, new_session, ..
        },
    ) = &mut model.project_modal
    else {
        return;
    };
    let activation = [
        UiProjectFormField::Agent,
        UiProjectFormField::SessionMode,
        UiProjectFormField::Thread,
        UiProjectFormField::Provider,
        UiProjectFormField::Directory,
    ];
    let handoff = [
        UiProjectFormField::Agent,
        UiProjectFormField::SessionMode,
        UiProjectFormField::Thread,
        UiProjectFormField::Provider,
        UiProjectFormField::Directory,
        UiProjectFormField::Confirmation,
        UiProjectFormField::Force,
    ];
    let fields = if is_handoff && *new_session {
        &[
            UiProjectFormField::Agent,
            UiProjectFormField::SessionMode,
            UiProjectFormField::Provider,
            UiProjectFormField::Directory,
            UiProjectFormField::Confirmation,
            UiProjectFormField::Force,
        ][..]
    } else if is_handoff {
        handoff.as_slice()
    } else if *new_session {
        &[
            UiProjectFormField::Agent,
            UiProjectFormField::SessionMode,
            UiProjectFormField::Provider,
            UiProjectFormField::Directory,
        ][..]
    } else {
        activation.as_slice()
    };
    let current = fields
        .iter()
        .position(|candidate| candidate == field)
        .unwrap_or(0);
    let next = if forward {
        (current + 1) % fields.len()
    } else {
        current.checked_sub(1).unwrap_or(fields.len() - 1)
    };
    *field = fields[next];
}

fn adjust_activation_selection(model: &mut UiModel, forward: bool) {
    let field = match &model.project_modal {
        Some(UiProjectModal::Activate { field, .. } | UiProjectModal::Handoff { field, .. }) => {
            *field
        }
        _ => return,
    };
    match field {
        UiProjectFormField::Agent => cycle_activation_agent(model),
        UiProjectFormField::Thread => cycle_activation_thread(model),
        UiProjectFormField::SessionMode => toggle_activation_mode(model),
        UiProjectFormField::Provider => cycle_activation_provider(model, forward),
        UiProjectFormField::Confirmation => {
            if let Some(UiProjectModal::Handoff { confirmed, .. }) = &mut model.project_modal {
                *confirmed = !*confirmed;
            }
        }
        UiProjectFormField::Force => {
            if let Some(UiProjectModal::Handoff { force_takeover, .. }) = &mut model.project_modal {
                *force_takeover = !*force_takeover;
            }
        }
        _ => {}
    }
    model.form.errors.remove(&UiFormField::Project(field));
    model.last_failure = None;
}

fn cycle_activation_provider(model: &mut UiModel, forward: bool) {
    let Some(
        UiProjectModal::Activate {
            providers,
            provider,
            new_session: true,
            ..
        }
        | UiProjectModal::Handoff {
            providers,
            provider,
            new_session: true,
            ..
        },
    ) = &mut model.project_modal
    else {
        return;
    };
    if let Some(selected) = cycle_provider_choice(providers, Some(provider), forward) {
        *provider = selected;
    }
}

fn cycle_activation_agent(model: &mut UiModel) {
    let Some(
        UiProjectModal::Activate {
            project,
            agents,
            providers,
            agent_id,
            thread,
            new_session,
            provider,
            ..
        }
        | UiProjectModal::Handoff {
            project,
            agents,
            providers,
            agent_id,
            thread,
            new_session,
            provider,
            ..
        },
    ) = &mut model.project_modal
    else {
        return;
    };
    if agents.is_empty() {
        return;
    }
    let current = agent_id
        .and_then(|id| agents.iter().position(|agent| agent.agent_id == id))
        .unwrap_or(agents.len() - 1);
    let selected = agents[(current + 1) % agents.len()].agent_id;
    *agent_id = Some(selected);
    *thread = project
        .threads
        .iter()
        .find(|candidate| candidate.agent_id == selected)
        .cloned();
    if *new_session {
        if let Some(selected) = default_provider_choice(providers) {
            *provider = selected;
        }
    } else if let Some(selected_thread) = thread {
        provider.clone_from(&selected_thread.provider);
    }
    model.last_failure = None;
}

fn cycle_activation_thread(model: &mut UiModel) {
    let Some(
        UiProjectModal::Activate {
            project,
            agent_id,
            thread,
            provider,
            ..
        }
        | UiProjectModal::Handoff {
            project,
            agent_id,
            thread,
            provider,
            ..
        },
    ) = &mut model.project_modal
    else {
        return;
    };
    let Some(agent_id) = *agent_id else { return };
    let candidates = project
        .threads
        .iter()
        .filter(|candidate| candidate.agent_id == agent_id)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        *thread = None;
        return;
    }
    let current = thread
        .as_ref()
        .and_then(|selected| {
            candidates
                .iter()
                .position(|candidate| **candidate == *selected)
        })
        .unwrap_or(candidates.len() - 1);
    let selected = candidates[(current + 1) % candidates.len()].clone();
    provider.clone_from(&selected.provider);
    *thread = Some(selected);
    model.last_failure = None;
}

fn toggle_activation_mode(model: &mut UiModel) {
    if let Some(
        UiProjectModal::Activate {
            new_session,
            project,
            providers,
            agent_id,
            thread,
            provider,
            ..
        }
        | UiProjectModal::Handoff {
            new_session,
            project,
            providers,
            agent_id,
            thread,
            provider,
            ..
        },
    ) = &mut model.project_modal
    {
        *new_session = !*new_session;
        if *new_session {
            *provider = default_provider_choice(providers).unwrap_or_default();
        } else if thread.is_none() {
            *thread = agent_id.and_then(|id| {
                project
                    .threads
                    .iter()
                    .find(|candidate| candidate.agent_id == id)
                    .cloned()
            });
        }
        if !*new_session && let Some(selected) = thread {
            provider.clone_from(&selected.provider);
        }
        model.last_failure = None;
    }
}

fn previous_char_boundary(value: &str, cursor: usize) -> usize {
    value[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_char_boundary(value: &str, cursor: usize) -> usize {
    value[cursor..]
        .chars()
        .next()
        .map_or(value.len(), |character| cursor + character.len_utf8())
}

fn edit_text(
    form: &mut UiFormState,
    field: UiFormField,
    target: &mut String,
    edit: TextEdit<'_>,
    max_bytes: usize,
) -> bool {
    let mut cursor = form
        .cursors
        .get(&field)
        .copied()
        .unwrap_or(target.len())
        .min(target.len());
    while !target.is_char_boundary(cursor) {
        cursor = cursor.saturating_sub(1);
    }
    let handled = match edit {
        TextEdit::Insert(value) => {
            if target.len().saturating_add(value.len()) > max_bytes {
                form.errors
                    .insert(field, format!("Keep this field under {max_bytes} bytes"));
                true
            } else {
                target.insert_str(cursor, value);
                cursor += value.len();
                form.errors.remove(&field);
                true
            }
        }
        TextEdit::Backspace if cursor > 0 => {
            let previous = previous_char_boundary(target, cursor);
            target.replace_range(previous..cursor, "");
            cursor = previous;
            form.errors.remove(&field);
            true
        }
        TextEdit::Delete if cursor < target.len() => {
            let next = next_char_boundary(target, cursor);
            target.replace_range(cursor..next, "");
            form.errors.remove(&field);
            true
        }
        TextEdit::Left if cursor > 0 => {
            cursor = previous_char_boundary(target, cursor);
            true
        }
        TextEdit::Right if cursor < target.len() => {
            cursor = next_char_boundary(target, cursor);
            true
        }
        TextEdit::Home if cursor != 0 => {
            cursor = 0;
            true
        }
        TextEdit::End if cursor != target.len() => {
            cursor = target.len();
            true
        }
        TextEdit::Backspace
        | TextEdit::Delete
        | TextEdit::Left
        | TextEdit::Right
        | TextEdit::Home
        | TextEdit::End => false,
    };
    form.cursors.insert(field, cursor);
    handled
}

fn edit_text_input(
    form: &mut UiFormState,
    field: UiFormField,
    target: &mut String,
    input: &UiInput,
    max_bytes: usize,
) -> bool {
    let mut encoded = [0_u8; 4];
    let edit = match input {
        UiInput::Character(value) => TextEdit::Insert(value.encode_utf8(&mut encoded)),
        UiInput::Paste(value) => TextEdit::Insert(value),
        UiInput::Backspace => TextEdit::Backspace,
        UiInput::Delete => TextEdit::Delete,
        UiInput::MoveCursorLeft => TextEdit::Left,
        UiInput::MoveCursorRight => TextEdit::Right,
        UiInput::MoveCursorHome => TextEdit::Home,
        UiInput::MoveCursorEnd => TextEdit::End,
        _ => return false,
    };
    edit_text(form, field, target, edit, max_bytes)
}

fn normalize_path_input(value: &str, home: Option<&str>) -> Result<String, &'static str> {
    if value.is_empty() {
        return Err("Enter a path");
    }
    let expanded = if value == "~" {
        home.ok_or("Your home directory is unavailable; enter an absolute path")?
            .to_owned()
    } else if let Some(relative) = value.strip_prefix("~/") {
        let home = home.ok_or("Your home directory is unavailable; enter an absolute path")?;
        Path::new(home)
            .join(relative)
            .to_string_lossy()
            .into_owned()
    } else {
        value.to_owned()
    };
    let path = Path::new(&expanded);
    if !path.is_absolute() {
        return Err("Use an absolute path, ~, or ~/…");
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or("This path cannot be displayed as terminal text")
}

fn edit_project_field(model: &mut UiModel, input: &UiInput) -> bool {
    if matches!(
        model.project_modal,
        Some(UiProjectModal::AddResource { .. })
    ) && model.form.focused != Some(UiFormField::Project(UiProjectFormField::Path))
    {
        return false;
    }
    let field = match &model.project_modal {
        Some(
            UiProjectModal::CreateExisting { field, .. }
            | UiProjectModal::CreateWorktree { field, .. }
            | UiProjectModal::Activate { field, .. }
            | UiProjectModal::Handoff { field, .. },
        ) => UiFormField::Project(*field),
        Some(UiProjectModal::SendInput { .. }) => UiFormField::Project(UiProjectFormField::Content),
        Some(UiProjectModal::AddResource { .. } | UiProjectModal::ReplaceResource { .. }) => {
            UiFormField::Project(UiProjectFormField::Path)
        }
        _ => return false,
    };
    let form = &mut model.form;
    let target = match &mut model.project_modal {
        Some(UiProjectModal::CreateExisting {
            name,
            brief,
            path,
            field,
            ..
        }) => match field {
            UiProjectFormField::Name => name,
            UiProjectFormField::Brief => brief,
            UiProjectFormField::Path => path,
            _ => return false,
        },
        Some(UiProjectModal::CreateWorktree {
            name,
            brief,
            source,
            destination,
            branch,
            base,
            field,
            ..
        }) => match field {
            UiProjectFormField::Name => name,
            UiProjectFormField::Brief => brief,
            UiProjectFormField::Source => source,
            UiProjectFormField::Destination => destination,
            UiProjectFormField::Branch => branch,
            UiProjectFormField::Base => base,
            _ => return false,
        },
        Some(UiProjectModal::SendInput { content, .. }) => content,
        Some(
            UiProjectModal::AddResource { path, .. } | UiProjectModal::ReplaceResource { path, .. },
        ) => path,
        Some(
            UiProjectModal::Activate {
                directory, field, ..
            }
            | UiProjectModal::Handoff {
                directory, field, ..
            },
        ) => match field {
            UiProjectFormField::Directory => directory,
            _ => return false,
        },
        _ => return false,
    };
    let changed = edit_text_input(form, field, target, input, MAX_PROJECT_TEXT_BYTES);
    if changed {
        model.last_failure = None;
    }
    changed
}

fn cycle_project_field(model: &mut UiModel, forward: bool) {
    default_existing_project_name(model);
    let (fields, selected) = match &model.project_modal {
        Some(UiProjectModal::CreateExisting { field, .. }) => (
            &[
                UiProjectFormField::Path,
                UiProjectFormField::Name,
                UiProjectFormField::Brief,
            ][..],
            *field,
        ),
        Some(UiProjectModal::CreateWorktree { field, .. }) => (
            &[
                UiProjectFormField::Name,
                UiProjectFormField::Brief,
                UiProjectFormField::Source,
                UiProjectFormField::Destination,
                UiProjectFormField::Branch,
                UiProjectFormField::Base,
            ][..],
            *field,
        ),
        _ => return,
    };
    let current = fields
        .iter()
        .position(|candidate| *candidate == selected)
        .unwrap_or(0);
    let next = if forward {
        (current + 1) % fields.len()
    } else {
        current.checked_sub(1).unwrap_or(fields.len() - 1)
    };
    if let Some(
        UiProjectModal::CreateExisting { field, .. } | UiProjectModal::CreateWorktree { field, .. },
    ) = &mut model.project_modal
    {
        *field = fields[next];
    }
}

fn default_existing_project_name(model: &mut UiModel) {
    let Some(UiProjectModal::CreateExisting {
        name, path, field, ..
    }) = &model.project_modal
    else {
        return;
    };
    if *field != UiProjectFormField::Path || !name.is_empty() || path.is_empty() {
        return;
    }
    let normalized = normalize_path_input(path, model.home_directory.as_deref())
        .unwrap_or_else(|_| path.clone());
    let Some(candidate) = Path::new(&normalized)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
    else {
        return;
    };
    if let Some(UiProjectModal::CreateExisting { name, .. }) = &mut model.project_modal {
        name.clone_from(&candidate);
    }
    model.form.cursors.insert(
        UiFormField::Project(UiProjectFormField::Name),
        candidate.len(),
    );
}

fn open_existing_project_form(model: &mut UiModel) {
    model.project_modal = Some(UiProjectModal::CreateExisting {
        name: String::new(),
        brief: String::new(),
        path: String::new(),
        field: UiProjectFormField::Path,
        submitting: false,
    });
    model.last_failure = None;
}

fn open_worktree_project_form(model: &mut UiModel) {
    model.project_modal = Some(UiProjectModal::CreateWorktree {
        name: String::new(),
        brief: String::new(),
        source: String::new(),
        destination: String::new(),
        branch: String::new(),
        base: String::new(),
        field: UiProjectFormField::Name,
        submitting: false,
    });
    model.last_failure = None;
}

#[allow(clippy::too_many_lines)]
fn submit_project_modal(model: &mut UiModel, effects: &mut Vec<UiEffect>) -> Result<bool, UiError> {
    fn reject(model: &mut UiModel, field: UiProjectFormField, message: &str) -> bool {
        if let Some(
            UiProjectModal::CreateExisting {
                field: selected, ..
            }
            | UiProjectModal::CreateWorktree {
                field: selected, ..
            }
            | UiProjectModal::Activate {
                field: selected, ..
            }
            | UiProjectModal::Handoff {
                field: selected, ..
            },
        ) = &mut model.project_modal
        {
            *selected = field;
        }
        model
            .form
            .errors
            .insert(UiFormField::Project(field), message.to_owned());
        model.last_failure = None;
        true
    }

    fn normalized_path(
        model: &mut UiModel,
        field: UiProjectFormField,
        value: &str,
    ) -> Option<String> {
        match normalize_path_input(value, model.home_directory.as_deref()) {
            Ok(path) => Some(path),
            Err(message) => {
                reject(model, field, message);
                None
            }
        }
    }

    default_existing_project_name(model);
    let action = match model.project_modal.clone() {
        Some(UiProjectModal::CreateExisting {
            name, brief, path, ..
        }) => {
            let Some(path) = normalized_path(model, UiProjectFormField::Path, &path) else {
                return Ok(true);
            };
            if name.is_empty() {
                reject(model, UiProjectFormField::Name, "Enter a project name");
                return Ok(true);
            }
            UiProjectAction::CreateExisting {
                name,
                brief: (!brief.is_empty()).then_some(brief),
                path,
            }
        }
        Some(UiProjectModal::CreateWorktree {
            name,
            brief,
            source,
            destination,
            branch,
            base,
            ..
        }) => {
            if name.is_empty() {
                reject(model, UiProjectFormField::Name, "Enter a project name");
                return Ok(true);
            }
            let Some(source) = normalized_path(model, UiProjectFormField::Source, &source) else {
                return Ok(true);
            };
            let Some(destination) =
                normalized_path(model, UiProjectFormField::Destination, &destination)
            else {
                return Ok(true);
            };
            if branch.is_empty() {
                reject(
                    model,
                    UiProjectFormField::Branch,
                    "Enter the new Git branch name",
                );
                return Ok(true);
            }
            UiProjectAction::CreateWorktree {
                name,
                brief: (!brief.is_empty()).then_some(brief),
                source,
                destination,
                branch,
                base: (!base.is_empty()).then_some(base),
            }
        }
        Some(UiProjectModal::SendInput {
            project, content, ..
        }) => {
            if content.is_empty() {
                reject(
                    model,
                    UiProjectFormField::Content,
                    "Enter instructions for the agent",
                );
                return Ok(true);
            }
            UiProjectAction::SendInput {
                project_id: project.project_id,
                content,
            }
        }
        Some(UiProjectModal::AddResource {
            project,
            path,
            make_primary,
            ..
        }) => {
            let Some(path) = normalized_path(model, UiProjectFormField::Path, &path) else {
                return Ok(true);
            };
            UiProjectAction::PreviewAddResource {
                project_id: project.project_id,
                path,
                make_primary,
            }
        }
        Some(UiProjectModal::ReplaceResource {
            project,
            resource_id,
            path,
            ..
        }) => {
            let Some(path) = normalized_path(model, UiProjectFormField::Path, &path) else {
                return Ok(true);
            };
            UiProjectAction::PreviewReplaceResource {
                project_id: project.project_id,
                resource_id,
                path,
            }
        }
        Some(UiProjectModal::Activate {
            project,
            providers,
            agent_id,
            thread,
            new_session,
            provider,
            directory,
            ..
        }) => {
            let Some(agent_id) = agent_id else {
                reject(
                    model,
                    UiProjectFormField::Agent,
                    "Choose an available agent",
                );
                return Ok(true);
            };
            if new_session && !provider_is_available(&providers, &provider) {
                reject(
                    model,
                    UiProjectFormField::Provider,
                    "No available agent service is selected",
                );
                return Ok(true);
            }
            if !new_session
                && thread
                    .as_ref()
                    .is_none_or(|selected| selected.provider != provider)
            {
                reject(
                    model,
                    UiProjectFormField::Thread,
                    "Choose an exact conversation to resume",
                );
                return Ok(true);
            }
            let Some(directory) = normalized_path(model, UiProjectFormField::Directory, &directory)
            else {
                return Ok(true);
            };
            UiProjectAction::Activate {
                project_id: project.project_id,
                agent_id,
                provider,
                resume_session: (!new_session)
                    .then(|| thread.as_ref().map(|value| value.session.clone()))
                    .flatten(),
                resume_thread: thread.map(|value| value.thread_id),
                launch_directory: directory,
            }
        }
        Some(UiProjectModal::Handoff {
            project,
            providers,
            agent_id,
            thread,
            new_session,
            provider,
            directory,
            confirmed,
            force_takeover,
            ..
        }) => {
            let Some(agent_id) = agent_id else {
                reject(
                    model,
                    UiProjectFormField::Agent,
                    "Choose the receiving agent",
                );
                return Ok(true);
            };
            let Some(thread) = thread else {
                reject(
                    model,
                    UiProjectFormField::Thread,
                    "Choose the project conversation to transfer",
                );
                return Ok(true);
            };
            if new_session && !provider_is_available(&providers, &provider) {
                reject(
                    model,
                    UiProjectFormField::Provider,
                    "No available agent service is selected",
                );
                return Ok(true);
            }
            if !new_session && thread.provider != provider {
                reject(
                    model,
                    UiProjectFormField::Thread,
                    "Choose a conversation from this provider",
                );
                return Ok(true);
            }
            let Some(directory) = normalized_path(model, UiProjectFormField::Directory, &directory)
            else {
                return Ok(true);
            };
            if !confirmed {
                reject(
                    model,
                    UiProjectFormField::Confirmation,
                    "Confirm that the project should move",
                );
                return Ok(true);
            }
            UiProjectAction::Handoff {
                project_id: project.project_id,
                agent_id,
                provider,
                resume_session: (!new_session).then_some(thread.session),
                thread_id: thread.thread_id,
                launch_directory: directory,
                force_takeover,
            }
        }
        _ => return Ok(false),
    };
    match &mut model.project_modal {
        Some(
            UiProjectModal::CreateExisting { submitting, .. }
            | UiProjectModal::CreateWorktree { submitting, .. }
            | UiProjectModal::SendInput { submitting, .. }
            | UiProjectModal::AddResource { submitting, .. }
            | UiProjectModal::ReplaceResource { submitting, .. }
            | UiProjectModal::Activate { submitting, .. }
            | UiProjectModal::Handoff { submitting, .. },
        ) => *submitting = true,
        _ => return Ok(false),
    }
    model.last_failure = None;
    model.submit_project(action, effects)?;
    Ok(true)
}

fn submit_project_preview(
    model: &mut UiModel,
    result: &UiProjectResult,
    input: &UiInput,
    effects: &mut Vec<UiEffect>,
) -> Result<bool, UiError> {
    if !matches!(input, UiInput::Activate) {
        return Ok(false);
    }
    if matches!(result.action, UiProjectAction::PreviewClose { .. }) {
        let UiProjectOutcome::ResourceChecks { checks } = &result.outcome else {
            return Ok(false);
        };
        let Some(project) = model
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .projects
                    .iter()
                    .find(|project| project.project_id == result.project_id)
            })
            .cloned()
        else {
            model.last_failure = Some(UiFailure {
                code: "project_target_stale".to_owned(),
                action: "reload and reselect the project before closing".to_owned(),
            });
            return Ok(true);
        };
        model.project_modal = Some(UiProjectModal::ConfirmClose {
            project,
            checks: checks.clone(),
            confirmed: false,
            force: false,
            submitting: false,
        });
        return Ok(true);
    }
    let UiProjectOutcome::ResourcePreview { conflicts, .. } = &result.outcome else {
        return Ok(false);
    };
    if !conflicts.is_empty() {
        model.last_failure = Some(UiFailure {
            code: "project_resource_claim_conflict".to_owned(),
            action: "choose a non-overlapping resource path".to_owned(),
        });
        return Ok(true);
    }
    let action = match &result.action {
        UiProjectAction::PreviewCreateExisting { name, brief, path } => {
            UiProjectAction::CreateExisting {
                name: name.clone(),
                brief: brief.clone(),
                path: path.clone(),
            }
        }
        UiProjectAction::PreviewAddResource {
            project_id,
            path,
            make_primary,
        } => UiProjectAction::AddResource {
            project_id: *project_id,
            path: path.clone(),
            make_primary: *make_primary,
        },
        UiProjectAction::PreviewReplaceResource {
            project_id,
            resource_id,
            path,
        } => UiProjectAction::ReplaceResource {
            project_id: *project_id,
            resource_id: *resource_id,
            path: path.clone(),
        },
        _ => return Ok(false),
    };
    model.submit_project(action, effects)?;
    Ok(true)
}

#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
fn apply_agent_modal_input(
    model: &mut UiModel,
    input: UiInput,
    effects: &mut Vec<UiEffect>,
) -> Result<bool, UiError> {
    if matches!(input, UiInput::Quit) {
        model.should_exit = true;
        effects.push(UiEffect::Exit);
        return Ok(false);
    }
    if matches!(input, UiInput::Escape) {
        if model.pending_agent.is_none() && model.pending_managed_session.is_none() {
            if let Some(UiAgentModal::Search { query }) = &model.agent_modal {
                model.agent_search.clone_from(query);
            }
            if let Some(UiGuidedPending::AgentCreation { project_id, .. }) =
                model.guided_pending.clone()
                && let Some(project) = model.snapshot.as_ref().and_then(|snapshot| {
                    snapshot
                        .projects
                        .iter()
                        .find(|project| project.project_id == project_id)
                        .cloned()
                })
            {
                model.guided_pending = None;
                model.new_modal = Some(guided_agent_picker(model, project));
            }
            model.agent_modal = None;
            return Ok(true);
        }
        return Ok(false);
    }
    match model.agent_modal.clone() {
        Some(UiAgentModal::Search { mut query }) => match input {
            UiInput::Character(_)
            | UiInput::Paste(_)
            | UiInput::Backspace
            | UiInput::Delete
            | UiInput::MoveCursorLeft
            | UiInput::MoveCursorRight
            | UiInput::MoveCursorHome
            | UiInput::MoveCursorEnd => {
                if !edit_text_input(
                    &mut model.form,
                    UiFormField::AgentSearch,
                    &mut query,
                    &input,
                    MAX_AGENT_TEXT_BYTES,
                ) {
                    return Ok(false);
                }
                update_agent_search(model, query);
                Ok(true)
            }
            UiInput::NextItem | UiInput::PreviousItem => {
                model.agent_search.clone_from(&query);
                select_agent_search_match(model, matches!(input, UiInput::NextItem));
                Ok(true)
            }
            UiInput::Activate => {
                model.agent_search = query;
                let Some(agent) = selected_agent(model).cloned() else {
                    return Ok(false);
                };
                let selected_session = default_agent_session(&agent);
                model.agent_modal = Some(UiAgentModal::Details {
                    agent,
                    selected_session,
                });
                Ok(true)
            }
            _ => Ok(false),
        },
        Some(UiAgentModal::Details {
            agent,
            selected_session,
        }) => match input {
            UiInput::NextItem | UiInput::PreviousItem => {
                let next = move_agent_session(
                    &agent,
                    selected_session.as_ref(),
                    matches!(input, UiInput::NextItem),
                );
                model.agent_modal = Some(UiAgentModal::Details {
                    agent,
                    selected_session: next,
                });
                Ok(true)
            }
            UiInput::Character(value) if value.eq_ignore_ascii_case(&'s') => {
                let providers = model
                    .snapshot
                    .as_ref()
                    .map_or_else(Vec::new, |snapshot| snapshot.providers.clone());
                let selected = default_provider_choice(&providers);
                if providers
                    .iter()
                    .filter(|provider| provider.available)
                    .count()
                    == 1
                {
                    let provider = selected.unwrap_or_default();
                    let switching = agent.sessions.iter().any(|session| session.selected);
                    let action = UiManagedSessionAction::Start {
                        agent_id: agent.agent_id,
                        provider,
                    };
                    begin_managed_session(model, agent, action, switching, effects)?;
                } else {
                    model.agent_modal = Some(UiAgentModal::ManagedProvider {
                        agent,
                        providers,
                        selected,
                    });
                }
                Ok(true)
            }
            UiInput::Character(value) if value.eq_ignore_ascii_case(&'e') => {
                let Some((provider, session)) = selected_session else {
                    model.last_failure = Some(UiFailure {
                        code: "managed_session_target_missing".to_owned(),
                        action: "select one exact durable provider session to resume".to_owned(),
                    });
                    return Ok(true);
                };
                let switching = !agent.sessions.iter().any(|candidate| {
                    candidate.provider == provider
                        && candidate.session == session
                        && candidate.selected
                });
                let action = UiManagedSessionAction::Resume {
                    agent_id: agent.agent_id,
                    provider,
                    session,
                };
                begin_managed_session(model, agent, action, switching, effects)?;
                Ok(true)
            }
            UiInput::Character(value) if value.eq_ignore_ascii_case(&'t') => {
                let Some((provider, _)) = selected_session else {
                    model.last_failure = Some(UiFailure {
                        code: "managed_session_provider_missing".to_owned(),
                        action: "select a durable provider session before stopping".to_owned(),
                    });
                    return Ok(true);
                };
                let action = UiManagedSessionAction::Stop {
                    agent_id: agent.agent_id,
                    provider,
                };
                begin_managed_session(model, agent, action, false, effects)?;
                Ok(true)
            }
            UiInput::Character(value) if value.eq_ignore_ascii_case(&'r') => {
                let Some((provider, session)) = selected_session else {
                    return Ok(false);
                };
                let display_name = agent
                    .sessions
                    .iter()
                    .find(|candidate| {
                        candidate.provider == provider && candidate.session == session
                    })
                    .and_then(|candidate| candidate.display_name.clone())
                    .unwrap_or_default();
                model.agent_modal = Some(UiAgentModal::RenameSession {
                    agent_id: agent.agent_id,
                    provider,
                    session,
                    display_name,
                    submitting: false,
                });
                Ok(true)
            }
            UiInput::Character(value) if value.eq_ignore_ascii_case(&'x') => {
                if agent.lifecycle != UiAgentLifecycle::Active {
                    model.last_failure = Some(UiFailure {
                        code: "agent_not_active".to_owned(),
                        action: "select one active unconflicted agent".to_owned(),
                    });
                    return Ok(true);
                }
                model.agent_modal = Some(UiAgentModal::ConfirmRetire {
                    agent,
                    force: false,
                    submitting: false,
                });
                Ok(true)
            }
            _ => Ok(false),
        },
        Some(UiAgentModal::Create {
            mut name,
            submitting,
        }) => match input {
            UiInput::Character(_)
            | UiInput::Paste(_)
            | UiInput::Backspace
            | UiInput::Delete
            | UiInput::MoveCursorLeft
            | UiInput::MoveCursorRight
            | UiInput::MoveCursorHome
            | UiInput::MoveCursorEnd
                if !submitting =>
            {
                if !edit_text_input(
                    &mut model.form,
                    UiFormField::AgentName,
                    &mut name,
                    &input,
                    MAX_AGENT_TEXT_BYTES,
                ) {
                    return Ok(false);
                }
                model.agent_modal = Some(UiAgentModal::Create {
                    name,
                    submitting: false,
                });
                Ok(true)
            }
            UiInput::Activate if !submitting => {
                if name.is_empty() {
                    model.form.errors.insert(
                        UiFormField::AgentName,
                        "Enter a permanent lowercase agent name".to_owned(),
                    );
                    model.last_failure = None;
                    return Ok(true);
                }
                let bytes = name.as_bytes();
                if !bytes[0].is_ascii_lowercase()
                    || !bytes.iter().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-'
                    })
                {
                    model.form.errors.insert(
                        UiFormField::AgentName,
                        "Use lowercase letters, numbers, and hyphens; start with a letter"
                            .to_owned(),
                    );
                    model.last_failure = None;
                    return Ok(true);
                }
                model.agent_modal = Some(UiAgentModal::Create {
                    name: name.clone(),
                    submitting: true,
                });
                model.submit_agent(UiAgentAction::Create { name }, effects)?;
                Ok(true)
            }
            _ => Ok(false),
        },
        Some(UiAgentModal::RenameSession {
            agent_id,
            provider,
            session,
            mut display_name,
            submitting,
        }) => match input {
            UiInput::Character(_)
            | UiInput::Paste(_)
            | UiInput::Backspace
            | UiInput::Delete
            | UiInput::MoveCursorLeft
            | UiInput::MoveCursorRight
            | UiInput::MoveCursorHome
            | UiInput::MoveCursorEnd
                if !submitting =>
            {
                if !edit_text_input(
                    &mut model.form,
                    UiFormField::SessionName,
                    &mut display_name,
                    &input,
                    MAX_AGENT_TEXT_BYTES,
                ) {
                    return Ok(false);
                }
                model.agent_modal = Some(UiAgentModal::RenameSession {
                    agent_id,
                    provider,
                    session,
                    display_name,
                    submitting: false,
                });
                Ok(true)
            }
            UiInput::Activate if !submitting => {
                let value = (!display_name.is_empty()).then_some(display_name.clone());
                model.agent_modal = Some(UiAgentModal::RenameSession {
                    agent_id,
                    provider: provider.clone(),
                    session: session.clone(),
                    display_name,
                    submitting: true,
                });
                model.submit_agent(
                    UiAgentAction::RenameSession {
                        agent_id,
                        provider,
                        session,
                        display_name: value,
                    },
                    effects,
                )?;
                Ok(true)
            }
            _ => Ok(false),
        },
        Some(UiAgentModal::ConfirmRetire {
            agent,
            mut force,
            submitting,
        }) => match input {
            UiInput::Character(value) if value.eq_ignore_ascii_case(&'f') && !submitting => {
                force = !force;
                model.agent_modal = Some(UiAgentModal::ConfirmRetire {
                    agent,
                    force,
                    submitting: false,
                });
                Ok(true)
            }
            UiInput::Activate if !submitting => {
                model.agent_modal = Some(UiAgentModal::ConfirmRetire {
                    agent: agent.clone(),
                    force,
                    submitting: true,
                });
                model.submit_agent(
                    UiAgentAction::Retire {
                        agent_id: agent.agent_id,
                        force,
                    },
                    effects,
                )?;
                Ok(true)
            }
            _ => Ok(false),
        },
        Some(UiAgentModal::ManagedProvider {
            agent,
            providers,
            mut selected,
        }) => match input {
            UiInput::NextItem | UiInput::PreviousItem => {
                selected = cycle_provider_choice(
                    &providers,
                    selected.as_deref(),
                    matches!(input, UiInput::NextItem),
                );
                model.agent_modal = Some(UiAgentModal::ManagedProvider {
                    agent,
                    providers,
                    selected,
                });
                model.last_failure = None;
                Ok(true)
            }
            UiInput::Activate => {
                let Some(provider) =
                    selected.filter(|selected| provider_is_available(&providers, selected))
                else {
                    return Ok(true);
                };
                let switching = agent.sessions.iter().any(|session| session.selected);
                let action = UiManagedSessionAction::Start {
                    agent_id: agent.agent_id,
                    provider,
                };
                begin_managed_session(model, agent, action, switching, effects)?;
                Ok(true)
            }
            _ => Ok(false),
        },
        Some(UiAgentModal::ConfirmManagedSession { agent, action }) => match input {
            UiInput::Activate => {
                model.agent_modal = Some(UiAgentModal::ManagingSession {
                    agent,
                    action: action.clone(),
                });
                model.submit_managed_session(action, effects)?;
                Ok(true)
            }
            _ => Ok(false),
        },
        Some(UiAgentModal::ManagingSession { .. } | UiAgentModal::ManagedSessionOutcome { .. })
        | None => Ok(false),
    }
}

fn begin_managed_session(
    model: &mut UiModel,
    agent: UiAgent,
    action: UiManagedSessionAction,
    switching: bool,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    if agent.lifecycle != UiAgentLifecycle::Active {
        model.last_failure = Some(UiFailure {
            code: "managed_session_agent_not_active".to_owned(),
            action: "select one active unconflicted named agent".to_owned(),
        });
        return Ok(());
    }
    model.last_failure = None;
    if switching {
        model.agent_modal = Some(UiAgentModal::ConfirmManagedSession { agent, action });
    } else {
        model.agent_modal = Some(UiAgentModal::ManagingSession {
            agent,
            action: action.clone(),
        });
        model.submit_managed_session(action, effects)?;
    }
    Ok(())
}

fn update_agent_search(model: &mut UiModel, query: String) {
    model.agent_search.clone_from(&query);
    model.agent_modal = Some(UiAgentModal::Search { query });
    model.last_failure = None;
    select_agent_search_match(model, false);
}

fn update_project_search(model: &mut UiModel, query: String) {
    model.project_search.clone_from(&query);
    model.project_modal = Some(UiProjectModal::Search { query });
    model.last_failure = None;
    select_project_search_match(model, false);
}

fn selected_project(model: &UiModel) -> Option<&UiProject> {
    let selected = model.selected_row.as_deref()?;
    model
        .snapshot
        .as_ref()?
        .projects
        .iter()
        .find(|project| agent_hex(project.project_id) == selected)
}

fn project_matches(project: &UiProject, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let query = query.to_lowercase();
    agent_hex(project.project_id).contains(&query)
        || project.name.to_lowercase().contains(&query)
        || project.resources.iter().any(|resource| {
            resource.display_path.to_lowercase().contains(&query)
                || resource.canonical_path.to_lowercase().contains(&query)
        })
}

fn default_project_resource(project: &UiProject) -> Option<[u8; 32]> {
    project
        .resources
        .iter()
        .find(|resource| resource.primary)
        .or_else(|| project.resources.first())
        .map(|resource| resource.resource_id)
}

fn move_project_resource(
    project: &UiProject,
    selected: Option<[u8; 32]>,
    forward: bool,
) -> Option<[u8; 32]> {
    if project.resources.is_empty() {
        return None;
    }
    let current = selected.and_then(|selected| {
        project
            .resources
            .iter()
            .position(|resource| resource.resource_id == selected)
    });
    let next = match (current, forward) {
        (Some(index), true) => (index + 1).min(project.resources.len() - 1),
        (Some(index), false) => index.saturating_sub(1),
        (None, _) => 0,
    };
    Some(project.resources[next].resource_id)
}

fn select_project_search_match(model: &mut UiModel, forward: bool) {
    if model.section != UiSection::Projects || model.project_search.is_empty() {
        return;
    }
    let Some(snapshot) = &model.snapshot else {
        return;
    };
    let matches = snapshot
        .projects
        .iter()
        .filter(|project| project_matches(project, &model.project_search))
        .map(|project| agent_hex(project.project_id))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return;
    }
    let current = model
        .selected_row
        .as_ref()
        .and_then(|selected| matches.iter().position(|candidate| candidate == selected));
    let next = match (current, forward) {
        (Some(index), true) => (index + 1) % matches.len(),
        (Some(index), false) => index.checked_sub(1).unwrap_or(matches.len() - 1),
        (None, _) => 0,
    };
    model.selected_row = Some(matches[next].clone());
}

fn selected_agent(model: &UiModel) -> Option<&UiAgent> {
    let selected = model.selected_row.as_deref()?;
    model
        .snapshot
        .as_ref()?
        .agents
        .iter()
        .find(|agent| agent_hex(agent.agent_id) == selected)
}

fn agent_hex(bytes: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(64);
    for byte in bytes {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    value
}

fn agent_matches(agent: &UiAgent, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let query = query.to_lowercase();
    agent_hex(agent.agent_id).contains(&query)
        || agent
            .names
            .iter()
            .any(|name| name.to_lowercase().contains(&query))
        || agent.sessions.iter().any(|session| {
            session.provider.to_lowercase().contains(&query)
                || session.session.to_lowercase().contains(&query)
                || session
                    .display_name
                    .as_ref()
                    .is_some_and(|name| name.to_lowercase().contains(&query))
        })
}

fn select_agent_search_match(model: &mut UiModel, forward: bool) {
    if model.section != UiSection::Agents || model.agent_search.is_empty() {
        return;
    }
    let Some(snapshot) = &model.snapshot else {
        return;
    };
    let matches = snapshot
        .agents
        .iter()
        .filter(|agent| agent_matches(agent, &model.agent_search))
        .map(|agent| agent_hex(agent.agent_id))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return;
    }
    let current = model
        .selected_row
        .as_ref()
        .and_then(|selected| matches.iter().position(|candidate| candidate == selected));
    let next = match (current, forward) {
        (Some(index), true) => (index + 1) % matches.len(),
        (Some(index), false) => index.checked_sub(1).unwrap_or(matches.len() - 1),
        (None, _) => 0,
    };
    model.selected_row = Some(matches[next].clone());
}

fn default_agent_session(agent: &UiAgent) -> Option<(String, String)> {
    agent
        .sessions
        .iter()
        .find(|session| session.selected)
        .or_else(|| agent.sessions.first())
        .map(|session| (session.provider.clone(), session.session.clone()))
}

fn move_agent_session(
    agent: &UiAgent,
    selected: Option<&(String, String)>,
    forward: bool,
) -> Option<(String, String)> {
    if agent.sessions.is_empty() {
        return None;
    }
    let current = selected.and_then(|(provider, session)| {
        agent
            .sessions
            .iter()
            .position(|candidate| candidate.provider == *provider && candidate.session == *session)
    });
    let next = match (current, forward) {
        (Some(index), true) => (index + 1).min(agent.sessions.len() - 1),
        (Some(index), false) => index.saturating_sub(1),
        (None, _) => 0,
    };
    let session = &agent.sessions[next];
    Some((session.provider.clone(), session.session.clone()))
}

fn refresh_agent_modal(model: &mut UiModel, snapshot: &UiSnapshot) {
    let identity = match &model.agent_modal {
        Some(
            UiAgentModal::Details { agent, .. }
            | UiAgentModal::ConfirmRetire { agent, .. }
            | UiAgentModal::ManagedProvider { agent, .. }
            | UiAgentModal::ConfirmManagedSession { agent, .. }
            | UiAgentModal::ManagingSession { agent, .. }
            | UiAgentModal::ManagedSessionOutcome { agent, .. },
        ) => Some(agent.agent_id),
        Some(UiAgentModal::RenameSession { agent_id, .. }) => Some(*agent_id),
        _ => None,
    };
    let Some(identity) = identity else { return };
    let current = snapshot
        .agents
        .iter()
        .find(|agent| agent.agent_id == identity)
        .cloned();
    match (&mut model.agent_modal, current) {
        (
            Some(UiAgentModal::Details {
                agent,
                selected_session,
            }),
            Some(current),
        ) => {
            *selected_session = selected_session
                .clone()
                .filter(|(provider, session)| {
                    current.sessions.iter().any(|candidate| {
                        candidate.provider == *provider && candidate.session == *session
                    })
                })
                .or_else(|| default_agent_session(&current));
            *agent = current;
        }
        (Some(UiAgentModal::ConfirmRetire { agent, .. }), Some(current)) => *agent = current,
        (
            Some(UiAgentModal::ManagedProvider {
                agent,
                providers,
                selected,
            }),
            Some(current),
        ) => {
            let stale = selected
                .as_deref()
                .is_some_and(|selected| !provider_is_available(&snapshot.providers, selected));
            if stale {
                model.last_failure = Some(UiFailure {
                    code: "provider_choice_stale".to_owned(),
                    action: "choose one of the agent services currently available".to_owned(),
                });
            }
            *selected = selected
                .clone()
                .filter(|selected| provider_is_available(&snapshot.providers, selected))
                .or_else(|| default_provider_choice(&snapshot.providers));
            providers.clone_from(&snapshot.providers);
            *agent = current;
        }
        (
            Some(
                UiAgentModal::ConfirmManagedSession { agent, .. }
                | UiAgentModal::ManagingSession { agent, .. }
                | UiAgentModal::ManagedSessionOutcome { agent, .. },
            ),
            Some(current),
        ) => {
            *agent = current;
        }
        (
            Some(UiAgentModal::RenameSession {
                provider, session, ..
            }),
            Some(current),
        ) if !current
            .sessions
            .iter()
            .any(|candidate| candidate.provider == *provider && candidate.session == *session) =>
        {
            model.last_failure = Some(UiFailure {
                code: "agent_session_stale".to_owned(),
                action: "cancel and reselect a current provider session".to_owned(),
            });
        }
        (_, None) => {
            model.last_failure = Some(UiFailure {
                code: "agent_target_stale".to_owned(),
                action: "cancel and reselect the agent from the authoritative catalog".to_owned(),
            });
        }
        _ => {}
    }
}

#[allow(clippy::too_many_lines)]
fn refresh_project_modal(model: &mut UiModel, snapshot: &UiSnapshot) {
    let identity = match &model.project_modal {
        Some(
            UiProjectModal::Details { project, .. }
            | UiProjectModal::SendInput { project, .. }
            | UiProjectModal::AddResource { project, .. }
            | UiProjectModal::ReplaceResource { project, .. }
            | UiProjectModal::ConfirmRemoveResource { project, .. }
            | UiProjectModal::ConfirmPrimaryResource { project, .. }
            | UiProjectModal::Activate { project, .. }
            | UiProjectModal::Handoff { project, .. }
            | UiProjectModal::ConfirmClose { project, .. }
            | UiProjectModal::ConfirmArchive { project, .. },
        ) => Some(project.project_id),
        _ => None,
    };
    let Some(identity) = identity else { return };
    let current = snapshot
        .projects
        .iter()
        .find(|project| project.project_id == identity)
        .cloned();
    match (&mut model.project_modal, current) {
        (
            Some(UiProjectModal::Details {
                project,
                selected_resource,
            }),
            Some(current),
        ) => {
            *selected_resource = selected_resource
                .filter(|selected| {
                    current
                        .resources
                        .iter()
                        .any(|resource| resource.resource_id == *selected)
                })
                .or_else(|| default_project_resource(&current));
            *project = current;
        }
        (
            Some(
                UiProjectModal::Activate {
                    project,
                    agents,
                    providers,
                    agent_id,
                    thread,
                    new_session,
                    provider,
                    ..
                }
                | UiProjectModal::Handoff {
                    project,
                    agents,
                    providers,
                    agent_id,
                    thread,
                    new_session,
                    provider,
                    ..
                },
            ),
            Some(current),
        ) => {
            let provider_stale =
                *new_session && !provider_is_available(&snapshot.providers, provider);
            if provider_stale && !provider.is_empty() {
                model.last_failure = Some(UiFailure {
                    code: "provider_choice_stale".to_owned(),
                    action: "choose one of the agent services currently available".to_owned(),
                });
            }
            providers.clone_from(&snapshot.providers);
            if provider_stale {
                *provider = default_provider_choice(providers).unwrap_or_default();
            }
            *agents = snapshot
                .agents
                .iter()
                .filter(|agent| {
                    agent.lifecycle == UiAgentLifecycle::Active
                        && agent
                            .mailboxes
                            .iter()
                            .any(|mailbox| mailbox.installation_id == current.home)
                })
                .cloned()
                .collect();
            if agent_id
                .is_some_and(|selected| !agents.iter().any(|agent| agent.agent_id == selected))
            {
                model.last_failure = Some(UiFailure {
                    code: "project_agent_target_stale".to_owned(),
                    action: "select a current local named agent before retrying".to_owned(),
                });
            }
            if thread.as_ref().is_some_and(|selected| {
                !current
                    .threads
                    .iter()
                    .any(|candidate| candidate == selected)
            }) {
                model.last_failure = Some(UiFailure {
                    code: "project_thread_target_stale".to_owned(),
                    action: "select a current exact project thread before retrying".to_owned(),
                });
            }
            *project = current;
        }
        (
            Some(
                UiProjectModal::SendInput { project, .. }
                | UiProjectModal::AddResource { project, .. }
                | UiProjectModal::ReplaceResource { project, .. }
                | UiProjectModal::ConfirmRemoveResource { project, .. }
                | UiProjectModal::ConfirmPrimaryResource { project, .. }
                | UiProjectModal::ConfirmClose { project, .. }
                | UiProjectModal::ConfirmArchive { project, .. },
            ),
            Some(current),
        ) => *project = current,
        (_, None) => {
            model.last_failure = Some(UiFailure {
                code: "project_target_stale".to_owned(),
                action: "cancel and reselect the project from the authoritative catalog".to_owned(),
            });
        }
        _ => {}
    }
}

#[allow(clippy::too_many_lines)]
fn mailbox_shortcut(
    model: &mut UiModel,
    character: char,
    effects: &mut Vec<UiEffect>,
) -> Result<bool, UiError> {
    if character == 'N' {
        model.open_draft(UiMailboxDraftTarget::SelfNote, effects)?;
        return Ok(true);
    }
    match character.to_ascii_lowercase() {
        'q' => {
            model.should_exit = true;
            effects.push(UiEffect::Exit);
            Ok(false)
        }
        'r' => {
            if model.section == UiSection::Agents {
                return Ok(false);
            }
            let Some(target) = selected_message_target(model).filter(|target| target.reply_allowed)
            else {
                return Ok(show_select_message_help(model));
            };
            model.open_draft(
                UiMailboxDraftTarget::Reply {
                    message_id: target.message_id,
                },
                effects,
            )?;
            Ok(true)
        }
        'd' => {
            open_direct_target_picker(model);
            Ok(true)
        }
        'n' => {
            model.new_modal = Some(UiNewModal::Launcher {
                selected: UiNewChoice::ProjectWork,
            });
            Ok(true)
        }
        'a' if model.section != UiSection::Agents => Ok(confirm_message_state(model, false)),
        'u' if model.section != UiSection::Agents => Ok(confirm_message_state(model, true)),
        '/' if model.section == UiSection::Agents => {
            model.agent_modal = Some(UiAgentModal::Search {
                query: model.agent_search.clone(),
            });
            Ok(true)
        }
        'c' if model.section == UiSection::Agents => {
            model.agent_modal = Some(UiAgentModal::Create {
                name: String::new(),
                submitting: false,
            });
            Ok(true)
        }
        '/' if model.section == UiSection::Projects => {
            model.project_modal = Some(UiProjectModal::Search {
                query: model.project_search.clone(),
            });
            Ok(true)
        }
        'c' if model.section == UiSection::Projects => {
            model.project_modal = Some(UiProjectModal::ChooseCreation {
                selected: UiProjectCreationChoice::ExistingFolder,
            });
            Ok(true)
        }
        'w' if model.section == UiSection::Projects => {
            open_worktree_project_form(model);
            Ok(true)
        }
        'h' => {
            if model.viewport.width < WIDE_WIDTH {
                model.change_section(model.section.previous());
                Ok(true)
            } else if model.viewport.width >= WIDE_WIDTH
                && matches!(model.focus, UiFocus::Content | UiFocus::Conversation)
            {
                model.focus = UiFocus::Navigation;
                Ok(true)
            } else {
                Ok(false)
            }
        }
        'l' => {
            if model.viewport.width < WIDE_WIDTH {
                model.change_section(model.section.next());
                Ok(true)
            } else if model.viewport.width >= WIDE_WIDTH && model.focus == UiFocus::Navigation {
                model.focus = UiFocus::Content;
                Ok(true)
            } else {
                Ok(false)
            }
        }
        'j' => Ok(match model.focus {
            UiFocus::Conversation => model.move_conversation_anchor(true),
            UiFocus::Navigation if model.viewport.width >= WIDE_WIDTH => {
                model.change_section(model.section.next());
                true
            }
            UiFocus::Navigation | UiFocus::Content => model.move_row_selection(true),
        }),
        'k' => Ok(match model.focus {
            UiFocus::Conversation => model.move_conversation_anchor(false),
            UiFocus::Navigation if model.viewport.width >= WIDE_WIDTH => {
                model.change_section(model.section.previous());
                true
            }
            UiFocus::Navigation | UiFocus::Content => model.move_row_selection(false),
        }),
        _ => Ok(false),
    }
}

fn selected_message_target(model: &UiModel) -> Option<UiMessageTarget> {
    let anchor = model.conversation_anchor.as_deref()?;
    model
        .conversation
        .as_ref()?
        .entries
        .iter()
        .find(|entry| entry.id == anchor)?
        .message_target
}

fn confirm_message_state(model: &mut UiModel, restore: bool) -> bool {
    let Some(target) = selected_message_target(model) else {
        return show_select_message_help(model);
    };
    let state = model
        .conversation
        .as_ref()
        .and_then(|conversation| {
            conversation
                .entries
                .iter()
                .find(|entry| entry.message_target == Some(target))
        })
        .and_then(|entry| entry.message_state);
    if (restore && state != Some(UiMessageState::Archived))
        || (!restore && state != Some(UiMessageState::Open))
    {
        return false;
    }
    model.mailbox_modal = Some(UiMailboxModal::Confirm {
        action: if restore {
            UiMailboxAction::Restore {
                target_message: target.message_id,
            }
        } else {
            UiMailboxAction::Archive {
                target_message: target.message_id,
            }
        },
    });
    true
}

fn show_select_message_help(model: &mut UiModel) -> bool {
    let help = if model.conversation.is_some() {
        Some(UiTransientHelp::SelectConversationMessage)
    } else if model.selected_row_is_conversation() {
        Some(UiTransientHelp::OpenConversationMessage)
    } else {
        None
    };
    model.transient_help = help;
    help.is_some()
}

fn draft_action(target: &UiMailboxDraftTarget) -> UiMailboxAction {
    match target {
        UiMailboxDraftTarget::Reply { message_id } => UiMailboxAction::Reply {
            target_message: *message_id,
        },
        UiMailboxDraftTarget::Direct {
            installation_id,
            mailbox_id,
        } => UiMailboxAction::Direct {
            recipient_installation: *installation_id,
            recipient_mailbox: *mailbox_id,
        },
        UiMailboxDraftTarget::SelfNote => UiMailboxAction::SelfNote,
    }
}

fn activate(model: &mut UiModel, effects: &mut Vec<UiEffect>) -> Result<bool, UiError> {
    if model.viewport.width >= WIDE_WIDTH && model.focus == UiFocus::Navigation {
        model.focus = UiFocus::Content;
        return Ok(true);
    }
    if model.focus == UiFocus::Conversation && model.conversation_anchor.is_some() {
        model.technical_visible = !model.technical_visible;
        return Ok(true);
    }
    if model.section == UiSection::Agents {
        let Some(agent) = selected_agent(model).cloned() else {
            return Ok(false);
        };
        let selected_session = default_agent_session(&agent);
        model.agent_modal = Some(UiAgentModal::Details {
            agent,
            selected_session,
        });
        return Ok(true);
    }
    if model.section == UiSection::Projects {
        let Some(project) = selected_project(model).cloned() else {
            return Ok(false);
        };
        let selected_resource = default_project_resource(&project);
        model.project_modal = Some(UiProjectModal::Details {
            project,
            selected_resource,
        });
        return Ok(true);
    }
    if !model.selected_row_is_conversation() {
        return Ok(false);
    }
    let row_id = model.selected_row.clone().unwrap_or_default();
    if model
        .conversation
        .as_ref()
        .is_some_and(|conversation| conversation.row_id == row_id)
    {
        model.focus = UiFocus::Conversation;
    } else {
        model.request_conversation(row_id, None, effects)?;
    }
    Ok(true)
}

fn load_more(model: &mut UiModel, effects: &mut Vec<UiEffect>) -> Result<bool, UiError> {
    let request = model.conversation.as_ref().and_then(|conversation| {
        conversation
            .next_cursor
            .clone()
            .map(|cursor| (conversation.row_id.clone(), cursor))
    });
    let Some((row_id, cursor)) = request else {
        return Ok(false);
    };
    model.request_conversation(row_id, Some(cursor), effects)?;
    Ok(true)
}

fn escape(model: &mut UiModel) -> bool {
    if model.technical_visible {
        model.technical_visible = false;
        true
    } else if model.conversation.is_some() {
        model.close_conversation();
        true
    } else {
        false
    }
}

fn timer_elapsed(
    model: &mut UiModel,
    effect_id: EffectId,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    if model.periodic_timer == Some(effect_id) {
        model.periodic_timer = None;
        model.schedule_timer(UiTimerKind::PeriodicRefresh, PERIODIC_REFRESH, effects)?;
        model.request_snapshot(effects)?;
        effects.push(UiEffect::RequestRedraw);
    } else if model.retry_timer == Some(effect_id) {
        model.retry_timer = None;
        model.connection = UiConnectionState::Connecting;
        model.request_snapshot(effects)?;
        effects.push(UiEffect::RequestRedraw);
    } else if model.autosave_timer == Some(effect_id) {
        model.autosave_timer = None;
        model.save_draft(effects)?;
    } else if model.completion_timer == Some(effect_id) {
        model.completion_timer = None;
        model.completion_notice = None;
        effects.push(UiEffect::RequestRedraw);
    }
    Ok(())
}

fn snapshot_loaded(
    model: &mut UiModel,
    effect_id: EffectId,
    snapshot: UiSnapshot,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    let Some(pending) = model
        .pending_snapshot
        .filter(|pending| pending.id == effect_id)
    else {
        return Ok(());
    };
    model.pending_snapshot = None;
    model.retry_timer = None;
    model.connection = UiConnectionState::Ready;
    model.last_failure = None;
    let current_revision = model.snapshot.as_ref().map_or(0, |value| value.revision);
    if snapshot.revision >= current_revision {
        model.apply_snapshot(snapshot);
        apply_guided_snapshot(model);
        apply_completion_context(model);
    }
    let observed_revision = model.snapshot.as_ref().map_or(0, |value| value.revision);
    let required_revision = model
        .required_revision
        .unwrap_or(pending.minimum_revision)
        .max(pending.minimum_revision);
    if observed_revision >= required_revision {
        model.required_revision = None;
    } else {
        model.required_revision = Some(required_revision);
        model.request_snapshot(effects)?;
    }
    if model.completion_context.is_some() && model.pending_snapshot.is_none() {
        let followup_requested = model
            .completion_context
            .as_ref()
            .is_some_and(|pending| pending.refresh == UiCompletionRefresh::Followup);
        if followup_requested {
            model.completion_context = None;
            model.last_failure = Some(UiFailure {
                code: "completion_target_stale".to_owned(),
                action: "reload and select the changed project or agent".to_owned(),
            });
        } else {
            if let Some(pending) = &mut model.completion_context {
                pending.refresh = UiCompletionRefresh::Followup;
            }
            model.request_snapshot(effects)?;
        }
    }
    if model.required_revision.is_none()
        && let Some(row_id) = model
            .conversation
            .as_ref()
            .map(|conversation| conversation.row_id.clone())
    {
        model.request_conversation(row_id, None, effects)?;
    }
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn conversation_loaded(
    model: &mut UiModel,
    effect_id: EffectId,
    page: UiConversationPage,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    let Some(pending) = model
        .pending_conversation
        .as_ref()
        .filter(|pending| pending.id == effect_id)
        .cloned()
    else {
        return Ok(());
    };
    if page.row_id != pending.row_id {
        return Err(UiError::ConversationRowMismatch);
    }
    model.pending_conversation = None;
    if model.selected_row.as_deref() != Some(page.row_id.as_str())
        || !model.selected_row_is_conversation()
    {
        return Ok(());
    }
    let previous_anchor = model.conversation_anchor.clone();
    if pending.cursor.is_some()
        && let Some(conversation) = &mut model.conversation
        && conversation.row_id == page.row_id
    {
        conversation.entries.extend(page.entries);
        conversation.next_cursor = page.next_cursor;
    } else {
        model.conversation = Some(UiConversation {
            row_id: page.row_id,
            entries: page.entries,
            next_cursor: page.next_cursor,
        });
    }
    model.conversation_anchor = model.conversation.as_ref().and_then(|conversation| {
        previous_anchor
            .filter(|anchor| conversation.entries.iter().any(|entry| &entry.id == anchor))
            .or_else(|| conversation.entries.first().map(|entry| entry.id.clone()))
    });
    model.focus = UiFocus::Conversation;
    model.last_failure = None;
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn conversation_failed(
    model: &mut UiModel,
    effect_id: EffectId,
    failure: UiFailure,
    effects: &mut Vec<UiEffect>,
) {
    if model
        .pending_conversation
        .as_ref()
        .map(|pending| pending.id)
        != Some(effect_id)
    {
        return;
    }
    model.pending_conversation = None;
    model.last_failure = Some(failure);
    effects.push(UiEffect::RequestRedraw);
}

fn draft_loaded(
    model: &mut UiModel,
    effect_id: EffectId,
    draft: UiMailboxDraft,
    effects: &mut Vec<UiEffect>,
) {
    if model.pending_mailbox
        != Some(PendingMailbox {
            id: effect_id,
            kind: PendingMailboxKind::OpenDraft,
        })
    {
        return;
    }
    let target_matches = matches!(
        &model.mailbox_modal,
        Some(UiMailboxModal::LoadingDraft { target }) if *target == draft.target
    );
    model.pending_mailbox = None;
    if !target_matches {
        return;
    }
    model.mailbox_modal = Some(UiMailboxModal::Compose {
        draft,
        dirty: false,
        submitting: false,
        closing: false,
    });
    model.last_failure = None;
    effects.push(UiEffect::RequestRedraw);
}

fn draft_saved(
    model: &mut UiModel,
    effect_id: EffectId,
    saved: &UiMailboxDraft,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    if model.pending_mailbox
        != Some(PendingMailbox {
            id: effect_id,
            kind: PendingMailboxKind::SaveDraft,
        })
    {
        return Ok(());
    }
    model.pending_mailbox = None;
    let Some(UiMailboxModal::Compose {
        draft,
        dirty: _,
        submitting,
        closing,
    }) = model.mailbox_modal.clone()
    else {
        return Ok(());
    };
    if draft.draft_id != saved.draft_id || draft.target != saved.target {
        return Ok(());
    }
    let content_is_saved = draft.content == saved.content;
    let current = UiMailboxDraft {
        version: saved.version,
        ..draft
    };
    model.mailbox_modal = Some(UiMailboxModal::Compose {
        draft: current.clone(),
        dirty: !content_is_saved,
        submitting,
        closing,
    });
    model.last_failure = None;
    if closing && content_is_saved {
        model.mailbox_modal = None;
        model.autosave_timer = None;
    } else if submitting && content_is_saved {
        model.submit_mailbox(
            Some(current.clone()),
            draft_action(&current.target),
            effects,
        )?;
    } else if !content_is_saved {
        if closing {
            model.save_draft(effects)?;
        } else {
            model.schedule_timer(UiTimerKind::AutosaveDraft, DRAFT_AUTOSAVE_DELAY, effects)?;
        }
    }
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn draft_failed(
    model: &mut UiModel,
    effect_id: EffectId,
    failure: UiFailure,
    current: Option<UiMailboxDraft>,
    effects: &mut Vec<UiEffect>,
) {
    let Some(pending) = model
        .pending_mailbox
        .filter(|pending| pending.id == effect_id)
    else {
        return;
    };
    if !matches!(
        pending.kind,
        PendingMailboxKind::OpenDraft | PendingMailboxKind::SaveDraft
    ) {
        return;
    }
    model.pending_mailbox = None;
    if let (
        PendingMailboxKind::SaveDraft,
        Some(UiMailboxModal::Compose {
            draft,
            dirty,
            submitting: _,
            closing,
        }),
        Some(server),
    ) = (pending.kind, &mut model.mailbox_modal, current)
        && draft.draft_id == server.draft_id
        && draft.target == server.target
    {
        draft.version = server.version;
        *dirty = true;
        *closing = false;
    }
    model.last_failure = Some(failure);
    effects.push(UiEffect::RequestRedraw);
}

fn mailbox_command_committed(
    model: &mut UiModel,
    effect_id: EffectId,
    revision: u64,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    if model.pending_mailbox
        != Some(PendingMailbox {
            id: effect_id,
            kind: PendingMailboxKind::SubmitCommand,
        })
    {
        return Ok(());
    }
    model.pending_mailbox = None;
    model.mailbox_modal = None;
    model.autosave_timer = None;
    model.last_failure = None;
    invalidated(model, revision, effects)?;
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn mailbox_command_failed(
    model: &mut UiModel,
    effect_id: EffectId,
    failure: UiFailure,
    effects: &mut Vec<UiEffect>,
) {
    if model.pending_mailbox
        != Some(PendingMailbox {
            id: effect_id,
            kind: PendingMailboxKind::SubmitCommand,
        })
    {
        return;
    }
    model.pending_mailbox = None;
    if let Some(UiMailboxModal::Compose {
        submitting,
        closing,
        ..
    }) = &mut model.mailbox_modal
    {
        *submitting = false;
        *closing = false;
    }
    model.last_failure = Some(failure);
    effects.push(UiEffect::RequestRedraw);
}

fn agent_command_committed(
    model: &mut UiModel,
    effect_id: EffectId,
    revision: u64,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    if model.pending_agent != Some(effect_id) {
        return Ok(());
    }
    model.pending_agent = None;
    model.agent_modal = None;
    model.last_failure = None;
    invalidated(model, revision, effects)?;
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn agent_command_failed(
    model: &mut UiModel,
    effect_id: EffectId,
    failure: UiFailure,
    effects: &mut Vec<UiEffect>,
) {
    if model.pending_agent != Some(effect_id) {
        return;
    }
    model.pending_agent = None;
    if let Some(
        UiAgentModal::Create { submitting, .. }
        | UiAgentModal::RenameSession { submitting, .. }
        | UiAgentModal::ConfirmRetire { submitting, .. },
    ) = &mut model.agent_modal
    {
        *submitting = false;
    }
    model.last_failure = Some(failure);
    effects.push(UiEffect::RequestRedraw);
}

fn managed_session_completed(
    model: &mut UiModel,
    effect_id: EffectId,
    result: UiManagedSessionResult,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    if model.pending_managed_session != Some(effect_id) {
        return Ok(());
    }
    let Some(UiAgentModal::ManagingSession { agent, action }) = model.agent_modal.clone() else {
        return Ok(());
    };
    if result.action != action {
        model.pending_managed_session = None;
        model.last_failure = Some(UiFailure {
            code: "managed_session_response_mismatch".to_owned(),
            action: "reload and reselect the exact managed-session target".to_owned(),
        });
        effects.push(UiEffect::RequestRedraw);
        return Ok(());
    }
    model.pending_managed_session = None;
    model.completion_notice = None;
    model.completion_timer = None;
    model.completion_context = None;
    match &result.outcome {
        UiManagedSessionOutcome::Rejected { code, .. } => {
            model.last_failure = Some(UiFailure {
                code: code.clone(),
                action: "reload durable sessions, then select an exact current target".to_owned(),
            });
            model.agent_modal = Some(UiAgentModal::ManagedSessionOutcome { agent, result });
        }
        UiManagedSessionOutcome::Uncertain { .. } => {
            model.last_failure = Some(UiFailure {
                code: "managed_session_uncertain".to_owned(),
                action: "keep this operation identity while HQ reconciles the same request"
                    .to_owned(),
            });
            model.agent_modal = Some(UiAgentModal::ManagedSessionOutcome { agent, result });
        }
        UiManagedSessionOutcome::Ready { session } => {
            let provider = managed_session_provider(&result.action).to_owned();
            model.last_failure = None;
            model.agent_modal = None;
            model.completion_context = Some(UiPendingCompletion {
                target: UiCompletionContext::Agent {
                    agent_id: agent.agent_id,
                    selected_session: Some((provider, session.clone())),
                },
                refresh: UiCompletionRefresh::Initial,
            });
            show_completion_notice(model, UiCompletionNotice::AgentReady, effects)?;
        }
        UiManagedSessionOutcome::Stopped => {
            let provider = managed_session_provider(&result.action);
            let selected_session = agent
                .sessions
                .iter()
                .find(|session| session.provider == provider && session.selected)
                .or_else(|| {
                    agent
                        .sessions
                        .iter()
                        .find(|session| session.provider == provider)
                })
                .map(|session| (session.provider.clone(), session.session.clone()));
            model.last_failure = None;
            model.agent_modal = None;
            model.completion_context = Some(UiPendingCompletion {
                target: UiCompletionContext::Agent {
                    agent_id: agent.agent_id,
                    selected_session,
                },
                refresh: UiCompletionRefresh::Initial,
            });
            show_completion_notice(model, UiCompletionNotice::AgentStopped, effects)?;
        }
    }
    model.request_snapshot(effects)?;
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn managed_session_provider(action: &UiManagedSessionAction) -> &str {
    match action {
        UiManagedSessionAction::Start { provider, .. }
        | UiManagedSessionAction::Resume { provider, .. }
        | UiManagedSessionAction::Stop { provider, .. } => provider,
    }
}

fn show_completion_notice(
    model: &mut UiModel,
    notice: UiCompletionNotice,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    model.completion_notice = Some(notice);
    model.completion_timer = None;
    model.schedule_timer(
        UiTimerKind::DismissCompletion,
        COMPLETION_NOTICE_DELAY,
        effects,
    )
}

fn apply_completion_context(model: &mut UiModel) {
    let Some(context) = model
        .completion_context
        .as_ref()
        .map(|pending| pending.target.clone())
    else {
        return;
    };
    match context {
        UiCompletionContext::Agent {
            agent_id,
            selected_session,
        } => {
            let Some(agent) = model.snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .agents
                    .iter()
                    .find(|agent| agent.agent_id == agent_id)
                    .cloned()
            }) else {
                return;
            };
            if let Some((provider, session)) = &selected_session
                && !agent.sessions.iter().any(|candidate| {
                    &candidate.provider == provider && &candidate.session == session
                })
            {
                return;
            }
            model.change_section(UiSection::Agents);
            model.selected_row = Some(agent_hex(agent_id));
            model.agent_modal = Some(UiAgentModal::Details {
                agent,
                selected_session,
            });
            model.project_modal = None;
            model.completion_context = None;
        }
        UiCompletionContext::Project {
            project_id,
            continuation,
        } => {
            let Some(project) = model.snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .projects
                    .iter()
                    .find(|project| project.project_id == project_id)
                    .cloned()
            }) else {
                return;
            };
            model.change_section(UiSection::Projects);
            model.selected_row = Some(agent_hex(project_id));
            model.agent_modal = None;
            model.project_modal = match continuation {
                UiProjectCompletionContinuation::Select => None,
                UiProjectCompletionContinuation::Details => Some(UiProjectModal::Details {
                    selected_resource: default_project_resource(&project),
                    project,
                }),
                UiProjectCompletionContinuation::ComposeInput => Some(UiProjectModal::SendInput {
                    project,
                    content: String::new(),
                    submitting: false,
                }),
            };
            model.completion_context = None;
        }
    }
}

fn apply_guided_snapshot(model: &mut UiModel) {
    let Some(pending) = model.guided_pending.clone() else {
        return;
    };
    match pending {
        UiGuidedPending::ProjectSnapshot { project_id } => {
            let Some(project) = model.snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .projects
                    .iter()
                    .find(|project| project.project_id == project_id)
                    .cloned()
            }) else {
                return;
            };
            model.guided_pending = None;
            open_guided_project(model, project);
        }
        UiGuidedPending::AgentCreation {
            project_id,
            expected_name: Some(expected_name),
        } => {
            let Some(snapshot) = model.snapshot.as_ref() else {
                return;
            };
            let Some(project) = snapshot
                .projects
                .iter()
                .find(|project| project.project_id == project_id)
                .cloned()
            else {
                return;
            };
            let matches = snapshot
                .agents
                .iter()
                .filter(|agent| {
                    agent.lifecycle == UiAgentLifecycle::Active
                        && agent.names.iter().any(|name| name == &expected_name)
                })
                .cloned()
                .collect::<Vec<_>>();
            let [agent] = matches.as_slice() else {
                return;
            };
            let agent = agent.clone();
            model.guided_pending = None;
            open_guided_agent(model, project, agent);
        }
        UiGuidedPending::Activation(submission) => {
            let Some(project) = model
                .snapshot
                .as_ref()
                .and_then(|snapshot| {
                    snapshot.projects.iter().find(|project| {
                        project.project_id == submission.project_id
                            && project.assignment.as_ref().is_some_and(|assignment| {
                                assignment.agent_id == submission.agent_id && assignment.runnable
                            })
                    })
                })
                .cloned()
            else {
                return;
            };
            open_project_message_composer(model, project);
        }
        UiGuidedPending::ProjectCreation
        | UiGuidedPending::AgentCreation {
            expected_name: None,
            ..
        } => {}
    }
}

fn refresh_new_modal(model: &mut UiModel, snapshot: &UiSnapshot) {
    model.new_modal = match model.new_modal.clone() {
        Some(UiNewModal::ChooseProject {
            selected,
            create_new,
            ..
        }) => {
            let projects = snapshot
                .projects
                .iter()
                .filter(|project| !project.archived)
                .cloned()
                .collect::<Vec<_>>();
            let selected = selected.filter(|selected| {
                projects
                    .iter()
                    .any(|project| project.project_id == *selected)
            });
            Some(UiNewModal::ChooseProject {
                create_new: create_new || projects.is_empty(),
                selected: selected.or_else(|| projects.first().map(|project| project.project_id)),
                projects,
            })
        }
        Some(UiNewModal::ChooseAgent {
            project,
            selected,
            create_new,
            ..
        }) => {
            let project = snapshot
                .projects
                .iter()
                .find(|candidate| candidate.project_id == project.project_id)
                .cloned()
                .unwrap_or(project);
            let picker = guided_agent_picker_from_snapshot(Some(snapshot), project);
            let UiNewModal::ChooseAgent {
                project,
                agents,
                selected: fallback,
                ..
            } = picker
            else {
                return;
            };
            let selected = selected
                .filter(|selected| agents.iter().any(|agent| agent.agent_id == *selected))
                .or(fallback);
            Some(UiNewModal::ChooseAgent {
                project,
                create_new: create_new || agents.is_empty(),
                agents,
                selected,
            })
        }
        Some(UiNewModal::ChooseProvider {
            project,
            agent,
            provider,
            ..
        }) => {
            let project = snapshot
                .projects
                .iter()
                .find(|candidate| candidate.project_id == project.project_id)
                .cloned()
                .unwrap_or(project);
            let agent = snapshot
                .agents
                .iter()
                .find(|candidate| candidate.agent_id == agent.agent_id)
                .cloned()
                .unwrap_or(agent);
            let historical = guided_thread(&project, agent.agent_id).is_some();
            let provider = if historical || provider_is_available(&snapshot.providers, &provider) {
                provider
            } else {
                default_provider_choice(&snapshot.providers).unwrap_or_default()
            };
            Some(UiNewModal::ChooseProvider {
                project,
                agent,
                provider,
                providers: snapshot.providers.clone(),
            })
        }
        other => other,
    };
}

fn managed_session_failed(
    model: &mut UiModel,
    effect_id: EffectId,
    failure: UiFailure,
    effects: &mut Vec<UiEffect>,
) {
    if model.pending_managed_session != Some(effect_id) {
        return;
    }
    model.pending_managed_session = None;
    if let Some(UiAgentModal::ManagingSession { agent, action }) = model.agent_modal.clone() {
        model.agent_modal = Some(UiAgentModal::ConfirmManagedSession { agent, action });
    }
    model.last_failure = Some(failure);
    effects.push(UiEffect::RequestRedraw);
}

fn project_command_completed(
    model: &mut UiModel,
    effect_id: EffectId,
    result: UiProjectResult,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    let Some(pending) = model
        .pending_project
        .clone()
        .filter(|pending| pending.id == effect_id)
    else {
        return Ok(());
    };
    if pending.action != result.action {
        model.pending_project = None;
        stop_guided_activation(model);
        model.last_failure = Some(UiFailure {
            code: "project_response_mismatch".to_owned(),
            action: "reload and reselect the exact project operation target".to_owned(),
        });
        effects.push(UiEffect::RequestRedraw);
        return Ok(());
    }
    model.pending_project = None;
    if guided_project_completed(model, &result, effects)? {
        model.request_snapshot(effects)?;
        effects.push(UiEffect::RequestRedraw);
        return Ok(());
    }
    model.completion_notice = None;
    model.completion_timer = None;
    model.completion_context = None;
    match &result.outcome {
        UiProjectOutcome::Rejected { code, .. } => {
            model.last_failure = Some(UiFailure {
                code: code.clone(),
                action: "reload and reselect current project state before retrying".to_owned(),
            });
            model.project_modal = Some(UiProjectModal::Outcome { result });
        }
        UiProjectOutcome::Reconcilable { code, .. } => {
            model.last_failure = Some(UiFailure {
                code: code.clone(),
                action: "inspect retained external state and reconcile this operation".to_owned(),
            });
            model.project_modal = Some(UiProjectModal::Outcome { result });
        }
        UiProjectOutcome::Running { .. }
        | UiProjectOutcome::ResourcePreview { .. }
        | UiProjectOutcome::ResourceChecks { .. } => {
            model.last_failure = None;
            model.project_modal = Some(UiProjectModal::Outcome { result });
        }
        UiProjectOutcome::Completed { .. } | UiProjectOutcome::InputSent { .. } => {
            let (notice, continuation) = project_completion_policy(&result);
            model.last_failure = None;
            model.project_modal = None;
            model.completion_context = Some(UiPendingCompletion {
                target: UiCompletionContext::Project {
                    project_id: result.project_id,
                    continuation,
                },
                refresh: UiCompletionRefresh::Initial,
            });
            show_completion_notice(model, notice, effects)?;
        }
    }
    model.request_snapshot(effects)?;
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn guided_project_completed(
    model: &mut UiModel,
    result: &UiProjectResult,
    effects: &mut Vec<UiEffect>,
) -> Result<bool, UiError> {
    match (&model.guided_pending, &result.action, &result.outcome) {
        (
            Some(UiGuidedPending::ProjectCreation),
            UiProjectAction::CreateExisting { .. } | UiProjectAction::CreateWorktree { .. },
            UiProjectOutcome::Completed { .. },
        ) => {
            model.guided_pending = Some(UiGuidedPending::ProjectSnapshot {
                project_id: result.project_id,
            });
            model.project_modal = None;
            model.new_modal = Some(UiNewModal::Working {
                project: "New project".to_owned(),
                agent: "Not chosen yet".to_owned(),
                stage: "Loading the new project…".to_owned(),
            });
            model.last_failure = None;
            show_completion_notice(model, UiCompletionNotice::ProjectCreated, effects)?;
            Ok(true)
        }
        (
            Some(UiGuidedPending::Activation(_)),
            UiProjectAction::Activate { .. } | UiProjectAction::Handoff { .. },
            UiProjectOutcome::Completed { .. },
        ) => {
            model.project_modal = None;
            model.last_failure = None;
            Ok(true)
        }
        (
            Some(UiGuidedPending::Activation(_)),
            UiProjectAction::Activate { .. } | UiProjectAction::Handoff { .. },
            UiProjectOutcome::Rejected { code, .. },
        ) => {
            model.guided_pending = None;
            model.new_modal = None;
            model.last_failure = Some(UiFailure {
                code: code.clone(),
                action: "reload the project state before choosing another action".to_owned(),
            });
            model.project_modal = Some(UiProjectModal::Outcome {
                result: result.clone(),
            });
            Ok(true)
        }
        (
            Some(UiGuidedPending::Activation(_)),
            UiProjectAction::Activate { .. } | UiProjectAction::Handoff { .. },
            UiProjectOutcome::Reconcilable { code, .. },
        ) => {
            model.guided_pending = None;
            model.new_modal = None;
            model.last_failure = Some(UiFailure {
                code: code.clone(),
                action: "inspect recovery evidence before choosing another action".to_owned(),
            });
            model.project_modal = Some(UiProjectModal::Outcome {
                result: result.clone(),
            });
            Ok(true)
        }
        (
            Some(UiGuidedPending::Activation(_)),
            UiProjectAction::Activate { .. } | UiProjectAction::Handoff { .. },
            UiProjectOutcome::Running { .. },
        ) => {
            model.new_modal = None;
            model.last_failure = None;
            model.project_modal = Some(UiProjectModal::Outcome {
                result: result.clone(),
            });
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn project_completion_policy(
    result: &UiProjectResult,
) -> (UiCompletionNotice, UiProjectCompletionContinuation) {
    if matches!(result.outcome, UiProjectOutcome::InputSent { .. }) {
        return (
            UiCompletionNotice::InstructionsSent,
            UiProjectCompletionContinuation::Details,
        );
    }
    match result.action {
        UiProjectAction::CreateExisting { .. } | UiProjectAction::CreateWorktree { .. } => (
            UiCompletionNotice::ProjectCreated,
            UiProjectCompletionContinuation::Select,
        ),
        UiProjectAction::Activate { .. } | UiProjectAction::Handoff { .. } => (
            UiCompletionNotice::ProjectWorkReady,
            UiProjectCompletionContinuation::ComposeInput,
        ),
        _ => (
            UiCompletionNotice::ProjectUpdated,
            UiProjectCompletionContinuation::Details,
        ),
    }
}

fn project_command_failed(
    model: &mut UiModel,
    effect_id: EffectId,
    failure: UiFailure,
    effects: &mut Vec<UiEffect>,
) {
    if model.pending_project.as_ref().map(|pending| pending.id) != Some(effect_id) {
        return;
    }
    model.pending_project = None;
    stop_guided_activation(model);
    if let Some(
        UiProjectModal::CreateExisting { submitting, .. }
        | UiProjectModal::CreateWorktree { submitting, .. }
        | UiProjectModal::SendInput { submitting, .. }
        | UiProjectModal::AddResource { submitting, .. }
        | UiProjectModal::ReplaceResource { submitting, .. }
        | UiProjectModal::ConfirmRemoveResource { submitting, .. }
        | UiProjectModal::ConfirmPrimaryResource { submitting, .. }
        | UiProjectModal::Activate { submitting, .. }
        | UiProjectModal::Handoff { submitting, .. }
        | UiProjectModal::ConfirmClose { submitting, .. }
        | UiProjectModal::ConfirmArchive { submitting, .. },
    ) = &mut model.project_modal
    {
        *submitting = false;
    }
    model.last_failure = Some(failure);
    effects.push(UiEffect::RequestRedraw);
}

fn snapshot_failed(
    model: &mut UiModel,
    effect_id: EffectId,
    failure: UiFailure,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    if model.pending_snapshot.map(|pending| pending.id) != Some(effect_id) {
        return Ok(());
    }
    model.pending_snapshot = None;
    model.connection = UiConnectionState::Reconnecting;
    model.last_failure = Some(failure);
    if model.retry_timer.is_none() {
        model.schedule_timer(UiTimerKind::RetrySnapshot, RETRY_DELAY, effects)?;
    }
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn invalidated(
    model: &mut UiModel,
    revision: u64,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    let current = model
        .snapshot
        .as_ref()
        .map_or(0, |snapshot| snapshot.revision);
    let required = model.required_revision.unwrap_or(current);
    if revision <= required {
        return Ok(());
    }
    model.required_revision = Some(revision);
    model.pending_conversation = None;
    if let Some(pending) = &mut model.pending_snapshot {
        pending.minimum_revision = pending.minimum_revision.max(revision);
    } else {
        model.request_snapshot(effects)?;
    }
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn connection_observed(
    model: &mut UiModel,
    generation: u64,
    state: UiConnectionState,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    if generation < model.connection_generation
        || (generation == model.connection_generation && state == model.connection)
    {
        return Ok(());
    }
    let became_ready =
        state == UiConnectionState::Ready && model.connection != UiConnectionState::Ready;
    model.connection_generation = generation;
    model.connection = state;
    if became_ready {
        model.request_snapshot(effects)?;
    }
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn client_failed(
    model: &mut UiModel,
    generation: u64,
    failure: UiFailure,
    effects: &mut Vec<UiEffect>,
) {
    if generation < model.connection_generation {
        return;
    }
    model.connection_generation = generation;
    model.connection = UiConnectionState::Reconnecting;
    model.pending_conversation = None;
    model.last_failure = Some(failure);
    effects.push(UiEffect::RequestRedraw);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::num::NonZeroU64;

    use super::{
        TextEdit, UiEffect, UiError, UiEvent, UiFormField, UiFormKind, UiFormState, UiHumanState,
        UiInput, UiModel, UiProject, UiProjectAction, UiProjectModal, UiProjectResourceCheck,
        UiSize, UiSnapshot, apply_project_modal_input, edit_text, normalize_path_input,
        refresh_project_modal, update,
    };

    #[test]
    fn effect_identity_exhaustion_is_explicit() {
        let mut model = UiModel::new(UiSize {
            width: 80,
            height: 24,
        });
        model.next_effect_id = NonZeroU64::new(u64::MAX);
        let error = update(model, UiEvent::Started).expect_err("second allocation exhausts");
        assert_eq!(error, UiError::EffectIdentityExhausted);
    }

    #[test]
    fn repeated_start_is_rejected() {
        let started = update(
            UiModel::new(UiSize {
                width: 80,
                height: 24,
            }),
            UiEvent::Started,
        )
        .expect("first start");
        assert!(matches!(started.effects[0], UiEffect::LoadSnapshot { .. }));
        assert_eq!(
            update(started.model, UiEvent::Started),
            Err(UiError::AlreadyStarted)
        );
    }

    #[test]
    fn clean_close_requires_confirmation_but_not_force() {
        let project = project("release");
        let mut model = UiModel::new(UiSize {
            width: 80,
            height: 24,
        });
        model.project_modal = Some(UiProjectModal::ConfirmClose {
            project,
            checks: vec![release_check("accepted", Some("clean"))],
            confirmed: false,
            force: false,
            submitting: false,
        });
        let mut effects = Vec::new();
        assert!(
            apply_project_modal_input(&mut model, UiInput::Character('c'), &mut effects)
                .expect("confirmation toggles")
        );
        assert!(
            apply_project_modal_input(&mut model, UiInput::Activate, &mut effects)
                .expect("clean close submits")
        );
        assert!(effects.iter().any(|effect| matches!(
            effect,
            UiEffect::SubmitProjectCommand {
                action: UiProjectAction::Close {
                    project_id,
                    force: false
                },
                ..
            } if *project_id == [1; 32]
        )));
    }

    #[test]
    fn authoritative_refresh_retains_close_evidence_and_user_authorization() {
        let mut model = UiModel::new(UiSize {
            width: 80,
            height: 24,
        });
        model.project_modal = Some(UiProjectModal::ConfirmClose {
            project: project("old name"),
            checks: vec![release_check("uncertain", None)],
            confirmed: true,
            force: true,
            submitting: false,
        });
        refresh_project_modal(
            &mut model,
            &UiSnapshot {
                revision: 2,
                human_state: UiHumanState::Ready,
                inbox_rows: Vec::new(),
                sent_rows: Vec::new(),
                archived_rows: Vec::new(),
                agent_rows: Vec::new(),
                project_rows: Vec::new(),
                direct_targets: Vec::new(),
                providers: Vec::new(),
                agents: Vec::new(),
                projects: vec![project("new name")],
            },
        );
        let retained = model.project_modal.expect("close modal retained");
        assert!(matches!(retained, UiProjectModal::ConfirmClose { .. }));
        if let UiProjectModal::ConfirmClose {
            project,
            checks,
            confirmed,
            force,
            ..
        } = retained
        {
            assert_eq!(project.name, "new name");
            assert_eq!(checks, vec![release_check("uncertain", None)]);
            assert!(confirmed);
            assert!(force);
        }
    }

    #[test]
    fn path_input_expands_only_the_current_user_shorthand_and_normalizes_lexically() {
        assert_eq!(
            normalize_path_input("~/src/./hq/../project", Some("/Users/example")),
            Ok("/Users/example/src/project".to_owned())
        );
        assert_eq!(
            normalize_path_input("~someone/project", Some("/Users/example")),
            Err("Use an absolute path, ~, or ~/…")
        );
        assert_eq!(
            normalize_path_input("$HOME/project", Some("/Users/example")),
            Err("Use an absolute path, ~, or ~/…")
        );
    }

    #[test]
    fn reusable_editor_rejects_an_oversized_paste_atomically() {
        let mut form = UiFormState {
            active: Some(UiFormKind::AgentCreate),
            ..UiFormState::default()
        };
        let mut value = "é".to_owned();
        assert!(edit_text(
            &mut form,
            UiFormField::AgentName,
            &mut value,
            TextEdit::Insert("abc"),
            4,
        ));
        assert_eq!(value, "é");
        assert!(form.errors.contains_key(&UiFormField::AgentName));
    }

    fn project(name: &str) -> UiProject {
        UiProject {
            project_id: [1; 32],
            home: [2; 32],
            name: name.to_owned(),
            lifecycle: "open".to_owned(),
            archived: false,
            claimable: true,
            assignment: None,
            threads: Vec::new(),
            head: [3; 32],
            input_sequence: 0,
            resources: Vec::new(),
        }
    }

    fn release_check(status: &str, release: Option<&str>) -> UiProjectResourceCheck {
        UiProjectResourceCheck {
            resource_id: [4; 32],
            status: status.to_owned(),
            health: Some("healthy".to_owned()),
            release: release.map(str::to_owned),
            observed_canonical_path: Some("/workspace/release".to_owned()),
            details: None,
            error_category: None,
            error_code: None,
            reconciliation_id: None,
        }
    }
}
