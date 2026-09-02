//! Pure identity-aware TUI transition algebra.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    num::NonZeroU64,
    path::{Component, Path, PathBuf},
    time::Duration,
};

const RETRY_DELAY: Duration = Duration::from_millis(250);
const DRAFT_AUTOSAVE_DELAY: Duration = Duration::from_millis(250);
const COMPLETION_NOTICE_DELAY: Duration = Duration::from_secs(4);
const MAX_DRAFT_BYTES: usize = 16 * 1024;
const MAX_AGENT_TEXT_BYTES: usize = 256;
const MAX_PROJECT_TEXT_BYTES: usize = 16 * 1024;
const MAX_RETAINED_CONVERSATION_PAGES: usize = 8;
pub(crate) const WIDE_WIDTH: u16 = 96;

/// Stable identity attached to an asynchronous UI effect and its completion.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EffectId(NonZeroU64);

impl EffectId {
    /// Returns the stable pure-model effect identity.
    pub const fn value(self) -> u64 {
        self.0.get()
    }
}

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
    /// Modeless message draft inside the Inbox workspace.
    Draft,
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
    /// Insert one line break in a multiline editor.
    InsertNewline,
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

/// Stable visual-row position within one conversation entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiConversationViewportPosition {
    /// Stable conversation entry identity.
    pub entry_id: String,
    /// Zero-based measured visual row within the entry.
    pub row: u16,
}

/// Exact measured height for one conversation entry in the current transcript layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiConversationEntryGeometry {
    /// Stable conversation entry identity.
    pub entry_id: String,
    /// Positive measured visual rows, including presentation spacing.
    pub height: u16,
}

/// Passive width-specific transcript geometry observed by the terminal renderer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiConversationViewportObservation {
    /// Stable selected conversation summary identity.
    pub conversation_id: String,
    /// Transcript columns used to measure every entry.
    pub width: u16,
    /// Rows available to paint transcript entries.
    pub height: u16,
    /// Ordered measured entries currently eligible for transcript presentation.
    pub entries: Vec<UiConversationEntryGeometry>,
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

/// Typed conversation destination retained with an Inbox summary row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiConversationTarget {
    /// One independently initiated project exchange.
    Project {
        /// Stable project receiving new input.
        project_id: [u8; 32],
        /// Exact existing exchange.
        thread_id: [u8; 32],
        /// Stable initiating message used to recover a newly created row.
        root_message: [u8; 32],
    },
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
    /// Exact destination for conversation-aware composition, when available.
    pub conversation_target: Option<UiConversationTarget>,
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

/// User-facing progress of one locally authored message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiMessageDelivery {
    /// The local submission is awaiting a durable receipt, or committed project work is queued.
    Pending,
    /// The message is durably authored but has no receipt evidence yet.
    Sent,
    /// Canonical evidence proves that the remote peer received the message.
    Received,
}

/// One human-facing message author resolved from exact conversation context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiConversationAuthor {
    /// The reserved local human mailbox.
    You,
    /// The singular resolved or honest fallback counterparty.
    Participant(String),
    /// Sender evidence did not match either proven participant.
    Unknown,
}

/// Closed activity family retained independently from display prose.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiConversationActivityKind {
    /// Generic operation status.
    Status,
    /// Provider-neutral agent-turn lifecycle.
    AgentTurn,
    /// Incremental progress.
    Progress,
    /// Plan or task state.
    Plan,
    /// Proposed-change snapshot.
    Diff,
    /// Durable completed command, file, or tool item.
    CompletedItem,
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

/// One terminal-safe changed-file presentation record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiCompletedFileChange {
    /// Provider-reported changed path.
    pub path: String,
    /// Optional diff retained for technical detail.
    pub diff: Option<String>,
    /// Whether the path was shortened.
    pub path_truncated: bool,
    /// Whether the diff was shortened.
    pub diff_truncated: bool,
}

/// Closed completed-item presentation independent from flattened activity detail.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiCompletedItemPresentation {
    /// Completed command execution.
    Command {
        /// Terminal-safe multiline command source.
        command: String,
        /// Optional terminal-safe multiline output.
        output: Option<String>,
        /// Provider-reported exit code.
        exit_code: Option<i64>,
        /// Whether command source was shortened.
        command_truncated: bool,
        /// Whether output was shortened.
        output_truncated: bool,
    },
    /// Completed file changes.
    FileChange {
        /// Bounded per-file presentation records.
        changes: Vec<UiCompletedFileChange>,
        /// Whether additional records were omitted.
        changes_truncated: bool,
    },
    /// Completed tool call.
    Tool {
        /// Retained server/tool or tool-family name.
        name: String,
        /// Whether the name was shortened.
        name_truncated: bool,
    },
    /// Completed web search.
    WebSearch {
        /// Retained query.
        query: String,
        /// Whether the query was shortened.
        query_truncated: bool,
    },
    /// Explicit unknown completed family.
    Unknown,
}

/// Closed ordinary presentation for one conversation entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiConversationEntryPresentation {
    /// One participant-authored message.
    Message {
        /// Typed author independent from the visible label.
        author: UiConversationAuthor,
        /// Bounded sanitized message body.
        body: String,
    },
    /// One non-actionable activity record.
    Activity {
        /// Closed activity family.
        kind: UiConversationActivityKind,
        /// Typed lifecycle state.
        status: UiActivityStatus,
        /// Short ordinary-language transcript line.
        summary: String,
        /// Exact bounded activity content for technical inspection.
        detail: String,
        /// Whether the provider boundary explicitly shortened the detail.
        truncated: bool,
        /// Structured completed-item presentation when available.
        completed: Option<UiCompletedItemPresentation>,
    },
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
        /// Exact source installation identity.
        source_installation: String,
        /// Exact source mailbox identity.
        source_mailbox: String,
        /// Exact provider namespace.
        provider: String,
        /// Exact provider-scoped session identity.
        session: String,
        /// Exact operation identity.
        operation: String,
        /// Optional provider item identity.
        item: Option<String>,
        /// Stable coalescing/history key.
        logical_key: String,
        /// Bounded runtime identity.
        runtime: String,
        /// Signed occurrence time in Unix milliseconds.
        occurred_at_unix_ms: i64,
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
    /// Typed ordinary presentation independent from action authority.
    pub presentation: UiConversationEntryPresentation,
    /// Typed message state; absent for non-actionable activity.
    pub message_state: Option<UiMessageState>,
    /// Delivery progress for a locally authored message; absent for incoming messages and activity.
    pub delivery: Option<UiMessageDelivery>,
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

/// Closed provider-neutral interaction class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiInteractionKind {
    /// Ask for text or one offered choice.
    Question,
    /// Approve command execution.
    CommandApproval,
    /// Approve file changes.
    FileApproval,
    /// Grant a permission scope.
    Permission,
    /// Resolve an MCP URL request.
    McpUrl,
    /// Supply an MCP form response.
    McpForm,
}

/// One untouched stable choice with a human-facing label.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiInteractionChoice {
    /// Stable value returned to the provider adapter.
    pub value: String,
    /// Human-facing label.
    pub label: String,
}

/// One pending provider interaction and its technical correlation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiInteraction {
    /// Named agent identity.
    pub agent_id: [u8; 32],
    /// Resolved ordinary agent name.
    pub agent_name: String,
    /// Stable project identity when this blocks project work.
    pub project_id: Option<[u8; 32]>,
    /// Resolved ordinary project name when this blocks project work.
    pub project_name: Option<String>,
    /// Neutral provider namespace.
    pub provider: String,
    /// Exact provider session.
    pub session: String,
    /// Provider-originated request identity.
    pub request_id: [u8; 32],
    /// Blocked operation identity.
    pub operation_id: [u8; 32],
    /// Typed request family.
    pub kind: UiInteractionKind,
    /// Exact bounded prompt.
    pub prompt: String,
    /// Source-ordered stable choices.
    pub choices: Vec<UiInteractionChoice>,
    /// Whether bounded free text is accepted.
    pub allow_text: bool,
}

/// Closed typed terminal response emitted by the pure model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiInteractionResponse {
    /// Bounded free text or structured form content.
    Text(String),
    /// Untouched stable offered value.
    Choice(String),
    /// Explicit approval or denial.
    Approval(bool),
    /// Explicit cancellation.
    Cancelled,
}

/// Terminal answer result returned by the local API.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiInteractionAnswerOutcome {
    /// The response reached the provider-session owner.
    Answered,
    /// Another responder or lifecycle transition already ended the request.
    Stale,
}

/// Current actionable provider interaction dialog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiInteractionModal {
    /// Awaiting a human choice, text response, or cancellation.
    Prompt {
        /// Complete request.
        interaction: UiInteraction,
        /// Selected offered choice.
        selected: usize,
        /// Bounded free-text draft.
        text: String,
    },
    /// One exact response command is in flight.
    Submitting {
        /// Complete request retained for recovery.
        interaction: UiInteraction,
    },
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

/// One accepted project instruction that has not yet been dispatched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiPendingProjectInput {
    /// Stable public message identity returned by project-input submission.
    pub message_id: [u8; 32],
    /// Immutable causal thread containing the instruction.
    pub thread_id: [u8; 32],
    /// Home-assigned contiguous input sequence.
    pub sequence: u64,
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
    /// Accepted project instructions that have not yet been dispatched.
    pub pending_inputs: Vec<UiPendingProjectInput>,
    /// Exact project head used for optimistic commands.
    pub head: [u8; 32],
    /// Next durable input sequence.
    pub input_sequence: u64,
    /// Desired resources in stable order.
    pub resources: Vec<UiProjectResource>,
}

/// Ordinary lifecycle shown by the Projects workspace without exposing reducer vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiProjectLifecycle {
    /// The project can own folders and accept new work.
    Open,
    /// A close workflow is still making progress or needs recovery.
    Closing,
    /// The project is visible but not accepting new work.
    Closed,
    /// The closed project is hidden from ordinary active work.
    Archived,
    /// Authoritative state could not be classified safely.
    NeedsAttention,
}

/// Plain ownership state for one project folder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiProjectFolderOwnership {
    /// This project currently owns the folder in HQ.
    Owned,
    /// Another project overlaps this folder.
    Conflicted,
    /// Ownership evidence is incomplete.
    NeedsAttention,
}

/// Decoupled ordinary presentation for one project folder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiProjectFolderSummary {
    /// Stable identity used for focus and exact commands.
    pub folder_id: [u8; 32],
    /// Familiar user-facing path.
    pub path: String,
    /// Whether this is the assignment's default working folder.
    pub working_folder: bool,
    /// Bounded authoritative health label.
    pub health: String,
    /// Typed ownership state.
    pub ownership: UiProjectFolderOwnership,
    /// Resolved conflicting project names, with honest unnamed fallbacks.
    pub conflicting_projects: Vec<String>,
}

/// Ordinary status of the agent responsible for one project.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiProjectAssignedAgentStatus {
    /// No agent is responsible yet.
    Unassigned,
    /// HQ is preparing the assigned agent.
    SettingUp,
    /// The assigned agent can receive project messages.
    Ready,
    /// Assignment state needs explicit attention.
    NeedsAttention,
}

/// Decoupled assigned-agent card presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiProjectAssignedAgentSummary {
    /// Exact assigned agent identity when one exists.
    pub agent_id: Option<[u8; 32]>,
    /// Resolved authoritative name; absent means `Unnamed agent`.
    pub name: Option<String>,
    /// Typed ordinary status.
    pub status: UiProjectAssignedAgentStatus,
    /// Working folder acknowledged by the assignment.
    pub working_folder: Option<String>,
}

/// Counts and exact row identities for project conversations owned by Inbox.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiProjectConversationSummary {
    /// Nonarchived project conversations available in Inbox.
    pub open: usize,
    /// Archived project conversations retained for history.
    pub archived: usize,
    /// Exact nonarchived Inbox row identities in authoritative order.
    pub open_rows: Vec<String>,
    /// Exact archived Inbox row identities in authoritative order.
    pub archived_rows: Vec<String>,
}

/// Typed reason the selected project needs an exceptional recovery surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiProjectRecoverySummary {
    /// One or more folders have conflicting or incomplete ownership.
    FolderOwnership,
    /// Assignment setup is explicitly blocked.
    AssignedAgentBlocked,
    /// More than one assignment candidate exists.
    AssignedAgentConflict,
}

/// Exact evidence kept out of the ordinary project summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiProjectTechnicalEvidence {
    /// Stable project identity.
    pub project_id: [u8; 32],
    /// Home installation identity.
    pub home: [u8; 32],
    /// Current authoritative project version.
    pub head: [u8; 32],
    /// Next accepted-input sequence.
    pub input_sequence: u64,
    /// Exact assignment epoch when assigned.
    pub assignment_id: Option<[u8; 32]>,
    /// Provider namespace when assigned.
    pub provider: Option<String>,
    /// Provider session when acknowledged.
    pub session: Option<String>,
    /// Immutable project thread when runnable.
    pub thread_id: Option<[u8; 32]>,
}

/// Selection-driven Projects workspace summary assembled from authoritative typed identities.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiProjectSummary {
    /// Stable selected project identity.
    pub project_id: [u8; 32],
    /// Current authoritative project name.
    pub name: String,
    /// Typed lifecycle presentation.
    pub lifecycle: UiProjectLifecycle,
    /// Inbox-owned conversation relationship.
    pub conversations: UiProjectConversationSummary,
    /// Assigned-agent relationship.
    pub assigned_agent: UiProjectAssignedAgentSummary,
    /// Folder cards in stable project order, working folder first.
    pub folders: Vec<UiProjectFolderSummary>,
    /// Exceptional recovery banners, absent during ordinary waiting.
    pub recovery: Vec<UiProjectRecoverySummary>,
    /// Exact progressively disclosed evidence.
    pub technical: UiProjectTechnicalEvidence,
}

/// Modeless navigation depth inside Projects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiProjectWorkspaceLevel {
    /// Select a project from the catalog.
    List,
    /// Inspect and collaborate from the selected project's summary.
    Summary,
    /// Choose one labeled administrative action.
    Manage,
    /// Inspect folders and choose one exact folder action.
    Folders,
}

/// Labeled state-dependent action selected in Manage project.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiProjectManagementAction {
    /// Open folder administration.
    Folders,
    /// Assign an agent to an unassigned project.
    AssignAgent,
    /// Change the agent responsible for an assigned project.
    ChangeAssignedAgent,
    /// Close an open project after fresh release assessment.
    CloseProject,
    /// Reopen one visible closed project.
    ReopenProject,
    /// Archive a visible project, closing it first when required.
    ArchiveProject,
    /// Restore an archived project as closed and visible.
    RestoreArchivedProject,
    /// Inspect exact IDs, bindings, and causal evidence.
    TechnicalDetails,
}

/// Labeled action selected in project folder administration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiProjectFolderAction {
    /// Add another folder to the project.
    AddFolder,
    /// Change the selected folder's path.
    ChangeFolderPath,
    /// Remove the selected folder from HQ while keeping disk state.
    RemoveFolder,
    /// Make the selected folder the default working folder.
    UseAsWorkingFolder,
    /// Refresh the selected folder's health and release evidence.
    CheckFolderNow,
    /// Refresh every project folder when more than one exists.
    CheckAllFolders,
}

/// Stable card focus within the selected project summary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiProjectSummaryFocus {
    /// Inbox-owned project conversations.
    Conversation,
    /// Named agent responsible for the project.
    AssignedAgent,
    /// Project folders and working-folder status.
    Folders,
    /// Exceptional recovery evidence.
    Recovery,
    /// Labeled project administration.
    Manage,
}

impl UiProjectSummaryFocus {
    const ALL: [Self; 5] = [
        Self::Conversation,
        Self::AssignedAgent,
        Self::Folders,
        Self::Recovery,
        Self::Manage,
    ];
}

/// Visible typed Inbox filter installed by Projects navigation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiProjectInboxFilter {
    /// Stable project identity; labels never become routing authority.
    pub project_id: [u8; 32],
    /// Current display-only project name.
    pub project_name: String,
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
pub enum UiProjectInteraction {
    ChooseCreation {
        selected: UiProjectCreationChoice,
    },
    Search {
        query: String,
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
    /// Send to a stable project, optionally continuing an exact exchange.
    Project {
        project_id: [u8; 32],
        thread_id: Option<[u8; 32]>,
    },
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

/// Definite mailbox-command result retained for exact post-refresh navigation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiMailboxCommandResult {
    /// Durable transaction revision.
    pub revision: u64,
    /// Public identity of the committed message, absent for state-only commands.
    pub message_id: Option<[u8; 32]>,
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
    /// Send to a stable project, optionally continuing an exact exchange.
    Project {
        project_id: [u8; 32],
        thread_id: Option<[u8; 32]>,
    },
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
    /// Confirm a reversible canonical state command.
    Confirm { action: UiMailboxAction },
}

/// Modeless durable drafting pane owned by the Inbox workspace.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiMailboxDraftPane {
    /// An applicable draft is being loaded or created.
    Loading { target: UiMailboxDraftTarget },
    /// Edit one durable draft in place.
    Editing {
        /// Latest local draft state, including unsaved content.
        draft: UiMailboxDraft,
        /// Whether text differs from the last acknowledged version.
        dirty: bool,
        /// Whether submit is waiting for the latest autosave.
        submitting: bool,
        /// Whether cancellation is waiting for the latest autosave.
        closing: bool,
    },
}

/// Passive bounded page returned by the ordinary local API client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiConversationPage {
    /// Stable summary-row identity requested by the model.
    pub row_id: String,
    /// Participant-oriented conversation heading.
    pub title: String,
    /// Optional project or other useful conversation context.
    pub context: Option<String>,
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
    /// Participant-oriented conversation heading.
    pub title: String,
    /// Optional project or other useful conversation context.
    pub context: Option<String>,
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

/// One snapshot and optional selected first page observed at the same authoritative revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiMaterializedConversationView {
    /// Complete passive snapshot for the observed revision.
    pub snapshot: UiSnapshot,
    /// Selected first page from the same revision, when the Inbox has an active interest.
    pub conversation: Option<UiConversationPage>,
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
    ProjectWorkReady,
}

