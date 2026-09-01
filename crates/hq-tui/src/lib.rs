//! Pure terminal UI state transitions and borrowed Ratatui rendering.
//!
//! This crate owns no terminal, clock, runtime, local transport, storage, or domain mutation
//! capability. A shell feeds [`UiEvent`] values into [`update`], executes returned [`UiEffect`]
//! values, and borrows the resulting [`UiModel`] for [`render`].

mod message_markdown;
mod model;
mod render;
mod theme;

pub use model::{
    EffectId, UiActivityStatus, UiAgent, UiAgentAction, UiAgentAssignmentPhase,
    UiAgentAttentionReason, UiAgentLifecycle, UiAgentMailbox, UiAgentModal,
    UiAgentProjectAssignment, UiAgentSession, UiAgentStatus, UiCompletedFileChange,
    UiCompletedItemPresentation, UiConnectionState, UiConversation, UiConversationActivityKind,
    UiConversationAuthor, UiConversationEntry, UiConversationEntryPresentation, UiConversationPage,
    UiConversationTarget, UiDirectTarget, UiEffect, UiError, UiEvent, UiFailure, UiFocus,
    UiHelpPage, UiHumanIssue, UiHumanMembershipEvidence, UiHumanMembershipStatus,
    UiHumanSelectionEvidence, UiHumanState, UiInput, UiMailboxAction, UiMailboxCommandResult,
    UiMailboxDraft, UiMailboxDraftPane, UiMailboxDraftTarget, UiMailboxModal,
    UiManagedSessionAction, UiManagedSessionOutcome, UiManagedSessionResult,
    UiMaterializedConversationView, UiMessageDelivery, UiMessageState, UiMessageTarget, UiModel,
    UiNewChoice, UiNewModal, UiPendingProjectInput, UiProject, UiProjectAction,
    UiProjectAssignedAgentStatus, UiProjectAssignedAgentSummary, UiProjectAssignment,
    UiProjectConversationSummary, UiProjectCreationChoice, UiProjectExternalWarning,
    UiProjectFolderAction, UiProjectFolderOwnership, UiProjectFolderSummary, UiProjectFormField,
    UiProjectInboxFilter, UiProjectInteraction, UiProjectLifecycle, UiProjectManagementAction,
    UiProjectOutcome, UiProjectRecoverySummary, UiProjectResource, UiProjectResourceCheck,
    UiProjectResourceConflict, UiProjectResult, UiProjectSummary, UiProjectSummaryFocus,
    UiProjectTechnicalEvidence, UiProjectThread, UiProjectWorkspaceLevel, UiProvider, UiRow,
    UiRowKind, UiRowState, UiSection, UiSize, UiSnapshot, UiTechnicalSection, UiTimerKind,
    UiTransition, update,
};
pub use render::{UiRenderCache, render, render_with_cache};
pub use theme::{Base16Palette, UiTheme, UiThemeRole};
