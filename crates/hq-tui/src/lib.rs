//! Pure terminal UI state transitions and borrowed Ratatui rendering.
//!
//! This crate owns no terminal, clock, runtime, local transport, storage, or domain mutation
//! capability. A shell feeds [`UiEvent`] values into [`update`], executes returned [`UiEffect`]
//! values, and borrows the resulting [`UiModel`] for [`render`].

mod model;
mod render;
mod theme;

pub use model::{
    EffectId, UiActivityStatus, UiAgent, UiAgentAction, UiAgentAssignmentPhase,
    UiAgentAttentionReason, UiAgentLifecycle, UiAgentMailbox, UiAgentModal,
    UiAgentProjectAssignment, UiAgentSession, UiAgentStatus, UiConnectionState, UiConversation,
    UiConversationActivityKind, UiConversationAuthor, UiConversationEntry,
    UiConversationEntryPresentation, UiConversationPage, UiConversationTarget, UiDirectTarget,
    UiEffect, UiError, UiEvent, UiFailure, UiFocus, UiHelpPage, UiHumanIssue,
    UiHumanMembershipEvidence, UiHumanMembershipStatus, UiHumanSelectionEvidence, UiHumanState,
    UiInput, UiMailboxAction, UiMailboxCommandResult, UiMailboxDraft, UiMailboxDraftPane,
    UiMailboxDraftTarget, UiMailboxModal, UiManagedSessionAction, UiManagedSessionOutcome,
    UiManagedSessionResult, UiMessageState, UiMessageTarget, UiModel, UiNewChoice, UiNewModal,
    UiPendingProjectInput, UiProject, UiProjectAction, UiProjectAssignment,
    UiProjectCreationChoice, UiProjectExternalWarning, UiProjectFormField, UiProjectModal,
    UiProjectOutcome, UiProjectResource, UiProjectResourceCheck, UiProjectResourceConflict,
    UiProjectResult, UiProjectThread, UiProvider, UiRow, UiRowKind, UiRowState, UiSection, UiSize,
    UiSnapshot, UiTechnicalSection, UiTimerKind, UiTransition, update,
};
pub use render::render;
pub use theme::{Base16Palette, UiTheme, UiThemeRole};