impl UiCompletionNotice {
    const fn text(self) -> &'static str {
        match self {
            Self::AgentReady => "Agent conversation ready",
            Self::AgentStopped => "Agent stopped; saved conversation kept",
            Self::ProjectCreated => "Project created",
            Self::ProjectUpdated => "Project updated",
            Self::ProjectWorkReady => "Project work is ready",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UiProjectCompletionContinuation {
    Select,
    Summary,
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
    Instruction(UiGuidedSubmission),
    InputSnapshot {
        submission: UiGuidedSubmission,
        message_id: [u8; 32],
    },
    Activation(UiGuidedSubmission),
}

/// Closed timer purpose owned by the shell effect executor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiTimerKind {
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
    /// Passive width-specific transcript geometry observed during a terminal draw.
    ConversationViewportObserved {
        /// Exact current conversation and entry measurements.
        observation: UiConversationViewportObservation,
    },
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
    /// One subscribed snapshot and selected first page became current together.
    MaterializedViewObserved {
        /// Coherent passive view from one serialized daemon revision.
        view: UiMaterializedConversationView,
    },
    /// The complete bounded pending-interaction queue changed.
    InteractionsObserved {
        /// Source-owner ordered pending requests.
        interactions: Vec<UiInteraction>,
    },
    /// One exact interaction response reached a terminal outcome.
    InteractionAnswered {
        /// Identity of the completed response effect.
        effect_id: EffectId,
        /// Typed terminal outcome.
        outcome: UiInteractionAnswerOutcome,
    },
    /// One interaction response could not reach a terminal outcome.
    InteractionAnswerFailed {
        /// Identity of the failed response effect.
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
        /// Public committed message identity, when this command authored one.
        message_id: Option<[u8; 32]>,
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
    /// Replace the subscribed observation owner's latest selected Inbox conversation.
    ObserveConversation {
        /// Stable summary-row identity, or no selected detail when outside the Inbox.
        row_id: Option<String>,
    },
    /// Execute or reconcile one exact terminal provider-interaction response.
    AnswerInteraction {
        /// Identity required on the completion event.
        id: EffectId,
        /// Complete request correlation.
        interaction: UiInteraction,
        /// Typed terminal human response.
        response: UiInteractionResponse,
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
    enter_on_load: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConversationFailure {
    row_id: String,
    cursor: Option<String>,
    failure: UiFailure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PendingMailboxKind {
    OpenDraft,
    SaveDraft,
    SubmitCommand(Box<PendingMailboxSubmission>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingMailboxSubmission {
    draft: Option<UiMailboxDraft>,
    action: UiMailboxAction,
    optimistic_entry: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingMailbox {
    id: EffectId,
    kind: PendingMailboxKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UiSectionWorkspace {
    selected_row: Option<String>,
    conversation: Option<UiConversation>,
    conversation_anchor: Option<String>,
    conversation_scroll_mode: ConversationScrollMode,
    conversation_viewport_position: Option<UiConversationViewportPosition>,
    technical_visible: bool,
    technical_scroll: u16,
    focus: UiFocus,
    conversation_failure: Option<ConversationFailure>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingProject {
    id: EffectId,
    action: UiProjectAction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RetainedConversationPage {
    revision: u64,
    page: UiConversationPage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UiObservationMode {
    SnapshotFallback,
    Materialized,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConversationScrollMode {
    Anchored,
    FollowTail,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConversationViewportGeometry {
    height: u16,
    entries: Vec<UiConversationEntryGeometry>,
}

impl ConversationViewportGeometry {
    fn total_height(&self) -> u64 {
        self.entries
            .iter()
            .map(|entry| u64::from(entry.height))
            .sum()
    }

    fn maximum_top(&self) -> u64 {
        self.total_height().saturating_sub(u64::from(self.height))
    }

    fn entry_start(&self, entry_id: &str) -> Option<(u64, u16)> {
        let mut start = 0_u64;
        for entry in &self.entries {
            if entry.entry_id == entry_id {
                return Some((start, entry.height));
            }
            start = start.saturating_add(u64::from(entry.height));
        }
        None
    }

    fn offset_for(&self, position: &UiConversationViewportPosition) -> Option<u64> {
        let (start, height) = self.entry_start(&position.entry_id)?;
        Some(
            start
                .saturating_add(u64::from(position.row.min(height.saturating_sub(1))))
                .min(self.maximum_top()),
        )
    }

    fn position_at(&self, offset: u64) -> Option<UiConversationViewportPosition> {
        let offset = offset.min(self.maximum_top());
        let mut start = 0_u64;
        for entry in &self.entries {
            let end = start.saturating_add(u64::from(entry.height));
            if offset < end {
                return Some(UiConversationViewportPosition {
                    entry_id: entry.entry_id.clone(),
                    row: u16::try_from(offset.saturating_sub(start)).unwrap_or(u16::MAX),
                });
            }
            start = end;
        }
        None
    }
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
    retained_conversations: BTreeMap<String, RetainedConversationPage>,
    retained_conversation_order: VecDeque<String>,
    desired_conversation: Option<String>,
    requested_conversation: Option<String>,
    observation_mode: UiObservationMode,
    conversation_anchor: Option<String>,
    conversation_scroll_mode: ConversationScrollMode,
    conversation_viewport_position: Option<UiConversationViewportPosition>,
    conversation_viewport_geometry: Option<ConversationViewportGeometry>,
    technical_visible: bool,
    technical_scroll: u16,
    mailbox_modal: Option<UiMailboxModal>,
    mailbox_draft: Option<UiMailboxDraftPane>,
    agent_modal: Option<UiAgentModal>,
    interactions: VecDeque<UiInteraction>,
    interaction_modal: Option<UiInteractionModal>,
    pending_interaction: Option<EffectId>,
    project_interaction: Option<UiProjectInteraction>,
    project_summary: Option<UiProjectSummary>,
    project_workspace_level: UiProjectWorkspaceLevel,
    project_summary_focus: UiProjectSummaryFocus,
    project_management_action: Option<UiProjectManagementAction>,
    project_folder_action: UiProjectFolderAction,
    project_folder_id: Option<[u8; 32]>,
    project_filter: Option<UiProjectInboxFilter>,
    project_filter_rows: Vec<UiRow>,
    new_modal: Option<UiNewModal>,
    agent_search: String,
    project_search: String,
    home_directory: Option<String>,
    form: UiFormState,
    help_page: Option<UiHelpPage>,
    required_revision: Option<u64>,
    pending_snapshot: Option<PendingSnapshot>,
    pending_conversation: Option<PendingConversation>,
    conversation_failure: Option<ConversationFailure>,
    pending_mailbox: Option<PendingMailbox>,
    pending_agent: Option<EffectId>,
    pending_managed_session: Option<EffectId>,
    pending_project: Option<PendingProject>,
    pending_project_conversation: Option<([u8; 32], [u8; 32])>,
    section_workspaces: [Option<UiSectionWorkspace>; 5],
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
            retained_conversations: BTreeMap::new(),
            retained_conversation_order: VecDeque::new(),
            desired_conversation: None,
            requested_conversation: None,
            observation_mode: UiObservationMode::SnapshotFallback,
            conversation_anchor: None,
            conversation_scroll_mode: ConversationScrollMode::Anchored,
            conversation_viewport_position: None,
            conversation_viewport_geometry: None,
            technical_visible: false,
            technical_scroll: 0,
            mailbox_modal: None,
            mailbox_draft: None,
            agent_modal: None,
            interactions: VecDeque::new(),
            interaction_modal: None,
            pending_interaction: None,
            project_interaction: None,
            project_summary: None,
            project_workspace_level: UiProjectWorkspaceLevel::List,
            project_summary_focus: UiProjectSummaryFocus::Conversation,
            project_management_action: None,
            project_folder_action: UiProjectFolderAction::AddFolder,
            project_folder_id: None,
            project_filter: None,
            project_filter_rows: Vec::new(),
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
            conversation_failure: None,
            pending_mailbox: None,
            pending_agent: None,
            pending_managed_session: None,
            pending_project: None,
            pending_project_conversation: None,
            section_workspaces: [None, None, None, None, None],
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
            return (matches!(
                self.project_interaction,
                Some(UiProjectInteraction::AddResource { .. })
            ) && field == UiProjectFormField::Path)
                || (matches!(
                    self.project_interaction,
                    Some(UiProjectInteraction::ConfirmClose { .. })
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
        match (
            &self.project_interaction,
            &self.agent_modal,
            &self.mailbox_modal,
            &self.mailbox_draft,
        ) {
            (Some(UiProjectInteraction::Search { .. }), _, _, _) => Some(UiFormKind::ProjectSearch),
            (Some(UiProjectInteraction::CreateExisting { .. }), _, _, _) => {
                Some(UiFormKind::ProjectCreateExisting)
            }
            (Some(UiProjectInteraction::CreateWorktree { .. }), _, _, _) => {
                Some(UiFormKind::ProjectCreateWorktree)
            }
            (Some(UiProjectInteraction::AddResource { .. }), _, _, _) => {
                Some(UiFormKind::ProjectAddResource)
            }
            (Some(UiProjectInteraction::ReplaceResource { .. }), _, _, _) => {
                Some(UiFormKind::ProjectReplaceResource)
            }
            (Some(UiProjectInteraction::Activate { .. }), _, _, _) => {
                Some(UiFormKind::ProjectActivate)
            }
            (Some(UiProjectInteraction::Handoff { .. }), _, _, _) => {
                Some(UiFormKind::ProjectHandoff)
            }
            (Some(UiProjectInteraction::ConfirmClose { .. }), _, _, _) => {
                Some(UiFormKind::ProjectConfirmClose)
            }
            (_, Some(UiAgentModal::Search { .. }), _, _) => Some(UiFormKind::AgentSearch),
            (_, Some(UiAgentModal::Create { .. }), _, _) => Some(UiFormKind::AgentCreate),
            (_, Some(UiAgentModal::RenameSession { .. }), _, _) => Some(UiFormKind::AgentRename),
            (_, _, _, Some(UiMailboxDraftPane::Editing { .. })) => Some(UiFormKind::MailboxCompose),
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
        self.snapshot.as_ref().map(|snapshot| {
            if self.section == UiSection::Inbox && self.project_filter.is_some() {
                self.project_filter_rows.as_slice()
            } else {
                snapshot.rows(self.section)
            }
        })
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

    /// Returns the stable visual row currently at the top of the transcript viewport.
    pub const fn conversation_viewport_position(&self) -> Option<&UiConversationViewportPosition> {
        self.conversation_viewport_position.as_ref()
    }

    /// Reports whether new conversation content should remain pinned to the transcript tail.
    pub const fn conversation_follows_tail(&self) -> bool {
        matches!(
            self.conversation_scroll_mode,
            ConversationScrollMode::FollowTail
        )
    }

    /// Reports whether typed technical disclosure is expanded.
    pub const fn technical_visible(&self) -> bool {
        self.technical_visible
    }

    /// Returns the selected detail inspector's logical vertical scroll offset.
    pub const fn technical_scroll(&self) -> u16 {
        self.technical_scroll
    }

    /// Borrows the current mailbox interaction, when a modal is open.
    pub const fn mailbox_modal(&self) -> Option<&UiMailboxModal> {
        self.mailbox_modal.as_ref()
    }

    /// Borrows the modeless Inbox draft pane.
    pub const fn mailbox_draft(&self) -> Option<&UiMailboxDraftPane> {
        self.mailbox_draft.as_ref()
    }

    /// Borrows the authoritative project name associated with the active draft, when applicable.
    pub fn draft_project_name(&self) -> Option<&str> {
        let project_id = self.draft_project_id()?;
        self.snapshot
            .as_ref()?
            .projects
            .iter()
            .find(|project| project.project_id == project_id)
            .map(|project| project.name.as_str())
    }

    /// Borrows the authoritative display name of the active draft's recipient.
    pub fn draft_recipient_name(&self) -> Option<&str> {
        match self.draft_target()? {
            UiMailboxDraftTarget::SelfNote => Some("You"),
            UiMailboxDraftTarget::Direct {
                installation_id,
                mailbox_id,
            } => self
                .snapshot
                .as_ref()?
                .direct_targets
                .iter()
                .find(|target| {
                    target.installation_id == *installation_id && target.mailbox_id == *mailbox_id
                })
                .map(|target| target.label.as_str()),
            UiMailboxDraftTarget::Reply { .. } => self
                .conversation
                .as_ref()
                .map(|conversation| conversation.title.as_str()),
            UiMailboxDraftTarget::Project {
                project_id,
                thread_id,
            } => self
                .project_draft_recipient(*project_id, *thread_id)
                .or_else(|| self.selected_project_conversation_recipient(*project_id, *thread_id)),
        }
    }

    fn project_draft_recipient(
        &self,
        project_id: [u8; 32],
        thread_id: Option<[u8; 32]>,
    ) -> Option<&str> {
        let snapshot = self.snapshot.as_ref()?;
        let project = snapshot
            .projects
            .iter()
            .find(|project| project.project_id == project_id)?;
        let agent_id = match thread_id {
            Some(thread_id) => {
                project
                    .threads
                    .iter()
                    .find(|thread| thread.thread_id == thread_id)?
                    .agent_id
            }
            None => match &self.guided_pending {
                Some(UiGuidedPending::Instruction(submission))
                    if submission.project_id == project_id =>
                {
                    submission.agent_id
                }
                _ => project.assignment.as_ref()?.agent_id,
            },
        };
        let agent = snapshot
            .agents
            .iter()
            .find(|agent| agent.agent_id == agent_id)?;
        match agent.names.as_slice() {
            [name] => Some(name.as_str()),
            _ => None,
        }
    }

    fn selected_project_conversation_recipient(
        &self,
        project_id: [u8; 32],
        thread_id: Option<[u8; 32]>,
    ) -> Option<&str> {
        let selected = self.selected_row_data()?;
        let UiConversationTarget::Project {
            project_id: selected_project,
            thread_id: selected_thread,
            ..
        } = selected.conversation_target?;
        if selected_project != project_id || Some(selected_thread) != thread_id {
            return None;
        }
        self.conversation
            .as_ref()
            .filter(|conversation| conversation.row_id == selected.id)
            .map(|conversation| conversation.title.as_str())
    }

    fn draft_target(&self) -> Option<&UiMailboxDraftTarget> {
        let target = match self.mailbox_draft.as_ref()? {
            UiMailboxDraftPane::Loading { target }
            | UiMailboxDraftPane::Editing {
                draft: UiMailboxDraft { target, .. },
                ..
            } => target,
        };
        Some(target)
    }

    fn draft_project_id(&self) -> Option<[u8; 32]> {
        Some(match self.draft_target()? {
            UiMailboxDraftTarget::Project { project_id, .. } => *project_id,
            UiMailboxDraftTarget::Reply { .. } => {
                match self.selected_row_data()?.conversation_target {
                    Some(UiConversationTarget::Project { project_id, .. }) => project_id,
                    None => return None,
                }
            }
            UiMailboxDraftTarget::Direct { .. } | UiMailboxDraftTarget::SelfNote => return None,
        })
    }

    fn new_project_draft_id(&self) -> Option<[u8; 32]> {
        match self.draft_target()? {
            UiMailboxDraftTarget::Project {
                project_id,
                thread_id: None,
            } => Some(*project_id),
            UiMailboxDraftTarget::Reply { .. }
            | UiMailboxDraftTarget::Direct { .. }
            | UiMailboxDraftTarget::SelfNote
            | UiMailboxDraftTarget::Project {
                thread_id: Some(_), ..
            } => None,
        }
    }

    /// Borrows the current named-agent interaction.
    pub const fn agent_modal(&self) -> Option<&UiAgentModal> {
        self.agent_modal.as_ref()
    }

    /// Borrows the current provider-interaction dialog.
    pub const fn interaction_modal(&self) -> Option<&UiInteractionModal> {
        self.interaction_modal.as_ref()
    }

    /// Borrows the current highest-priority live interaction.
    pub fn current_interaction(&self) -> Option<&UiInteraction> {
        self.interactions.front()
    }

    /// Borrows the current project interaction.
    pub const fn project_interaction(&self) -> Option<&UiProjectInteraction> {
        self.project_interaction.as_ref()
    }

    /// Borrows the selected project's decoupled authoritative workspace summary.
    pub const fn project_summary(&self) -> Option<&UiProjectSummary> {
        self.project_summary.as_ref()
    }

    /// Returns the current modeless Projects navigation depth.
    pub const fn project_workspace_level(&self) -> UiProjectWorkspaceLevel {
        self.project_workspace_level
    }

    /// Returns the stable selected card within a project summary.
    pub const fn project_summary_focus(&self) -> Option<UiProjectSummaryFocus> {
        match self.project_workspace_level {
            UiProjectWorkspaceLevel::List => None,
            UiProjectWorkspaceLevel::Summary
            | UiProjectWorkspaceLevel::Manage
            | UiProjectWorkspaceLevel::Folders => Some(self.project_summary_focus),
        }
    }

    /// Returns the selected labeled Manage project action.
    pub const fn project_management_action(&self) -> Option<UiProjectManagementAction> {
        self.project_management_action
    }

    /// Returns the selected labeled folder action.
    pub const fn project_folder_action(&self) -> Option<UiProjectFolderAction> {
        match self.project_workspace_level {
            UiProjectWorkspaceLevel::Folders => Some(self.project_folder_action),
            UiProjectWorkspaceLevel::List
            | UiProjectWorkspaceLevel::Summary
            | UiProjectWorkspaceLevel::Manage => None,
        }
    }

    /// Returns the exact folder currently selected for object-bearing actions.
    pub const fn selected_project_folder(&self) -> Option<[u8; 32]> {
        self.project_folder_id
    }

    /// Borrows the visible typed project filter currently applied to Inbox.
    pub const fn project_filter(&self) -> Option<&UiProjectInboxFilter> {
        self.project_filter.as_ref()
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
        match &self.pending_mailbox {
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

    /// Returns whether an older page is loading for the selected conversation.
    pub fn conversation_older_loading(&self) -> bool {
        self.pending_conversation.as_ref().is_some_and(|pending| {
            pending.cursor.is_some() && self.selected_row.as_ref() == Some(&pending.row_id)
        })
    }

    /// Borrows a failure scoped to the currently selected conversation row.
    pub fn conversation_failure(&self) -> Option<&UiFailure> {
        self.conversation_failure
            .as_ref()
            .filter(|failure| self.selected_row.as_ref() == Some(&failure.row_id))
            .map(|failure| &failure.failure)
    }

    /// Returns whether the visible conversation failure came from an older-page request.
    pub fn conversation_failure_is_older(&self) -> bool {
        self.conversation_failure.as_ref().is_some_and(|failure| {
            failure.cursor.is_some() && self.selected_row.as_ref() == Some(&failure.row_id)
        })
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
        cursor: String,
        enter_on_load: bool,
        effects: &mut Vec<UiEffect>,
    ) -> Result<(), UiError> {
        if let Some(pending) = &mut self.pending_conversation
            && pending.row_id == row_id
            && pending.cursor.as_ref() == Some(&cursor)
        {
            pending.enter_on_load |= enter_on_load;
            return Ok(());
        }
        let id = self.allocate_effect()?;
        self.pending_conversation = Some(PendingConversation {
            id,
            row_id: row_id.clone(),
            cursor: Some(cursor.clone()),
            enter_on_load,
        });
        self.conversation_failure = None;
        effects.push(UiEffect::LoadConversation {
            id,
            row_id,
            cursor: Some(cursor),
        });
        Ok(())
    }

    fn request_inbox_preview(&mut self, effects: &mut Vec<UiEffect>) {
        if self.section != UiSection::Inbox {
            if self.requested_conversation.take().is_some() {
                effects.push(UiEffect::ObserveConversation { row_id: None });
            }
            return;
        }
        let row_id = self
            .desired_conversation
            .clone()
            .or_else(|| self.selected_row.clone());
        let Some(row_id) = row_id else {
            return;
        };
        if self.requested_conversation.as_ref() != Some(&row_id) {
            self.requested_conversation = Some(row_id.clone());
            effects.push(UiEffect::ObserveConversation {
                row_id: Some(row_id.clone()),
            });
        }
        if self.desired_conversation.is_some()
            || self
                .conversation
                .as_ref()
                .is_none_or(|conversation| conversation.row_id != row_id)
        {
            self.install_retained_conversation(&row_id);
        }
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
        self.change_section(UiSection::Inbox);
        if let UiMailboxDraftTarget::Project {
            project_id,
            thread_id: None,
        } = target
            && let Some(project) = self.snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .projects
                    .iter()
                    .find(|project| project.project_id == project_id)
                    .cloned()
            })
        {
            self.install_project_draft_conversation(&project);
        }
        self.focus = UiFocus::Draft;
        self.mailbox_modal = None;
        self.mailbox_draft = Some(UiMailboxDraftPane::Loading {
            target: target.clone(),
        });
        effects.push(UiEffect::OpenDraft { id, target });
        Ok(())
    }

    fn save_draft(&mut self, effects: &mut Vec<UiEffect>) -> Result<(), UiError> {
        if self.pending_mailbox.is_some() {
            return Ok(());
        }
        let Some(UiMailboxDraftPane::Editing {
            draft, dirty: true, ..
        }) = &self.mailbox_draft
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
        let optimistic_entry = draft
            .as_ref()
            .and_then(|draft| append_pending_message(self, draft, id));
        self.pending_mailbox = Some(PendingMailbox {
            id,
            kind: PendingMailboxKind::SubmitCommand(Box::new(PendingMailboxSubmission {
                draft: draft.clone(),
                action: action.clone(),
                optimistic_entry,
            })),
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
            conversation_scroll_mode: self.conversation_scroll_mode,
            conversation_viewport_position: self.conversation_viewport_position.clone(),
            technical_visible: self.technical_visible,
            technical_scroll: self.technical_scroll,
            focus: self.focus,
            conversation_failure: self.conversation_failure.clone(),
        });
    }

    fn restore_section_workspace(&mut self) {
        let workspace = self.section_workspaces[self.section.index()].clone();
        if let Some(workspace) = workspace {
            self.selected_row = workspace.selected_row;
            self.conversation = workspace.conversation;
            self.conversation_anchor = workspace.conversation_anchor;
            self.conversation_scroll_mode = workspace.conversation_scroll_mode;
            self.conversation_viewport_position = workspace.conversation_viewport_position;
            self.conversation_viewport_geometry = None;
            self.technical_visible = workspace.technical_visible;
            self.technical_scroll = workspace.technical_scroll;
            self.focus = workspace.focus;
            self.conversation_failure = workspace.conversation_failure;
        } else {
            self.selected_row = None;
            self.conversation = None;
            self.conversation_anchor = None;
            self.conversation_scroll_mode = ConversationScrollMode::Anchored;
            self.conversation_viewport_position = None;
            self.conversation_viewport_geometry = None;
            self.technical_visible = false;
            self.technical_scroll = 0;
            self.focus = UiFocus::Navigation;
            self.conversation_failure = None;
        }
        self.pending_conversation = None;
        self.conversation_failure = None;
    }

    fn change_section(&mut self, next: UiSection) {
        if self.section == next {
            return;
        }
        self.save_section_workspace();
        self.section = next;
        self.restore_section_workspace();
        self.reconcile_current_section();
        self.refresh_selected_project_summary();
    }

    fn schedule_timer(
        &mut self,
        kind: UiTimerKind,
        after: Duration,
        effects: &mut Vec<UiEffect>,
    ) -> Result<(), UiError> {
        let id = self.allocate_effect()?;
        match kind {
            UiTimerKind::RetrySnapshot => self.retry_timer = Some(id),
            UiTimerKind::AutosaveDraft => self.autosave_timer = Some(id),
            UiTimerKind::DismissCompletion => self.completion_timer = Some(id),
        }
        effects.push(UiEffect::ScheduleTimer { id, kind, after });
        Ok(())
    }

    fn move_row_selection(&mut self, forward: bool) -> bool {
        let Some(rows) = self.rows() else {
            return false;
        };
        if rows.is_empty() {
            return false;
        }
        let current_selection = if self.section == UiSection::Inbox {
            self.desired_conversation
                .as_ref()
                .or(self.selected_row.as_ref())
        } else {
            self.selected_row.as_ref()
        };
        let current =
            current_selection.and_then(|selected| rows.iter().position(|row| &row.id == selected));
        let next = match (current, forward) {
            (Some(index), true) => (index + 1).min(rows.len() - 1),
            (Some(index), false) => index.saturating_sub(1),
            (None, _) => 0,
        };
        let selected = rows[next].id.clone();
        if current_selection == Some(&selected) {
            false
        } else if self.section == UiSection::Inbox {
            self.desired_conversation = Some(selected);
            true
        } else {
            self.selected_row = Some(selected);
            self.close_conversation();
            if self.section == UiSection::Projects {
                self.project_summary_focus = UiProjectSummaryFocus::Conversation;
                self.refresh_selected_project_summary();
            }
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
            self.conversation_scroll_mode = ConversationScrollMode::Anchored;
            self.reveal_conversation_entry_start();
            self.technical_visible = false;
            self.technical_scroll = 0;
            true
        }
    }

    fn observe_conversation_viewport(
        &mut self,
        observation: UiConversationViewportObservation,
    ) -> bool {
        let Some(conversation) = self
            .conversation
            .as_ref()
            .filter(|conversation| conversation.row_id == observation.conversation_id)
        else {
            return false;
        };
        if observation.width == 0
            || observation.height == 0
            || observation.entries.is_empty()
            || observation.entries.iter().any(|entry| entry.height == 0)
        {
            return false;
        }
        let mut previous_index = None;
        for measured in &observation.entries {
            let Some(index) = conversation
                .entries
                .iter()
                .position(|entry| entry.id == measured.entry_id)
            else {
                return false;
            };
            if previous_index.is_some_and(|previous| index <= previous) {
                return false;
            }
            previous_index = Some(index);
        }
        let geometry = ConversationViewportGeometry {
            height: observation.height,
            entries: observation.entries,
        };
        let previous = self.conversation_viewport_position.clone();
        self.conversation_viewport_position =
            if self.conversation_scroll_mode == ConversationScrollMode::FollowTail {
                geometry.position_at(geometry.maximum_top())
            } else if let Some(position) = previous.as_ref() {
                geometry
                    .offset_for(position)
                    .and_then(|offset| geometry.position_at(offset))
                    .or_else(|| self.anchor_start_in(&geometry))
            } else {
                self.anchor_start_in(&geometry)
            };
        let changed = previous != self.conversation_viewport_position;
        self.conversation_viewport_geometry = Some(geometry);
        changed
    }

    fn anchor_start_in(
        &self,
        geometry: &ConversationViewportGeometry,
    ) -> Option<UiConversationViewportPosition> {
        self.conversation_anchor
            .as_deref()
            .and_then(|anchor| geometry.entry_start(anchor))
            .and_then(|(start, _)| geometry.position_at(start))
            .or_else(|| geometry.position_at(geometry.maximum_top()))
    }

    fn scroll_conversation_viewport(&mut self, forward: bool) -> bool {
        let Some(geometry) = self.conversation_viewport_geometry.as_ref() else {
            return false;
        };
        let current = self
            .conversation_viewport_position
            .as_ref()
            .and_then(|position| geometry.offset_for(position))
            .unwrap_or_else(|| geometry.maximum_top());
        let next = if forward {
            current.saturating_add(1).min(geometry.maximum_top())
        } else {
            current.saturating_sub(1)
        };
        let next_mode = if forward && next == geometry.maximum_top() {
            ConversationScrollMode::FollowTail
        } else {
            ConversationScrollMode::Anchored
        };
        let position = geometry.position_at(next);
        if position == self.conversation_viewport_position
            && next_mode == self.conversation_scroll_mode
        {
            return false;
        }
        self.conversation_viewport_position = position;
        self.conversation_scroll_mode = next_mode;
        true
    }

    fn reveal_conversation_entry_start(&mut self) -> bool {
        let Some(anchor) = self.conversation_anchor.clone() else {
            return false;
        };
        let requested = UiConversationViewportPosition {
            entry_id: anchor,
            row: 0,
        };
        let position = self
            .conversation_viewport_geometry
            .as_ref()
            .and_then(|geometry| {
                geometry
                    .offset_for(&requested)
                    .and_then(|offset| geometry.position_at(offset))
            })
            .unwrap_or(requested);
        let changed = self.conversation_viewport_position.as_ref() != Some(&position)
            || self.conversation_scroll_mode != ConversationScrollMode::Anchored;
        self.conversation_viewport_position = Some(position);
        self.conversation_scroll_mode = ConversationScrollMode::Anchored;
        changed
    }

    fn reveal_conversation_entry_end(&mut self) -> bool {
        let Some(geometry) = self.conversation_viewport_geometry.as_ref() else {
            return false;
        };
        let Some(anchor) = self.conversation_anchor.as_deref() else {
            return false;
        };
        let Some((start, height)) = geometry.entry_start(anchor) else {
            return false;
        };
        let top_within_entry = height.saturating_sub(geometry.height);
        let position = geometry.position_at(
            start
                .saturating_add(u64::from(top_within_entry))
                .min(geometry.maximum_top()),
        );
        let changed = position != self.conversation_viewport_position
            || self.conversation_scroll_mode != ConversationScrollMode::Anchored;
        self.conversation_viewport_position = position;
        self.conversation_scroll_mode = ConversationScrollMode::Anchored;
        changed
    }

    fn follow_conversation_tail(&mut self) -> bool {
        let tail = self
            .conversation
            .as_ref()
            .and_then(|conversation| conversation.entries.last())
            .map(|entry| entry.id.clone());
        let position = tail.as_ref().and_then(|_| {
            self.conversation_viewport_geometry
                .as_ref()
                .and_then(|geometry| geometry.position_at(geometry.maximum_top()))
        });
        let mode = if tail.is_some() {
            ConversationScrollMode::FollowTail
        } else {
            ConversationScrollMode::Anchored
        };
        let changed = self.conversation_anchor != tail
            || self.conversation_scroll_mode != mode
            || self.conversation_viewport_position != position
            || self.technical_visible
            || self.technical_scroll != 0;
        self.conversation_anchor = tail;
        self.conversation_scroll_mode = mode;
        self.conversation_viewport_position = position;
        self.close_technical_details();
        changed
    }

    fn toggle_technical_details(&mut self) -> bool {
        if self.focus != UiFocus::Conversation || self.conversation_anchor.is_none() {
            return false;
        }
        self.technical_visible = !self.technical_visible;
        self.technical_scroll = 0;
        true
    }

    fn close_technical_details(&mut self) {
        self.technical_visible = false;
        self.technical_scroll = 0;
    }

    fn scroll_technical_details(&mut self, forward: bool) -> bool {
        if !self.technical_visible {
            return false;
        }
        let next = if forward {
            self.technical_scroll.saturating_add(1)
        } else {
            self.technical_scroll.saturating_sub(1)
        };
        if next == self.technical_scroll {
            false
        } else {
            self.technical_scroll = next;
            true
        }
    }

    fn selected_row_is_conversation(&self) -> bool {
        self.selected_row.as_ref().is_some_and(|selected| {
            self.rows().is_some_and(|rows| {
                rows.iter()
                    .any(|row| &row.id == selected && row.kind == UiRowKind::Conversation)
            })
        })
    }

    fn close_conversation(&mut self) {
        self.conversation = None;
        self.conversation_anchor = None;
        self.conversation_scroll_mode = ConversationScrollMode::Anchored;
        self.conversation_viewport_position = None;
        self.conversation_viewport_geometry = None;
        self.close_technical_details();
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
        refresh_project_interaction(self, &snapshot);
        refresh_new_modal(self, &snapshot);
        self.refresh_project_filter(&snapshot);
        self.snapshot = Some(snapshot);
        if let Some((project_id, root_message)) = self.pending_project_conversation
            && let Some(row_id) = self.snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .inbox_rows
                    .iter()
                    .find_map(|row| match row.conversation_target {
                        Some(UiConversationTarget::Project {
                            project_id: candidate_project,
                            root_message: candidate_root,
                            ..
                        }) if candidate_project == project_id && candidate_root == root_message => {
                            Some(row.id.clone())
                        }
                        _ => None,
                    })
            })
        {
            self.selected_row = Some(row_id);
            self.pending_project_conversation = None;
        }
        self.reconcile_current_section();
        self.refresh_selected_project_summary();
        select_agent_search_match(self, false);
        select_project_search_match(self, false);
    }

    fn retain_conversation(&mut self, revision: u64, page: UiConversationPage) {
        let row_id = page.row_id.clone();
        self.retained_conversation_order
            .retain(|candidate| candidate != &row_id);
        self.retained_conversation_order.push_back(row_id.clone());
        self.retained_conversations
            .insert(row_id, RetainedConversationPage { revision, page });
        while self.retained_conversation_order.len() > MAX_RETAINED_CONVERSATION_PAGES {
            if let Some(evicted) = self.retained_conversation_order.pop_front() {
                self.retained_conversations.remove(&evicted);
            }
        }
    }

    fn install_retained_conversation(&mut self, row_id: &str) -> bool {
        let Some(revision) = self.snapshot.as_ref().map(|snapshot| snapshot.revision) else {
            return false;
        };
        let Some(retained) = self.retained_conversations.get(row_id) else {
            return false;
        };
        if retained.revision != revision {
            return false;
        }
        let page = retained.page.clone();
        self.install_first_conversation_page(page);
        self.selected_row = Some(row_id.to_owned());
        self.desired_conversation = None;
        true
    }

    fn install_first_conversation_page(&mut self, mut page: UiConversationPage) {
        let same_conversation = self
            .conversation
            .as_ref()
            .is_some_and(|conversation| conversation.row_id == page.row_id);
        let previous_anchor = self.conversation_anchor.clone();
        let follow_tail = !same_conversation
            || self.conversation_scroll_mode == ConversationScrollMode::FollowTail;
        apply_pending_project_delivery(self.snapshot.as_ref(), &mut page.entries);
        self.conversation = Some(UiConversation {
            row_id: page.row_id,
            title: page.title,
            context: page.context,
            entries: page.entries,
            next_cursor: page.next_cursor,
        });
        if let Some(conversation) = &mut self.conversation {
            place_live_activity_at_tail(&mut conversation.entries);
        }
        self.conversation_anchor = self.conversation.as_ref().and_then(|conversation| {
            previous_anchor
                .filter(|anchor| conversation.entries.iter().any(|entry| &entry.id == anchor))
                .or_else(|| conversation.entries.last().map(|entry| entry.id.clone()))
        });
        self.conversation_scroll_mode = if follow_tail && self.conversation_anchor.is_some() {
            ConversationScrollMode::FollowTail
        } else {
            ConversationScrollMode::Anchored
        };
        if !same_conversation {
            self.conversation_viewport_position = None;
        }
        self.conversation_viewport_geometry = None;
        self.conversation_failure = None;
        self.last_failure = None;
    }

    fn refresh_project_filter(&mut self, snapshot: &UiSnapshot) {
        let Some(mut filter) = self.project_filter.take() else {
            self.project_filter_rows.clear();
            return;
        };
        let Some(project) = snapshot
            .projects
            .iter()
            .find(|project| project.project_id == filter.project_id)
        else {
            self.project_filter_rows.clear();
            return;
        };
        filter.project_name.clone_from(&project.name);
        self.project_filter_rows = project_conversation_rows(snapshot, filter.project_id);
        let targets_local_draft = self.new_project_draft_id() == Some(filter.project_id);
        let awaits_authoritative_row =
            self.pending_project_conversation
                .is_some_and(|(project_id, root_message)| {
                    project_id == filter.project_id
                        && !has_project_conversation_root(
                            &self.project_filter_rows,
                            project_id,
                            root_message,
                        )
                });
        if targets_local_draft || awaits_authoritative_row {
            let row = project_draft_conversation_row(project);
            if let Some(conversation) = &mut self.conversation
                && conversation.row_id == row.id
            {
                conversation.title.clone_from(&project.name);
            }
            self.project_filter_rows.insert(0, row);
        }
        self.project_filter = Some(filter);
    }

    fn refresh_selected_project_summary(&mut self) {
        self.project_summary = self.snapshot.as_ref().and_then(|snapshot| {
            let selected = self.selected_row.as_deref()?;
            let project = snapshot
                .projects
                .iter()
                .find(|project| agent_hex(project.project_id) == selected)?;
            Some(project_summary(snapshot, project))
        });
        if self.project_summary.is_none() {
            self.project_workspace_level = UiProjectWorkspaceLevel::List;
            self.project_summary_focus = UiProjectSummaryFocus::Conversation;
        }
    }

    fn install_project_filter(&mut self, project: &UiProject) {
        self.project_filter = Some(UiProjectInboxFilter {
            project_id: project.project_id,
            project_name: project.name.clone(),
        });
        self.project_filter_rows = self.snapshot.as_ref().map_or_else(Vec::new, |snapshot| {
            project_conversation_rows(snapshot, project.project_id)
        });
    }

    fn install_project_draft_conversation(&mut self, project: &UiProject) {
        self.install_project_filter(project);
        let row = project_draft_conversation_row(project);
        self.project_filter_rows
            .retain(|candidate| candidate.id != row.id);
        self.project_filter_rows.insert(0, row.clone());
        self.selected_row = Some(row.id.clone());
        self.conversation = Some(UiConversation {
            row_id: row.id,
            title: project.name.clone(),
            context: Some("New project conversation".to_owned()),
            entries: Vec::new(),
            next_cursor: None,
        });
        self.conversation_anchor = None;
        self.conversation_scroll_mode = ConversationScrollMode::Anchored;
        self.close_technical_details();
    }

    fn clear_project_filter(&mut self) {
        self.project_filter = None;
        self.project_filter_rows.clear();
        self.reconcile_current_section();
    }

    fn reconcile_current_section(&mut self) {
        if self.snapshot.is_none() {
            self.selected_row = None;
            self.close_conversation();
            return;
        }
        let rows = self
            .rows()
            .unwrap_or_default()
            .iter()
            .map(|row| (row.id.clone(), row.kind))
            .collect::<Vec<_>>();
        let keep = self.selected_row.as_ref().and_then(|selected| {
            rows.iter()
                .find(|(row_id, _)| row_id == selected)
                .map(|(row_id, _)| row_id.clone())
        });
        self.selected_row = keep.or_else(|| rows.first().map(|(row_id, _)| row_id.clone()));
        let conversation_survives = self.conversation.as_ref().is_some_and(|conversation| {
            self.selected_row.as_ref() == Some(&conversation.row_id)
                && rows.iter().any(|(row_id, kind)| {
                    *row_id == conversation.row_id && *kind == UiRowKind::Conversation
                })
        });
        if !conversation_survives {
            self.close_conversation();
        }
    }
}

#[allow(clippy::too_many_lines)]
fn project_summary(snapshot: &UiSnapshot, project: &UiProject) -> UiProjectSummary {
    let lifecycle = if project.archived {
        UiProjectLifecycle::Archived
    } else {
        match project.lifecycle.as_str() {
            "open" => UiProjectLifecycle::Open,
            "closing" => UiProjectLifecycle::Closing,
            "closed" => UiProjectLifecycle::Closed,
            _ => UiProjectLifecycle::NeedsAttention,
        }
    };
    let conversations = project_conversation_summary(snapshot, project.project_id);
    let assigned_agent = match &project.assignment {
        None => UiProjectAssignedAgentSummary {
            agent_id: None,
            name: None,
            status: UiProjectAssignedAgentStatus::Unassigned,
            working_folder: None,
        },
        Some(assignment) => {
            let name = snapshot
                .agents
                .iter()
                .find(|agent| agent.agent_id == assignment.agent_id)
                .and_then(|agent| match agent.names.as_slice() {
                    [name] => Some(name.clone()),
                    _ => None,
                });
            let status = if assignment.cardinality_conflicted || assignment.blocked.is_some() {
                UiProjectAssignedAgentStatus::NeedsAttention
            } else if assignment.runnable {
                UiProjectAssignedAgentStatus::Ready
            } else {
                UiProjectAssignedAgentStatus::SettingUp
            };
            UiProjectAssignedAgentSummary {
                agent_id: Some(assignment.agent_id),
                name,
                status,
                working_folder: assignment.launch_directory.clone(),
            }
        }
    };
    let mut folders = project
        .resources
        .iter()
        .map(|resource| UiProjectFolderSummary {
            folder_id: resource.resource_id,
            path: resource.display_path.clone(),
            working_folder: resource.primary,
            health: resource.health.clone(),
            ownership: if resource.active_claim {
                UiProjectFolderOwnership::Owned
            } else if !resource.conflicting_projects.is_empty() {
                UiProjectFolderOwnership::Conflicted
            } else {
                UiProjectFolderOwnership::NeedsAttention
            },
            conflicting_projects: resource
                .conflicting_projects
                .iter()
                .map(|project_id| {
                    snapshot
                        .projects
                        .iter()
                        .find(|candidate| candidate.project_id == *project_id)
                        .map_or_else(
                            || "Unnamed project".to_owned(),
                            |candidate| candidate.name.clone(),
                        )
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    folders.sort_by_key(|folder| !folder.working_folder);
    let mut recovery = Vec::new();
    if !project.claimable
        || folders
            .iter()
            .any(|folder| folder.ownership != UiProjectFolderOwnership::Owned)
    {
        recovery.push(UiProjectRecoverySummary::FolderOwnership);
    }
    if let Some(assignment) = &project.assignment {
        if assignment.cardinality_conflicted {
            recovery.push(UiProjectRecoverySummary::AssignedAgentConflict);
        } else if assignment.blocked.is_some() {
            recovery.push(UiProjectRecoverySummary::AssignedAgentBlocked);
        }
    }
    let assignment = project.assignment.as_ref();
    UiProjectSummary {
        project_id: project.project_id,
        name: project.name.clone(),
        lifecycle,
        conversations,
        assigned_agent,
        folders,
        recovery,
        technical: UiProjectTechnicalEvidence {
            project_id: project.project_id,
            home: project.home,
            head: project.head,
            input_sequence: project.input_sequence,
            assignment_id: assignment.map(|assignment| assignment.assignment_id),
            provider: assignment.map(|assignment| assignment.provider.clone()),
            session: assignment.and_then(|assignment| assignment.session.clone()),
            thread_id: assignment.and_then(|assignment| assignment.thread_id),
        },
    }
}

fn project_conversation_summary(
    snapshot: &UiSnapshot,
    project_id: [u8; 32],
) -> UiProjectConversationSummary {
    let open_rows = project_conversation_row_ids(&snapshot.inbox_rows, project_id);
    let archived_rows = project_conversation_row_ids(&snapshot.archived_rows, project_id);
    UiProjectConversationSummary {
        open: open_rows.len(),
        archived: archived_rows.len(),
        open_rows,
        archived_rows,
    }
}

fn project_conversation_row_ids(rows: &[UiRow], project_id: [u8; 32]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    rows.iter()
        .filter(|row| {
            matches!(
                row.conversation_target,
                Some(UiConversationTarget::Project {
                    project_id: candidate,
                    ..
                }) if candidate == project_id
            )
        })
        .filter(|row| seen.insert(row.id.clone()))
        .map(|row| row.id.clone())
        .collect()
}

fn project_conversation_rows(snapshot: &UiSnapshot, project_id: [u8; 32]) -> Vec<UiRow> {
    let mut seen = BTreeSet::new();
    snapshot
        .inbox_rows
        .iter()
        .chain(snapshot.archived_rows.iter())
        .filter(|row| {
            matches!(
                row.conversation_target,
                Some(UiConversationTarget::Project {
                    project_id: candidate,
                    ..
                }) if candidate == project_id
            )
        })
        .filter(|row| seen.insert(row.id.clone()))
        .cloned()
        .collect()
}

fn has_project_conversation_root(
    rows: &[UiRow],
    project_id: [u8; 32],
    root_message: [u8; 32],
) -> bool {
    rows.iter().any(|row| {
        matches!(
            row.conversation_target,
            Some(UiConversationTarget::Project {
                project_id: candidate_project,
                root_message: candidate_root,
                ..
            }) if candidate_project == project_id && candidate_root == root_message
        )
    })
}

fn project_draft_conversation_row(project: &UiProject) -> UiRow {
    UiRow {
        id: project_draft_conversation_id(project.project_id),
        title: project.name.clone(),
        detail: "New project conversation".to_owned(),
        state: UiRowState::Open,
        kind: UiRowKind::Conversation,
        conversation_target: None,
    }
}

fn project_draft_conversation_id(project_id: [u8; 32]) -> String {
    format!("project-draft:{}", agent_hex(project_id))
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
                model.conversation_viewport_geometry = None;
                effects.push(UiEffect::RequestRedraw);
            }
        }
        UiEvent::ConversationViewportObserved { observation } => {
            if model.observe_conversation_viewport(observation) {
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
        UiEvent::MaterializedViewObserved { view } => {
            materialized_view_observed(&mut model, view, &mut effects)?;
        }
        UiEvent::InteractionsObserved { interactions } => {
            interactions_observed(&mut model, interactions, &mut effects);
        }
        UiEvent::InteractionAnswered { effect_id, outcome } => {
            interaction_answered(&mut model, effect_id, outcome, &mut effects);
        }
        UiEvent::InteractionAnswerFailed { effect_id, failure } => {
            interaction_answer_failed(&mut model, effect_id, failure, &mut effects);
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
            message_id,
        } => mailbox_command_committed(&mut model, effect_id, revision, message_id, &mut effects)?,
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
    if model.snapshot.is_none() {
        model.request_snapshot(effects)?;
    }
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn materialized_view_observed(
    model: &mut UiModel,
    view: UiMaterializedConversationView,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    let revision = view.snapshot.revision;
    if model
        .snapshot
        .as_ref()
        .is_some_and(|snapshot| revision < snapshot.revision)
    {
        return Ok(());
    }
    let selected_row = view
        .conversation
        .as_ref()
        .map(|conversation| conversation.row_id.as_str());
    if selected_row.is_some_and(|row_id| {
        !view
            .snapshot
            .inbox_rows
            .iter()
            .any(|row| row.id == row_id && row.kind == UiRowKind::Conversation)
    }) {
        return Ok(());
    }
    if let Some(desired) = model.desired_conversation.as_deref()
        && selected_row != Some(desired)
    {
        return Ok(());
    }
    if let Some(requested) = model.requested_conversation.as_deref()
        && selected_row.is_some()
        && selected_row != Some(requested)
    {
        return Ok(());
    }
    let agent_finished = view
        .conversation
        .as_ref()
        .is_some_and(|page| agent_turn_just_finished(model.conversation.as_ref(), page));

    model.pending_snapshot = None;
    model.retry_timer = None;
    model.connection = UiConnectionState::Ready;
    model.last_failure = None;
    model.observation_mode = UiObservationMode::Materialized;
    model.apply_snapshot(view.snapshot);
    apply_guided_snapshot(model, effects)?;
    if let Some(page) = view.conversation {
        let row_id = page.row_id.clone();
        model.retain_conversation(revision, page);
        model.selected_row = Some(row_id.clone());
        model.desired_conversation = None;
        model.requested_conversation = Some(row_id.clone());
        let _ = model.install_retained_conversation(&row_id);
    } else if model
        .snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.inbox_rows.is_empty())
    {
        model.selected_row = None;
        model.desired_conversation = None;
        model.requested_conversation = None;
        model.close_conversation();
    } else {
        model.request_inbox_preview(effects);
    }
    reconcile_pending_mailbox_view(model, revision, effects)?;
    if model
        .required_revision
        .is_some_and(|required| revision >= required)
    {
        model.required_revision = None;
    }
    apply_completion_context(model);
    if agent_finished {
        open_automatic_followup_draft(model, effects)?;
    }
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn interactions_observed(
    model: &mut UiModel,
    interactions: Vec<UiInteraction>,
    effects: &mut Vec<UiEffect>,
) {
    model.interactions = interactions.into();
    match model.interaction_modal.take() {
        Some(UiInteractionModal::Submitting { interaction }) => {
            model.interaction_modal = Some(UiInteractionModal::Submitting { interaction });
        }
        Some(UiInteractionModal::Prompt {
            interaction,
            selected,
            text,
        }) => {
            if let Some(current) = model
                .interactions
                .iter()
                .find(|candidate| candidate.request_id == interaction.request_id)
                .cloned()
            {
                model.interaction_modal = Some(UiInteractionModal::Prompt {
                    selected: selected.min(current.choices.len().saturating_sub(1)),
                    interaction: current,
                    text,
                });
            } else {
                show_next_interaction(model);
            }
        }
        None => show_next_interaction(model),
    }
    effects.push(UiEffect::RequestRedraw);
}

fn show_next_interaction(model: &mut UiModel) {
    model.interaction_modal =
        model
            .interactions
            .front()
            .cloned()
            .map(|interaction| UiInteractionModal::Prompt {
                interaction,
                selected: 0,
                text: String::new(),
            });
}

fn interaction_answered(
    model: &mut UiModel,
    effect_id: EffectId,
    outcome: UiInteractionAnswerOutcome,
    effects: &mut Vec<UiEffect>,
) {
    if model.pending_interaction != Some(effect_id) {
        return;
    }
    model.pending_interaction = None;
    let request_id = match model.interaction_modal.take() {
        Some(UiInteractionModal::Submitting { interaction }) => Some(interaction.request_id),
        other => {
            model.interaction_modal = other;
            None
        }
    };
    if let Some(request_id) = request_id {
        model
            .interactions
            .retain(|interaction| interaction.request_id != request_id);
    }
    if outcome == UiInteractionAnswerOutcome::Stale {
        model.last_failure = Some(UiFailure {
            code: "interaction_already_resolved".to_owned(),
            action:
                "review the next request; another responder or the agent already ended this one"
                    .to_owned(),
        });
    }
    show_next_interaction(model);
    effects.push(UiEffect::RequestRedraw);
}

fn interaction_answer_failed(
    model: &mut UiModel,
    effect_id: EffectId,
    failure: UiFailure,
    effects: &mut Vec<UiEffect>,
) {
    if model.pending_interaction != Some(effect_id) {
        return;
    }
    model.pending_interaction = None;
    if let Some(UiInteractionModal::Submitting { interaction }) = model.interaction_modal.take() {
        model.interaction_modal = Some(UiInteractionModal::Prompt {
            interaction,
            selected: 0,
            text: String::new(),
        });
    }
    model.last_failure = Some(failure);
    effects.push(UiEffect::RequestRedraw);
}

fn reconcile_pending_mailbox_view(
    model: &mut UiModel,
    revision: u64,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    let Some(pending) = model.pending_mailbox.as_ref() else {
        return Ok(());
    };
    let PendingMailboxKind::SubmitCommand(submission) = &pending.kind else {
        return Ok(());
    };
    let Some(draft) = submission.draft.as_ref() else {
        return Ok(());
    };
    let effect_id = pending.id;
    let draft = draft.clone();
    let optimistic_entry = submission.optimistic_entry.clone();
    let expected_message = draft.draft_id;
    let canonical_entry = model.conversation.as_ref().and_then(|conversation| {
        conversation.entries.iter().find(|entry| {
            entry
                .message_target
                .is_some_and(|target| target.message_id == expected_message)
        })
    });
    if canonical_entry.is_some() {
        return mailbox_command_committed(
            model,
            effect_id,
            revision,
            Some(expected_message),
            effects,
        );
    }
    if optimistic_entry.as_deref().is_some_and(|entry_id| {
        model.conversation.as_ref().is_some_and(|conversation| {
            conversation
                .entries
                .iter()
                .any(|entry| entry.id == entry_id)
        })
    }) {
        return Ok(());
    }
    let _ = append_pending_message(model, &draft, effect_id);
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
    if model.help_page.is_some() {
        let changed = apply_help_input(model, input, effects);
        if changed {
            effects.push(UiEffect::RequestRedraw);
        }
        return Ok(());
    }
    if let Some(changed) = apply_open_modal_input(model, input, effects)? {
        if changed || dismissed_completion {
            effects.push(UiEffect::RequestRedraw);
        }
        return Ok(());
    }
    if let Some(changed) = apply_project_workspace_input(model, input, effects)? {
        if changed || dismissed_completion {
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
                UiFocus::Draft => UiFocus::Draft,
            };
            true
        }
        UiInput::PreviousFocus => {
            model.focus = match model.focus {
                UiFocus::Navigation if model.conversation.is_some() => UiFocus::Conversation,
                UiFocus::Navigation | UiFocus::Conversation => UiFocus::Content,
                UiFocus::Content => UiFocus::Navigation,
                UiFocus::Draft => UiFocus::Draft,
            };
            true
        }
        UiInput::NextSection => {
            if model.viewport.width >= WIDE_WIDTH && model.focus == UiFocus::Navigation {
                model.focus = UiFocus::Content;
            } else if model.viewport.width < WIDE_WIDTH {
                model.change_section(model.section.next());
            } else {
                return Ok(());
            }
            true
        }
        UiInput::MoveCursorRight => match model.focus {
            UiFocus::Navigation => {
                if model.viewport.width >= WIDE_WIDTH {
                    model.focus = UiFocus::Content;
                } else {
                    model.change_section(model.section.next());
                }
                true
            }
            UiFocus::Content if model.conversation.is_some() => {
                model.focus = UiFocus::Conversation;
                model.follow_conversation_tail();
                true
            }
            UiFocus::Content | UiFocus::Conversation | UiFocus::Draft => false,
        },
        UiInput::PreviousSection => {
            if model.viewport.width >= WIDE_WIDTH
                && matches!(model.focus, UiFocus::Content | UiFocus::Conversation)
            {
                model.focus = UiFocus::Navigation;
            } else if model.viewport.width < WIDE_WIDTH {
                model.change_section(model.section.previous());
            } else {
                return Ok(());
            }
            true
        }
        UiInput::MoveCursorLeft => match model.focus {
            UiFocus::Conversation => {
                if model.technical_visible {
                    model.close_technical_details();
                } else {
                    model.focus = UiFocus::Content;
                }
                true
            }
            UiFocus::Content => {
                model.focus = UiFocus::Navigation;
                true
            }
            UiFocus::Navigation => {
                if model.viewport.width < WIDE_WIDTH {
                    model.change_section(model.section.previous());
                    true
                } else {
                    false
                }
            }
            UiFocus::Draft => false,
        },
        UiInput::NextItem => match model.focus {
            UiFocus::Conversation if model.technical_visible => {
                model.scroll_technical_details(true)
            }
            UiFocus::Conversation => model.scroll_conversation_viewport(true),
            UiFocus::Navigation if model.viewport.width >= WIDE_WIDTH => {
                model.change_section(model.section.next());
                true
            }
            UiFocus::Navigation | UiFocus::Content => model.move_row_selection(true),
            UiFocus::Draft => false,
        },
        UiInput::PreviousItem => match model.focus {
            UiFocus::Conversation if model.technical_visible => {
                model.scroll_technical_details(false)
            }
            UiFocus::Conversation => model.scroll_conversation_viewport(false),
            UiFocus::Navigation if model.viewport.width >= WIDE_WIDTH => {
                model.change_section(model.section.previous());
                true
            }
            UiFocus::Navigation | UiFocus::Content => model.move_row_selection(false),
            UiFocus::Draft => false,
        },
        UiInput::Activate => activate(model, effects)?,
        UiInput::LoadMore => load_more(model, effects)?,
        UiInput::Escape => escape(model),
        UiInput::Character('?') => {
            model.help_page = Some(UiHelpPage::Context);
            true
        }
        UiInput::Character(character) => mailbox_shortcut(model, *character, effects)?,
        UiInput::MoveCursorHome if model.focus == UiFocus::Conversation => {
            model.reveal_conversation_entry_start()
        }
        UiInput::MoveCursorEnd if model.focus == UiFocus::Conversation => {
            model.reveal_conversation_entry_end()
        }
        UiInput::Paste(_)
        | UiInput::InsertNewline
        | UiInput::Help
        | UiInput::Refresh
        | UiInput::Backspace
        | UiInput::MoveCursorHome
        | UiInput::MoveCursorEnd
        | UiInput::Delete => false,
    };
    if changed {
        model.request_inbox_preview(effects);
    }
    if changed || dismissed_transient_help || dismissed_completion {
        effects.push(UiEffect::RequestRedraw);
    }
    Ok(())
}

fn apply_project_workspace_input(
    model: &mut UiModel,
    input: &UiInput,
    effects: &mut Vec<UiEffect>,
) -> Result<Option<bool>, UiError> {
    if model.section == UiSection::Inbox
        && model.project_filter.is_some()
        && model.focus != UiFocus::Conversation
        && matches!(input, UiInput::Escape)
    {
        model.clear_project_filter();
        return Ok(Some(true));
    }
    let project_content_focused = model.focus == UiFocus::Content
        || (model.viewport.width < WIDE_WIDTH && model.focus == UiFocus::Navigation);
    if model.section != UiSection::Projects || !project_content_focused {
        return Ok(None);
    }
    match (model.project_workspace_level, input) {
        (UiProjectWorkspaceLevel::List, UiInput::MoveCursorRight | UiInput::Character('l')) => {
            if model.project_summary.is_none() {
                Ok(Some(false))
            } else {
                model.project_workspace_level = UiProjectWorkspaceLevel::Summary;
                model.project_summary_focus = UiProjectSummaryFocus::Conversation;
                model.focus = UiFocus::Content;
                Ok(Some(true))
            }
        }
        (
            UiProjectWorkspaceLevel::Summary
            | UiProjectWorkspaceLevel::Manage
            | UiProjectWorkspaceLevel::Folders,
            UiInput::MoveCursorLeft | UiInput::Character('h') | UiInput::Escape,
        ) => {
            model.project_workspace_level = match model.project_workspace_level {
                UiProjectWorkspaceLevel::Folders => UiProjectWorkspaceLevel::Manage,
                UiProjectWorkspaceLevel::Manage => UiProjectWorkspaceLevel::Summary,
                UiProjectWorkspaceLevel::Summary | UiProjectWorkspaceLevel::List => {
                    UiProjectWorkspaceLevel::List
                }
            };
            Ok(Some(true))
        }
        (UiProjectWorkspaceLevel::Summary, UiInput::NextItem | UiInput::PreviousItem) => {
            let forward = matches!(input, UiInput::NextItem);
            Ok(Some(move_project_summary_focus(model, forward)))
        }
        (UiProjectWorkspaceLevel::List | UiProjectWorkspaceLevel::Summary, UiInput::Activate)
            if model.project_workspace_level == UiProjectWorkspaceLevel::List
                || model.project_summary_focus == UiProjectSummaryFocus::Conversation =>
        {
            open_selected_project_conversations(model, effects).map(Some)
        }
        (UiProjectWorkspaceLevel::Summary, UiInput::Activate)
            if model.project_summary_focus == UiProjectSummaryFocus::Manage =>
        {
            model.project_workspace_level = UiProjectWorkspaceLevel::Manage;
            model.project_management_action = selected_project(model)
                .and_then(|project| project_management_actions(project).first().copied());
            Ok(Some(true))
        }
        (UiProjectWorkspaceLevel::Summary, UiInput::Activate)
            if model.project_summary_focus == UiProjectSummaryFocus::Folders =>
        {
            enter_project_folders(model);
            Ok(Some(true))
        }
        (UiProjectWorkspaceLevel::Summary, UiInput::Activate)
            if model.project_summary_focus == UiProjectSummaryFocus::AssignedAgent =>
        {
            Ok(Some(open_project_agent(model)))
        }
        (UiProjectWorkspaceLevel::Summary, UiInput::Activate)
            if model.project_summary_focus == UiProjectSummaryFocus::Recovery =>
        {
            model.project_workspace_level = UiProjectWorkspaceLevel::Manage;
            model.project_management_action = selected_project(model)
                .and_then(|project| project_management_actions(project).first().copied());
            Ok(Some(true))
        }
        (UiProjectWorkspaceLevel::Manage, UiInput::NextItem | UiInput::PreviousItem) => Ok(Some(
            move_project_management_action(model, matches!(input, UiInput::NextItem)),
        )),
        (UiProjectWorkspaceLevel::Manage, UiInput::Activate) => {
            activate_project_management_action(model, effects).map(Some)
        }
        (UiProjectWorkspaceLevel::Folders, UiInput::NextItem | UiInput::PreviousItem) => Ok(Some(
            move_project_folder_action(model, matches!(input, UiInput::NextItem)),
        )),
        (UiProjectWorkspaceLevel::Folders, UiInput::NextFocus | UiInput::PreviousFocus) => {
            Ok(Some(move_project_folder(
                model,
                matches!(input, UiInput::NextFocus),
            )))
        }
        (UiProjectWorkspaceLevel::Folders, UiInput::Activate) => {
            activate_project_folder_action(model, effects).map(Some)
        }
        _ => Ok(None),
    }
}

fn project_management_actions(project: &UiProject) -> Vec<UiProjectManagementAction> {
    let mut actions = vec![UiProjectManagementAction::Folders];
    if project.lifecycle == "open" && !project.archived {
        actions.push(if project.assignment.is_some() {
            UiProjectManagementAction::ChangeAssignedAgent
        } else {
            UiProjectManagementAction::AssignAgent
        });
        actions.push(UiProjectManagementAction::CloseProject);
        actions.push(UiProjectManagementAction::ArchiveProject);
    } else if project.archived {
        actions.push(UiProjectManagementAction::RestoreArchivedProject);
    } else if project.lifecycle == "closed" {
        actions.push(UiProjectManagementAction::ReopenProject);
        actions.push(UiProjectManagementAction::ArchiveProject);
    }
    actions.push(UiProjectManagementAction::TechnicalDetails);
    actions
}

fn move_project_management_action(model: &mut UiModel, forward: bool) -> bool {
    let Some(project) = selected_project(model) else {
        return false;
    };
    let actions = project_management_actions(project);
    move_selected_value(&actions, &mut model.project_management_action, forward)
}

fn activate_project_management_action(
    model: &mut UiModel,
    effects: &mut Vec<UiEffect>,
) -> Result<bool, UiError> {
    let Some(project) = selected_project(model).cloned() else {
        return Ok(false);
    };
    let Some(action) = model
        .project_management_action
        .filter(|action| project_management_actions(&project).contains(action))
    else {
        return Ok(false);
    };
    match action {
        UiProjectManagementAction::Folders => enter_project_folders(model),
        UiProjectManagementAction::AssignAgent => open_project_activation(model, project, false),
        UiProjectManagementAction::ChangeAssignedAgent => {
            open_project_activation(model, project, true);
        }
        UiProjectManagementAction::CloseProject => {
            model.submit_project(
                UiProjectAction::PreviewClose {
                    project_id: project.project_id,
                },
                effects,
            )?;
        }
        UiProjectManagementAction::ReopenProject => {
            model.submit_project(
                UiProjectAction::Open {
                    project_id: project.project_id,
                },
                effects,
            )?;
        }
        UiProjectManagementAction::ArchiveProject
        | UiProjectManagementAction::RestoreArchivedProject => {
            model.project_interaction = Some(UiProjectInteraction::ConfirmArchive {
                archived: matches!(action, UiProjectManagementAction::ArchiveProject),
                project,
                submitting: false,
            });
        }
        UiProjectManagementAction::TechnicalDetails => {
            model.help_page = Some(UiHelpPage::Technical);
        }
    }
    Ok(true)
}

fn enter_project_folders(model: &mut UiModel) {
    model.project_workspace_level = UiProjectWorkspaceLevel::Folders;
    model.project_folder_id = model
        .project_summary
        .as_ref()
        .and_then(|summary| {
            model.project_folder_id.filter(|folder_id| {
                summary
                    .folders
                    .iter()
                    .any(|folder| folder.folder_id == *folder_id)
            })
        })
        .or_else(|| {
            model
                .project_summary
                .as_ref()
                .and_then(|summary| summary.folders.first().map(|folder| folder.folder_id))
        });
    model.project_folder_action = project_folder_actions(model)
        .first()
        .copied()
        .unwrap_or(UiProjectFolderAction::AddFolder);
}

fn project_folder_actions(model: &UiModel) -> Vec<UiProjectFolderAction> {
    let Some(project) = selected_project(model) else {
        return Vec::new();
    };
    if project.lifecycle != "open" || project.archived {
        return Vec::new();
    }
    let mut actions = vec![UiProjectFolderAction::AddFolder];
    if let Some(folder_id) = model.project_folder_id
        && let Some(folder) = project
            .resources
            .iter()
            .find(|folder| folder.resource_id == folder_id)
    {
        actions.extend([
            UiProjectFolderAction::ChangeFolderPath,
            UiProjectFolderAction::RemoveFolder,
        ]);
        if !folder.primary {
            actions.push(UiProjectFolderAction::UseAsWorkingFolder);
        }
        actions.push(UiProjectFolderAction::CheckFolderNow);
    }
    if project.resources.len() > 1 {
        actions.push(UiProjectFolderAction::CheckAllFolders);
    }
    actions
}

fn move_project_folder_action(model: &mut UiModel, forward: bool) -> bool {
    let actions = project_folder_actions(model);
    let mut selected = Some(model.project_folder_action);
    let changed = move_selected_value(&actions, &mut selected, forward);
    if let Some(selected) = selected {
        model.project_folder_action = selected;
    }
    changed
}

fn move_project_folder(model: &mut UiModel, forward: bool) -> bool {
    let Some(summary) = &model.project_summary else {
        return false;
    };
    let folders = summary
        .folders
        .iter()
        .map(|folder| folder.folder_id)
        .collect::<Vec<_>>();
    let changed = move_selected_value(&folders, &mut model.project_folder_id, forward);
    if changed {
        model.project_folder_action = project_folder_actions(model)
            .first()
            .copied()
            .unwrap_or(UiProjectFolderAction::AddFolder);
    }
    changed
}

fn activate_project_folder_action(
    model: &mut UiModel,
    effects: &mut Vec<UiEffect>,
) -> Result<bool, UiError> {
    let Some(project) = selected_project(model).cloned() else {
        return Ok(false);
    };
    if !project_folder_actions(model).contains(&model.project_folder_action) {
        return Ok(false);
    }
    let folder_id = model.project_folder_id;
    match model.project_folder_action {
        UiProjectFolderAction::AddFolder => {
            model.project_interaction = Some(UiProjectInteraction::AddResource {
                project,
                path: String::new(),
                make_primary: false,
                submitting: false,
            });
        }
        UiProjectFolderAction::ChangeFolderPath => {
            let Some(resource_id) = folder_id else {
                return Ok(false);
            };
            model.project_interaction = Some(UiProjectInteraction::ReplaceResource {
                project,
                resource_id,
                path: String::new(),
                submitting: false,
            });
        }
        UiProjectFolderAction::RemoveFolder => {
            let Some(resource_id) = folder_id else {
                return Ok(false);
            };
            model.project_interaction = Some(UiProjectInteraction::ConfirmRemoveResource {
                project,
                resource_id,
                force: false,
                submitting: false,
            });
        }
        UiProjectFolderAction::UseAsWorkingFolder => {
            let Some(resource_id) = folder_id else {
                return Ok(false);
            };
            model.submit_project(
                UiProjectAction::SetPrimaryResource {
                    project_id: project.project_id,
                    resource_id,
                },
                effects,
            )?;
        }
        UiProjectFolderAction::CheckFolderNow => {
            let Some(resource_id) = folder_id else {
                return Ok(false);
            };
            model.submit_project(
                UiProjectAction::CheckResources {
                    project_id: project.project_id,
                    resource_id: Some(resource_id),
                },
                effects,
            )?;
        }
        UiProjectFolderAction::CheckAllFolders => {
            model.submit_project(
                UiProjectAction::CheckResources {
                    project_id: project.project_id,
                    resource_id: None,
                },
                effects,
            )?;
        }
    }
    Ok(true)
}

fn move_selected_value<T: Copy + Eq>(
    values: &[T],
    selected: &mut Option<T>,
    forward: bool,
) -> bool {
    if values.is_empty() {
        return false;
    }
    let current = selected
        .and_then(|selected| values.iter().position(|value| *value == selected))
        .unwrap_or(0);
    let next = if forward {
        (current + 1).min(values.len() - 1)
    } else {
        current.saturating_sub(1)
    };
    let next = values[next];
    if *selected == Some(next) {
        false
    } else {
        *selected = Some(next);
        true
    }
}

fn open_project_agent(model: &mut UiModel) -> bool {
    let Some(agent_id) = model
        .project_summary
        .as_ref()
        .and_then(|summary| summary.assigned_agent.agent_id)
    else {
        return false;
    };
    let Some(agent) = model.snapshot.as_ref().and_then(|snapshot| {
        snapshot
            .agents
            .iter()
            .find(|agent| agent.agent_id == agent_id)
            .cloned()
    }) else {
        return false;
    };
    model.change_section(UiSection::Agents);
    model.selected_row = Some(agent_hex(agent_id));
    model.agent_modal = Some(UiAgentModal::Details {
        selected_session: default_agent_session(&agent),
        agent,
    });
    true
}

fn move_project_summary_focus(model: &mut UiModel, forward: bool) -> bool {
    let Some(summary) = &model.project_summary else {
        return false;
    };
    let available = UiProjectSummaryFocus::ALL
        .into_iter()
        .filter(|focus| *focus != UiProjectSummaryFocus::Recovery || !summary.recovery.is_empty())
        .collect::<Vec<_>>();
    let current = available
        .iter()
        .position(|focus| *focus == model.project_summary_focus)
        .unwrap_or(0);
    let next = if forward {
        (current + 1).min(available.len().saturating_sub(1))
    } else {
        current.saturating_sub(1)
    };
    let selected = available[next];
    if selected == model.project_summary_focus {
        false
    } else {
        model.project_summary_focus = selected;
        true
    }
}

fn open_selected_project_conversations(
    model: &mut UiModel,
    effects: &mut Vec<UiEffect>,
) -> Result<bool, UiError> {
    let Some(project) = selected_project(model).cloned() else {
        return Ok(false);
    };
    let conversations = model
        .project_summary
        .as_ref()
        .filter(|summary| summary.project_id == project.project_id)
        .map_or_else(
            || {
                model.snapshot.as_ref().map_or(
                    UiProjectConversationSummary {
                        open: 0,
                        archived: 0,
                        open_rows: Vec::new(),
                        archived_rows: Vec::new(),
                    },
                    |snapshot| project_conversation_summary(snapshot, project.project_id),
                )
            },
            |summary| summary.conversations.clone(),
        );
    model.install_project_filter(&project);
    model.change_section(UiSection::Inbox);
    match conversations.open_rows.as_slice() {
        [] if project.lifecycle == "open" && !project.archived && project.claimable => {
            model.open_draft(
                UiMailboxDraftTarget::Project {
                    project_id: project.project_id,
                    thread_id: None,
                },
                effects,
            )?;
        }
        [row_id] => {
            model.selected_row = Some(row_id.clone());
            model.desired_conversation = Some(row_id.clone());
            model.request_inbox_preview(effects);
        }
        [first, _, ..] => {
            model.selected_row = Some(first.clone());
            model.close_conversation();
            model.focus = UiFocus::Content;
        }
        [] => {
            model.selected_row = conversations.archived_rows.first().cloned();
            model.close_conversation();
            model.focus = UiFocus::Content;
        }
    }
    Ok(true)
}

fn normalize_vim_navigation(model: &UiModel, input: &UiInput) -> UiInput {
    if text_input_is_active(model) {
        return input.clone();
    }
    match input {
        UiInput::Character('j' | 'k')
            if model.focus == UiFocus::Conversation && model.interaction_modal.is_none() =>
        {
            input.clone()
        }
        UiInput::Character('j') => UiInput::NextItem,
        UiInput::Character('k') => UiInput::PreviousItem,
        _ => input.clone(),
    }
}

fn text_input_is_active(model: &UiModel) -> bool {
    if matches!(
        model.interaction_modal,
        Some(UiInteractionModal::Prompt {
            ref interaction,
            ..
        }) if interaction.allow_text
    ) {
        return true;
    }
    if model.new_modal.is_some() {
        return false;
    }
    if let Some(modal) = &model.project_interaction {
        return match modal {
            UiProjectInteraction::Search { .. }
            | UiProjectInteraction::CreateExisting { .. }
            | UiProjectInteraction::CreateWorktree { .. }
            | UiProjectInteraction::ReplaceResource { .. } => true,
            UiProjectInteraction::AddResource { .. } => {
                model.form.focused == Some(UiFormField::Project(UiProjectFormField::Path))
            }
            UiProjectInteraction::Activate { field, .. }
            | UiProjectInteraction::Handoff { field, .. } => {
                *field == UiProjectFormField::Directory
            }
            UiProjectInteraction::ChooseCreation { .. }
            | UiProjectInteraction::ConfirmRemoveResource { .. }
            | UiProjectInteraction::ConfirmClose { .. }
            | UiProjectInteraction::ConfirmArchive { .. }
            | UiProjectInteraction::Outcome { .. } => false,
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
    matches!(
        model.mailbox_draft,
        Some(UiMailboxDraftPane::Editing { .. })
    )
}

fn apply_open_modal_input(
    model: &mut UiModel,
    input: &UiInput,
    effects: &mut Vec<UiEffect>,
) -> Result<Option<bool>, UiError> {
    if model.interaction_modal.is_some() {
        return apply_interaction_modal_input(model, input.clone(), effects).map(Some);
    }
    if model.new_modal.is_some() {
        return apply_new_modal_input(model, input.clone(), effects).map(Some);
    }
    if model.project_interaction.is_some() {
        return apply_project_interaction_input(model, input.clone(), effects).map(Some);
    }
    if model.agent_modal.is_some() {
        return apply_agent_modal_input(model, input.clone(), effects).map(Some);
    }
    if model.mailbox_modal.is_some() {
        return apply_mailbox_modal_input(model, input.clone(), effects).map(Some);
    }
    if model.mailbox_draft.is_some() {
        return apply_draft_input(model, input.clone(), effects).map(Some);
    }
    Ok(None)
}

fn apply_interaction_modal_input(
    model: &mut UiModel,
    input: UiInput,
    effects: &mut Vec<UiEffect>,
) -> Result<bool, UiError> {
    let Some(interaction_state) = model.interaction_modal.take() else {
        return Ok(false);
    };
    let UiInteractionModal::Prompt {
        interaction,
        mut selected,
        mut text,
    } = interaction_state
    else {
        model.interaction_modal = Some(interaction_state);
        return Ok(false);
    };
    let response = match input {
        UiInput::Escape => Some(UiInteractionResponse::Cancelled),
        UiInput::NextItem if !interaction.choices.is_empty() => {
            selected = (selected + 1) % interaction.choices.len();
            None
        }
        UiInput::PreviousItem if !interaction.choices.is_empty() => {
            selected = (selected + interaction.choices.len() - 1) % interaction.choices.len();
            None
        }
        UiInput::Activate if interaction.allow_text && !text.trim().is_empty() => {
            Some(UiInteractionResponse::Text(text.clone()))
        }
        UiInput::Activate => interaction
            .choices
            .get(selected)
            .map(|choice| UiInteractionResponse::Choice(choice.value.clone())),
        UiInput::Character(value) if interaction.allow_text => {
            if text.len().saturating_add(value.len_utf8()) <= MAX_DRAFT_BYTES {
                text.push(value);
            }
            None
        }
        UiInput::Paste(value) if interaction.allow_text => {
            for value in value.chars() {
                if text.len().saturating_add(value.len_utf8()) > MAX_DRAFT_BYTES {
                    break;
                }
                text.push(value);
            }
            None
        }
        UiInput::Backspace if interaction.allow_text => {
            text.pop();
            None
        }
        _ => None,
    };
    if let Some(response) = response {
        let id = model.allocate_effect()?;
        model.pending_interaction = Some(id);
        model.interaction_modal = Some(UiInteractionModal::Submitting {
            interaction: interaction.clone(),
        });
        effects.push(UiEffect::AnswerInteraction {
            id,
            interaction,
            response,
        });
        model.follow_conversation_tail();
    } else {
        model.interaction_modal = Some(UiInteractionModal::Prompt {
            interaction,
            selected,
            text,
        });
    }
    Ok(true)
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
                model.change_section(UiSection::Projects);
                model.focus = UiFocus::Content;
                model.project_interaction = Some(UiProjectInteraction::ChooseCreation {
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
                open_guided_project(model, project, effects)?;
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
                open_guided_agent(model, project, agent, effects)?;
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
                continue_guided_provider(model, project, agent, provider, effects)?;
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
            if let Some(thread) = guided_thread(&project, agent.agent_id).cloned() {
                submit_guided_project(
                    model,
                    project,
                    &agent,
                    provider,
                    thread.thread_id,
                    Some(thread.session),
                    effects,
                )?;
            } else {
                open_guided_instruction(model, &project, &agent, provider, effects)?;
            }
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
                model.project_workspace_level = UiProjectWorkspaceLevel::Summary;
                model.project_summary_focus = UiProjectSummaryFocus::Conversation;
                model.refresh_selected_project_summary();
            }
            Ok(true)
        }
        UiNewModal::ProjectUnavailable { project, .. } if matches!(input, UiInput::Activate) => {
            model.new_modal = None;
            model.change_section(UiSection::Projects);
            model.selected_row = Some(agent_hex(project.project_id));
            model.project_workspace_level = UiProjectWorkspaceLevel::Summary;
            model.project_summary_focus = UiProjectSummaryFocus::Conversation;
            model.refresh_selected_project_summary();
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

fn open_guided_project(
    model: &mut UiModel,
    project: UiProject,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
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
        return Ok(());
    }
    if let Some(assignment) = &project.assignment
        && assignment.runnable
    {
        open_project_inbox_draft(model, &project, effects)?;
        return Ok(());
    }
    model.new_modal = Some(guided_agent_picker(model, project));
    Ok(())
}

fn open_project_inbox_draft(
    model: &mut UiModel,
    project: &UiProject,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    let thread_id = project
        .assignment
        .as_ref()
        .filter(|assignment| assignment.runnable)
        .and_then(|assignment| assignment.thread_id);
    model.guided_pending = None;
    model.new_modal = None;
    model.project_interaction = None;
    if let Some(thread_id) = thread_id {
        select_project_conversation(model, project.project_id, thread_id);
    }
    model.open_draft(
        UiMailboxDraftTarget::Project {
            project_id: project.project_id,
            thread_id,
        },
        effects,
    )
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

fn open_guided_agent(
    model: &mut UiModel,
    project: UiProject,
    agent: UiAgent,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
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
        return Ok(());
    }
    if project
        .assignment
        .as_ref()
        .is_some_and(|assignment| assignment.agent_id == agent.agent_id && assignment.runnable)
    {
        open_project_inbox_draft(model, &project, effects)?;
        return Ok(());
    }
    let historical = guided_thread(&project, agent.agent_id).cloned();
    let provider = historical
        .as_ref()
        .map(|thread| thread.provider.clone())
        .or_else(|| default_provider_choice_from_model(model))
        .unwrap_or_default();
    let available = model.snapshot.as_ref().map_or(0, |snapshot| {
        snapshot
            .providers
            .iter()
            .filter(|provider| provider.available)
            .count()
    });
    if historical.is_some() || available == 1 {
        continue_guided_provider(model, project, agent, provider, effects)
    } else {
        model.new_modal = Some(guided_provider_picker(model, project, agent, provider));
        Ok(())
    }
}

fn continue_guided_provider(
    model: &mut UiModel,
    project: UiProject,
    agent: UiAgent,
    provider: String,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    let historical = guided_thread(&project, agent.agent_id).cloned();
    let moves_project = project
        .assignment
        .as_ref()
        .is_some_and(|assignment| assignment.agent_id != agent.agent_id);
    if moves_project {
        model.new_modal = Some(UiNewModal::ReviewProject {
            project,
            agent,
            provider,
            resumes_existing: historical.is_some(),
            moves_project: true,
            submitting: false,
        });
        return Ok(());
    }
    if let Some(thread) = historical {
        submit_guided_project(
            model,
            project,
            &agent,
            provider,
            thread.thread_id,
            Some(thread.session),
            effects,
        )
    } else {
        open_guided_instruction(model, &project, &agent, provider, effects)
    }
}

fn open_guided_instruction(
    model: &mut UiModel,
    project: &UiProject,
    agent: &UiAgent,
    provider: String,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    model.guided_pending = Some(UiGuidedPending::Instruction(UiGuidedSubmission {
        project_id: project.project_id,
        agent_id: agent.agent_id,
        provider,
    }));
    model.new_modal = None;
    model.project_interaction = None;
    model.open_draft(
        UiMailboxDraftTarget::Project {
            project_id: project.project_id,
            thread_id: None,
        },
        effects,
    )
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
    thread_id: [u8; 32],
    resume_session: Option<String>,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    let agent_name = agent
        .names
        .first()
        .cloned()
        .unwrap_or_else(|| "selected agent".to_owned());
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
        .map(|resource| resource.canonical_path.clone())
        .unwrap_or_default();
    if directory.is_empty() {
        model.last_failure = Some(UiFailure {
            code: "guided_project_has_no_folder".to_owned(),
            action: "add a project folder before starting agent work".to_owned(),
        });
        return Ok(());
    }
    let action = if let Some(current) = project.assignment.as_ref() {
        if current.thread_id.is_none() {
            model.last_failure = Some(UiFailure {
                code: "guided_handoff_not_ready".to_owned(),
                action: "inspect the current project setup before moving it to another agent"
                    .to_owned(),
            });
            return Ok(());
        }
        UiProjectAction::Handoff {
            project_id: project.project_id,
            agent_id: agent.agent_id,
            provider,
            resume_session,
            thread_id,
            launch_directory: directory,
            force_takeover: false,
        }
    } else {
        UiProjectAction::Activate {
            project_id: project.project_id,
            agent_id: agent.agent_id,
            provider,
            resume_session,
            resume_thread: Some(thread_id),
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
fn apply_mailbox_modal_input(
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
        model.mailbox_modal = None;
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
        None => Ok(false),
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

#[allow(clippy::needless_pass_by_value)]
fn apply_draft_input(
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
        if matches!(
            model.mailbox_draft,
            Some(UiMailboxDraftPane::Loading { .. })
        ) {
            model.pending_mailbox = None;
            finish_draft_close(model);
            return Ok(true);
        }
        if let Some(UiMailboxDraftPane::Editing { draft, dirty, .. }) = model.mailbox_draft.clone()
        {
            if dirty {
                model.mailbox_draft = Some(UiMailboxDraftPane::Editing {
                    draft,
                    dirty: true,
                    submitting: false,
                    closing: true,
                });
                model.autosave_timer = None;
                if model.pending_mailbox.is_none() {
                    model.save_draft(effects)?;
                }
            } else {
                finish_draft_close(model);
            }
        }
        return Ok(true);
    }

    match model.mailbox_draft.clone() {
        Some(UiMailboxDraftPane::Loading { .. }) | None => Ok(false),
        Some(UiMailboxDraftPane::Editing {
            mut draft,
            dirty,
            submitting,
            closing,
        }) => match input {
            UiInput::Character(_)
            | UiInput::Paste(_)
            | UiInput::InsertNewline
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
                    model.mailbox_draft = Some(UiMailboxDraftPane::Editing {
                        draft,
                        dirty: true,
                        submitting: true,
                        closing: false,
                    });
                    model.autosave_timer = None;
                    model.save_draft(effects)?;
                } else {
                    let action = draft_action(&draft.target);
                    model.mailbox_draft = Some(UiMailboxDraftPane::Editing {
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
    }
}

fn finish_draft_close(model: &mut UiModel) {
    model.mailbox_draft = None;
    model.focus = UiFocus::Conversation;
    model.follow_conversation_tail();
    let Some(UiGuidedPending::Instruction(submission)) = model.guided_pending.take() else {
        return;
    };
    if let Some(project) = model.snapshot.as_ref().and_then(|snapshot| {
        snapshot
            .projects
            .iter()
            .find(|project| project.project_id == submission.project_id)
            .cloned()
    }) {
        model.new_modal = Some(guided_agent_picker(model, project));
    }
}

fn update_composer(
    model: &mut UiModel,
    draft: UiMailboxDraft,
    dirty: bool,
    submitting: bool,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    model.mailbox_draft = Some(UiMailboxDraftPane::Editing {
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
fn apply_project_interaction_input(
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
            if let Some(UiProjectInteraction::Outcome {
                result:
                    UiProjectResult {
                        action: UiProjectAction::PreviewCreateExisting { name, brief, path },
                        outcome: UiProjectOutcome::ResourcePreview { .. },
                        ..
                    },
            }) = model.project_interaction.clone()
            {
                model.project_interaction = Some(UiProjectInteraction::CreateExisting {
                    name,
                    brief: brief.unwrap_or_default(),
                    path,
                    field: UiProjectFormField::Path,
                    submitting: false,
                });
                model.last_failure = None;
                return Ok(true);
            }
            if let Some(UiProjectInteraction::Search { query }) = &model.project_interaction {
                model.project_search.clone_from(query);
            }
            if matches!(model.guided_pending, Some(UiGuidedPending::ProjectCreation)) {
                model.guided_pending = None;
                model.new_modal = Some(UiNewModal::Launcher {
                    selected: UiNewChoice::ProjectWork,
                });
            }
            model.project_interaction = None;
            return Ok(true);
        }
        return Ok(false);
    }

    match model.project_interaction.clone() {
        Some(UiProjectInteraction::ChooseCreation { mut selected }) => match input {
            UiInput::NextItem | UiInput::PreviousItem => {
                selected = match selected {
                    UiProjectCreationChoice::ExistingFolder => {
                        UiProjectCreationChoice::IsolatedWorktree
                    }
                    UiProjectCreationChoice::IsolatedWorktree => {
                        UiProjectCreationChoice::ExistingFolder
                    }
                };
                model.project_interaction = Some(UiProjectInteraction::ChooseCreation { selected });
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
        Some(UiProjectInteraction::Search { mut query }) => match input {
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
                if selected_project(model).is_none() {
                    return Ok(false);
                }
                model.project_interaction = None;
                model.project_workspace_level = UiProjectWorkspaceLevel::Summary;
                model.project_summary_focus = UiProjectSummaryFocus::Conversation;
                model.focus = UiFocus::Content;
                Ok(true)
            }
            _ => Ok(false),
        },
        Some(
            UiProjectInteraction::CreateExisting { submitting, .. }
            | UiProjectInteraction::CreateWorktree { submitting, .. }
            | UiProjectInteraction::AddResource { submitting, .. }
            | UiProjectInteraction::ReplaceResource { submitting, .. },
        ) => {
            if submitting {
                return Ok(false);
            }
            match input {
                UiInput::NextFocus | UiInput::PreviousFocus => {
                    if matches!(
                        model.project_interaction,
                        Some(UiProjectInteraction::AddResource { .. })
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
                        model.project_interaction,
                        Some(UiProjectInteraction::AddResource { .. })
                    ) && model.form.focused
                        == Some(UiFormField::Project(UiProjectFormField::Primary)) =>
                {
                    if let Some(UiProjectInteraction::AddResource { make_primary, .. }) =
                        &mut model.project_interaction
                    {
                        *make_primary = !*make_primary;
                    }
                    Ok(true)
                }
                UiInput::Activate => submit_project_interaction(model, effects),
                _ => Ok(edit_project_field(model, &input)),
            }
        }
        Some(UiProjectInteraction::ConfirmRemoveResource {
            project,
            resource_id,
            mut force,
            submitting,
        }) => match input {
            UiInput::Character(value) if value.eq_ignore_ascii_case(&'f') && !submitting => {
                force = !force;
                model.project_interaction = Some(UiProjectInteraction::ConfirmRemoveResource {
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
                model.project_interaction = Some(UiProjectInteraction::ConfirmRemoveResource {
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
        Some(UiProjectInteraction::ConfirmClose {
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
                model.project_interaction = Some(UiProjectInteraction::ConfirmClose {
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
                model.project_interaction = Some(UiProjectInteraction::ConfirmClose {
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
                model.project_interaction = Some(UiProjectInteraction::ConfirmClose {
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
                model.project_interaction = Some(UiProjectInteraction::ConfirmClose {
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
        Some(UiProjectInteraction::ConfirmArchive {
            project,
            archived,
            submitting,
        }) if matches!(input, UiInput::Activate) && !submitting => {
            model.project_interaction = Some(UiProjectInteraction::ConfirmArchive {
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
            UiProjectInteraction::Activate { submitting, .. }
            | UiProjectInteraction::Handoff { submitting, .. },
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
                UiInput::Activate => submit_project_interaction(model, effects),
                _ => Ok(edit_project_field(model, &input)),
            }
        }
        Some(UiProjectInteraction::Outcome { result }) => {
            submit_project_preview(model, &result, &input, effects)
        }
        Some(UiProjectInteraction::ConfirmArchive { .. }) | None => Ok(false),
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
    if model.project_interaction.is_none()
        && model.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .projects
                .iter()
                .any(|project| project.project_id == project_id)
        })
    {
        model.change_section(UiSection::Projects);
        model.selected_row = Some(agent_hex(project_id));
        model.project_workspace_level = UiProjectWorkspaceLevel::Summary;
        model.project_summary_focus = UiProjectSummaryFocus::Conversation;
        model.refresh_selected_project_summary();
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
    model.project_interaction = Some(if handoff {
        UiProjectInteraction::Handoff {
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
        UiProjectInteraction::Activate {
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
    let is_handoff = matches!(
        &model.project_interaction,
        Some(UiProjectInteraction::Handoff { .. })
    );
    let Some(
        UiProjectInteraction::Activate {
            field, new_session, ..
        }
        | UiProjectInteraction::Handoff {
            field, new_session, ..
        },
    ) = &mut model.project_interaction
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
    let field = match &model.project_interaction {
        Some(
            UiProjectInteraction::Activate { field, .. }
            | UiProjectInteraction::Handoff { field, .. },
        ) => *field,
        _ => return,
    };
    match field {
        UiProjectFormField::Agent => cycle_activation_agent(model),
        UiProjectFormField::Thread => cycle_activation_thread(model),
        UiProjectFormField::SessionMode => toggle_activation_mode(model),
        UiProjectFormField::Provider => cycle_activation_provider(model, forward),
        UiProjectFormField::Confirmation => {
            if let Some(UiProjectInteraction::Handoff { confirmed, .. }) =
                &mut model.project_interaction
            {
                *confirmed = !*confirmed;
            }
        }
        UiProjectFormField::Force => {
            if let Some(UiProjectInteraction::Handoff { force_takeover, .. }) =
                &mut model.project_interaction
            {
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
        UiProjectInteraction::Activate {
            providers,
            provider,
            new_session: true,
            ..
        }
        | UiProjectInteraction::Handoff {
            providers,
            provider,
            new_session: true,
            ..
        },
    ) = &mut model.project_interaction
    else {
        return;
    };
    if let Some(selected) = cycle_provider_choice(providers, Some(provider), forward) {
        *provider = selected;
    }
}

fn cycle_activation_agent(model: &mut UiModel) {
    let Some(
        UiProjectInteraction::Activate {
            project,
            agents,
            providers,
            agent_id,
            thread,
            new_session,
            provider,
            ..
        }
        | UiProjectInteraction::Handoff {
            project,
            agents,
            providers,
            agent_id,
            thread,
            new_session,
            provider,
            ..
        },
    ) = &mut model.project_interaction
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
        UiProjectInteraction::Activate {
            project,
            agent_id,
            thread,
            provider,
            ..
        }
        | UiProjectInteraction::Handoff {
            project,
            agent_id,
            thread,
            provider,
            ..
        },
    ) = &mut model.project_interaction
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
        UiProjectInteraction::Activate {
            new_session,
            project,
            providers,
            agent_id,
            thread,
            provider,
            ..
        }
        | UiProjectInteraction::Handoff {
            new_session,
            project,
            providers,
            agent_id,
            thread,
            provider,
            ..
        },
    ) = &mut model.project_interaction
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
        UiInput::InsertNewline => TextEdit::Insert("\n"),
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
        model.project_interaction,
        Some(UiProjectInteraction::AddResource { .. })
    ) && model.form.focused != Some(UiFormField::Project(UiProjectFormField::Path))
    {
        return false;
    }
    let field = match &model.project_interaction {
        Some(
            UiProjectInteraction::CreateExisting { field, .. }
            | UiProjectInteraction::CreateWorktree { field, .. }
            | UiProjectInteraction::Activate { field, .. }
            | UiProjectInteraction::Handoff { field, .. },
        ) => UiFormField::Project(*field),
        Some(
            UiProjectInteraction::AddResource { .. } | UiProjectInteraction::ReplaceResource { .. },
        ) => UiFormField::Project(UiProjectFormField::Path),
        _ => return false,
    };
    let form = &mut model.form;
    let target = match &mut model.project_interaction {
        Some(UiProjectInteraction::CreateExisting {
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
        Some(UiProjectInteraction::CreateWorktree {
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
        Some(
            UiProjectInteraction::AddResource { path, .. }
            | UiProjectInteraction::ReplaceResource { path, .. },
        ) => path,
        Some(
            UiProjectInteraction::Activate {
                directory, field, ..
            }
            | UiProjectInteraction::Handoff {
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
    let (fields, selected) = match &model.project_interaction {
        Some(UiProjectInteraction::CreateExisting { field, .. }) => (
            &[
                UiProjectFormField::Path,
                UiProjectFormField::Name,
                UiProjectFormField::Brief,
            ][..],
            *field,
        ),
        Some(UiProjectInteraction::CreateWorktree { field, .. }) => (
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
        UiProjectInteraction::CreateExisting { field, .. }
        | UiProjectInteraction::CreateWorktree { field, .. },
    ) = &mut model.project_interaction
    {
        *field = fields[next];
    }
}

fn default_existing_project_name(model: &mut UiModel) {
    let Some(UiProjectInteraction::CreateExisting {
        name, path, field, ..
    }) = &model.project_interaction
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
    if let Some(UiProjectInteraction::CreateExisting { name, .. }) = &mut model.project_interaction
    {
        name.clone_from(&candidate);
    }
    model.form.cursors.insert(
        UiFormField::Project(UiProjectFormField::Name),
        candidate.len(),
    );
}

fn open_existing_project_form(model: &mut UiModel) {
    model.project_interaction = Some(UiProjectInteraction::CreateExisting {
        name: String::new(),
        brief: String::new(),
        path: String::new(),
        field: UiProjectFormField::Path,
        submitting: false,
    });
    model.last_failure = None;
}

fn open_worktree_project_form(model: &mut UiModel) {
    model.project_interaction = Some(UiProjectInteraction::CreateWorktree {
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
fn submit_project_interaction(
    model: &mut UiModel,
    effects: &mut Vec<UiEffect>,
) -> Result<bool, UiError> {
    fn reject(model: &mut UiModel, field: UiProjectFormField, message: &str) -> bool {
        if let Some(
            UiProjectInteraction::CreateExisting {
                field: selected, ..
            }
            | UiProjectInteraction::CreateWorktree {
                field: selected, ..
            }
            | UiProjectInteraction::Activate {
                field: selected, ..
            }
            | UiProjectInteraction::Handoff {
                field: selected, ..
            },
        ) = &mut model.project_interaction
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
    let action = match model.project_interaction.clone() {
        Some(UiProjectInteraction::CreateExisting {
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
        Some(UiProjectInteraction::CreateWorktree {
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
        Some(UiProjectInteraction::AddResource {
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
        Some(UiProjectInteraction::ReplaceResource {
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
        Some(UiProjectInteraction::Activate {
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
        Some(UiProjectInteraction::Handoff {
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
    match &mut model.project_interaction {
        Some(
            UiProjectInteraction::CreateExisting { submitting, .. }
            | UiProjectInteraction::CreateWorktree { submitting, .. }
            | UiProjectInteraction::AddResource { submitting, .. }
            | UiProjectInteraction::ReplaceResource { submitting, .. }
            | UiProjectInteraction::Activate { submitting, .. }
            | UiProjectInteraction::Handoff { submitting, .. },
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
        model.project_interaction = Some(UiProjectInteraction::ConfirmClose {
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
    model.project_interaction = Some(UiProjectInteraction::Search { query });
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
fn refresh_project_interaction(model: &mut UiModel, snapshot: &UiSnapshot) {
    let identity = match &model.project_interaction {
        Some(
            UiProjectInteraction::AddResource { project, .. }
            | UiProjectInteraction::ReplaceResource { project, .. }
            | UiProjectInteraction::ConfirmRemoveResource { project, .. }
            | UiProjectInteraction::Activate { project, .. }
            | UiProjectInteraction::Handoff { project, .. }
            | UiProjectInteraction::ConfirmClose { project, .. }
            | UiProjectInteraction::ConfirmArchive { project, .. },
        ) => Some(project.project_id),
        _ => None,
    };
    let Some(identity) = identity else { return };
    let current = snapshot
        .projects
        .iter()
        .find(|project| project.project_id == identity)
        .cloned();
    match (&mut model.project_interaction, current) {
        (
            Some(
                UiProjectInteraction::Activate {
                    project,
                    agents,
                    providers,
                    agent_id,
                    thread,
                    new_session,
                    provider,
                    ..
                }
                | UiProjectInteraction::Handoff {
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
                UiProjectInteraction::AddResource { project, .. }
                | UiProjectInteraction::ReplaceResource { project, .. }
                | UiProjectInteraction::ConfirmRemoveResource { project, .. }
                | UiProjectInteraction::ConfirmClose { project, .. }
                | UiProjectInteraction::ConfirmArchive { project, .. },
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
            if let Some(UiConversationTarget::Project {
                project_id,
                thread_id,
                ..
            }) = selected_conversation_target(model)
            {
                model.open_draft(
                    UiMailboxDraftTarget::Project {
                        project_id,
                        thread_id: Some(thread_id),
                    },
                    effects,
                )?;
                return Ok(true);
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
            model.project_interaction = Some(UiProjectInteraction::Search {
                query: model.project_search.clone(),
            });
            Ok(true)
        }
        'c' if model.section == UiSection::Projects => {
            model.project_interaction = Some(UiProjectInteraction::ChooseCreation {
                selected: UiProjectCreationChoice::ExistingFolder,
            });
            Ok(true)
        }
        'c' if model.section == UiSection::Inbox => {
            let Some(UiConversationTarget::Project { project_id, .. }) =
                selected_conversation_target(model)
            else {
                return Ok(false);
            };
            model.open_draft(
                UiMailboxDraftTarget::Project {
                    project_id,
                    thread_id: None,
                },
                effects,
            )?;
            Ok(true)
        }
        'w' if model.section == UiSection::Projects => {
            open_worktree_project_form(model);
            Ok(true)
        }
        'h' => {
            match model.focus {
                UiFocus::Conversation if model.technical_visible => {
                    model.close_technical_details();
                }
                UiFocus::Conversation => model.focus = UiFocus::Content,
                UiFocus::Content => model.focus = UiFocus::Navigation,
                UiFocus::Navigation if model.viewport.width < WIDE_WIDTH => {
                    model.change_section(model.section.previous());
                }
                UiFocus::Navigation | UiFocus::Draft => return Ok(false),
            }
            Ok(true)
        }
        't' => Ok(model.toggle_technical_details()),
        'l' => match model.focus {
            UiFocus::Navigation if model.viewport.width < WIDE_WIDTH => {
                model.change_section(model.section.next());
                Ok(true)
            }
            UiFocus::Navigation => {
                model.focus = UiFocus::Content;
                Ok(true)
            }
            UiFocus::Content => activate(model, effects),
            UiFocus::Conversation | UiFocus::Draft => Ok(false),
        },
        'j' => Ok(match model.focus {
            UiFocus::Conversation if model.technical_visible => {
                model.scroll_technical_details(true)
            }
            UiFocus::Conversation => model.move_conversation_anchor(true),
            UiFocus::Navigation if model.viewport.width >= WIDE_WIDTH => {
                model.change_section(model.section.next());
                true
            }
            UiFocus::Navigation | UiFocus::Content => model.move_row_selection(true),
            UiFocus::Draft => false,
        }),
        'k' => Ok(match model.focus {
            UiFocus::Conversation if model.technical_visible => {
                model.scroll_technical_details(false)
            }
            UiFocus::Conversation => model.move_conversation_anchor(false),
            UiFocus::Navigation if model.viewport.width >= WIDE_WIDTH => {
                model.change_section(model.section.previous());
                true
            }
            UiFocus::Navigation | UiFocus::Content => model.move_row_selection(false),
            UiFocus::Draft => false,
        }),
        _ => Ok(false),
    }
}

fn selected_conversation_target(model: &UiModel) -> Option<UiConversationTarget> {
    model
        .selected_row_data()
        .and_then(|row| row.conversation_target)
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
        UiMailboxDraftTarget::Project {
            project_id,
            thread_id,
        } => UiMailboxAction::Project {
            project_id: *project_id,
            thread_id: *thread_id,
        },
    }
}

fn select_project_conversation(model: &mut UiModel, project_id: [u8; 32], thread_id: [u8; 32]) {
    model.change_section(UiSection::Inbox);
    model.selected_row = Some(format!(
        "project:{}:{}",
        agent_hex(project_id),
        agent_hex(thread_id)
    ));
    model.focus = UiFocus::Conversation;
}

fn activate(model: &mut UiModel, effects: &mut Vec<UiEffect>) -> Result<bool, UiError> {
    if model.viewport.width >= WIDE_WIDTH && model.focus == UiFocus::Navigation {
        model.focus = UiFocus::Content;
        return Ok(true);
    }
    if model.focus == UiFocus::Conversation && model.conversation_anchor.is_some() {
        return Ok(model.toggle_technical_details());
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
        return open_selected_project_conversations(model, effects);
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
        model.follow_conversation_tail();
    } else {
        model.desired_conversation = Some(row_id);
        model.request_inbox_preview(effects);
        model.focus = UiFocus::Conversation;
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
    model.request_conversation(row_id, cursor, true, effects)?;
    Ok(true)
}

fn escape(model: &mut UiModel) -> bool {
    if model.technical_visible {
        model.close_technical_details();
        true
    } else {
        match model.focus {
            UiFocus::Conversation => {
                model.focus = UiFocus::Content;
                true
            }
            UiFocus::Content => {
                model.focus = UiFocus::Navigation;
                true
            }
            UiFocus::Navigation | UiFocus::Draft => false,
        }
    }
}

fn timer_elapsed(
    model: &mut UiModel,
    effect_id: EffectId,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    if model.retry_timer == Some(effect_id) {
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
    if snapshot.revision >= current_revision
        && model.observation_mode == UiObservationMode::SnapshotFallback
    {
        model.apply_snapshot(snapshot);
        apply_guided_snapshot(model, effects)?;
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
    if model.required_revision.is_none() {
        model.request_inbox_preview(effects);
    }
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn conversation_loaded(
    model: &mut UiModel,
    effect_id: EffectId,
    mut page: UiConversationPage,
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
    let agent_finished =
        pending.cursor.is_none() && agent_turn_just_finished(model.conversation.as_ref(), &page);
    let previous_anchor = model.conversation_anchor.clone();
    let previous_message = previous_anchor.as_deref().and_then(|anchor| {
        model
            .conversation
            .as_ref()?
            .entries
            .iter()
            .find(|entry| entry.id == anchor)?
            .message_target
            .map(|target| target.message_id)
    });
    let followed_tail = pending.cursor.is_none()
        && (model.conversation_scroll_mode == ConversationScrollMode::FollowTail
            || previous_anchor.as_ref().is_some_and(|anchor| {
                model
                    .conversation
                    .as_ref()
                    .and_then(|conversation| conversation.entries.last())
                    .is_some_and(|entry| &entry.id == anchor)
            }));
    apply_pending_project_delivery(model.snapshot.as_ref(), &mut page.entries);
    if pending.cursor.is_some()
        && let Some(conversation) = &mut model.conversation
        && conversation.row_id == page.row_id
    {
        conversation.title = page.title;
        conversation.context = page.context;
        conversation.entries.extend(page.entries);
        conversation.next_cursor = page.next_cursor;
    } else {
        model.conversation = Some(UiConversation {
            row_id: page.row_id,
            title: page.title,
            context: page.context,
            entries: page.entries,
            next_cursor: page.next_cursor,
        });
    }
    if let Some(conversation) = &mut model.conversation {
        place_live_activity_at_tail(&mut conversation.entries);
    }
    model.conversation_anchor = model.conversation.as_ref().and_then(|conversation| {
        previous_anchor
            .filter(|anchor| conversation.entries.iter().any(|entry| &entry.id == anchor))
            .or_else(|| {
                previous_message.and_then(|message_id| {
                    conversation
                        .entries
                        .iter()
                        .find(|entry| {
                            entry
                                .message_target
                                .is_some_and(|target| target.message_id == message_id)
                        })
                        .map(|entry| entry.id.clone())
                })
            })
            .or_else(|| conversation.entries.last().map(|entry| entry.id.clone()))
    });
    if model.conversation_anchor.is_none() {
        model.conversation_scroll_mode = ConversationScrollMode::Anchored;
    } else if followed_tail {
        model.conversation_scroll_mode = ConversationScrollMode::FollowTail;
    }
    model.conversation_viewport_geometry = None;
    if pending.enter_on_load {
        model.focus = UiFocus::Conversation;
    }
    model.conversation_failure = None;
    model.last_failure = None;
    if agent_finished {
        open_automatic_followup_draft(model, effects)?;
    }
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn agent_turn_just_finished(previous: Option<&UiConversation>, next: &UiConversationPage) -> bool {
    let Some(previous) = previous.filter(|conversation| conversation.row_id == next.row_id) else {
        return false;
    };
    let was_running = previous.entries.iter().any(is_running_agent_turn);
    let remains_running = next.entries.iter().any(is_running_agent_turn);
    let has_new_terminal_turn = next.entries.iter().any(|entry| {
        is_terminal_agent_turn(entry)
            && !previous
                .entries
                .iter()
                .any(|candidate| candidate.id == entry.id && is_terminal_agent_turn(candidate))
    });
    was_running && !remains_running && has_new_terminal_turn
}

fn is_running_agent_turn(entry: &UiConversationEntry) -> bool {
    matches!(
        entry.presentation,
        UiConversationEntryPresentation::Activity {
            kind: UiConversationActivityKind::AgentTurn,
            status: UiActivityStatus::Running,
            ..
        }
    )
}

fn is_terminal_agent_turn(entry: &UiConversationEntry) -> bool {
    matches!(
        entry.presentation,
        UiConversationEntryPresentation::Activity {
            kind: UiConversationActivityKind::AgentTurn,
            status: UiActivityStatus::Succeeded
                | UiActivityStatus::Failed { .. }
                | UiActivityStatus::Interrupted,
            ..
        }
    )
}

fn open_automatic_followup_draft(
    model: &mut UiModel,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    if model.mailbox_draft.is_some() || model.pending_mailbox.is_some() {
        return Ok(());
    }
    let target = match selected_conversation_target(model) {
        Some(UiConversationTarget::Project {
            project_id,
            thread_id,
            ..
        }) => Some(UiMailboxDraftTarget::Project {
            project_id,
            thread_id: Some(thread_id),
        }),
        None => model.conversation.as_ref().and_then(|conversation| {
            conversation.entries.iter().rev().find_map(|entry| {
                entry
                    .message_target
                    .filter(|target| target.reply_allowed)
                    .map(|target| UiMailboxDraftTarget::Reply {
                        message_id: target.message_id,
                    })
            })
        }),
    };
    if let Some(target) = target {
        model.open_draft(target, effects)?;
    }
    Ok(())
}

fn place_live_activity_at_tail(entries: &mut Vec<UiConversationEntry>) {
    let (mut settled, running): (Vec<_>, Vec<_>) =
        std::mem::take(entries).into_iter().partition(|entry| {
            !matches!(
                entry.presentation,
                UiConversationEntryPresentation::Activity {
                    kind: UiConversationActivityKind::AgentTurn
                        | UiConversationActivityKind::Progress,
                    status: UiActivityStatus::Running,
                    ..
                }
            )
        });
    settled.extend(running);
    *entries = settled;
}

fn apply_pending_project_delivery(
    snapshot: Option<&UiSnapshot>,
    entries: &mut [UiConversationEntry],
) {
    let Some(snapshot) = snapshot else {
        return;
    };
    for entry in entries {
        let Some(target) = entry.message_target else {
            continue;
        };
        let locally_authored = matches!(
            entry.presentation,
            UiConversationEntryPresentation::Message {
                author: UiConversationAuthor::You,
                ..
            }
        );
        if locally_authored
            && snapshot.projects.iter().any(|project| {
                project
                    .pending_inputs
                    .iter()
                    .any(|input| input.message_id == target.message_id)
            })
        {
            entry.delivery = Some(UiMessageDelivery::Pending);
        }
    }
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
    let Some(pending) = model.pending_conversation.take() else {
        return;
    };
    model.conversation_failure = Some(ConversationFailure {
        row_id: pending.row_id,
        cursor: pending.cursor,
        failure: failure.clone(),
    });
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
        &model.mailbox_draft,
        Some(UiMailboxDraftPane::Loading { target }) if *target == draft.target
    );
    model.pending_mailbox = None;
    if !target_matches {
        return;
    }
    model.mailbox_draft = Some(UiMailboxDraftPane::Editing {
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
    let Some(UiMailboxDraftPane::Editing {
        draft,
        dirty: _,
        submitting,
        closing,
    }) = model.mailbox_draft.clone()
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
    model.mailbox_draft = Some(UiMailboxDraftPane::Editing {
        draft: current.clone(),
        dirty: !content_is_saved,
        submitting,
        closing,
    });
    model.last_failure = None;
    if closing && content_is_saved {
        finish_draft_close(model);
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
        .as_ref()
        .filter(|pending| pending.id == effect_id)
    else {
        return;
    };
    let pending_kind = pending.kind.clone();
    if !matches!(
        pending_kind,
        PendingMailboxKind::OpenDraft | PendingMailboxKind::SaveDraft
    ) {
        return;
    }
    model.pending_mailbox = None;
    if let (
        PendingMailboxKind::SaveDraft,
        Some(UiMailboxDraftPane::Editing {
            draft,
            dirty,
            submitting: _,
            closing,
        }),
        Some(server),
    ) = (pending_kind, &mut model.mailbox_draft, current)
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
    message_id: Option<[u8; 32]>,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    let Some(pending) = model.pending_mailbox.as_ref() else {
        return Ok(());
    };
    let PendingMailboxKind::SubmitCommand(submission) = &pending.kind else {
        return Ok(());
    };
    if pending.id != effect_id {
        return Ok(());
    }
    let committed_draft = submission.draft.clone();
    let action = submission.action.clone();
    let optimistic_entry = submission.optimistic_entry.clone();
    let project_target = committed_draft
        .as_ref()
        .and_then(|draft| match draft.target {
            UiMailboxDraftTarget::Project {
                project_id,
                thread_id,
            } => Some((project_id, thread_id)),
            _ => None,
        });
    model.pending_mailbox = None;
    model.mailbox_modal = None;
    model.mailbox_draft = None;
    model.autosave_timer = None;
    model.last_failure = None;
    if let (Some(draft), Some(message_id)) = (committed_draft.as_ref(), message_id) {
        reconcile_committed_message(
            model,
            draft,
            message_id,
            optimistic_entry.as_deref(),
            matches!(action, UiMailboxAction::Project { .. }),
        );
    }
    if let (Some(UiGuidedPending::Instruction(submission)), Some(message_id)) =
        (model.guided_pending.clone(), message_id)
    {
        model.guided_pending = Some(UiGuidedPending::InputSnapshot {
            submission,
            message_id,
        });
    } else if let Some((project_id, thread_id)) = project_target {
        if let Some(thread_id) = thread_id {
            select_project_conversation(model, project_id, thread_id);
        } else if let Some(message_id) = message_id {
            model.pending_project_conversation = Some((project_id, message_id));
        }
    }
    invalidated(model, revision, effects)?;
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn draft_targets_open_conversation(model: &UiModel, draft: &UiMailboxDraft) -> bool {
    match draft.target {
        UiMailboxDraftTarget::Reply { message_id } => {
            model.conversation.as_ref().is_some_and(|conversation| {
                conversation.entries.iter().any(|entry| {
                    entry
                        .message_target
                        .is_some_and(|target| target.message_id == message_id)
                })
            })
        }
        UiMailboxDraftTarget::Project {
            project_id,
            thread_id: Some(thread_id),
        } => matches!(
            selected_conversation_target(model),
            Some(UiConversationTarget::Project {
                project_id: selected_project,
                thread_id: selected_thread,
                ..
            }) if selected_project == project_id && selected_thread == thread_id
        ),
        UiMailboxDraftTarget::Project {
            project_id,
            thread_id: None,
        } => model.conversation.as_ref().is_some_and(|conversation| {
            conversation.row_id == project_draft_conversation_id(project_id)
                && model.selected_row.as_ref() == Some(&conversation.row_id)
        }),
        UiMailboxDraftTarget::Direct { .. } | UiMailboxDraftTarget::SelfNote => false,
    }
}

fn append_pending_message(
    model: &mut UiModel,
    draft: &UiMailboxDraft,
    effect_id: EffectId,
) -> Option<String> {
    if !draft_targets_open_conversation(model, draft) {
        return None;
    }
    let id = format!("pending-mailbox-message:{}", effect_id.0.get());
    let conversation = model.conversation.as_mut()?;
    conversation.entries.retain(|entry| entry.id != id);
    conversation.entries.push(UiConversationEntry {
        id: id.clone(),
        presentation: UiConversationEntryPresentation::Message {
            author: UiConversationAuthor::You,
            body: draft.content.clone(),
        },
        message_state: Some(UiMessageState::Open),
        delivery: Some(UiMessageDelivery::Pending),
        message_target: None,
        technical: Vec::new(),
    });
    place_live_activity_at_tail(&mut conversation.entries);
    model.conversation_anchor = conversation.entries.last().map(|entry| entry.id.clone());
    model.conversation_scroll_mode = if model.conversation_anchor.is_some() {
        ConversationScrollMode::FollowTail
    } else {
        ConversationScrollMode::Anchored
    };
    model.focus = UiFocus::Conversation;
    model.conversation_viewport_position = None;
    model.conversation_viewport_geometry = None;
    model.close_technical_details();
    Some(id)
}

fn reconcile_committed_message(
    model: &mut UiModel,
    draft: &UiMailboxDraft,
    message_id: [u8; 32],
    optimistic_entry: Option<&str>,
    project_message: bool,
) {
    if !draft_targets_open_conversation(model, draft) {
        return;
    }
    if let Some(existing_id) = model.conversation.as_ref().and_then(|conversation| {
        conversation.entries.iter().find_map(|entry| {
            entry
                .message_target
                .filter(|target| target.message_id == message_id)
                .map(|_| entry.id.clone())
        })
    }) {
        if let Some(optimistic_entry) = optimistic_entry
            && optimistic_entry != existing_id
            && let Some(conversation) = &mut model.conversation
        {
            conversation
                .entries
                .retain(|entry| entry.id != optimistic_entry);
        }
        replace_viewport_entry_identity(model, optimistic_entry, &existing_id);
        retain_sent_message_anchor(model, existing_id);
        return;
    }
    let id = format!("committed-message:{message_id:?}");
    let Some(conversation) = &mut model.conversation else {
        return;
    };
    if let Some(optimistic_entry) = optimistic_entry
        && let Some(entry) = conversation
            .entries
            .iter_mut()
            .find(|entry| entry.id == optimistic_entry)
    {
        entry.id.clone_from(&id);
        entry.delivery = Some(if project_message {
            UiMessageDelivery::Pending
        } else {
            UiMessageDelivery::Sent
        });
        entry.message_target = Some(UiMessageTarget {
            message_id,
            reply_allowed: false,
        });
        replace_viewport_entry_identity(model, Some(optimistic_entry), &id);
        retain_sent_message_anchor(model, id);
        return;
    }
    conversation.entries.push(UiConversationEntry {
        id: id.clone(),
        presentation: UiConversationEntryPresentation::Message {
            author: UiConversationAuthor::You,
            body: draft.content.clone(),
        },
        message_state: Some(UiMessageState::Open),
        delivery: Some(if project_message {
            UiMessageDelivery::Pending
        } else {
            UiMessageDelivery::Sent
        }),
        message_target: Some(UiMessageTarget {
            message_id,
            reply_allowed: false,
        }),
        technical: Vec::new(),
    });
    place_live_activity_at_tail(&mut conversation.entries);
    retain_sent_message_anchor(model, id);
}

fn retain_sent_message_anchor(model: &mut UiModel, sent_entry: String) {
    model.conversation_anchor =
        if model.conversation_scroll_mode == ConversationScrollMode::FollowTail {
            model
                .conversation
                .as_ref()
                .and_then(|conversation| conversation.entries.last())
                .map(|entry| entry.id.clone())
        } else {
            Some(sent_entry)
        };
    if model.conversation_anchor.is_none() {
        model.conversation_scroll_mode = ConversationScrollMode::Anchored;
    }
    model.focus = UiFocus::Conversation;
    model.close_technical_details();
}

fn replace_viewport_entry_identity(
    model: &mut UiModel,
    previous_entry: Option<&str>,
    current_entry: &str,
) {
    if let Some(previous_entry) = previous_entry
        && let Some(position) = &mut model.conversation_viewport_position
        && position.entry_id == previous_entry
    {
        current_entry.clone_into(&mut position.entry_id);
    }
    model.conversation_viewport_geometry = None;
}

fn mailbox_command_failed(
    model: &mut UiModel,
    effect_id: EffectId,
    failure: UiFailure,
    effects: &mut Vec<UiEffect>,
) {
    let Some(pending) = model.pending_mailbox.as_ref() else {
        return;
    };
    let PendingMailboxKind::SubmitCommand(submission) = &pending.kind else {
        return;
    };
    if pending.id != effect_id {
        return;
    }
    if failure.code == "mailbox_command_uncertain" {
        model.last_failure = Some(failure);
        effects.push(UiEffect::RequestRedraw);
        return;
    }
    let draft = submission.draft.clone();
    let optimistic_entry = submission.optimistic_entry.clone();
    model.pending_mailbox = None;
    if let Some(optimistic_entry) = optimistic_entry
        && let Some(conversation) = &mut model.conversation
    {
        conversation
            .entries
            .retain(|entry| entry.id != optimistic_entry);
    }
    if let Some(draft) = draft {
        model.mailbox_draft = Some(UiMailboxDraftPane::Editing {
            draft,
            dirty: false,
            submitting: false,
            closing: false,
        });
        model.focus = UiFocus::Draft;
    } else if let Some(UiMailboxDraftPane::Editing {
        submitting,
        closing,
        ..
    }) = &mut model.mailbox_draft
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
            model.project_interaction = None;
            model.completion_context = None;
        }
        UiCompletionContext::Project {
            project_id,
            continuation,
        } => {
            let project_exists = model.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot
                    .projects
                    .iter()
                    .any(|project| project.project_id == project_id)
            });
            if !project_exists {
                return;
            }
            model.change_section(UiSection::Projects);
            model.selected_row = Some(agent_hex(project_id));
            model.agent_modal = None;
            model.project_interaction = None;
            model.project_workspace_level = match continuation {
                UiProjectCompletionContinuation::Select => UiProjectWorkspaceLevel::List,
                UiProjectCompletionContinuation::Summary => UiProjectWorkspaceLevel::Summary,
            };
            model.refresh_selected_project_summary();
            model.completion_context = None;
        }
    }
}

#[allow(clippy::too_many_lines)]
fn apply_guided_snapshot(model: &mut UiModel, effects: &mut Vec<UiEffect>) -> Result<(), UiError> {
    let Some(pending) = model.guided_pending.clone() else {
        return Ok(());
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
                return Ok(());
            };
            model.guided_pending = None;
            open_guided_project(model, project, effects)?;
        }
        UiGuidedPending::AgentCreation {
            project_id,
            expected_name: Some(expected_name),
        } => {
            let Some(snapshot) = model.snapshot.as_ref() else {
                return Ok(());
            };
            let Some(project) = snapshot
                .projects
                .iter()
                .find(|project| project.project_id == project_id)
                .cloned()
            else {
                return Ok(());
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
                return Ok(());
            };
            let agent = agent.clone();
            model.guided_pending = None;
            open_guided_agent(model, project, agent, effects)?;
        }
        UiGuidedPending::InputSnapshot {
            submission,
            message_id,
        } => {
            let Some((project, agent)) = model.snapshot.as_ref().and_then(|snapshot| {
                let project = snapshot
                    .projects
                    .iter()
                    .find(|project| project.project_id == submission.project_id)?
                    .clone();
                let agent = snapshot
                    .agents
                    .iter()
                    .find(|agent| agent.agent_id == submission.agent_id)?
                    .clone();
                Some((project, agent))
            }) else {
                return Ok(());
            };
            if project.assignment.as_ref().is_some_and(|assignment| {
                assignment.agent_id == submission.agent_id && assignment.runnable
            }) {
                if let Some(thread_id) = project
                    .assignment
                    .as_ref()
                    .and_then(|assignment| assignment.thread_id)
                {
                    model.guided_pending = None;
                    model.new_modal = None;
                    select_project_conversation(model, project.project_id, thread_id);
                    model.request_inbox_preview(effects);
                }
                return Ok(());
            }
            let Some(input) = project
                .pending_inputs
                .iter()
                .find(|input| input.message_id == message_id)
                .copied()
            else {
                return Ok(());
            };
            select_project_conversation(model, project.project_id, input.thread_id);
            model.request_inbox_preview(effects);
            submit_guided_project(
                model,
                project,
                &agent,
                submission.provider,
                input.thread_id,
                None,
                effects,
            )?;
            model.new_modal = None;
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
                return Ok(());
            };
            if let Some(thread_id) = project
                .assignment
                .as_ref()
                .and_then(|assignment| assignment.thread_id)
            {
                model.guided_pending = None;
                model.new_modal = None;
                select_project_conversation(model, project.project_id, thread_id);
                model.request_inbox_preview(effects);
            }
        }
        UiGuidedPending::ProjectCreation
        | UiGuidedPending::Instruction(_)
        | UiGuidedPending::AgentCreation {
            expected_name: None,
            ..
        } => {}
    }
    Ok(())
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
            model.project_interaction = Some(UiProjectInteraction::Outcome { result });
        }
        UiProjectOutcome::Reconcilable { code, .. } => {
            model.last_failure = Some(UiFailure {
                code: code.clone(),
                action: "inspect retained external state and reconcile this operation".to_owned(),
            });
            model.project_interaction = Some(UiProjectInteraction::Outcome { result });
        }
        UiProjectOutcome::Running { .. }
        | UiProjectOutcome::ResourcePreview { .. }
        | UiProjectOutcome::ResourceChecks { .. } => {
            model.last_failure = None;
            model.project_interaction = Some(UiProjectInteraction::Outcome { result });
        }
        UiProjectOutcome::Completed { .. } => {
            let (notice, continuation) = project_completion_policy(&result);
            model.last_failure = None;
            model.project_interaction = None;
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
            model.project_interaction = None;
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
            model.project_interaction = None;
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
            model.project_interaction = Some(UiProjectInteraction::Outcome {
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
            model.project_interaction = Some(UiProjectInteraction::Outcome {
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
            model.project_interaction = Some(UiProjectInteraction::Outcome {
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
    match result.action {
        UiProjectAction::CreateExisting { .. } | UiProjectAction::CreateWorktree { .. } => (
            UiCompletionNotice::ProjectCreated,
            UiProjectCompletionContinuation::Select,
        ),
        UiProjectAction::Activate { .. } | UiProjectAction::Handoff { .. } => (
            UiCompletionNotice::ProjectWorkReady,
            UiProjectCompletionContinuation::Select,
        ),
        _ => (
            UiCompletionNotice::ProjectUpdated,
            UiProjectCompletionContinuation::Summary,
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
        UiProjectInteraction::CreateExisting { submitting, .. }
        | UiProjectInteraction::CreateWorktree { submitting, .. }
        | UiProjectInteraction::AddResource { submitting, .. }
        | UiProjectInteraction::ReplaceResource { submitting, .. }
        | UiProjectInteraction::ConfirmRemoveResource { submitting, .. }
        | UiProjectInteraction::Activate { submitting, .. }
        | UiProjectInteraction::Handoff { submitting, .. }
        | UiProjectInteraction::ConfirmClose { submitting, .. }
        | UiProjectInteraction::ConfirmArchive { submitting, .. },
    ) = &mut model.project_interaction
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
    } else if model.observation_mode == UiObservationMode::SnapshotFallback {
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
    if became_ready && model.observation_mode == UiObservationMode::SnapshotFallback {
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

    use super::{
        TextEdit, UiAgent, UiAgentLifecycle, UiAgentStatus, UiEffect, UiError, UiEvent,
        UiFormField, UiFormKind, UiFormState, UiGuidedPending, UiGuidedSubmission, UiHumanState,
        UiInput, UiInteraction, UiInteractionAnswerOutcome, UiInteractionChoice, UiInteractionKind,
        UiInteractionModal, UiInteractionResponse, UiMailboxDraftPane, UiMailboxDraftTarget,
        UiModel, UiProject, UiProjectAction, UiProjectAssignment, UiProjectInteraction,
        UiProjectResourceCheck, UiProjectThread, UiSize, UiSnapshot,
        apply_project_interaction_input, edit_text, normalize_path_input,
        refresh_project_interaction, update,
    };

    #[test]
    fn effect_identity_exhaustion_is_explicit() {
        let mut model = UiModel::new(UiSize {
            width: 80,
            height: 24,
        });
        model.next_effect_id = None;
        let error = update(model, UiEvent::Started).expect_err("allocation exhausts");
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
    fn pending_interaction_opens_and_submits_the_exact_stable_choice() {
        let interaction = interaction(1, false, &[("accept", "Allow once"), ("decline", "Deny")]);
        let observed = update(
            model(),
            UiEvent::InteractionsObserved {
                interactions: vec![interaction.clone()],
            },
        )
        .expect("interaction observed");
        assert!(matches!(
            observed.model.interaction_modal,
            Some(UiInteractionModal::Prompt { ref interaction, selected: 0, .. })
                if interaction.request_id == [1; 32]
        ));

        let selected = update(observed.model, UiEvent::Input(UiInput::NextItem))
            .expect("second choice selected");
        let submitted =
            update(selected.model, UiEvent::Input(UiInput::Activate)).expect("choice submitted");
        assert!(submitted.effects.iter().any(|effect| matches!(
            effect,
            UiEffect::AnswerInteraction {
                interaction: submitted_interaction,
                response: UiInteractionResponse::Choice(value),
                ..
            } if submitted_interaction == &interaction && value == "decline"
        )));
    }

    #[test]
    fn free_text_interaction_submits_the_complete_draft() {
        let observed = update(
            model(),
            UiEvent::InteractionsObserved {
                interactions: vec![interaction(2, true, &[])],
            },
        )
        .expect("interaction observed");
        let typed = update(
            observed.model,
            UiEvent::Input(UiInput::Paste("release now".to_owned())),
        )
        .expect("text entered");
        let submitted =
            update(typed.model, UiEvent::Input(UiInput::Activate)).expect("text submitted");
        assert!(submitted.effects.iter().any(|effect| matches!(
            effect,
            UiEffect::AnswerInteraction {
                response: UiInteractionResponse::Text(value),
                ..
            } if value == "release now"
        )));
    }

    #[test]
    fn escape_explicitly_cancels_an_interaction() {
        let observed = update(
            model(),
            UiEvent::InteractionsObserved {
                interactions: vec![interaction(3, false, &[])],
            },
        )
        .expect("interaction observed");
        let submitted = update(observed.model, UiEvent::Input(UiInput::Escape))
            .expect("cancellation submitted");
        assert!(submitted.effects.iter().any(|effect| matches!(
            effect,
            UiEffect::AnswerInteraction {
                response: UiInteractionResponse::Cancelled,
                ..
            }
        )));
    }

    #[test]
    fn stale_interaction_advances_to_the_next_request() {
        let observed = update(
            model(),
            UiEvent::InteractionsObserved {
                interactions: vec![
                    interaction(4, false, &[("accept", "Allow")]),
                    interaction(5, false, &[("accept", "Allow")]),
                ],
            },
        )
        .expect("interactions observed");
        let submitted =
            update(observed.model, UiEvent::Input(UiInput::Activate)).expect("first submitted");
        let effect_id = submitted
            .effects
            .iter()
            .find_map(|effect| match effect {
                UiEffect::AnswerInteraction { id, .. } => Some(*id),
                _ => None,
            })
            .expect("answer effect");
        let resolved = update(
            submitted.model,
            UiEvent::InteractionAnswered {
                effect_id,
                outcome: UiInteractionAnswerOutcome::Stale,
            },
        )
        .expect("stale handled");
        assert!(matches!(
            resolved.model.interaction_modal,
            Some(UiInteractionModal::Prompt { ref interaction, .. })
                if interaction.request_id == [5; 32]
        ));
        assert_eq!(
            resolved
                .model
                .last_failure
                .as_ref()
                .map(|failure| failure.code.as_str()),
            Some("interaction_already_resolved")
        );
    }

    #[test]
    fn failed_interaction_answer_restores_the_prompt() {
        let observed = update(
            model(),
            UiEvent::InteractionsObserved {
                interactions: vec![interaction(6, false, &[("accept", "Allow")])],
            },
        )
        .expect("interaction observed");
        let submitted =
            update(observed.model, UiEvent::Input(UiInput::Activate)).expect("submitted");
        let effect_id = submitted
            .effects
            .iter()
            .find_map(|effect| match effect {
                UiEffect::AnswerInteraction { id, .. } => Some(*id),
                _ => None,
            })
            .expect("answer effect");
        let failed = update(
            submitted.model,
            UiEvent::InteractionAnswerFailed {
                effect_id,
                failure: super::UiFailure {
                    code: "transport_lost".to_owned(),
                    action: "retry".to_owned(),
                },
            },
        )
        .expect("failure handled");
        assert!(matches!(
            failed.model.interaction_modal,
            Some(UiInteractionModal::Prompt { ref interaction, .. })
                if interaction.request_id == [6; 32]
        ));
    }

    #[test]
    fn project_draft_recipient_uses_the_exact_thread_owner() {
        let mut release = project("release");
        release.assignment = Some(project_assignment([7; 32]));
        release.threads.push(UiProjectThread {
            agent_id: [8; 32],
            provider: "codex".to_owned(),
            session: "historical-session".to_owned(),
            thread_id: [9; 32],
        });
        let mut model = model();
        model.snapshot = Some(UiSnapshot {
            revision: 1,
            human_state: UiHumanState::Ready,
            inbox_rows: Vec::new(),
            sent_rows: Vec::new(),
            archived_rows: Vec::new(),
            agent_rows: Vec::new(),
            project_rows: Vec::new(),
            direct_targets: Vec::new(),
            providers: Vec::new(),
            agents: vec![agent([7; 32], "bob"), agent([8; 32], "alice")],
            projects: vec![release],
        });
        model.mailbox_draft = Some(UiMailboxDraftPane::Loading {
            target: UiMailboxDraftTarget::Project {
                project_id: [1; 32],
                thread_id: Some([9; 32]),
            },
        });

        assert_eq!(model.draft_recipient_name(), Some("alice"));
    }

    #[test]
    fn new_project_draft_recipient_uses_the_current_assignment() {
        let mut release = project("release");
        release.assignment = Some(project_assignment([7; 32]));
        let mut model = model();
        model.snapshot = Some(UiSnapshot {
            revision: 1,
            human_state: UiHumanState::Ready,
            inbox_rows: Vec::new(),
            sent_rows: Vec::new(),
            archived_rows: Vec::new(),
            agent_rows: Vec::new(),
            project_rows: Vec::new(),
            direct_targets: Vec::new(),
            providers: Vec::new(),
            agents: vec![agent([7; 32], "alice")],
            projects: vec![release],
        });
        model.mailbox_draft = Some(UiMailboxDraftPane::Loading {
            target: UiMailboxDraftTarget::Project {
                project_id: [1; 32],
                thread_id: None,
            },
        });

        assert_eq!(model.draft_recipient_name(), Some("alice"));
    }

    #[test]
    fn new_project_draft_recipient_uses_the_guided_agent_before_assignment_exists() {
        let mut model = model();
        model.snapshot = Some(UiSnapshot {
            revision: 1,
            human_state: UiHumanState::Ready,
            inbox_rows: Vec::new(),
            sent_rows: Vec::new(),
            archived_rows: Vec::new(),
            agent_rows: Vec::new(),
            project_rows: Vec::new(),
            direct_targets: Vec::new(),
            providers: Vec::new(),
            agents: vec![agent([7; 32], "alice")],
            projects: vec![project("release")],
        });
        model.guided_pending = Some(UiGuidedPending::Instruction(UiGuidedSubmission {
            project_id: [1; 32],
            agent_id: [7; 32],
            provider: "codex".to_owned(),
        }));
        model.mailbox_draft = Some(UiMailboxDraftPane::Loading {
            target: UiMailboxDraftTarget::Project {
                project_id: [1; 32],
                thread_id: None,
            },
        });

        assert_eq!(model.draft_recipient_name(), Some("alice"));
    }

    #[test]
    fn clean_close_requires_confirmation_but_not_force() {
        let project = project("release");
        let mut model = UiModel::new(UiSize {
            width: 80,
            height: 24,
        });
        model.project_interaction = Some(UiProjectInteraction::ConfirmClose {
            project,
            checks: vec![release_check("accepted", Some("clean"))],
            confirmed: false,
            force: false,
            submitting: false,
        });
        let mut effects = Vec::new();
        assert!(
            apply_project_interaction_input(&mut model, UiInput::Character('c'), &mut effects)
                .expect("confirmation toggles")
        );
        assert!(
            apply_project_interaction_input(&mut model, UiInput::Activate, &mut effects)
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
        model.project_interaction = Some(UiProjectInteraction::ConfirmClose {
            project: project("old name"),
            checks: vec![release_check("uncertain", None)],
            confirmed: true,
            force: true,
            submitting: false,
        });
        refresh_project_interaction(
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
        let retained = model.project_interaction.expect("close modal retained");
        assert!(matches!(
            retained,
            UiProjectInteraction::ConfirmClose { .. }
        ));
        if let UiProjectInteraction::ConfirmClose {
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

    fn model() -> UiModel {
        UiModel::new(UiSize {
            width: 80,
            height: 24,
        })
    }

    fn interaction(identity: u8, allow_text: bool, choices: &[(&str, &str)]) -> UiInteraction {
        UiInteraction {
            agent_id: [9; 32],
            agent_name: "alice".to_owned(),
            project_id: Some([8; 32]),
            project_name: Some("release".to_owned()),
            provider: "codex".to_owned(),
            session: "session-1".to_owned(),
            request_id: [identity; 32],
            operation_id: [7; 32],
            kind: UiInteractionKind::CommandApproval,
            prompt: "Run the command?".to_owned(),
            choices: choices
                .iter()
                .map(|(value, label)| UiInteractionChoice {
                    value: (*value).to_owned(),
                    label: (*label).to_owned(),
                })
                .collect(),
            allow_text,
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
            pending_inputs: Vec::new(),
            head: [3; 32],
            input_sequence: 0,
            resources: Vec::new(),
        }
    }

    fn project_assignment(agent_id: [u8; 32]) -> UiProjectAssignment {
        UiProjectAssignment {
            assignment_id: [6; 32],
            agent_id,
            provider: "codex".to_owned(),
            session: Some("current-session".to_owned()),
            phase: "runnable".to_owned(),
            thread_id: Some([10; 32]),
            launch_directory: Some("/workspace/release".to_owned()),
            blocked: None,
            cardinality_conflicted: false,
            runnable: true,
        }
    }

    fn agent(agent_id: [u8; 32], name: &str) -> UiAgent {
        UiAgent {
            agent_id,
            names: vec![name.to_owned()],
            mailboxes: Vec::new(),
            lifecycle: UiAgentLifecycle::Active,
            runnable: true,
            status: UiAgentStatus::Unassigned,
            sessions: Vec::new(),
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
