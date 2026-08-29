//! Pure identity-aware TUI transition algebra.

use std::{num::NonZeroU64, time::Duration};

const PERIODIC_REFRESH: Duration = Duration::from_secs(300);
const RETRY_DELAY: Duration = Duration::from_millis(250);
const DRAFT_AUTOSAVE_DELAY: Duration = Duration::from_millis(250);
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiHumanState {
    /// No uniquely selected active human account is currently available.
    Unavailable,
    /// One uniquely selected active human account is available.
    Ready,
    /// Local selection or authority history is present but ambiguous.
    Ambiguous,
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

/// Shell-normalized terminal input understood by the pure model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiInput {
    /// Exit the UI.
    Quit,
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

/// Current project catalog, creation, input, or outcome interaction.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiProjectModal {
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
        provider: String,
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

/// Closed timer purpose owned by the shell effect executor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiTimerKind {
    /// Periodic full-snapshot repair.
    PeriodicRefresh,
    /// Bounded retry after a failed snapshot request.
    RetrySnapshot,
    /// Debounced local draft autosave.
    AutosaveDraft,
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
    agent_search: String,
    project_search: String,
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
    next_effect_id: Option<NonZeroU64>,
    last_failure: Option<UiFailure>,
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
            agent_search: String::new(),
            project_search: String::new(),
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
            next_effect_id: NonZeroU64::new(1),
            last_failure: None,
            started: false,
            should_exit: false,
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
    pub fn human_state(&self) -> Option<UiHumanState> {
        self.snapshot.as_ref().map(|snapshot| snapshot.human_state)
    }

    /// Reports whether a fresh authoritative snapshot is loading behind retained content.
    pub const fn refreshing(&self) -> bool {
        self.snapshot.is_some() && self.pending_snapshot.is_some()
    }

    /// Returns the selected stable row identity.
    pub fn selected_row(&self) -> Option<&str> {
        self.selected_row.as_deref()
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
        UiEvent::Input(value) => apply_input(&mut model, value, &mut effects)?,
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

fn apply_input(
    model: &mut UiModel,
    input: UiInput,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    if model.project_modal.is_some() {
        let changed = apply_project_modal_input(model, input, effects)?;
        if changed {
            effects.push(UiEffect::RequestRedraw);
        }
        return Ok(());
    }
    if model.agent_modal.is_some() {
        let changed = apply_agent_modal_input(model, input, effects)?;
        if changed {
            effects.push(UiEffect::RequestRedraw);
        }
        return Ok(());
    }
    if model.mailbox_modal.is_some() {
        let changed = apply_modal_input(model, input, effects)?;
        if changed {
            effects.push(UiEffect::RequestRedraw);
        }
        return Ok(());
    }
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
        UiInput::NextSection => match (model.viewport.width >= WIDE_WIDTH, model.focus) {
            (true, UiFocus::Navigation) => {
                model.focus = UiFocus::Content;
                true
            }
            (false, _) => {
                model.change_section(model.section.next());
                true
            }
            _ => false,
        },
        UiInput::PreviousSection => match (model.viewport.width >= WIDE_WIDTH, model.focus) {
            (true, UiFocus::Content | UiFocus::Conversation) => {
                model.focus = UiFocus::Navigation;
                true
            }
            (false, _) => {
                model.change_section(model.section.previous());
                true
            }
            _ => false,
        },
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
        UiInput::Character(character) => mailbox_shortcut(model, character, effects)?,
        UiInput::Paste(_) | UiInput::Backspace => false,
    };
    if changed {
        effects.push(UiEffect::RequestRedraw);
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
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
            UiInput::Character(character) if !submitting && !closing => {
                let mut encoded = [0_u8; 4];
                let value = character.encode_utf8(&mut encoded);
                if draft.content.len().saturating_add(value.len()) > MAX_DRAFT_BYTES {
                    model.last_failure = Some(UiFailure {
                        code: "draft_content_too_large".to_owned(),
                        action: "shorten the draft before submitting".to_owned(),
                    });
                    return Ok(true);
                }
                draft.content.push(character);
                update_composer(model, draft, true, false, effects)?;
                Ok(true)
            }
            UiInput::Paste(value) if !submitting && !closing => {
                let available = MAX_DRAFT_BYTES.saturating_sub(draft.content.len());
                if value.len() > available {
                    model.last_failure = Some(UiFailure {
                        code: "draft_content_too_large".to_owned(),
                        action: "shorten the pasted text before submitting".to_owned(),
                    });
                    return Ok(true);
                }
                draft.content.push_str(&value);
                update_composer(model, draft, true, false, effects)?;
                Ok(true)
            }
            UiInput::Backspace if !submitting && !closing => {
                if draft.content.pop().is_none() {
                    return Ok(false);
                }
                update_composer(model, draft, true, false, effects)?;
                Ok(true)
            }
            UiInput::Activate if !submitting && !closing => {
                if draft.content.is_empty() {
                    model.last_failure = Some(UiFailure {
                        code: "draft_content_empty".to_owned(),
                        action: "enter message text before submitting".to_owned(),
                    });
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

#[allow(clippy::too_many_lines)]
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
            if let Some(UiProjectModal::Search { query }) = &model.project_modal {
                model.project_search.clone_from(query);
            }
            model.project_modal = None;
            return Ok(true);
        }
        return Ok(false);
    }

    match model.project_modal.clone() {
        Some(UiProjectModal::Search { mut query }) => match input {
            UiInput::Character(value) => {
                push_project_text(&mut query, &value.to_string());
                update_project_search(model, query);
                Ok(true)
            }
            UiInput::Paste(value) => {
                push_project_text(&mut query, &value);
                update_project_search(model, query);
                Ok(true)
            }
            UiInput::Backspace => {
                if query.pop().is_none() {
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
            UiInput::Character(value @ ('k' | 'K')) => {
                let resource_id = (value == 'k').then_some(selected_resource).flatten();
                if value == 'k' && resource_id.is_none() {
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
                UiInput::NextItem | UiInput::PreviousItem => {
                    if let Some(UiProjectModal::AddResource { make_primary, .. }) =
                        &mut model.project_modal
                    {
                        *make_primary = !*make_primary;
                    } else {
                        cycle_project_field(model, matches!(input, UiInput::NextItem));
                    }
                    Ok(true)
                }
                UiInput::Character(value) => {
                    let mut encoded = [0_u8; 4];
                    Ok(edit_project_field(
                        model,
                        Some(value.encode_utf8(&mut encoded)),
                        false,
                    ))
                }
                UiInput::Paste(value) => Ok(edit_project_field(model, Some(&value), false)),
                UiInput::Backspace => Ok(edit_project_field(model, None, true)),
                UiInput::Activate => submit_project_modal(model, effects),
                _ => Ok(false),
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
            UiInput::Character('c') if !submitting => {
                confirmed = !confirmed;
                model.project_modal = Some(UiProjectModal::ConfirmClose {
                    project,
                    checks,
                    confirmed,
                    force,
                    submitting: false,
                });
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
                model.last_failure = None;
                Ok(true)
            }
            UiInput::Activate if !submitting => {
                if !confirmed {
                    model.last_failure = Some(UiFailure {
                        code: "project_close_confirmation_required".to_owned(),
                        action: "toggle confirmation before closing the project".to_owned(),
                    });
                    return Ok(true);
                }
                let force_required = checks.iter().any(|check| {
                    check.status != "accepted"
                        || !matches!(check.release.as_deref(), Some("clean" | "not_applicable"))
                });
                if force_required && !force {
                    model.last_failure = Some(UiFailure {
                        code: "project_close_force_required".to_owned(),
                        action: "review dirty or unknown release evidence and explicitly authorize force".to_owned(),
                    });
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
                    adjust_activation_selection(model);
                    Ok(true)
                }
                UiInput::Character(value) => {
                    let mut encoded = [0_u8; 4];
                    Ok(edit_project_field(
                        model,
                        Some(value.encode_utf8(&mut encoded)),
                        false,
                    ))
                }
                UiInput::Paste(value) => Ok(edit_project_field(model, Some(&value), false)),
                UiInput::Backspace => Ok(edit_project_field(model, None, true)),
                UiInput::Activate => submit_project_modal(model, effects),
                _ => Ok(false),
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

fn open_project_activation(model: &mut UiModel, project: UiProject, handoff: bool) {
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
    let provider = thread
        .as_ref()
        .map(|thread| thread.provider.clone())
        .or_else(|| {
            agent_id.and_then(|agent_id| {
                agents
                    .iter()
                    .find(|agent| agent.agent_id == agent_id)
                    .and_then(|agent| agent.sessions.first())
                    .map(|session| session.provider.clone())
            })
        })
        .unwrap_or_default();
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
    let Some(UiProjectModal::Activate { field, .. } | UiProjectModal::Handoff { field, .. }) =
        &mut model.project_modal
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
    let fields = if is_handoff {
        handoff.as_slice()
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

fn adjust_activation_selection(model: &mut UiModel) {
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
    model.last_failure = None;
}

fn cycle_activation_agent(model: &mut UiModel) {
    let Some(
        UiProjectModal::Activate {
            project,
            agents,
            agent_id,
            thread,
            provider,
            ..
        }
        | UiProjectModal::Handoff {
            project,
            agents,
            agent_id,
            thread,
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
    if let Some(selected_thread) = thread {
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
            agent_id,
            thread,
            provider,
            ..
        }
        | UiProjectModal::Handoff {
            new_session,
            project,
            agent_id,
            thread,
            provider,
            ..
        },
    ) = &mut model.project_modal
    {
        *new_session = !*new_session;
        if !*new_session && thread.is_none() {
            *thread = agent_id.and_then(|id| {
                project
                    .threads
                    .iter()
                    .find(|candidate| candidate.agent_id == id)
                    .cloned()
            });
        }
        if let Some(selected) = thread {
            provider.clone_from(&selected.provider);
        }
        model.last_failure = None;
    }
}

fn push_project_text(target: &mut String, value: &str) {
    if target.len().saturating_add(value.len()) <= MAX_PROJECT_TEXT_BYTES {
        target.push_str(value);
    }
}

fn edit_project_field(model: &mut UiModel, value: Option<&str>, backspace: bool) -> bool {
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
                provider,
                directory,
                field,
                ..
            }
            | UiProjectModal::Handoff {
                provider,
                directory,
                field,
                ..
            },
        ) => match field {
            UiProjectFormField::Provider => provider,
            UiProjectFormField::Directory => directory,
            _ => return false,
        },
        _ => return false,
    };
    let changed = if backspace {
        target.pop().is_some()
    } else if let Some(value) = value {
        let before = target.len();
        push_project_text(target, value);
        target.len() != before
    } else {
        false
    };
    if changed {
        model.last_failure = None;
    }
    changed
}

fn cycle_project_field(model: &mut UiModel, forward: bool) {
    let (fields, selected) = match &model.project_modal {
        Some(UiProjectModal::CreateExisting { field, .. }) => (
            &[
                UiProjectFormField::Name,
                UiProjectFormField::Brief,
                UiProjectFormField::Path,
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

#[allow(clippy::too_many_lines)]
fn submit_project_modal(model: &mut UiModel, effects: &mut Vec<UiEffect>) -> Result<bool, UiError> {
    let action = match model.project_modal.clone() {
        Some(UiProjectModal::CreateExisting {
            name, brief, path, ..
        }) if !name.is_empty() && !path.is_empty() => UiProjectAction::CreateExisting {
            name,
            brief: (!brief.is_empty()).then_some(brief),
            path,
        },
        Some(UiProjectModal::CreateWorktree {
            name,
            brief,
            source,
            destination,
            branch,
            base,
            ..
        }) if !name.is_empty()
            && !source.is_empty()
            && !destination.is_empty()
            && !branch.is_empty() =>
        {
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
        }) if !content.is_empty() => UiProjectAction::SendInput {
            project_id: project.project_id,
            content,
        },
        Some(UiProjectModal::AddResource {
            project,
            path,
            make_primary,
            ..
        }) if !path.is_empty() => UiProjectAction::PreviewAddResource {
            project_id: project.project_id,
            path,
            make_primary,
        },
        Some(UiProjectModal::ReplaceResource {
            project,
            resource_id,
            path,
            ..
        }) if !path.is_empty() => UiProjectAction::PreviewReplaceResource {
            project_id: project.project_id,
            resource_id,
            path,
        },
        Some(UiProjectModal::Activate {
            project,
            agent_id: Some(agent_id),
            thread,
            new_session,
            provider,
            directory,
            ..
        }) if !provider.is_empty()
            && !directory.is_empty()
            && (new_session
                || thread
                    .as_ref()
                    .is_some_and(|selected| selected.provider == provider)) =>
        {
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
            agent_id: Some(agent_id),
            thread: Some(thread),
            new_session,
            provider,
            directory,
            confirmed: true,
            force_takeover,
            ..
        }) if !provider.is_empty()
            && !directory.is_empty()
            && (new_session || thread.provider == provider) =>
        {
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
        Some(UiProjectModal::CreateExisting { .. }) => {
            model.last_failure = Some(UiFailure {
                code: "project_create_fields_empty".to_owned(),
                action: "enter a project name and existing working-tree path".to_owned(),
            });
            return Ok(true);
        }
        Some(UiProjectModal::CreateWorktree { .. }) => {
            model.last_failure = Some(UiFailure {
                code: "project_worktree_fields_empty".to_owned(),
                action: "enter name, source, destination, and branch".to_owned(),
            });
            return Ok(true);
        }
        Some(UiProjectModal::SendInput { .. }) => {
            model.last_failure = Some(UiFailure {
                code: "project_input_empty".to_owned(),
                action: "enter project input before submitting".to_owned(),
            });
            return Ok(true);
        }
        Some(UiProjectModal::AddResource { .. } | UiProjectModal::ReplaceResource { .. }) => {
            model.last_failure = Some(UiFailure {
                code: "project_resource_path_empty".to_owned(),
                action: "enter an absolute existing resource path".to_owned(),
            });
            return Ok(true);
        }
        Some(UiProjectModal::Activate { .. }) => {
            model.last_failure = Some(UiFailure {
                code: "project_activation_target_incomplete".to_owned(),
                action:
                    "select an agent and exact thread for resume, then enter provider and directory"
                        .to_owned(),
            });
            return Ok(true);
        }
        Some(UiProjectModal::Handoff { confirmed, .. }) => {
            model.last_failure = Some(UiFailure {
                code: if confirmed {
                    "project_handoff_target_incomplete"
                } else {
                    "project_handoff_confirmation_required"
                }
                .to_owned(),
                action: "select an exact target and separately confirm the handoff".to_owned(),
            });
            return Ok(true);
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

#[allow(clippy::too_many_lines)]
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
            model.agent_modal = None;
            return Ok(true);
        }
        return Ok(false);
    }
    match model.agent_modal.clone() {
        Some(UiAgentModal::Search { mut query }) => match input {
            UiInput::Character(value) => {
                push_bounded(&mut query, &value.to_string());
                update_agent_search(model, query);
                Ok(true)
            }
            UiInput::Paste(value) => {
                push_bounded(&mut query, &value);
                update_agent_search(model, query);
                Ok(true)
            }
            UiInput::Backspace => {
                if query.pop().is_none() {
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
                let provider = selected_session
                    .as_ref()
                    .map(|(provider, _)| provider.clone())
                    .unwrap_or_default();
                model.agent_modal = Some(UiAgentModal::ManagedProvider { agent, provider });
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
            UiInput::Character(value) if !submitting => {
                push_bounded(&mut name, &value.to_string());
                model.agent_modal = Some(UiAgentModal::Create {
                    name,
                    submitting: false,
                });
                model.last_failure = None;
                Ok(true)
            }
            UiInput::Paste(value) if !submitting => {
                push_bounded(&mut name, &value);
                model.agent_modal = Some(UiAgentModal::Create {
                    name,
                    submitting: false,
                });
                model.last_failure = None;
                Ok(true)
            }
            UiInput::Backspace if !submitting => {
                if name.pop().is_none() {
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
                    model.last_failure = Some(UiFailure {
                        code: "agent_name_empty".to_owned(),
                        action: "enter a permanent lowercase agent name".to_owned(),
                    });
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
            UiInput::Character(value) if !submitting => {
                push_bounded(&mut display_name, &value.to_string());
                model.agent_modal = Some(UiAgentModal::RenameSession {
                    agent_id,
                    provider,
                    session,
                    display_name,
                    submitting: false,
                });
                model.last_failure = None;
                Ok(true)
            }
            UiInput::Paste(value) if !submitting => {
                push_bounded(&mut display_name, &value);
                model.agent_modal = Some(UiAgentModal::RenameSession {
                    agent_id,
                    provider,
                    session,
                    display_name,
                    submitting: false,
                });
                model.last_failure = None;
                Ok(true)
            }
            UiInput::Backspace if !submitting => {
                if display_name.pop().is_none() {
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
            mut provider,
        }) => match input {
            UiInput::Character(value) => {
                push_bounded(&mut provider, &value.to_string());
                model.agent_modal = Some(UiAgentModal::ManagedProvider { agent, provider });
                model.last_failure = None;
                Ok(true)
            }
            UiInput::Paste(value) => {
                push_bounded(&mut provider, &value);
                model.agent_modal = Some(UiAgentModal::ManagedProvider { agent, provider });
                model.last_failure = None;
                Ok(true)
            }
            UiInput::Backspace => {
                if provider.pop().is_none() {
                    return Ok(false);
                }
                model.agent_modal = Some(UiAgentModal::ManagedProvider { agent, provider });
                Ok(true)
            }
            UiInput::Activate => {
                if provider.is_empty() {
                    model.last_failure = Some(UiFailure {
                        code: "managed_session_provider_empty".to_owned(),
                        action: "enter an exact provider namespace".to_owned(),
                    });
                    return Ok(true);
                }
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

fn push_bounded(target: &mut String, value: &str) {
    if target.len().saturating_add(value.len()) <= MAX_AGENT_TEXT_BYTES {
        target.push_str(value);
    }
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
            Some(
                UiAgentModal::ManagedProvider { agent, .. }
                | UiAgentModal::ConfirmManagedSession { agent, .. }
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
                    agent_id,
                    thread,
                    ..
                }
                | UiProjectModal::Handoff {
                    project,
                    agents,
                    agent_id,
                    thread,
                    ..
                },
            ),
            Some(current),
        ) => {
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
                return Ok(false);
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
            if model.section == UiSection::Agents {
                return Ok(false);
            }
            let targets = model
                .snapshot
                .as_ref()
                .map_or_else(Vec::new, |snapshot| snapshot.direct_targets.clone());
            let selected = targets
                .first()
                .map(|target| (target.installation_id, target.mailbox_id));
            model.mailbox_modal = Some(UiMailboxModal::SelectDirect { targets, selected });
            Ok(true)
        }
        'n' => {
            if model.section == UiSection::Agents {
                return Ok(false);
            }
            model.open_draft(UiMailboxDraftTarget::SelfNote, effects)?;
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
            model.project_modal = Some(UiProjectModal::CreateExisting {
                name: String::new(),
                brief: String::new(),
                path: String::new(),
                field: UiProjectFormField::Name,
                submitting: false,
            });
            Ok(true)
        }
        'w' if model.section == UiSection::Projects => {
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
        return false;
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
    model.last_failure = match &result.outcome {
        UiManagedSessionOutcome::Rejected { code, .. } => Some(UiFailure {
            code: code.clone(),
            action: "reload durable sessions, then select an exact current target".to_owned(),
        }),
        UiManagedSessionOutcome::Uncertain { .. } => Some(UiFailure {
            code: "managed_session_uncertain".to_owned(),
            action: "keep this operation identity while HQ reconciles the same request".to_owned(),
        }),
        UiManagedSessionOutcome::Ready { .. } | UiManagedSessionOutcome::Stopped => None,
    };
    model.agent_modal = Some(UiAgentModal::ManagedSessionOutcome { agent, result });
    model.request_snapshot(effects)?;
    effects.push(UiEffect::RequestRedraw);
    Ok(())
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
        model.last_failure = Some(UiFailure {
            code: "project_response_mismatch".to_owned(),
            action: "reload and reselect the exact project operation target".to_owned(),
        });
        effects.push(UiEffect::RequestRedraw);
        return Ok(());
    }
    model.pending_project = None;
    model.last_failure = match &result.outcome {
        UiProjectOutcome::Rejected { code, .. } => Some(UiFailure {
            code: code.clone(),
            action: "reload and reselect current project state before retrying".to_owned(),
        }),
        UiProjectOutcome::Reconcilable { code, .. } => Some(UiFailure {
            code: code.clone(),
            action: "inspect retained external state and reconcile this operation".to_owned(),
        }),
        UiProjectOutcome::Completed { .. }
        | UiProjectOutcome::Running { .. }
        | UiProjectOutcome::InputSent { .. }
        | UiProjectOutcome::ResourcePreview { .. }
        | UiProjectOutcome::ResourceChecks { .. } => None,
    };
    model.project_modal = Some(UiProjectModal::Outcome { result });
    model.request_snapshot(effects)?;
    effects.push(UiEffect::RequestRedraw);
    Ok(())
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
        UiEffect, UiError, UiEvent, UiHumanState, UiInput, UiModel, UiProject, UiProjectAction,
        UiProjectModal, UiProjectResourceCheck, UiSize, UiSnapshot, apply_project_modal_input,
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
