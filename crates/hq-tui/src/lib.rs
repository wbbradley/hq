//! Pure terminal UI state transitions and borrowed Ratatui rendering.
//!
//! This crate owns no terminal, clock, runtime, local transport, storage, or domain mutation
//! capability. A shell feeds [`UiEvent`] values into [`update`], executes returned [`UiEffect`]
//! values, and borrows the resulting [`UiModel`] for [`render`].

mod model;
mod render;

pub use model::{
    EffectId, UiActivityStatus, UiAgent, UiAgentAction, UiAgentAssignmentPhase,
    UiAgentAttentionReason, UiAgentLifecycle, UiAgentMailbox, UiAgentModal,
    UiAgentProjectAssignment, UiAgentSession, UiAgentStatus, UiConnectionState, UiConversation,
    UiConversationEntry, UiConversationEntryKind, UiConversationPage, UiDirectTarget, UiEffect,
    UiError, UiEvent, UiFailure, UiFocus, UiHumanState, UiInput, UiMailboxAction, UiMailboxDraft,
    UiMailboxDraftTarget, UiMailboxModal, UiManagedSessionAction, UiManagedSessionOutcome,
    UiManagedSessionResult, UiMessageState, UiMessageTarget, UiModel, UiProject, UiProjectAction,
    UiProjectAssignment, UiProjectExternalWarning, UiProjectFormField, UiProjectModal,
    UiProjectOutcome, UiProjectResource, UiProjectResourceCheck, UiProjectResourceConflict,
    UiProjectResult, UiProjectThread, UiRow, UiRowKind, UiRowState, UiSection, UiSize, UiSnapshot,
    UiTechnicalSection, UiTimerKind, UiTransition, update,
};
pub use render::render;
