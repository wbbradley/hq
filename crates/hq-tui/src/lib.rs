//! Pure terminal UI state transitions and borrowed Ratatui rendering.
//!
//! This crate owns no terminal, clock, runtime, local transport, storage, or domain mutation
//! capability. A shell feeds [`UiEvent`] values into [`update`], executes returned [`UiEffect`]
//! values, and borrows the resulting [`UiModel`] for [`render`].

mod message_markdown;
mod model;
mod render;
mod shell_highlight;
mod theme;

pub use model::{
    EffectId, UiActivityStatus, UiAgent, UiAgentAction, UiAgentAssignmentPhase,
    UiAgentAttentionReason, UiAgentLifecycle, UiAgentMailbox, UiAgentModal,
    UiAgentProjectAssignment, UiAgentSession, UiAgentStatus, UiCommandApproval,
    UiCompletedFileChange, UiCompletedItemPresentation, UiConfigField, UiConfiguration,
    UiConnectionState, UiConversation, UiConversationActivityKind, UiConversationAuthor,
    UiConversationEntry, UiConversationEntryGeometry, UiConversationEntryPresentation,
    UiConversationPage, UiConversationTarget, UiConversationViewportObservation,
    UiConversationViewportPosition, UiDirectTarget, UiEffect, UiError, UiEvent, UiFailure, UiFocus,
    UiHelpPage, UiHumanIssue, UiHumanMembershipEvidence, UiHumanMembershipStatus,
    UiHumanSelectionEvidence, UiHumanState, UiInput, UiInteraction, UiInteractionAnswerOutcome,
    UiInteractionChoice, UiInteractionKind, UiInteractionModal, UiInteractionResponse,
    UiInteractionTarget, UiInteractionTargetIssue, UiMailboxAction, UiMailboxCommandResult,
    UiMailboxDraft, UiMailboxDraftPane, UiMailboxDraftTarget, UiMailboxModal,
    UiManagedSessionAction, UiManagedSessionOutcome, UiManagedSessionResult,
    UiMaterializedConversationView, UiMessageDelivery, UiMessageState, UiMessageTarget, UiModel,
    UiNewChoice, UiNewModal, UiPendingProjectInput, UiProject, UiProjectAction,
    UiProjectAssignedAgentStatus, UiProjectAssignedAgentSummary, UiProjectAssignment,
    UiProjectConversationSetup, UiProjectConversationSummary, UiProjectCreationChoice,
    UiProjectExternalWarning, UiProjectFolderAction, UiProjectFolderOwnership,
    UiProjectFolderSummary, UiProjectFormField, UiProjectInboxFilter, UiProjectInteraction,
    UiProjectLifecycle, UiProjectManagementAction, UiProjectOutcome, UiProjectRecoverySummary,
    UiProjectResource, UiProjectResourceCheck, UiProjectResourceCondition,
    UiProjectResourceConflict, UiProjectResult, UiProjectSummary, UiProjectSummaryFocus,
    UiProjectTechnicalEvidence, UiProjectThread, UiProjectWorkspaceLevel, UiProvider,
    UiReconnectCause, UiReconnectFailureKind, UiReconnectOperation, UiRow, UiRowKind, UiRowState,
    UiSection, UiSize, UiSnapshot, UiTechnicalSection, UiThemeChoice, UiTimerKind, UiTransition,
    update,
};
pub use render::{UiRenderCache, render, render_with_cache};
pub use theme::{Base16Palette, UiTheme, UiThemeRole};
