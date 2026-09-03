//! Pure TUI transition and stale-effect contracts.

#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use hq_tui::{
    UiActivityStatus, UiAgent, UiAgentAction, UiAgentAssignmentPhase, UiAgentLifecycle,
    UiAgentMailbox, UiAgentModal, UiAgentProjectAssignment, UiAgentSession, UiAgentStatus,
    UiCompletedItemPresentation, UiConnectionState, UiConversationActivityKind,
    UiConversationAuthor, UiConversationEntry, UiConversationEntryGeometry,
    UiConversationEntryPresentation, UiConversationPage, UiConversationTarget,
    UiConversationViewportObservation, UiConversationViewportPosition, UiDirectTarget, UiEffect,
    UiEvent, UiFailure, UiFocus, UiHelpPage, UiHumanState, UiInput, UiInteraction,
    UiInteractionChoice, UiInteractionKind, UiInteractionModal, UiInteractionResponse,
    UiMailboxAction, UiMailboxDraft, UiMailboxDraftPane, UiMailboxDraftTarget, UiMailboxModal,
    UiManagedSessionAction, UiManagedSessionOutcome, UiManagedSessionResult,
    UiMaterializedConversationView, UiMessageDelivery, UiMessageState, UiMessageTarget, UiModel,
    UiNewChoice, UiNewModal, UiPendingProjectInput, UiProject, UiProjectAction,
    UiProjectAssignment, UiProjectCreationChoice, UiProjectFolderAction, UiProjectFormField,
    UiProjectInteraction, UiProjectManagementAction, UiProjectOutcome, UiProjectResource,
    UiProjectResourceCheck, UiProjectResourceConflict, UiProjectResult, UiProjectSummaryFocus,
    UiProjectThread, UiProjectWorkspaceLevel, UiProvider, UiRow, UiRowKind, UiRowState, UiSection,
    UiSize, UiSnapshot, UiTechnicalSection, UiTimerKind, update,
};

#[test]
fn new_launcher_keeps_project_direct_and_personal_intents_distinct() {
    let mut snapshot = snapshot(1, &[]);
    snapshot.direct_targets = vec![direct_target("Ada", 4)];
    let model = loaded_model(snapshot);

    let launcher = update(model, UiEvent::Input(UiInput::Character('n')))
        .expect("open New launcher")
        .model;
    assert!(matches!(
        launcher.new_modal(),
        Some(UiNewModal::Launcher {
            selected: UiNewChoice::ProjectWork
        })
    ));

    let direct = update(launcher, UiEvent::Input(UiInput::NextItem))
        .expect("choose direct message")
        .model;
    let direct = update(direct, UiEvent::Input(UiInput::Activate)).expect("open recipients");
    assert!(matches!(
        direct.model.mailbox_modal(),
        Some(UiMailboxModal::SelectDirect { targets, .. }) if targets.len() == 1
    ));

    let launcher = update(direct.model, UiEvent::Input(UiInput::Escape))
        .expect("close recipients")
        .model;
    let launcher = update(launcher, UiEvent::Input(UiInput::Character('n')))
        .expect("reopen New launcher")
        .model;
    let note = update(launcher, UiEvent::Input(UiInput::NextItem))
        .expect("choose direct message")
        .model;
    let note = update(note, UiEvent::Input(UiInput::NextItem))
        .expect("choose personal note")
        .model;
    let note = update(note, UiEvent::Input(UiInput::Activate)).expect("prepare note");
    assert!(matches!(
        open_draft_effect(&note.effects).1,
        UiMailboxDraftTarget::SelfNote
    ));
}

#[test]
fn vim_vertical_keys_navigate_choices_without_stealing_text_input() {
    let project = project(5, "release", "/work/release");
    let agent = project_agent(7, [9; 32]);
    let mut model = loaded_projects_model_with_agents_and_providers(
        1,
        vec![project],
        vec![agent],
        vec![
            available_provider("alpha", "Alpha", false),
            available_provider("codex", "Codex", true),
        ],
    );

    model = update(model, UiEvent::Input(UiInput::Character('n')))
        .expect("open launcher")
        .model;
    model = update(model, UiEvent::Input(UiInput::Character('k')))
        .expect("k at the first intent is inert")
        .model;
    assert!(matches!(
        model.new_modal(),
        Some(UiNewModal::Launcher {
            selected: UiNewChoice::ProjectWork,
        })
    ));
    model = update(model, UiEvent::Input(UiInput::Character('j')))
        .expect("j selects the next intent")
        .model;
    assert!(matches!(
        model.new_modal(),
        Some(UiNewModal::Launcher {
            selected: UiNewChoice::DirectMessage,
        })
    ));
    model = update(model, UiEvent::Input(UiInput::Character('k')))
        .expect("k selects the previous intent")
        .model;
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("open project picker")
        .model;
    model = update(model, UiEvent::Input(UiInput::Character('j')))
        .expect("j selects project creation")
        .model;
    assert!(matches!(
        model.new_modal(),
        Some(UiNewModal::ChooseProject {
            create_new: true,
            ..
        })
    ));
    model = update(model, UiEvent::Input(UiInput::Character('k')))
        .expect("k returns to the project")
        .model;
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("open agent picker")
        .model;
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("choose agent")
        .model;
    model = update(model, UiEvent::Input(UiInput::Character('k')))
        .expect("k selects the previous provider")
        .model;
    assert!(matches!(
        model.new_modal(),
        Some(UiNewModal::ChooseProvider { provider, .. }) if provider == "alpha"
    ));

    let mut project_model = loaded_projects_model(1, vec![]);
    project_model = update(project_model, UiEvent::Input(UiInput::Character('c')))
        .expect("open project creation chooser")
        .model;
    project_model = update(project_model, UiEvent::Input(UiInput::Character('j')))
        .expect("j selects the next creation choice")
        .model;
    assert!(matches!(
        project_model.project_interaction(),
        Some(UiProjectInteraction::ChooseCreation {
            selected: UiProjectCreationChoice::IsolatedWorktree,
        })
    ));
    project_model = update(project_model, UiEvent::Input(UiInput::Character('k')))
        .expect("k selects the previous creation choice")
        .model;
    project_model = update(project_model, UiEvent::Input(UiInput::Activate))
        .expect("open existing-folder form")
        .model;
    project_model = update(project_model, UiEvent::Input(UiInput::Character('j')))
        .expect("j remains text in a path field")
        .model;
    assert!(matches!(
        project_model.project_interaction(),
        Some(UiProjectInteraction::CreateExisting { path, .. }) if path == "j"
    ));
}

#[test]
fn vim_keys_never_cross_focus_or_activate_the_current_item() {
    let previewed = materialized_transition(
        snapshot(1, &["thread-a"]),
        UiConversationPage {
            title: "Alice".to_owned(),
            context: None,
            row_id: "thread-a".to_owned(),
            entries: vec![entry("message-1", false)],
            next_cursor: None,
        },
    );

    let navigation = update(
        previewed.model.clone(),
        UiEvent::Input(UiInput::Character('l')),
    )
    .expect("l in navigation is inert");
    assert_eq!(navigation.model.focus(), UiFocus::Navigation);
    assert_eq!(navigation.model.section(), UiSection::Inbox);

    let content = update(previewed.model, UiEvent::Input(UiInput::NextFocus))
        .expect("Tab focuses Inbox content");
    let unchanged = update(content.model, UiEvent::Input(UiInput::Character('h')))
        .expect("h in content is inert");
    assert_eq!(unchanged.model.focus(), UiFocus::Content);
    let unchanged = update(unchanged.model, UiEvent::Input(UiInput::Character('l')))
        .expect("l in content does not activate the conversation");
    assert_eq!(unchanged.model.focus(), UiFocus::Content);
}

#[test]
fn guided_project_work_resumes_ready_assignment_without_session_setup() {
    let agent = project_agent(7, [9; 32]);
    let mut project = project(5, "release", "/work/release");
    project.assignment = Some(UiProjectAssignment {
        assignment_id: [8; 32],
        agent_id: agent.agent_id,
        provider: "codex".to_owned(),
        session: Some("session-7".to_owned()),
        phase: "runnable".to_owned(),
        thread_id: Some([6; 32]),
        launch_directory: Some("/work/release".to_owned()),
        blocked: None,
        cardinality_conflicted: false,
        runnable: true,
    });
    let mut model = loaded_projects_model_with_agents(1, vec![project], vec![agent]);
    model = update(model, UiEvent::Input(UiInput::Character('n')))
        .expect("launcher")
        .model;
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("project intent")
        .model;
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("select project")
        .model;
    assert!(matches!(
        model.mailbox_draft(),
        Some(UiMailboxDraftPane::Loading {
            target: UiMailboxDraftTarget::Project {
                project_id,
                thread_id: Some(thread_id),
            }
        }) if *project_id == [5; 32] && *thread_id == [6; 32]
    ));
    assert!(model.new_modal().is_none());
}

#[test]
fn cancelling_a_guided_first_instruction_returns_to_the_project_conversation() {
    let agent = project_agent(7, [9; 32]);
    let project = project(5, "release", "/work/release");
    let mut model = loaded_projects_model_with_agents(1, vec![project], vec![agent]);
    for input in [
        UiInput::Character('n'),
        UiInput::Activate,
        UiInput::Activate,
        UiInput::Activate,
    ] {
        model = update(model, UiEvent::Input(input))
            .expect("guided step")
            .model;
    }

    let cancelled = update(model, UiEvent::Input(UiInput::Escape)).expect("cancel instruction");
    assert!(
        cancelled
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitProjectCommand { .. }))
    );
    assert!(cancelled.model.project_interaction().is_none());
    assert!(cancelled.model.new_modal().is_none());
    assert!(cancelled.model.mailbox_draft().is_none());
    assert_eq!(cancelled.model.focus(), UiFocus::Conversation);
}

#[test]
fn guided_project_work_names_an_agents_competing_project_without_mutation() {
    let mut assigned = project_agent(7, [9; 32]);
    assigned.status = UiAgentStatus::Assigned(UiAgentProjectAssignment {
        project_id: [4; 32],
        project_name: "payments".to_owned(),
        assignment_id: [3; 32],
        provider: "codex".to_owned(),
        session: Some("busy".to_owned()),
        phase: UiAgentAssignmentPhase::Ready,
        blocked: None,
        cardinality_conflicted: false,
    });
    let mut model = loaded_projects_model_with_agents(
        1,
        vec![project(5, "release", "/work/release")],
        vec![assigned],
    );
    for input in [
        UiInput::Character('n'),
        UiInput::Activate,
        UiInput::Activate,
        UiInput::Activate,
    ] {
        model = update(model, UiEvent::Input(input))
            .expect("guided step")
            .model;
    }
    assert!(matches!(
        model.new_modal(),
        Some(UiNewModal::AgentUnavailable {
            competing_project,
            ..
        }) if competing_project == "payments"
    ));
    assert!(
        update(model, UiEvent::Input(UiInput::Activate))
            .expect("inspect conflict")
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitProjectCommand { .. }))
    );
}

#[test]
fn guided_project_work_can_create_its_missing_project_and_continue() {
    let mut model = loaded_projects_model(1, Vec::new());
    for input in [
        UiInput::Character('n'),
        UiInput::Activate,
        UiInput::Activate,
        UiInput::Activate,
        UiInput::Paste("/work/new-project".to_owned()),
        UiInput::NextFocus,
    ] {
        model = update(model, UiEvent::Input(input))
            .expect("creation step")
            .model;
    }
    let creating = update(model, UiEvent::Input(UiInput::Activate)).expect("create");
    let (create_id, create_action) = project_effect(&creating.effects);
    assert!(matches!(
        create_action,
        UiProjectAction::CreateExisting { ref path, .. } if path == "/work/new-project"
    ));
    let completed = update(
        creating.model,
        UiEvent::ProjectCommandCompleted {
            effect_id: create_id,
            result: UiProjectResult {
                action: create_action,
                command_id: [22; 32],
                operation_id: [23; 32],
                project_id: [5; 32],
                runtime_state: None,
                runtime_code: None,
                outcome: UiProjectOutcome::Completed {
                    project_head: Some([24; 32]),
                },
            },
        },
    )
    .expect("created");
    let snapshot_id = snapshot_effect(&completed.effects);
    let resumed = update(
        completed.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot: projects_snapshot(2, vec![project(5, "new-project", "/work/new-project")]),
        },
    )
    .expect("new project loaded");
    assert!(matches!(
        resumed.model.new_modal(),
        Some(UiNewModal::ChooseAgent {
            project,
            create_new: true,
            ..
        }) if project.project_id == [5; 32]
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn guided_project_work_can_create_its_missing_agent_and_continue() {
    let project = project(5, "release", "/work/release");
    let mut model = loaded_projects_model(1, vec![project.clone()]);
    for input in [
        UiInput::Character('n'),
        UiInput::Activate,
        UiInput::Activate,
        UiInput::Activate,
        UiInput::Paste("builder".to_owned()),
    ] {
        model = update(model, UiEvent::Input(input))
            .expect("agent step")
            .model;
    }
    let creating = update(model, UiEvent::Input(UiInput::Activate)).expect("create agent");
    let (agent_id, action) = agent_action_effect(&creating.effects);
    assert!(matches!(action, UiAgentAction::Create { name } if name == "builder"));
    let committed = update(
        creating.model,
        UiEvent::AgentCommandCommitted {
            effect_id: agent_id,
            revision: 2,
        },
    )
    .expect("agent committed");
    let snapshot_id = snapshot_effect(&committed.effects);
    let builder = project_agent(7, [9; 32]);
    let mut builder = UiAgent {
        names: vec!["builder".to_owned()],
        ..builder
    };
    builder.status = UiAgentStatus::Unassigned;
    let mut refreshed = projects_snapshot(2, vec![project]);
    let agent_source = agents_snapshot(2, vec![builder]);
    refreshed.agents = agent_source.agents;
    refreshed.agent_rows = agent_source.agent_rows;
    let resumed = update(
        committed.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot: refreshed,
        },
    )
    .expect("new agent loaded");
    assert!(resumed.model.new_modal().is_none());
    assert!(matches!(
        resumed.model.mailbox_draft(),
        Some(UiMailboxDraftPane::Loading {
            target: UiMailboxDraftTarget::Project {
                project_id,
                thread_id: None,
            }
        }) if *project_id == [5; 32]
    ));
    assert_project_draft_context(&resumed.model, [5; 32], "release", None);
    let (open_id, target) = open_draft_effect(&resumed.effects);
    let composing = update(
        resumed.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft: UiMailboxDraft {
                draft_id: [31; 32],
                version: 0,
                target: target.clone(),
                content: String::new(),
            },
        },
    )
    .expect("first-message composer loaded");

    let refreshing = update(composing.model, UiEvent::Invalidated { revision: 3 })
        .expect("refresh while composing new project conversation");
    let snapshot_id = snapshot_effect(&refreshing.effects);
    let mut current = refreshing
        .model
        .snapshot()
        .expect("current snapshot")
        .clone();
    current.revision = 3;
    let refreshed = update(
        refreshing.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot: current,
        },
    )
    .expect("project draft snapshot refreshed");
    assert!(
        refreshed
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::LoadConversation { .. }))
    );
    assert_project_draft_context(&refreshed.model, [5; 32], "release", None);

    let closed = update(refreshed.model, UiEvent::Input(UiInput::Escape))
        .expect("close the first-message composer");
    assert!(closed.model.new_modal().is_none());
    assert!(closed.model.mailbox_draft().is_none());
    assert_eq!(closed.model.focus(), UiFocus::Conversation);
    let activated = update(closed.model, UiEvent::Input(UiInput::Activate))
        .expect("activate the retained empty conversation");
    assert!(activated.model.new_modal().is_none());
    assert!(activated.model.mailbox_draft().is_none());
    assert!(activated.effects.iter().all(|effect| !matches!(
        effect,
        UiEffect::OpenDraft { .. }
            | UiEffect::SubmitProjectCommand { .. }
            | UiEffect::SubmitMailboxCommand { .. }
    )));
}

#[test]
#[allow(clippy::too_many_lines)]
fn guided_project_failure_does_not_rearm_the_submission() {
    let agent = project_agent(7, [9; 32]);
    let project = project(5, "release", "/work/release");
    let mut model =
        loaded_projects_model_with_agents(1, vec![project.clone()], vec![agent.clone()]);
    for input in [
        UiInput::Character('n'),
        UiInput::Activate,
        UiInput::Activate,
    ] {
        model = update(model, UiEvent::Input(input))
            .expect("guided step")
            .model;
    }
    let opening = update(model, UiEvent::Input(UiInput::Activate)).expect("open project draft");
    let (open_id, target) = open_draft_effect(&opening.effects);
    let loaded = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft: UiMailboxDraft {
                draft_id: [31; 32],
                target: target.clone(),
                content: String::new(),
                version: 1,
            },
        },
    )
    .expect("project draft loaded");
    model = update(
        loaded.model,
        UiEvent::Input(UiInput::Paste("Retain this instruction".to_owned())),
    )
    .expect("instruction")
    .model;
    let sending = update(model, UiEvent::Input(UiInput::Activate)).expect("send instruction");
    let (save_id, save_input) = save_draft_effect(&sending.effects);
    let submitting = update(
        sending.model,
        UiEvent::DraftSaved {
            effect_id: save_id,
            draft: UiMailboxDraft {
                version: 2,
                ..save_input.clone()
            },
        },
    )
    .expect("saved project draft submits");
    let send_id = submitting
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::SubmitMailboxCommand {
                id,
                action:
                    UiMailboxAction::Project {
                        project_id,
                        thread_id: None,
                    },
                ..
            } if *project_id == [5; 32] => Some(*id),
            _ => None,
        })
        .expect("project mailbox command");
    let message_id = [32; 32];
    let sent = update(
        submitting.model,
        UiEvent::MailboxCommandCommitted {
            effect_id: send_id,
            revision: 2,
            message_id: Some(message_id),
        },
    )
    .expect("instruction accepted");
    let snapshot_id = snapshot_effect(&sent.effects);
    let mut pending_project = project;
    pending_project.pending_inputs = vec![UiPendingProjectInput {
        message_id,
        thread_id: [33; 32],
        sequence: 2,
    }];
    let mut pending_snapshot = projects_snapshot(2, vec![pending_project]);
    let conversation_row = format!("project:{}:{}", agent_row_id(5), agent_row_id(33));
    pending_snapshot.inbox_rows.push(UiRow {
        id: conversation_row.clone(),
        title: "builder".to_owned(),
        detail: "Retain this instruction".to_owned(),
        state: UiRowState::Open,
        kind: UiRowKind::Conversation,
        conversation_target: Some(UiConversationTarget::Project {
            project_id: [5; 32],
            thread_id: [33; 32],
            root_message: message_id,
        }),
    });
    let agent_source = agents_snapshot(2, vec![agent]);
    pending_snapshot.agents = agent_source.agents;
    pending_snapshot.agent_rows = agent_source.agent_rows;
    let submitting = update(
        sent.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot: pending_snapshot,
        },
    )
    .expect("activation submitted");
    assert!(submitting.model.new_modal().is_none());
    assert_eq!(
        submitting.model.selected_row(),
        Some(conversation_row.as_str())
    );
    assert!(submitting.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::ObserveConversation { row_id: Some(row_id) } if row_id == &conversation_row
    )));
    let (effect_id, action) = project_effect(&submitting.effects);
    for outcome in [
        UiProjectOutcome::Rejected {
            category: "conflict".to_owned(),
            code: "agent_changed".to_owned(),
        },
        UiProjectOutcome::Reconcilable {
            stage: "runtime".to_owned(),
            category: "uncertain".to_owned(),
            code: "response_lost".to_owned(),
            warning: None,
        },
    ] {
        let result = update(
            submitting.model.clone(),
            UiEvent::ProjectCommandCompleted {
                effect_id,
                result: UiProjectResult {
                    action: action.clone(),
                    command_id: [30; 32],
                    operation_id: [31; 32],
                    project_id: [5; 32],
                    runtime_state: None,
                    runtime_code: None,
                    outcome,
                },
            },
        )
        .expect("typed exceptional result");
        assert!(matches!(
            result.model.project_interaction(),
            Some(UiProjectInteraction::Outcome { .. })
        ));
        let recovered = update(result.model, UiEvent::Input(UiInput::Escape))
            .expect("close the outcome")
            .model;
        assert!(recovered.new_modal().is_none());
        assert!(recovered.project_interaction().is_none());
        let next = update(recovered, UiEvent::Input(UiInput::Activate))
            .expect("ordinary project navigation resumes");
        assert!(
            next.effects
                .iter()
                .all(|effect| !matches!(effect, UiEffect::SubmitProjectCommand { .. }))
        );
    }

    let failed = update(
        submitting.model,
        UiEvent::ProjectCommandFailed {
            effect_id,
            failure: UiFailure {
                code: "node_unavailable".to_owned(),
                action: "reload project state".to_owned(),
            },
        },
    )
    .expect("transport failure exits guided submission");
    assert!(failed.model.new_modal().is_none());
    assert_eq!(
        failed
            .model
            .project_summary()
            .map(|project| project.project_id),
        Some([5; 32])
    );
    assert_eq!(
        failed.model.project_workspace_level(),
        UiProjectWorkspaceLevel::Summary
    );
    assert!(
        update(failed.model, UiEvent::Input(UiInput::Activate))
            .expect("details remain ordinary navigation")
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitProjectCommand { .. }))
    );
}

#[test]
fn guided_project_work_only_offers_available_provider_choices_when_needed() {
    let providers = vec![
        available_provider("alpha", "Alpha", false),
        available_provider("codex", "Codex", true),
        UiProvider {
            provider: "offline".to_owned(),
            name: "Offline".to_owned(),
            available: false,
            configured_default: false,
        },
    ];
    let mut model = loaded_projects_model_with_agents_and_providers(
        1,
        vec![project(5, "release", "/work/release")],
        vec![project_agent(7, [9; 32])],
        providers,
    );
    for input in [
        UiInput::Character('n'),
        UiInput::Activate,
        UiInput::Activate,
        UiInput::Activate,
    ] {
        model = update(model, UiEvent::Input(input))
            .expect("guided step")
            .model;
    }
    assert!(matches!(
        model.new_modal(),
        Some(UiNewModal::ChooseProvider { provider, .. }) if provider == "codex"
    ));
    model = update(model, UiEvent::Input(UiInput::PreviousItem))
        .expect("choose previous available provider")
        .model;
    assert!(matches!(
        model.new_modal(),
        Some(UiNewModal::ChooseProvider { provider, .. }) if provider == "alpha"
    ));
    let composing = update(model, UiEvent::Input(UiInput::Activate)).expect("choose provider");
    assert!(matches!(
        composing.model.mailbox_draft(),
        Some(UiMailboxDraftPane::Loading {
            target: UiMailboxDraftTarget::Project { project_id, thread_id: None }
        }) if *project_id == [5; 32]
    ));
    assert!(
        composing
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitProjectCommand { .. }))
    );
}

#[test]
fn guided_project_work_keeps_an_actionable_state_without_a_provider() {
    let mut model = loaded_projects_model_with_agents_and_providers(
        1,
        vec![project(5, "release", "/work/release")],
        vec![project_agent(7, [9; 32])],
        Vec::new(),
    );
    for input in [
        UiInput::Character('n'),
        UiInput::Activate,
        UiInput::Activate,
        UiInput::Activate,
    ] {
        model = update(model, UiEvent::Input(input))
            .expect("guided step")
            .model;
    }
    assert!(matches!(
        model.new_modal(),
        Some(UiNewModal::ChooseProvider { providers, .. }) if providers.is_empty()
    ));
    let blocked = update(model, UiEvent::Input(UiInput::Activate)).expect("provider gate");
    assert_eq!(
        blocked
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("guided_provider_unavailable")
    );
    assert!(
        blocked
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitProjectCommand { .. }))
    );
}

#[test]
fn guided_project_work_resumes_an_exact_historical_thread_without_provider_confirmation() {
    let agent = project_agent(7, [9; 32]);
    let mut target = project(5, "release", "/work/release");
    target.threads.push(UiProjectThread {
        agent_id: agent.agent_id,
        provider: "codex".to_owned(),
        session: "saved-session".to_owned(),
        thread_id: [44; 32],
    });
    let mut model =
        loaded_projects_model_with_agents_and_providers(1, vec![target], vec![agent], Vec::new());
    for input in [
        UiInput::Character('n'),
        UiInput::Activate,
        UiInput::Activate,
    ] {
        model = update(model, UiEvent::Input(input))
            .expect("guided step")
            .model;
    }
    let resuming = update(model, UiEvent::Input(UiInput::Activate)).expect("select agent");
    assert!(
        resuming
            .model
            .new_modal()
            .is_some_and(|modal| matches!(modal, UiNewModal::Working { .. }))
    );
    assert!(matches!(
        project_effect(&resuming.effects).1,
        UiProjectAction::Activate {
            provider,
            resume_session: Some(session),
            resume_thread: Some(thread),
            ..
        } if provider == "codex" && session == "saved-session" && thread == [44; 32]
    ));
}

#[test]
fn guided_project_work_reviews_a_real_handoff_before_submitting_it() {
    let receiving_agent = project_agent(7, [9; 32]);
    let current_agent = project_agent(8, [9; 32]);
    let mut target = project(5, "release", "/work/release");
    target.assignment = Some(UiProjectAssignment {
        assignment_id: [9; 32],
        agent_id: current_agent.agent_id,
        provider: "codex".to_owned(),
        session: Some("current-session".to_owned()),
        phase: "blocked".to_owned(),
        thread_id: Some([10; 32]),
        launch_directory: Some("/work/release".to_owned()),
        blocked: Some("runtime_unavailable".to_owned()),
        cardinality_conflicted: false,
        runnable: false,
    });
    target.threads.push(UiProjectThread {
        agent_id: receiving_agent.agent_id,
        provider: "codex".to_owned(),
        session: "receiving-session".to_owned(),
        thread_id: [44; 32],
    });
    let mut model = loaded_projects_model_with_agents_and_providers(
        1,
        vec![target],
        vec![receiving_agent, current_agent],
        Vec::new(),
    );
    for input in [
        UiInput::Character('n'),
        UiInput::Activate,
        UiInput::Activate,
        UiInput::Activate,
    ] {
        model = update(model, UiEvent::Input(input))
            .expect("guided handoff step")
            .model;
    }
    assert!(matches!(
        model.new_modal(),
        Some(UiNewModal::ReviewProject {
            resumes_existing: true,
            moves_project: true,
            ..
        })
    ));

    let submitted = update(model, UiEvent::Input(UiInput::Activate)).expect("confirm handoff");
    assert!(matches!(
        project_effect(&submitted.effects).1,
        UiProjectAction::Handoff {
            agent_id,
            provider,
            resume_session: Some(session),
            thread_id,
            force_takeover: false,
            ..
        } if agent_id == [7; 32]
            && provider == "codex"
            && session == "receiving-session"
            && thread_id == [44; 32]
    ));
}

#[test]
fn guided_project_work_refreshes_provider_choices() {
    let project = project(5, "release", "/work/release");
    let agent = project_agent(7, [9; 32]);
    let mut model = loaded_projects_model_with_agents_and_providers(
        1,
        vec![project.clone()],
        vec![agent.clone()],
        vec![
            available_provider("alpha", "Alpha", false),
            available_provider("codex", "Codex", true),
        ],
    );
    for input in [
        UiInput::Character('n'),
        UiInput::Activate,
        UiInput::Activate,
        UiInput::Activate,
    ] {
        model = update(model, UiEvent::Input(input))
            .expect("guided step")
            .model;
    }
    let invalidated = update(model, UiEvent::Invalidated { revision: 2 }).expect("refresh");
    let snapshot_id = snapshot_effect(&invalidated.effects);
    let mut refreshed = projects_snapshot(2, vec![project]);
    let agent_source = agents_snapshot(2, vec![agent]);
    refreshed.agents = agent_source.agents;
    refreshed.agent_rows = agent_source.agent_rows;
    refreshed.providers = vec![available_provider("alpha", "Alpha", true)];
    let reloaded = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot: refreshed,
        },
    )
    .expect("provider catalog replaced");
    assert!(matches!(
        reloaded.model.new_modal(),
        Some(UiNewModal::ChooseProvider {
            provider,
            providers,
            ..
        }) if provider == "alpha" && providers.len() == 1
    ));
}

#[test]
fn guided_project_work_names_a_competing_resource_owner_before_setup() {
    let mut target = project(5, "release", "/work/release");
    target.claimable = false;
    target.resources[0].active_claim = false;
    target.resources[0].conflicting_projects = vec![[4; 32]];
    let competitor = project(4, "payments", "/work");
    let mut model = loaded_projects_model(1, vec![target, competitor]);
    for input in [
        UiInput::Character('n'),
        UiInput::Activate,
        UiInput::Activate,
    ] {
        model = update(model, UiEvent::Input(input))
            .expect("guided step")
            .model;
    }
    assert!(matches!(
        model.new_modal(),
        Some(UiNewModal::ProjectUnavailable {
            project,
            competing_project: Some(competing_project),
            reason,
        }) if project.name == "release"
            && competing_project == "payments"
            && reason == "folder ownership needs attention"
    ));
    assert!(
        update(model, UiEvent::Input(UiInput::Activate))
            .expect("inspect conflict")
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitProjectCommand { .. }))
    );
}

#[test]
fn startup_allocates_explicit_snapshot_and_redraw_effects() {
    let transition = update(
        UiModel::new(UiSize {
            width: 120,
            height: 32,
        }),
        UiEvent::Started,
    )
    .expect("startup transition");
    assert_eq!(transition.model.connection(), UiConnectionState::Connecting);
    assert_eq!(transition.effects.len(), 2);
    let UiEffect::LoadSnapshot { id: snapshot_id } = &transition.effects[0] else {
        panic!("first effect loads a snapshot");
    };
    let snapshot_id = *snapshot_id;
    assert_eq!(transition.model.pending_snapshot(), Some(snapshot_id));
    assert_eq!(transition.effects[1], UiEffect::RequestRedraw);
}

#[test]
fn stale_snapshot_success_and_failure_cannot_overwrite_newer_state() {
    let started = started_model();
    let first_id = snapshot_effect(&started.effects);
    let invalidated = update(started.model, UiEvent::Invalidated { revision: 8 })
        .expect("invalidation coalesces");
    let first_loaded = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: first_id,
            snapshot: snapshot(4, &["old"]),
        },
    )
    .expect("old snapshot triggers follow-up");
    let second_id = snapshot_effect(&first_loaded.effects);
    assert_ne!(first_id, second_id);

    let ready = update(
        first_loaded.model,
        UiEvent::SnapshotLoaded {
            effect_id: second_id,
            snapshot: snapshot(9, &["current"]),
        },
    )
    .expect("current snapshot applies");
    assert_eq!(ready.model.snapshot().map(|value| value.revision), Some(9));
    let stale_success = update(
        ready.model,
        UiEvent::SnapshotLoaded {
            effect_id: first_id,
            snapshot: snapshot(3, &["stale"]),
        },
    )
    .expect("stale success is inert");
    assert!(stale_success.effects.is_empty());
    assert_eq!(
        stale_success.model.snapshot().map(|value| value.revision),
        Some(9)
    );
    let stale_failure = update(
        stale_success.model,
        UiEvent::SnapshotFailed {
            effect_id: first_id,
            failure: UiFailure {
                code: "stale_failure".to_owned(),
                action: "ignore this old result".to_owned(),
            },
        },
    )
    .expect("stale failure is inert");
    assert_eq!(stale_failure.model.connection(), UiConnectionState::Ready);
    assert!(stale_failure.model.last_failure().is_none());
}

#[test]
fn invalidations_coalesce_and_one_matching_failure_schedules_one_retry() {
    let started = started_model();
    let request_id = snapshot_effect(&started.effects);
    let first =
        update(started.model, UiEvent::Invalidated { revision: 10 }).expect("first invalidation");
    assert_eq!(first.model.required_revision(), Some(10));
    assert_eq!(redraw_count(&first.effects), 1);
    let second =
        update(first.model, UiEvent::Invalidated { revision: 7 }).expect("older invalidation");
    assert_eq!(second.model.required_revision(), Some(10));
    assert!(second.effects.is_empty());
    let failed = update(
        second.model,
        UiEvent::SnapshotFailed {
            effect_id: request_id,
            failure: UiFailure {
                code: "node_unavailable".to_owned(),
                action: "waiting to reconnect".to_owned(),
            },
        },
    )
    .expect("matching failure applies");
    assert_eq!(failed.model.connection(), UiConnectionState::Reconnecting);
    assert_eq!(
        failed.model.last_failure().map(|value| value.code.as_str()),
        Some("node_unavailable")
    );
    assert!(matches!(
        failed.effects.as_slice(),
        [
            UiEffect::ScheduleTimer {
                kind: UiTimerKind::RetrySnapshot,
                after,
                ..
            },
            UiEffect::RequestRedraw
        ] if *after == Duration::from_millis(250)
    ));
}

#[test]
fn logical_selection_focus_section_resize_and_quit_are_pure_transitions() {
    let started = started_model();
    let request_id = snapshot_effect(&started.effects);
    let loaded = update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: request_id,
            snapshot: snapshot(1, &["alpha", "beta", "gamma"]),
        },
    )
    .expect("snapshot applies");
    assert_eq!(loaded.model.selected_row(), Some("alpha"));

    let down = update(loaded.model, UiEvent::Input(UiInput::NextItem)).expect("move down");
    assert_eq!(down.model.selected_row(), Some("alpha"));
    assert!(down.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::ObserveConversation { row_id: Some(row_id) } if row_id == "beta"
    )));
    let focused = update(down.model, UiEvent::Input(UiInput::NextFocus)).expect("change focus");
    assert_eq!(focused.model.focus(), UiFocus::Content);
    let section =
        update(focused.model, UiEvent::Input(UiInput::NextSection)).expect("change section");
    assert_eq!(section.model.section(), UiSection::Sent);
    let resized = update(
        section.model,
        UiEvent::Resized(UiSize {
            width: 70,
            height: 18,
        }),
    )
    .expect("resize transition");
    assert_eq!(
        resized.model.viewport(),
        UiSize {
            width: 70,
            height: 18,
        }
    );
    let quit = update(resized.model, UiEvent::Input(UiInput::Quit)).expect("quit transition");
    assert!(quit.model.should_exit());
    assert_eq!(quit.effects, vec![UiEffect::Exit]);
}

#[test]
fn contextual_help_freezes_background_actions_and_survives_resize() {
    let loaded = loaded_model(snapshot(1, &["alpha", "beta"]));
    let selected = loaded.selected_row().map(str::to_owned);

    let opened = update(loaded, UiEvent::Input(UiInput::Character('?'))).expect("open help");
    assert_eq!(opened.model.help_page(), Some(UiHelpPage::Context));
    assert_eq!(redraw_count(&opened.effects), 1);

    let frozen = update(opened.model, UiEvent::Input(UiInput::NextItem))
        .expect("help owns navigation input");
    assert_eq!(frozen.model.selected_row(), selected.as_deref());
    assert_eq!(frozen.model.help_page(), Some(UiHelpPage::Context));
    assert!(frozen.effects.is_empty());

    let quit = update(
        frozen.model.clone(),
        UiEvent::Input(UiInput::Character('q')),
    )
    .expect("quit remains available from help");
    assert!(quit.model.should_exit());
    assert_eq!(quit.effects, vec![UiEffect::Exit]);

    let technical =
        update(frozen.model, UiEvent::Input(UiInput::Character('t'))).expect("show technical help");
    assert_eq!(technical.model.help_page(), Some(UiHelpPage::Technical));
    assert_eq!(redraw_count(&technical.effects), 1);

    let invalidated = update(technical.model, UiEvent::Invalidated { revision: 2 })
        .expect("refresh while help is open");
    let refresh_id = snapshot_effect(&invalidated.effects);
    let refreshed = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: refresh_id,
            snapshot: snapshot(2, &["alpha", "beta"]),
        },
    )
    .expect("authoritative refresh while help is open");
    assert_eq!(refreshed.model.help_page(), Some(UiHelpPage::Technical));

    let resized = update(
        refreshed.model,
        UiEvent::Resized(UiSize {
            width: 64,
            height: 18,
        }),
    )
    .expect("resize while help is open");
    assert_eq!(resized.model.help_page(), Some(UiHelpPage::Technical));

    let closed = update(resized.model, UiEvent::Input(UiInput::Escape)).expect("close help");
    assert_eq!(closed.model.help_page(), None);
    assert_eq!(redraw_count(&closed.effects), 1);
}

#[test]
fn explicit_help_and_refresh_work_without_discarding_an_open_dialog() {
    let loaded = loaded_model(snapshot(1, &[]));
    let launcher = update(loaded, UiEvent::Input(UiInput::Character('n')))
        .expect("open launcher")
        .model;
    let retained = launcher.new_modal().cloned();

    let helped = update(launcher, UiEvent::Input(UiInput::Help)).expect("F1 help");
    assert_eq!(helped.model.help_page(), Some(UiHelpPage::Context));
    assert_eq!(helped.model.new_modal(), retained.as_ref());
    let escaped = update(helped.model, UiEvent::Input(UiInput::Escape))
        .expect("Escape closes help before the underlying dialog");
    assert_eq!(escaped.model.help_page(), None);
    assert_eq!(escaped.model.new_modal(), retained.as_ref());
    let reopened = update(escaped.model, UiEvent::Input(UiInput::Help)).expect("reopen F1 help");
    let closed = update(reopened.model, UiEvent::Input(UiInput::Help)).expect("close F1 help");
    assert_eq!(closed.model.help_page(), None);
    assert_eq!(closed.model.new_modal(), retained.as_ref());

    let refreshed = update(closed.model, UiEvent::Input(UiInput::Refresh)).expect("F5 refresh");
    assert_eq!(refreshed.model.new_modal(), retained.as_ref());
    assert!(
        refreshed
            .effects
            .iter()
            .any(|effect| matches!(effect, UiEffect::LoadSnapshot { .. }))
    );
}

#[test]
fn wide_sidebar_uses_arrow_keys_and_clamps_at_section_boundaries() {
    let started = update(
        UiModel::new(UiSize {
            width: 120,
            height: 30,
        }),
        UiEvent::Started,
    )
    .expect("start wide model");
    let request = snapshot_effect(&started.effects);
    let mut source = snapshot(1, &["inbox"]);
    source.sent_rows = snapshot_for(UiSection::Sent, 1, &["sent-a", "sent-b"]).sent_rows;
    let loaded = update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: request,
            snapshot: source,
        },
    )
    .expect("load complete snapshot");

    let inbox = update(loaded.model, UiEvent::Input(UiInput::PreviousItem))
        .expect("up at first section is inert");
    assert_eq!(inbox.model.section(), UiSection::Inbox);

    let sent = update(inbox.model, UiEvent::Input(UiInput::NextItem)).expect("down in sidebar");
    assert_eq!(sent.model.section(), UiSection::Sent);
    assert_eq!(sent.model.focus(), UiFocus::Navigation);
    assert_eq!(sent.model.selected_row(), Some("sent-a"));

    let unchanged = update(sent.model, UiEvent::Input(UiInput::Character('j')))
        .expect("j cannot leave the sidebar control");
    assert_eq!(unchanged.model.section(), UiSection::Sent);
    let unchanged = update(unchanged.model, UiEvent::Input(UiInput::Character('k')))
        .expect("k cannot leave the sidebar control");
    assert_eq!(unchanged.model.section(), UiSection::Sent);

    let content = update(unchanged.model, UiEvent::Input(UiInput::NextSection))
        .expect("right focuses content");
    assert_eq!(content.model.section(), UiSection::Sent);
    assert_eq!(content.model.focus(), UiFocus::Content);
    let second =
        update(content.model, UiEvent::Input(UiInput::NextItem)).expect("down moves content row");
    assert_eq!(second.model.selected_row(), Some("sent-b"));
    let navigation = update(second.model, UiEvent::Input(UiInput::PreviousSection))
        .expect("left returns to sidebar");
    assert_eq!(navigation.model.section(), UiSection::Sent);
    assert_eq!(navigation.model.focus(), UiFocus::Navigation);
}

#[test]
fn narrow_section_navigation_clamps_and_vim_keys_do_not_wrap_config_to_inbox() {
    let started = update(
        UiModel::new(UiSize {
            width: 70,
            height: 30,
        }),
        UiEvent::Started,
    )
    .expect("start compact model");
    let request = snapshot_effect(&started.effects);
    let mut model = update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: request,
            snapshot: snapshot(1, &["inbox"]),
        },
    )
    .expect("load compact model")
    .model;

    model = update(model, UiEvent::Input(UiInput::MoveCursorLeft))
        .expect("left at Inbox is inert")
        .model;
    assert_eq!(model.section(), UiSection::Inbox);
    for _ in 0..5 {
        model = update(model, UiEvent::Input(UiInput::NextSection))
            .expect("visit next section")
            .model;
    }
    assert_eq!(model.section(), UiSection::Config);

    for input in [
        UiInput::MoveCursorRight,
        UiInput::Character('h'),
        UiInput::Character('j'),
        UiInput::Character('k'),
        UiInput::Character('l'),
    ] {
        model = update(model, UiEvent::Input(input))
            .expect("boundary input is inert")
            .model;
        assert_eq!(model.section(), UiSection::Config);
        assert_eq!(model.focus(), UiFocus::Navigation);
    }
}

#[test]
fn authoritative_refresh_retains_visible_rows_until_replacement_arrives() {
    let model = loaded_model(snapshot(1, &["retained"]));
    let refreshing =
        update(model, UiEvent::Invalidated { revision: 2 }).expect("start background refresh");
    assert!(refreshing.model.refreshing());
    assert_eq!(refreshing.model.selected_row(), Some("retained"));
    assert_eq!(
        refreshing
            .model
            .rows()
            .and_then(|rows| rows.first())
            .map(|row| row.id.as_str()),
        Some("retained")
    );
    assert!(
        refreshing
            .effects
            .iter()
            .any(|effect| matches!(effect, UiEffect::LoadSnapshot { .. }))
    );
}

#[test]
fn authoritative_refresh_replaces_the_typed_human_recovery_condition() {
    let loaded = loaded_model(snapshot(1, &["thread"]));
    assert_eq!(loaded.human_state(), Some(&UiHumanState::Ready));

    let invalidated = update(loaded, UiEvent::Invalidated { revision: 2 })
        .expect("request authoritative refresh");
    let refresh_id = snapshot_effect(&invalidated.effects);
    let mut refreshed_snapshot = snapshot(2, &["thread"]);
    refreshed_snapshot.human_state =
        UiHumanState::NeedsAttention(hq_tui::UiHumanIssue::SelectedWithoutAuthority {
            account_id: [7; 32],
            selection_frontier: vec![[8; 32]],
        });
    let refreshed = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: refresh_id,
            snapshot: refreshed_snapshot,
        },
    )
    .expect("replace human recovery condition");
    assert!(matches!(
        refreshed.model.human_state(),
        Some(UiHumanState::NeedsAttention(
            hq_tui::UiHumanIssue::SelectedWithoutAuthority { account_id, .. }
        )) if *account_id == [7; 32]
    ));
}

#[test]
fn reload_preserves_a_logical_selection_and_falls_back_when_it_disappears() {
    let started = started_model();
    let first_id = snapshot_effect(&started.effects);
    let first = update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: first_id,
            snapshot: snapshot(1, &["alpha", "beta"]),
        },
    )
    .expect("first snapshot");
    let selected = update(first.model, UiEvent::Input(UiInput::NextItem)).expect("select beta");
    let invalidated =
        update(selected.model, UiEvent::Invalidated { revision: 2 }).expect("request reload");
    let second_id = snapshot_effect(&invalidated.effects);
    let preserved = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: second_id,
            snapshot: snapshot(2, &["gamma", "beta"]),
        },
    )
    .expect("selection preserved");
    assert_eq!(preserved.model.selected_row(), Some("gamma"));

    let invalidated = update(preserved.model, UiEvent::Invalidated { revision: 3 })
        .expect("request another reload");
    let third_id = snapshot_effect(&invalidated.effects);
    let replaced = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: third_id,
            snapshot: snapshot(3, &["delta"]),
        },
    )
    .expect("missing selection falls back");
    assert_eq!(replaced.model.selected_row(), Some("delta"));
}

#[test]
fn section_change_uses_the_complete_in_flight_snapshot_without_another_request() {
    let started = started_model();
    let snapshot_id = snapshot_effect(&started.effects);
    let sent = update(started.model, UiEvent::Input(UiInput::NextSection))
        .expect("section changes while complete snapshot is pending");
    assert_eq!(sent.model.section(), UiSection::Sent);

    let mut complete = snapshot(4, &["inbox"]);
    complete.sent_rows = snapshot_for(UiSection::Sent, 4, &["sent"]).sent_rows;
    let loaded = update(
        sent.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot: complete,
        },
    )
    .expect("complete snapshot applies to selected section");
    assert_eq!(loaded.model.selected_row(), Some("sent"));
    assert!(
        !loaded
            .effects
            .iter()
            .any(|effect| matches!(effect, UiEffect::LoadSnapshot { .. }))
    );

    let inbox = update(loaded.model, UiEvent::Input(UiInput::PreviousSection))
        .expect("cached inbox is immediately available");
    assert_eq!(inbox.model.section(), UiSection::Inbox);
    assert_eq!(inbox.model.selected_row(), Some("inbox"));
    assert!(
        !inbox
            .effects
            .iter()
            .any(|effect| matches!(effect, UiEffect::LoadSnapshot { .. }))
    );
}

#[test]
fn connection_observations_ignore_older_generations() {
    let started = started_model();
    let reconnecting = update(
        started.model,
        UiEvent::ConnectionObserved {
            generation: 4,
            state: UiConnectionState::Reconnecting,
        },
    )
    .expect("new generation applies");
    assert_eq!(
        reconnecting.model.connection(),
        UiConnectionState::Reconnecting
    );

    let stale = update(
        reconnecting.model,
        UiEvent::ConnectionObserved {
            generation: 3,
            state: UiConnectionState::Ready,
        },
    )
    .expect("old generation is inert");
    assert_eq!(stale.model.connection(), UiConnectionState::Reconnecting);
    assert!(stale.effects.is_empty());

    let recovered = update(
        stale.model,
        UiEvent::ConnectionObserved {
            generation: 4,
            state: UiConnectionState::Ready,
        },
    )
    .expect("current generation applies");
    assert_eq!(recovered.model.connection(), UiConnectionState::Ready);
    assert_eq!(redraw_count(&recovered.effects), 1);
}

#[test]
fn client_failures_are_scoped_to_the_current_connection_generation() {
    let started = started_model();
    let current = update(
        started.model,
        UiEvent::ClientFailed {
            generation: 4,
            failure: UiFailure {
                code: "connection_lost".to_owned(),
                action: "waiting to reconnect".to_owned(),
            },
        },
    )
    .expect("current failure applies");
    assert_eq!(current.model.connection(), UiConnectionState::Reconnecting);
    assert_eq!(
        current
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("connection_lost")
    );
    assert_eq!(redraw_count(&current.effects), 1);

    let stale = update(
        current.model,
        UiEvent::ClientFailed {
            generation: 3,
            failure: UiFailure {
                code: "stale_failure".to_owned(),
                action: "ignore old generation".to_owned(),
            },
        },
    )
    .expect("older failure is inert");
    assert_eq!(
        stale
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("connection_lost")
    );
    assert!(stale.effects.is_empty());
}

#[test]
fn stale_timer_completions_cannot_repeat_effects() {
    let started = started_model();
    let snapshot_id = snapshot_effect(&started.effects);
    let failed = update(
        started.model,
        UiEvent::SnapshotFailed {
            effect_id: snapshot_id,
            failure: UiFailure {
                code: "connection_lost".to_owned(),
                action: "retry".to_owned(),
            },
        },
    )
    .expect("snapshot failure schedules retry");
    let retry_id = timer_effect(&failed.effects, UiTimerKind::RetrySnapshot);
    let elapsed = update(
        failed.model,
        UiEvent::TimerElapsed {
            effect_id: retry_id,
        },
    )
    .expect("current timer applies");
    assert!(
        elapsed
            .effects
            .iter()
            .any(|effect| matches!(effect, UiEffect::LoadSnapshot { .. }))
    );

    let stale = update(
        elapsed.model,
        UiEvent::TimerElapsed {
            effect_id: retry_id,
        },
    )
    .expect("stale timer is inert");
    assert!(stale.effects.is_empty());
}

#[test]
fn conversation_pages_preserve_reducer_order_and_use_stable_entry_anchors() {
    let opened = materialized_transition(
        snapshot(1, &["thread-a"]),
        UiConversationPage {
            title: "Alice".to_owned(),
            context: Some("Project · Release".to_owned()),
            row_id: "thread-a".to_owned(),
            entries: vec![entry("message-1", false), entry("activity-2", true)],
            next_cursor: Some("next-page".to_owned()),
        },
    );
    assert_eq!(opened.model.conversation_anchor(), Some("activity-2"));
    let conversation = opened.model.conversation().expect("conversation");
    assert_eq!(conversation.title, "Alice");
    assert_eq!(conversation.context.as_deref(), Some("Project · Release"));
    assert_eq!(
        conversation
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        ["message-1", "activity-2"]
    );

    let focused =
        update(opened.model, UiEvent::Input(UiInput::Activate)).expect("focus conversation");
    let earlier =
        update(focused.model, UiEvent::Input(UiInput::Character('k'))).expect("move earlier");
    assert_eq!(earlier.model.conversation_anchor(), Some("message-1"));
    let moved =
        update(earlier.model, UiEvent::Input(UiInput::Character('j'))).expect("return to tail");
    assert_eq!(moved.model.conversation_anchor(), Some("activity-2"));
    let technical = update(moved.model, UiEvent::Input(UiInput::Activate)).expect("show details");
    assert!(technical.model.technical_visible());
    let resized = update(
        technical.model,
        UiEvent::Resized(UiSize {
            width: 66,
            height: 17,
        }),
    )
    .expect("resize");
    assert_eq!(resized.model.conversation_anchor(), Some("activity-2"));
    assert!(resized.model.technical_visible());

    let observed =
        observe_conversation_viewport(resized.model, &[("message-1", 3), ("activity-2", 5)], 4);
    let more = update(observed.model, UiEvent::Input(UiInput::LoadMore)).expect("load more");
    let (more_id, more_row, more_cursor) = conversation_effect(&more.effects);
    assert_eq!(more_row, "thread-a");
    assert_eq!(more_cursor, Some("next-page"));
    let appended = update(
        more.model,
        UiEvent::ConversationLoaded {
            effect_id: more_id,
            page: UiConversationPage {
                title: "Alice".to_owned(),
                context: Some("Project · Release".to_owned()),
                row_id: "thread-a".to_owned(),
                entries: vec![entry("message-3", false)],
                next_cursor: None,
            },
        },
    )
    .expect("next page appends");
    assert_eq!(
        appended
            .model
            .conversation()
            .expect("conversation")
            .entries
            .len(),
        3
    );
    assert_eq!(appended.model.conversation_anchor(), Some("activity-2"));
    assert_eq!(
        appended.model.conversation_viewport_position(),
        Some(&UiConversationViewportPosition {
            entry_id: "activity-2".to_owned(),
            row: 0,
        }),
        "older-page application preserves the stable top row until fresh geometry arrives"
    );
}

#[test]
fn conversation_viewport_scrolls_visual_rows_independently_from_entry_navigation() {
    let model = opened_conversation(vec![
        entry("message-1", false),
        entry("message-2", false),
        entry("message-3", false),
    ]);
    let observed = observe_conversation_viewport(
        model,
        &[("message-1", 3), ("message-2", 10), ("message-3", 3)],
        5,
    );
    assert_eq!(
        observed.model.conversation_viewport_position(),
        Some(&UiConversationViewportPosition {
            entry_id: "message-2".to_owned(),
            row: 8,
        })
    );
    assert!(observed.model.conversation_follows_tail());

    let scrolled = update(observed.model, UiEvent::Input(UiInput::PreviousItem))
        .expect("scroll one visual row");
    assert_eq!(scrolled.model.conversation_anchor(), Some("message-3"));
    assert_eq!(
        scrolled.model.conversation_viewport_position(),
        Some(&UiConversationViewportPosition {
            entry_id: "message-2".to_owned(),
            row: 7,
        })
    );
    assert!(!scrolled.model.conversation_follows_tail());

    let selected = update(scrolled.model, UiEvent::Input(UiInput::Character('k')))
        .expect("jump to previous entry");
    assert_eq!(selected.model.conversation_anchor(), Some("message-2"));
    assert_eq!(
        selected.model.conversation_viewport_position(),
        Some(&UiConversationViewportPosition {
            entry_id: "message-2".to_owned(),
            row: 0,
        })
    );

    let within_entry = update(selected.model, UiEvent::Input(UiInput::NextItem))
        .expect("scroll inside selected entry");
    assert_eq!(within_entry.model.conversation_anchor(), Some("message-2"));
    assert_eq!(
        within_entry.model.conversation_viewport_position(),
        Some(&UiConversationViewportPosition {
            entry_id: "message-2".to_owned(),
            row: 1,
        })
    );
}

#[test]
fn entering_a_conversation_and_submitting_an_interaction_restore_follow_tail() {
    let model = opened_conversation(vec![
        entry("message-1", false),
        entry("message-2", false),
        entry("activity-3", true),
    ]);
    let observed = observe_conversation_viewport(
        model,
        &[("message-1", 3), ("message-2", 4), ("activity-3", 3)],
        4,
    );
    let scrolled = update(observed.model, UiEvent::Input(UiInput::PreviousItem))
        .expect("manual upward scroll");
    assert!(!scrolled.model.conversation_follows_tail());

    let content = update(scrolled.model, UiEvent::Input(UiInput::MoveCursorLeft))
        .expect("leave conversation focus");
    let reopened = update(content.model, UiEvent::Input(UiInput::MoveCursorRight))
        .expect("reenter conversation");
    assert_eq!(reopened.model.conversation_anchor(), Some("activity-3"));
    assert!(reopened.model.conversation_follows_tail());
    assert_eq!(
        reopened.model.conversation_viewport_position(),
        Some(&UiConversationViewportPosition {
            entry_id: "message-2".to_owned(),
            row: 3,
        })
    );

    let scrolled_again = update(reopened.model, UiEvent::Input(UiInput::PreviousItem))
        .expect("manual upward scroll remains available");
    let prompted = update(
        scrolled_again.model,
        UiEvent::InteractionsObserved {
            interactions: vec![UiInteraction {
                agent_id: [6; 32],
                agent_name: "alice".to_owned(),
                project_id: None,
                project_name: None,
                provider: "codex".to_owned(),
                session: "conversation-1".to_owned(),
                request_id: [7; 32],
                operation_id: [8; 32],
                kind: UiInteractionKind::CommandApproval,
                prompt: "Run command?".to_owned(),
                choices: vec![UiInteractionChoice {
                    value: "deny".to_owned(),
                    label: "Deny".to_owned(),
                }],
                allow_text: false,
            }],
        },
    )
    .expect("interaction opens");
    let submitted =
        update(prompted.model, UiEvent::Input(UiInput::Activate)).expect("denial submitted");
    assert_eq!(submitted.model.conversation_anchor(), Some("activity-3"));
    assert!(submitted.model.conversation_follows_tail());

    let scrolled_again = update(submitted.model, UiEvent::Input(UiInput::PreviousItem))
        .expect("manual scroll after interaction");
    let content = update(
        scrolled_again.model,
        UiEvent::Input(UiInput::MoveCursorLeft),
    )
    .expect("return to inbox selection");
    let activated = update(content.model, UiEvent::Input(UiInput::Activate))
        .expect("activate selected conversation");
    assert_eq!(activated.model.conversation_anchor(), Some("activity-3"));
    assert!(activated.model.conversation_follows_tail());
}

#[test]
fn command_approval_owns_vim_navigation_over_conversation_focus() {
    let model = opened_conversation(vec![entry("activity", true)]);
    assert_eq!(model.focus(), UiFocus::Conversation);
    let prompted = update(
        model,
        UiEvent::InteractionsObserved {
            interactions: vec![UiInteraction {
                agent_id: [6; 32],
                agent_name: "alice".to_owned(),
                project_id: None,
                project_name: None,
                provider: "codex".to_owned(),
                session: "conversation-1".to_owned(),
                request_id: [7; 32],
                operation_id: [8; 32],
                kind: UiInteractionKind::CommandApproval,
                prompt: "Command approval".to_owned(),
                choices: vec![
                    UiInteractionChoice {
                        value: "accept".to_owned(),
                        label: "accept".to_owned(),
                    },
                    UiInteractionChoice {
                        value: "acceptForSession".to_owned(),
                        label: "acceptForSession".to_owned(),
                    },
                    UiInteractionChoice {
                        value: "decline".to_owned(),
                        label: "decline".to_owned(),
                    },
                ],
                allow_text: false,
            }],
        },
    )
    .expect("approval opens");

    let next = update(prompted.model, UiEvent::Input(UiInput::Character('j')))
        .expect("j selects next approval choice");
    assert!(matches!(
        next.model.interaction_modal(),
        Some(UiInteractionModal::Prompt { selected: 1, .. })
    ));
    let previous = update(next.model, UiEvent::Input(UiInput::Character('k')))
        .expect("k selects previous approval choice");
    assert!(matches!(
        previous.model.interaction_modal(),
        Some(UiInteractionModal::Prompt { selected: 0, .. })
    ));
    let next = update(previous.model, UiEvent::Input(UiInput::Character('j')))
        .expect("select session approval");
    let submitted =
        update(next.model, UiEvent::Input(UiInput::Activate)).expect("submit selected choice");
    assert!(submitted.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::AnswerInteraction {
            response: UiInteractionResponse::Choice(value),
            ..
        } if value == "acceptForSession"
    )));
}

#[test]
fn conversation_viewport_home_end_and_geometry_refresh_preserve_stable_entry_rows() {
    let model = opened_conversation(vec![entry("message-1", false), entry("message-2", false)]);
    let observed = observe_conversation_viewport(model, &[("message-1", 4), ("message-2", 12)], 5);
    let selected = update(observed.model, UiEvent::Input(UiInput::Character('k')))
        .expect("select first entry");
    let ended = update(selected.model, UiEvent::Input(UiInput::MoveCursorEnd))
        .expect("show selected entry end");
    assert_eq!(
        ended.model.conversation_viewport_position(),
        Some(&UiConversationViewportPosition {
            entry_id: "message-1".to_owned(),
            row: 0,
        })
    );

    let selected = update(ended.model, UiEvent::Input(UiInput::Character('j')))
        .expect("select oversized entry");
    let ended = update(selected.model, UiEvent::Input(UiInput::MoveCursorEnd))
        .expect("show oversized entry end");
    assert_eq!(
        ended.model.conversation_viewport_position(),
        Some(&UiConversationViewportPosition {
            entry_id: "message-2".to_owned(),
            row: 7,
        })
    );
    let refreshed =
        observe_conversation_viewport(ended.model, &[("message-2", 8), ("message-1", 4)], 5);
    assert_eq!(
        refreshed.model.conversation_viewport_position(),
        Some(&UiConversationViewportPosition {
            entry_id: "message-2".to_owned(),
            row: 7,
        }),
        "the same stable entry and nearby row survive reordering"
    );
    let homed = update(refreshed.model, UiEvent::Input(UiInput::MoveCursorHome))
        .expect("show entry beginning");
    assert_eq!(
        homed.model.conversation_viewport_position(),
        Some(&UiConversationViewportPosition {
            entry_id: "message-2".to_owned(),
            row: 0,
        })
    );
}

#[test]
fn conversation_viewport_clamps_restores_workspace_and_remeasures_after_resize() {
    let model = opened_conversation(vec![
        entry("message-1", false),
        entry("message-2", false),
        entry("message-3", false),
    ]);
    let observed = observe_conversation_viewport(
        model,
        &[("message-1", 3), ("message-2", 4), ("message-3", 3)],
        4,
    );
    let at_bottom =
        update(observed.model, UiEvent::Input(UiInput::NextItem)).expect("bottom clamp is inert");
    assert!(at_bottom.model.conversation_follows_tail());
    assert_eq!(redraw_count(&at_bottom.effects), 0);

    let mut scrolled = at_bottom.model;
    for _ in 0..20 {
        scrolled = update(scrolled, UiEvent::Input(UiInput::PreviousItem))
            .expect("scroll toward top")
            .model;
    }
    assert_eq!(
        scrolled.conversation_viewport_position(),
        Some(&UiConversationViewportPosition {
            entry_id: "message-1".to_owned(),
            row: 0,
        })
    );
    assert!(!scrolled.conversation_follows_tail());

    let resized = update(
        scrolled,
        UiEvent::Resized(UiSize {
            width: 100,
            height: 30,
        }),
    )
    .expect("resize preserves stable position");
    assert_eq!(
        resized.model.conversation_viewport_position(),
        Some(&UiConversationViewportPosition {
            entry_id: "message-1".to_owned(),
            row: 0,
        })
    );
    let without_geometry = update(resized.model, UiEvent::Input(UiInput::NextItem))
        .expect("stale geometry cannot move the viewport");
    assert_eq!(redraw_count(&without_geometry.effects), 0);
    let remeasured = observe_conversation_viewport(
        without_geometry.model,
        &[("message-1", 2), ("message-2", 3), ("message-3", 2)],
        6,
    );
    assert_eq!(
        remeasured.model.conversation_viewport_position(),
        Some(&UiConversationViewportPosition {
            entry_id: "message-1".to_owned(),
            row: 0,
        })
    );

    let content = update(remeasured.model, UiEvent::Input(UiInput::MoveCursorLeft))
        .expect("leave conversation");
    let navigation =
        update(content.model, UiEvent::Input(UiInput::MoveCursorLeft)).expect("focus navigation");
    let sent =
        update(navigation.model, UiEvent::Input(UiInput::NextItem)).expect("visit Sent workspace");
    let restored =
        update(sent.model, UiEvent::Input(UiInput::PreviousItem)).expect("restore Inbox workspace");
    assert_eq!(restored.model.section(), UiSection::Inbox);
    assert_eq!(
        restored.model.conversation_viewport_position(),
        Some(&UiConversationViewportPosition {
            entry_id: "message-1".to_owned(),
            row: 0,
        })
    );
}

#[test]
fn materialized_views_install_list_and_detail_atomically_without_first_page_loading() {
    let first_page = UiConversationPage {
        title: "Alice".to_owned(),
        context: None,
        row_id: "thread-a".to_owned(),
        entries: vec![entry("a-message", false)],
        next_cursor: None,
    };
    let observed = update(
        UiModel::new(UiSize {
            width: 100,
            height: 30,
        }),
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot: snapshot(1, &["thread-a", "thread-b"]),
                conversation: Some(first_page),
            },
        },
    )
    .expect("initial coherent view");
    let started = update(observed.model, UiEvent::Started).expect("start from observed state");
    assert!(started.effects.iter().all(|effect| !matches!(
        effect,
        UiEffect::LoadSnapshot { .. } | UiEffect::LoadConversation { .. }
    )));
    assert_eq!(started.model.selected_row(), Some("thread-a"));
    assert_eq!(
        started
            .model
            .conversation()
            .and_then(|conversation| conversation.entries.last())
            .map(|entry| entry.id.as_str()),
        Some("a-message")
    );

    let content =
        update(started.model, UiEvent::Input(UiInput::NextFocus)).expect("focus Inbox content");
    let selecting = update(content.model, UiEvent::Input(UiInput::NextItem))
        .expect("request second conversation");
    assert!(selecting.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::ObserveConversation { row_id: Some(row_id) } if row_id == "thread-b"
    )));
    assert_eq!(selecting.model.selected_row(), Some("thread-a"));
    assert_eq!(
        selecting
            .model
            .conversation()
            .and_then(|conversation| conversation.entries.last())
            .map(|entry| entry.id.as_str()),
        Some("a-message"),
        "the prior coherent pair remains visible while the new view is pending"
    );

    let selected = update(
        selecting.model,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot: snapshot(1, &["thread-a", "thread-b"]),
                conversation: Some(UiConversationPage {
                    title: "Bob".to_owned(),
                    context: None,
                    row_id: "thread-b".to_owned(),
                    entries: vec![entry("b-message", false)],
                    next_cursor: None,
                }),
            },
        },
    )
    .expect("second coherent view");
    assert_eq!(selected.model.selected_row(), Some("thread-b"));
    assert_eq!(
        selected
            .model
            .conversation()
            .and_then(|conversation| conversation.entries.last())
            .map(|entry| entry.id.as_str()),
        Some("b-message")
    );
}

#[test]
fn inbox_selection_eagerly_replaces_preview_loads_without_stealing_list_focus() {
    let loaded = materialized_transition(
        snapshot(1, &["thread-a", "thread-b"]),
        UiConversationPage {
            title: "Alice".to_owned(),
            context: None,
            row_id: "thread-a".to_owned(),
            entries: vec![entry("first-message", false)],
            next_cursor: None,
        },
    );

    let content = update(loaded.model, UiEvent::Input(UiInput::NextFocus)).expect("list focus");
    assert_eq!(content.model.focus(), UiFocus::Content);
    let moved = update(content.model, UiEvent::Input(UiInput::NextItem)).expect("move selection");
    assert_eq!(moved.model.selected_row(), Some("thread-a"));
    assert!(moved.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::ObserveConversation { row_id: Some(row_id) } if row_id == "thread-b"
    )));

    let stale = update(
        moved.model,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot: snapshot(1, &["thread-a", "thread-b"]),
                conversation: Some(UiConversationPage {
                    title: "Alice".to_owned(),
                    context: None,
                    row_id: "thread-a".to_owned(),
                    entries: vec![entry("wrong-message", false)],
                    next_cursor: None,
                }),
            },
        },
    )
    .expect("superseded page is inert");
    assert_eq!(stale.model.conversation_anchor(), Some("first-message"));
    let previewed = update(
        stale.model,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot: snapshot(1, &["thread-a", "thread-b"]),
                conversation: Some(UiConversationPage {
                    title: "Bob".to_owned(),
                    context: None,
                    row_id: "thread-b".to_owned(),
                    entries: vec![entry("right-message", false)],
                    next_cursor: None,
                }),
            },
        },
    )
    .expect("selected preview applies");
    assert_eq!(previewed.model.focus(), UiFocus::Content);
    assert_eq!(previewed.model.conversation_anchor(), Some("right-message"));
}

#[test]
fn entering_a_materialized_conversation_requires_no_page_request() {
    let loaded = materialized_transition(
        snapshot(1, &["thread-a"]),
        UiConversationPage {
            title: "Alice".to_owned(),
            context: None,
            row_id: "thread-a".to_owned(),
            entries: vec![entry("message-1", false)],
            next_cursor: None,
        },
    );
    let content = update(loaded.model, UiEvent::Input(UiInput::NextFocus)).expect("list focus");
    let entering = update(content.model, UiEvent::Input(UiInput::Activate)).expect("entry intent");
    assert_eq!(entering.model.focus(), UiFocus::Conversation);
    assert!(
        entering
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::LoadConversation { .. }))
    );
}

#[test]
fn materialized_first_page_retention_is_lru_bounded() {
    let row_ids = (b'a'..=b'j')
        .map(|byte| format!("thread-{}", char::from(byte)))
        .collect::<Vec<_>>();
    let row_refs = row_ids.iter().map(String::as_str).collect::<Vec<_>>();
    let mut transition = materialized_transition(
        snapshot(1, &row_refs),
        UiConversationPage {
            title: "A".to_owned(),
            context: None,
            row_id: row_ids[0].clone(),
            entries: vec![entry("message-a", false)],
            next_cursor: None,
        },
    );
    transition =
        update(transition.model, UiEvent::Input(UiInput::NextFocus)).expect("focus Inbox rows");
    for (index, row_id) in row_ids.iter().enumerate().take(9).skip(1) {
        let selecting =
            update(transition.model, UiEvent::Input(UiInput::NextItem)).expect("select next row");
        transition = update(
            selecting.model,
            UiEvent::MaterializedViewObserved {
                view: UiMaterializedConversationView {
                    snapshot: snapshot(1, &row_refs),
                    conversation: Some(UiConversationPage {
                        title: row_id.clone(),
                        context: None,
                        row_id: row_id.clone(),
                        entries: vec![entry(&format!("message-{index}"), false)],
                        next_cursor: None,
                    }),
                },
            },
        )
        .expect("observe selected row");
    }

    for expected in (1..8).rev() {
        transition = update(transition.model, UiEvent::Input(UiInput::PreviousItem))
            .expect("reuse retained page");
        assert_eq!(
            transition.model.selected_row(),
            Some(row_ids[expected].as_str())
        );
    }
    let evicted = update(transition.model, UiEvent::Input(UiInput::PreviousItem))
        .expect("request evicted page again");
    assert_eq!(evicted.model.selected_row(), Some(row_ids[1].as_str()));
    assert!(evicted.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::ObserveConversation { row_id: Some(row_id) } if row_id == &row_ids[0]
    )));
}

#[test]
fn inbox_arrow_navigation_moves_one_visible_level_at_a_time() {
    let previewed = materialized_transition(
        snapshot(1, &["thread-a"]),
        UiConversationPage {
            title: "Alice".to_owned(),
            context: None,
            row_id: "thread-a".to_owned(),
            entries: vec![entry("message-1", false)],
            next_cursor: None,
        },
    );
    let content = update(previewed.model, UiEvent::Input(UiInput::NextFocus)).expect("list focus");
    let conversation = update(content.model, UiEvent::Input(UiInput::MoveCursorRight))
        .expect("enter conversation");
    assert_eq!(conversation.model.focus(), UiFocus::Conversation);
    let back_to_list =
        update(conversation.model, UiEvent::Input(UiInput::MoveCursorLeft)).expect("back to list");
    assert_eq!(back_to_list.model.focus(), UiFocus::Content);
    let back_to_navigation = update(back_to_list.model, UiEvent::Input(UiInput::MoveCursorLeft))
        .expect("back to navigation");
    assert_eq!(back_to_navigation.model.focus(), UiFocus::Navigation);
}

#[test]
fn inbox_back_closes_technical_details_before_leaving_the_conversation() {
    let model = opened_conversation(vec![entry("message-1", false)]);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("open details");
    assert!(details.model.technical_visible());

    let closed =
        update(details.model, UiEvent::Input(UiInput::MoveCursorLeft)).expect("close details");
    assert!(!closed.model.technical_visible());
    assert_eq!(closed.model.focus(), UiFocus::Conversation);

    let back = update(closed.model, UiEvent::Input(UiInput::MoveCursorLeft))
        .expect("return to Inbox list");
    assert_eq!(back.model.focus(), UiFocus::Content);
}

#[test]
fn escape_pops_composer_conversation_inbox_and_navigation_one_level_at_a_time() {
    let model = opened_conversation(vec![actionable_entry("question", [4; 32])]);
    let opening =
        update(model, UiEvent::Input(UiInput::Character('r'))).expect("open reply composer");
    let (effect_id, target) = open_draft_effect(&opening.effects);
    let composing = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id,
            draft: UiMailboxDraft {
                draft_id: [5; 32],
                target: target.clone(),
                content: String::new(),
                version: 1,
            },
        },
    )
    .expect("composer loaded");

    let conversation = update(composing.model, UiEvent::Input(UiInput::Escape))
        .expect("composer pops to conversation");
    assert!(conversation.model.mailbox_draft().is_none());
    assert_eq!(conversation.model.focus(), UiFocus::Conversation);
    assert!(conversation.model.conversation().is_some());

    let inbox = update(conversation.model, UiEvent::Input(UiInput::Escape))
        .expect("conversation pops to Inbox");
    assert_eq!(inbox.model.focus(), UiFocus::Content);
    assert!(inbox.model.conversation().is_some());

    let navigation = update(inbox.model, UiEvent::Input(UiInput::Escape))
        .expect("Inbox pops to main navigation");
    assert_eq!(navigation.model.focus(), UiFocus::Navigation);
    assert!(navigation.model.conversation().is_some());

    let root = update(navigation.model.clone(), UiEvent::Input(UiInput::Escape))
        .expect("main navigation is the root");
    assert_eq!(root.model, navigation.model);
    assert!(root.effects.is_empty());
}

#[test]
fn escape_closes_technical_details_before_popping_conversation_focus() {
    let model = opened_conversation(vec![entry("message-1", false)]);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("open details");
    let conversation =
        update(details.model, UiEvent::Input(UiInput::Escape)).expect("close technical details");
    assert!(!conversation.model.technical_visible());
    assert_eq!(conversation.model.focus(), UiFocus::Conversation);

    let inbox = update(conversation.model, UiEvent::Input(UiInput::Escape))
        .expect("leave conversation on the next escape");
    assert_eq!(inbox.model.focus(), UiFocus::Content);
}

#[test]
fn t_toggles_selected_details_and_jk_scroll_the_full_output() {
    let model = opened_conversation(vec![entry("message-1", false)]);

    let details =
        update(model, UiEvent::Input(UiInput::Character('t'))).expect("open details with t");
    assert!(details.model.technical_visible());
    assert_eq!(details.model.technical_scroll(), 0);

    let scrolled = update(details.model, UiEvent::Input(UiInput::Character('j')))
        .expect("scroll details down");
    assert_eq!(scrolled.model.technical_scroll(), 1);
    let restored =
        update(scrolled.model, UiEvent::Input(UiInput::Character('k'))).expect("scroll details up");
    assert_eq!(restored.model.technical_scroll(), 0);

    let closed = update(restored.model, UiEvent::Input(UiInput::Character('t')))
        .expect("close details with t");
    assert!(!closed.model.technical_visible());
}

#[test]
fn older_page_failure_preserves_transcript_anchor_and_retry_cursor() {
    let opened = materialized_transition(
        snapshot(1, &["thread-a"]),
        UiConversationPage {
            title: "Alice".to_owned(),
            context: None,
            row_id: "thread-a".to_owned(),
            entries: vec![entry("message-1", false), entry("message-2", false)],
            next_cursor: Some("older-2".to_owned()),
        },
    );
    let focused =
        update(opened.model, UiEvent::Input(UiInput::Activate)).expect("focus conversation");
    let anchored = update(focused.model, UiEvent::Input(UiInput::Character('j')))
        .expect("select second message");
    let loading =
        update(anchored.model, UiEvent::Input(UiInput::LoadMore)).expect("load older page");
    assert!(loading.model.conversation_older_loading());
    let (older_page_id, _, cursor) = conversation_effect(&loading.effects);
    assert_eq!(cursor, Some("older-2"));

    let failed = update(
        loading.model,
        UiEvent::ConversationFailed {
            effect_id: older_page_id,
            failure: UiFailure {
                code: "offline".to_owned(),
                action: "try again when connected".to_owned(),
            },
        },
    )
    .expect("older page failure");
    assert!(failed.model.conversation_failure_is_older());
    assert_eq!(failed.model.conversation_anchor(), Some("message-2"));
    assert_eq!(
        failed
            .model
            .conversation()
            .map(|conversation| conversation.entries.len()),
        Some(2)
    );

    let retry = update(failed.model, UiEvent::Input(UiInput::LoadMore)).expect("retry older page");
    let (_, _, retry_cursor) = conversation_effect(&retry.effects);
    assert_eq!(retry_cursor, Some("older-2"));
}

#[test]
fn materialized_refresh_preserves_anchor_and_ignores_a_stale_view() {
    let opened = materialized_transition(
        snapshot(1, &["thread-a"]),
        UiConversationPage {
            title: "Alice".to_owned(),
            context: None,
            row_id: "thread-a".to_owned(),
            entries: vec![entry("message-1", false), entry("message-2", false)],
            next_cursor: None,
        },
    );
    let fresh = update(
        opened.model,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot: snapshot(2, &["thread-a"]),
                conversation: Some(UiConversationPage {
                    title: "Alice".to_owned(),
                    context: None,
                    row_id: "thread-a".to_owned(),
                    entries: vec![entry("message-0", true), entry("message-2", false)],
                    next_cursor: None,
                }),
            },
        },
    )
    .expect("fresh view applies");
    let stale = update(
        fresh.model,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot: snapshot(1, &["thread-a"]),
                conversation: Some(UiConversationPage {
                    title: "Alice".to_owned(),
                    context: None,
                    row_id: "thread-a".to_owned(),
                    entries: vec![entry("stale", false)],
                    next_cursor: None,
                }),
            },
        },
    )
    .expect("stale view ignored");
    assert_eq!(stale.model.conversation_anchor(), Some("message-2"));
}

#[test]
fn reconnect_preserves_the_open_conversation_until_authoritative_repair() {
    let opened = materialized_transition(
        snapshot(1, &["thread-a"]),
        UiConversationPage {
            title: "Alice".to_owned(),
            context: None,
            row_id: "thread-a".to_owned(),
            entries: vec![entry("message-1", false)],
            next_cursor: None,
        },
    );
    let failed = update(
        opened.model,
        UiEvent::ClientFailed {
            generation: 2,
            failure: UiFailure {
                code: "connection_lost".to_owned(),
                action: "waiting to reconnect".to_owned(),
            },
        },
    )
    .expect("disconnect");
    assert_eq!(failed.model.conversation_anchor(), Some("message-1"));
    let connected = update(
        failed.model,
        UiEvent::ConnectionObserved {
            generation: 3,
            state: UiConnectionState::Ready,
        },
    )
    .expect("reconnect");
    assert!(connected.effects.iter().all(|effect| !matches!(
        effect,
        UiEffect::LoadSnapshot { .. } | UiEffect::LoadConversation { .. }
    )));
    assert_eq!(connected.model.conversation_anchor(), Some("message-1"));
    let repaired = update(
        connected.model,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot: snapshot(2, &["thread-a"]),
                conversation: Some(UiConversationPage {
                    title: "Alice".to_owned(),
                    context: None,
                    row_id: "thread-a".to_owned(),
                    entries: vec![entry("message-2", false)],
                    next_cursor: None,
                }),
            },
        },
    )
    .expect("authoritative repair");
    assert_eq!(repaired.model.conversation_anchor(), Some("message-2"));
}

#[test]
fn self_note_draft_autosaves_and_survives_resize_reconnect_and_reload() {
    let loaded = loaded_model(snapshot(1, &["thread-a"]));
    let opening =
        update(loaded, UiEvent::Input(UiInput::Character('N'))).expect("open self-note draft");
    let (open_id, target) = open_draft_effect(&opening.effects);
    assert_eq!(target, &UiMailboxDraftTarget::SelfNote);
    let draft = UiMailboxDraft {
        draft_id: [7; 32],
        target: UiMailboxDraftTarget::SelfNote,
        content: String::new(),
        version: 1,
    };
    let opened = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft,
        },
    )
    .expect("draft loaded");
    let typed = update(
        opened.model,
        UiEvent::Input(UiInput::Paste("remember this".to_owned())),
    )
    .expect("text entered");
    let autosave_id = timer_effect(&typed.effects, UiTimerKind::AutosaveDraft);
    let resized = update(
        typed.model,
        UiEvent::Resized(UiSize {
            width: 61,
            height: 16,
        }),
    )
    .expect("resize preserves editor");
    let reconnecting = update(
        resized.model,
        UiEvent::ConnectionObserved {
            generation: 3,
            state: UiConnectionState::Reconnecting,
        },
    )
    .expect("reconnect state preserves editor");
    assert!(matches!(
        reconnecting.model.mailbox_draft(),
        Some(UiMailboxDraftPane::Editing { draft, dirty: true, .. })
            if draft.content == "remember this"
    ));
    let saving = update(
        reconnecting.model,
        UiEvent::TimerElapsed {
            effect_id: autosave_id,
        },
    )
    .expect("debounce saves");
    let (save_id, saved_input) = save_draft_effect(&saving.effects);
    assert_eq!(saved_input.content, "remember this");
    let saved = update(
        saving.model,
        UiEvent::DraftSaved {
            effect_id: save_id,
            draft: UiMailboxDraft {
                version: 2,
                ..saved_input.clone()
            },
        },
    )
    .expect("save acknowledged");
    assert!(matches!(
        saved.model.mailbox_draft(),
        Some(UiMailboxDraftPane::Editing { draft, dirty: false, submitting: false, .. })
            if draft.version == 2 && draft.content == "remember this"
    ));
}

#[test]
fn activity_never_becomes_a_reply_or_state_action_target() {
    let opened = opened_conversation(vec![
        actionable_entry("message", [1; 32]),
        entry("activity", true),
    ]);
    let activity =
        update(opened, UiEvent::Input(UiInput::Character('j'))).expect("select activity");
    for shortcut in ['r', 'a', 'u'] {
        let guided = update(
            activity.model.clone(),
            UiEvent::Input(UiInput::Character(shortcut)),
        )
        .expect("activity shortcut explains its prerequisite");
        assert!(guided.model.mailbox_modal().is_none());
        assert_eq!(
            guided.model.transient_help(),
            Some("select a message; activity updates cannot be replied to, archived, or restored")
        );
        assert_eq!(redraw_count(&guided.effects), 1);
    }

    let message =
        update(activity.model, UiEvent::Input(UiInput::Character('k'))).expect("select message");
    let reply = update(
        message.model.clone(),
        UiEvent::Input(UiInput::Character('r')),
    )
    .expect("reply opens typed target");
    assert!(matches!(
        open_draft_effect(&reply.effects).1,
        UiMailboxDraftTarget::Reply { message_id } if *message_id == [1; 32]
    ));
    let confirm = update(message.model, UiEvent::Input(UiInput::Character('a')))
        .expect("archive requires confirmation");
    assert!(matches!(
        confirm.model.mailbox_modal(),
        Some(UiMailboxModal::Confirm { action: UiMailboxAction::Archive { target_message } })
            if *target_message == [1; 32]
    ));
    let cancelled =
        update(confirm.model, UiEvent::Input(UiInput::Escape)).expect("cancel confirmation");
    assert!(cancelled.model.mailbox_modal().is_none());
    assert!(
        !cancelled
            .effects
            .iter()
            .any(|effect| matches!(effect, UiEffect::SubmitMailboxCommand { .. }))
    );
}

#[test]
fn summary_state_shortcuts_explain_that_an_exact_message_must_be_selected() {
    let loaded = loaded_model(snapshot(1, &["thread-a"]));

    for shortcut in ['a', 'u'] {
        let attempted = update(loaded.clone(), UiEvent::Input(UiInput::Character(shortcut)))
            .expect("summary state shortcut provides guidance");
        assert!(attempted.model.last_failure().is_none());
        assert_eq!(
            attempted.model.transient_help(),
            Some(
                "open the conversation with Enter, then select a message to reply, archive, or restore"
            )
        );
        assert_eq!(redraw_count(&attempted.effects), 1);
        assert!(attempted.model.mailbox_modal().is_none());

        let dismissed = update(attempted.model, UiEvent::Input(UiInput::NextItem))
            .expect("the next input dismisses transient guidance");
        assert!(dismissed.model.transient_help().is_none());
        assert_eq!(redraw_count(&dismissed.effects), 1);
    }
}

#[test]
fn compose_newline_inserts_at_the_caret_and_plain_enter_still_submits() {
    let opened = opened_conversation(vec![actionable_entry("question", [3; 32])]);
    let opening = update(opened, UiEvent::Input(UiInput::Character('r'))).expect("reply");
    let (open_id, target) = open_draft_effect(&opening.effects);
    let target = target.clone();
    let loaded = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft: UiMailboxDraft {
                draft_id: [4; 32],
                target: UiMailboxDraftTarget::Reply {
                    message_id: [3; 32],
                },
                content: "ab".to_owned(),
                version: 1,
            },
        },
    )
    .expect("draft");
    let moved =
        update(loaded.model, UiEvent::Input(UiInput::MoveCursorLeft)).expect("move caret before b");
    let multiline =
        update(moved.model, UiEvent::Input(UiInput::InsertNewline)).expect("insert newline");
    assert!(matches!(
        multiline.model.mailbox_draft(),
        Some(UiMailboxDraftPane::Editing { draft, dirty: true, submitting: false, .. })
            if draft.content == "a\nb"
    ));
    assert!(multiline.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::ScheduleTimer {
            kind: UiTimerKind::AutosaveDraft,
            ..
        }
    )));
    assert!(
        multiline
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitMailboxCommand { .. }))
    );

    let saving = update(multiline.model, UiEvent::Input(UiInput::Activate))
        .expect("plain Enter starts submission");
    let (_, saved) = save_draft_effect(&saving.effects);
    assert_eq!(saved.content, "a\nb");

    let opened = opened_conversation(vec![actionable_entry("question", [3; 32])]);
    let opening = update(opened, UiEvent::Input(UiInput::Character('r'))).expect("reply");
    let (open_id, _) = open_draft_effect(&opening.effects);
    let full = "x".repeat(16 * 1024);
    let loaded = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft: UiMailboxDraft {
                draft_id: [5; 32],
                target,
                content: full.clone(),
                version: 1,
            },
        },
    )
    .expect("full draft");
    let bounded = update(loaded.model, UiEvent::Input(UiInput::InsertNewline))
        .expect("newline remains bounded");
    assert!(matches!(
        bounded.model.mailbox_draft(),
        Some(UiMailboxDraftPane::Editing { draft, .. }) if draft.content == full
    ));
}

#[test]
fn dirty_reply_saves_before_submit_and_stale_rejection_preserves_text() {
    let opened = opened_conversation(vec![actionable_entry("question", [3; 32])]);
    let opening = update(opened, UiEvent::Input(UiInput::Character('r'))).expect("reply");
    let (open_id, _) = open_draft_effect(&opening.effects);
    let draft = UiMailboxDraft {
        draft_id: [4; 32],
        target: UiMailboxDraftTarget::Reply {
            message_id: [3; 32],
        },
        content: String::new(),
        version: 1,
    };
    let loaded = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft,
        },
    )
    .expect("draft");
    let typed = update(
        loaded.model,
        UiEvent::Input(UiInput::Paste("answer text".to_owned())),
    )
    .expect("type");
    let saving =
        update(typed.model, UiEvent::Input(UiInput::Activate)).expect("submit waits for save");
    let (save_id, save_input) = save_draft_effect(&saving.effects);
    assert!(
        !saving
            .effects
            .iter()
            .any(|effect| matches!(effect, UiEffect::SubmitMailboxCommand { .. }))
    );
    let submitting = update(
        saving.model,
        UiEvent::DraftSaved {
            effect_id: save_id,
            draft: UiMailboxDraft {
                version: 2,
                ..save_input.clone()
            },
        },
    )
    .expect("saved draft submits");
    let command_id = submitting
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::SubmitMailboxCommand {
                id,
                draft: Some(draft),
                action: UiMailboxAction::Reply { target_message },
            } if draft.content == "answer text" && *target_message == [3; 32] => Some(*id),
            _ => None,
        })
        .expect("typed reply command");
    let rejected = update(
        submitting.model,
        UiEvent::MailboxCommandFailed {
            effect_id: command_id,
            failure: UiFailure {
                code: "mailbox_target_stale".to_owned(),
                action: "reselect the target; the draft text is preserved".to_owned(),
            },
        },
    )
    .expect("stale rejection");
    assert!(matches!(
        rejected.model.mailbox_draft(),
        Some(UiMailboxDraftPane::Editing { draft, submitting: false, .. })
            if draft.content == "answer text"
    ));
    assert_eq!(
        rejected
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("mailbox_target_stale")
    );
}

#[test]
fn committed_reply_dismisses_the_editor_and_appears_as_sent_at_the_conversation_tail() {
    let opened = opened_conversation(vec![actionable_entry("question", [3; 32])]);
    let opening = update(opened, UiEvent::Input(UiInput::Character('r'))).expect("reply");
    let (open_id, _) = open_draft_effect(&opening.effects);
    let loaded = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft: UiMailboxDraft {
                draft_id: [4; 32],
                target: UiMailboxDraftTarget::Reply {
                    message_id: [3; 32],
                },
                content: "answer text".to_owned(),
                version: 1,
            },
        },
    )
    .expect("draft");
    let submitting = update(loaded.model, UiEvent::Input(UiInput::Activate)).expect("submit reply");
    let command_id = submitting
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::SubmitMailboxCommand { id, .. } => Some(*id),
            _ => None,
        })
        .expect("mailbox command");

    let pending = submitting
        .model
        .conversation()
        .and_then(|conversation| conversation.entries.last())
        .expect("optimistic reply appears with the command effect");
    assert!(matches!(
        &pending.presentation,
        UiConversationEntryPresentation::Message {
            author: UiConversationAuthor::You,
            body,
        } if body == "answer text"
    ));
    assert_eq!(pending.delivery, Some(UiMessageDelivery::Pending));
    assert!(pending.message_target.is_none());
    let pending_id = pending.id.clone();
    assert_eq!(
        submitting.model.conversation_anchor(),
        Some(pending.id.as_str())
    );
    let submitting = observe_conversation_viewport(
        submitting.model,
        &[("question", 3), (pending_id.as_str(), 3)],
        2,
    );
    assert_eq!(
        submitting.model.conversation_viewport_position(),
        Some(&UiConversationViewportPosition {
            entry_id: pending_id,
            row: 1,
        })
    );

    let committed = update(
        submitting.model,
        UiEvent::MailboxCommandCommitted {
            effect_id: command_id,
            revision: 2,
            message_id: Some([5; 32]),
        },
    )
    .expect("committed reply");

    assert!(committed.model.mailbox_draft().is_none());
    let sent = committed
        .model
        .conversation()
        .and_then(|conversation| conversation.entries.last())
        .expect("sent reply in conversation");
    assert!(matches!(
        &sent.presentation,
        UiConversationEntryPresentation::Message {
            author: UiConversationAuthor::You,
            body,
        } if body == "answer text"
    ));
    assert_eq!(sent.delivery, Some(UiMessageDelivery::Sent));
    assert_eq!(
        sent.message_target.map(|target| target.message_id),
        Some([5; 32])
    );
    assert_eq!(
        committed.model.conversation_anchor(),
        Some(sent.id.as_str())
    );
    assert_eq!(
        committed.model.conversation_viewport_position(),
        Some(&UiConversationViewportPosition {
            entry_id: sent.id.clone(),
            row: 1,
        }),
        "optimistic geometry keeps its stable row when the canonical identity replaces it"
    );
}

#[test]
fn sent_agent_message_follows_the_live_tail_through_automatic_followup() {
    let opened = opened_conversation(vec![
        actionable_entry("question", [3; 32]),
        agent_turn_entry("turn-running", UiActivityStatus::Running),
    ]);
    let question =
        update(opened, UiEvent::Input(UiInput::Character('k'))).expect("select question");
    let opening = update(question.model, UiEvent::Input(UiInput::Character('r'))).expect("reply");
    let (open_id, _) = open_draft_effect(&opening.effects);
    let loaded = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft: UiMailboxDraft {
                draft_id: [4; 32],
                target: UiMailboxDraftTarget::Reply {
                    message_id: [3; 32],
                },
                content: "answer text".to_owned(),
                version: 1,
            },
        },
    )
    .expect("draft");
    let submitting = update(loaded.model, UiEvent::Input(UiInput::Activate)).expect("send reply");
    let command_id = submitting
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::SubmitMailboxCommand { id, .. } => Some(*id),
            _ => None,
        })
        .expect("mailbox command");

    assert_eq!(
        submitting.model.conversation_anchor(),
        Some("turn-running"),
        "sending leaves the conversation following the visible tail"
    );

    let committed = update(
        submitting.model,
        UiEvent::MailboxCommandCommitted {
            effect_id: command_id,
            revision: 2,
            message_id: Some([5; 32]),
        },
    )
    .expect("committed reply");
    assert_eq!(committed.model.conversation_anchor(), Some("turn-running"));

    let sent = committed
        .model
        .conversation()
        .and_then(|conversation| {
            conversation.entries.iter().find(|entry| {
                entry
                    .message_target
                    .is_some_and(|target| target.message_id == [5; 32])
            })
        })
        .cloned()
        .expect("committed message");
    let finished = update(
        committed.model,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot: snapshot(2, &["thread-a"]),
                conversation: Some(UiConversationPage {
                    title: "Alice".to_owned(),
                    context: None,
                    row_id: "thread-a".to_owned(),
                    entries: vec![
                        actionable_entry("question", [3; 32]),
                        sent,
                        actionable_entry("agent-response", [6; 32]),
                        agent_turn_entry("turn-finished", UiActivityStatus::Succeeded),
                    ],
                    next_cursor: None,
                }),
            },
        },
    )
    .expect("terminal turn observed");

    assert_eq!(finished.model.focus(), UiFocus::Draft);
    assert_eq!(
        finished.model.conversation_anchor(),
        Some("turn-finished"),
        "the automatic composer opens with every latest message visible"
    );
    assert!(matches!(
        finished.model.mailbox_draft(),
        Some(UiMailboxDraftPane::Loading {
            target: UiMailboxDraftTarget::Reply { message_id },
        }) if *message_id == [6; 32]
    ));
}

#[test]
fn definite_mailbox_rejection_removes_optimistic_reply_and_restores_exact_draft() {
    let opened = opened_conversation(vec![actionable_entry("question", [3; 32])]);
    let opening = update(opened, UiEvent::Input(UiInput::Character('r'))).expect("reply");
    let (open_id, _) = open_draft_effect(&opening.effects);
    let draft = UiMailboxDraft {
        draft_id: [4; 32],
        target: UiMailboxDraftTarget::Reply {
            message_id: [3; 32],
        },
        content: "exact answer".to_owned(),
        version: 7,
    };
    let loaded = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft: draft.clone(),
        },
    )
    .expect("draft");
    let submitting = update(loaded.model, UiEvent::Input(UiInput::Activate)).expect("submit");
    let command_id = submitting
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::SubmitMailboxCommand { id, .. } => Some(*id),
            _ => None,
        })
        .expect("command");
    assert_eq!(
        submitting
            .model
            .conversation()
            .expect("conversation")
            .entries
            .len(),
        2
    );

    let rejected = update(
        submitting.model,
        UiEvent::MailboxCommandFailed {
            effect_id: command_id,
            failure: UiFailure {
                code: "mailbox_target_stale".to_owned(),
                action: "reselect and retry".to_owned(),
            },
        },
    )
    .expect("rejected");

    assert_eq!(
        rejected
            .model
            .conversation()
            .expect("conversation")
            .entries
            .len(),
        1
    );
    assert!(matches!(
        rejected.model.mailbox_draft(),
        Some(UiMailboxDraftPane::Editing {
            draft: restored,
            dirty: false,
            submitting: false,
            closing: false,
        }) if restored == &draft
    ));
    assert_eq!(rejected.model.focus(), UiFocus::Draft);
}

#[test]
fn uncertain_mailbox_response_retains_one_optimistic_reply_and_command_identity() {
    let opened = opened_conversation(vec![actionable_entry("question", [3; 32])]);
    let opening = update(opened, UiEvent::Input(UiInput::Character('r'))).expect("reply");
    let (open_id, _) = open_draft_effect(&opening.effects);
    let loaded = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft: UiMailboxDraft {
                draft_id: [4; 32],
                target: UiMailboxDraftTarget::Reply {
                    message_id: [3; 32],
                },
                content: "possibly sent".to_owned(),
                version: 1,
            },
        },
    )
    .expect("draft");
    let submitting = update(loaded.model, UiEvent::Input(UiInput::Activate)).expect("submit");
    let command_id = submitting
        .model
        .pending_mailbox()
        .expect("command identity");
    let uncertain = update(
        submitting.model,
        UiEvent::MailboxCommandFailed {
            effect_id: command_id,
            failure: UiFailure {
                code: "mailbox_command_uncertain".to_owned(),
                action: "HQ is reconciling the receipt".to_owned(),
            },
        },
    )
    .expect("uncertain");

    assert_eq!(uncertain.model.pending_mailbox(), Some(command_id));
    let authored = uncertain
        .model
        .conversation()
        .expect("conversation")
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                &entry.presentation,
                UiConversationEntryPresentation::Message {
                    author: UiConversationAuthor::You,
                    body,
                } if body == "possibly sent"
            )
        })
        .count();
    assert_eq!(authored, 1);
}

#[test]
fn committed_reply_does_not_duplicate_a_message_loaded_by_an_earlier_invalidation() {
    let opened = opened_conversation(vec![actionable_entry("question", [3; 32])]);
    let opening = update(opened, UiEvent::Input(UiInput::Character('r'))).expect("reply");
    let (open_id, _) = open_draft_effect(&opening.effects);
    let loaded = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft: UiMailboxDraft {
                draft_id: [5; 32],
                target: UiMailboxDraftTarget::Reply {
                    message_id: [3; 32],
                },
                content: "answer text".to_owned(),
                version: 1,
            },
        },
    )
    .expect("draft");
    let submitting = update(loaded.model, UiEvent::Input(UiInput::Activate)).expect("submit reply");
    let command_id = submitting
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::SubmitMailboxCommand { id, .. } => Some(*id),
            _ => None,
        })
        .expect("mailbox command");

    let invalidated = update(submitting.model, UiEvent::Invalidated { revision: 2 })
        .expect("invalidation may precede command receipt");
    let mut authoritative = entry("authoritative-fact", false);
    authoritative.presentation = UiConversationEntryPresentation::Message {
        author: UiConversationAuthor::You,
        body: "answer text".to_owned(),
    };
    authoritative.message_target = Some(UiMessageTarget {
        message_id: [5; 32],
        reply_allowed: false,
    });
    authoritative.delivery = Some(UiMessageDelivery::Sent);
    let reloaded = update(
        invalidated.model,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot: snapshot(2, &["thread-a"]),
                conversation: Some(UiConversationPage {
                    title: "Alice".to_owned(),
                    context: None,
                    row_id: "thread-a".to_owned(),
                    entries: vec![actionable_entry("question", [3; 32]), authoritative],
                    next_cursor: None,
                }),
            },
        },
    )
    .expect("authoritative conversation");

    let committed = update(
        reloaded.model,
        UiEvent::MailboxCommandCommitted {
            effect_id: command_id,
            revision: 2,
            message_id: Some([5; 32]),
        },
    )
    .expect("committed reply");

    let conversation = committed.model.conversation().expect("conversation");
    let authored = conversation
        .entries
        .iter()
        .filter(|entry| {
            entry
                .message_target
                .is_some_and(|target| target.message_id == [5; 32])
        })
        .collect::<Vec<_>>();
    assert_eq!(authored.len(), 1);
    assert_eq!(
        committed.model.conversation_anchor(),
        Some("authoritative-fact")
    );
}

#[test]
fn accepted_undispatched_project_reply_is_presented_as_pending() {
    let project_id = [5; 32];
    let thread_id = [6; 32];
    let message_id = [7; 32];
    let mut target = project(5, "hq", "/workspace/hq");
    target.pending_inputs.push(UiPendingProjectInput {
        message_id,
        thread_id,
        sequence: 1,
    });
    let mut snapshot = projects_snapshot(1, vec![target]);
    snapshot.inbox_rows.push(UiRow {
        id: "project-thread".to_owned(),
        title: "Alice".to_owned(),
        detail: "pending reply".to_owned(),
        state: UiRowState::Open,
        kind: UiRowKind::Conversation,
        conversation_target: Some(UiConversationTarget::Project {
            project_id,
            thread_id,
            root_message: [8; 32],
        }),
    });
    let mut pending_reply = entry("pending-reply", false);
    pending_reply.delivery = Some(UiMessageDelivery::Sent);
    pending_reply.message_target = Some(UiMessageTarget {
        message_id,
        reply_allowed: false,
    });

    let opened = materialized_transition(
        snapshot,
        UiConversationPage {
            title: "Alice".to_owned(),
            context: Some("hq".to_owned()),
            row_id: "project-thread".to_owned(),
            entries: vec![pending_reply],
            next_cursor: None,
        },
    );

    assert_eq!(
        opened.model.conversation().and_then(|conversation| {
            conversation
                .entries
                .first()
                .and_then(|entry| entry.delivery)
        }),
        Some(UiMessageDelivery::Pending)
    );
}

#[test]
fn live_agent_status_stays_at_the_presentation_tail_after_new_authoritative_output() {
    let mut working = entry("working", true);
    working.presentation = UiConversationEntryPresentation::Activity {
        kind: UiConversationActivityKind::AgentTurn,
        status: UiActivityStatus::Running,
        summary: "Agent is working…".to_owned(),
        detail: "turn running".to_owned(),
        truncated: false,
        completed: None,
    };
    let mut completed_item = entry("completed-item", true);
    completed_item.presentation = UiConversationEntryPresentation::Activity {
        kind: UiConversationActivityKind::CompletedItem,
        status: UiActivityStatus::Succeeded,
        summary: "Completed an item".to_owned(),
        detail: "item complete".to_owned(),
        truncated: false,
        completed: Some(UiCompletedItemPresentation::Unknown),
    };
    let opened = opened_conversation(vec![entry("question", false), working.clone()]);
    assert_eq!(opened.conversation_anchor(), Some("working"));

    let completed = update(
        opened,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot: snapshot(2, &["thread-a"]),
                conversation: Some(UiConversationPage {
                    title: "Alice".to_owned(),
                    context: None,
                    row_id: "thread-a".to_owned(),
                    entries: vec![
                        entry("question", false),
                        working,
                        entry("alice-reply", false),
                        completed_item,
                    ],
                    next_cursor: None,
                }),
            },
        },
    )
    .expect("refreshed conversation");

    let entry_ids = completed
        .model
        .conversation()
        .expect("conversation")
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        entry_ids,
        ["question", "alice-reply", "completed-item", "working"]
    );
    assert_eq!(completed.model.conversation_anchor(), Some("working"));
}

#[test]
fn terminal_agent_turn_automatically_opens_the_exact_project_continuation_draft() {
    let project_id = [5; 32];
    let thread_id = [6; 32];
    let mut initial = snapshot(1, &["thread-a"]);
    initial.inbox_rows[0].conversation_target = Some(UiConversationTarget::Project {
        project_id,
        thread_id,
        root_message: [7; 32],
    });
    let mut running = entry("turn-running", true);
    running.presentation = UiConversationEntryPresentation::Activity {
        kind: UiConversationActivityKind::AgentTurn,
        status: UiActivityStatus::Running,
        summary: "Agent is working…".to_owned(),
        detail: "turn running".to_owned(),
        truncated: false,
        completed: None,
    };
    let opened = materialized_transition(
        initial,
        UiConversationPage {
            title: "Alice".to_owned(),
            context: Some("hq".to_owned()),
            row_id: "thread-a".to_owned(),
            entries: vec![entry("question", false), running],
            next_cursor: None,
        },
    );

    let mut terminal = entry("turn-finished", true);
    terminal.presentation = UiConversationEntryPresentation::Activity {
        kind: UiConversationActivityKind::AgentTurn,
        status: UiActivityStatus::Succeeded,
        summary: "Agent finished".to_owned(),
        detail: "turn completed".to_owned(),
        truncated: false,
        completed: None,
    };
    let mut refreshed = snapshot(2, &["thread-a"]);
    refreshed.inbox_rows[0].conversation_target = Some(UiConversationTarget::Project {
        project_id,
        thread_id,
        root_message: [7; 32],
    });
    let finished = update(
        opened.model,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot: refreshed,
                conversation: Some(UiConversationPage {
                    title: "Alice".to_owned(),
                    context: Some("hq".to_owned()),
                    row_id: "thread-a".to_owned(),
                    entries: vec![entry("question", false), terminal],
                    next_cursor: None,
                }),
            },
        },
    )
    .expect("terminal turn observed");

    assert_eq!(finished.model.focus(), UiFocus::Draft);
    assert!(matches!(
        finished.model.mailbox_draft(),
        Some(UiMailboxDraftPane::Loading {
            target: UiMailboxDraftTarget::Project {
                project_id: selected_project,
                thread_id: Some(selected_thread),
            },
        }) if *selected_project == project_id && *selected_thread == thread_id
    ));
    assert!(finished.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::OpenDraft {
            target: UiMailboxDraftTarget::Project {
                project_id: selected_project,
                thread_id: Some(selected_thread),
            },
            ..
        } if *selected_project == project_id && *selected_thread == thread_id
    )));
}

#[test]
fn initially_opening_an_already_finished_turn_does_not_open_a_draft() {
    let mut terminal = entry("turn-finished", true);
    terminal.presentation = UiConversationEntryPresentation::Activity {
        kind: UiConversationActivityKind::AgentTurn,
        status: UiActivityStatus::Succeeded,
        summary: "Agent finished".to_owned(),
        detail: "turn completed".to_owned(),
        truncated: false,
        completed: None,
    };
    let opened = materialized_transition(
        snapshot(1, &["thread-a"]),
        UiConversationPage {
            title: "Alice".to_owned(),
            context: None,
            row_id: "thread-a".to_owned(),
            entries: vec![entry("question", false), terminal],
            next_cursor: None,
        },
    );

    assert!(opened.model.mailbox_draft().is_none());
    assert!(
        opened
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::OpenDraft { .. }))
    );
}

#[test]
fn terminal_agent_turn_automatically_replies_to_the_latest_direct_message() {
    let message_id = [8; 32];
    let opened = materialized_transition(
        snapshot(1, &["thread-a"]),
        UiConversationPage {
            title: "Alice".to_owned(),
            context: None,
            row_id: "thread-a".to_owned(),
            entries: vec![
                actionable_entry("alice-message", message_id),
                agent_turn_entry("turn-running", UiActivityStatus::Running),
            ],
            next_cursor: None,
        },
    );
    let finished = update(
        opened.model,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot: snapshot(2, &["thread-a"]),
                conversation: Some(UiConversationPage {
                    title: "Alice".to_owned(),
                    context: None,
                    row_id: "thread-a".to_owned(),
                    entries: vec![
                        actionable_entry("alice-message", message_id),
                        agent_turn_entry("turn-finished", UiActivityStatus::Succeeded),
                    ],
                    next_cursor: None,
                }),
            },
        },
    )
    .expect("terminal direct turn observed");

    assert!(matches!(
        finished.model.mailbox_draft(),
        Some(UiMailboxDraftPane::Loading {
            target: UiMailboxDraftTarget::Reply {
                message_id: selected_message,
            },
        }) if *selected_message == message_id
    ));
}

#[test]
fn terminal_agent_turn_does_not_replace_an_existing_draft() {
    let message_id = [8; 32];
    let opened = materialized_transition(
        snapshot(1, &["thread-a"]),
        UiConversationPage {
            title: "Alice".to_owned(),
            context: None,
            row_id: "thread-a".to_owned(),
            entries: vec![
                actionable_entry("alice-message", message_id),
                agent_turn_entry("turn-running", UiActivityStatus::Running),
            ],
            next_cursor: None,
        },
    );
    let composing = update(opened.model, UiEvent::Input(UiInput::Character('N')))
        .expect("open existing note draft");
    let finished = update(
        composing.model,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot: snapshot(2, &["thread-a"]),
                conversation: Some(UiConversationPage {
                    title: "Alice".to_owned(),
                    context: None,
                    row_id: "thread-a".to_owned(),
                    entries: vec![
                        actionable_entry("alice-message", message_id),
                        agent_turn_entry("turn-finished", UiActivityStatus::Succeeded),
                    ],
                    next_cursor: None,
                }),
            },
        },
    )
    .expect("terminal turn observed while composing");

    assert!(matches!(
        finished.model.mailbox_draft(),
        Some(UiMailboxDraftPane::Loading {
            target: UiMailboxDraftTarget::SelfNote,
        })
    ));
    assert!(
        finished
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::OpenDraft { .. }))
    );
}

#[test]
fn automatic_followup_waits_until_every_agent_turn_is_terminal() {
    let message_id = [8; 32];
    let opened = materialized_transition(
        snapshot(1, &["thread-a"]),
        UiConversationPage {
            title: "Alice".to_owned(),
            context: None,
            row_id: "thread-a".to_owned(),
            entries: vec![
                actionable_entry("alice-message", message_id),
                agent_turn_entry("turn-a-running", UiActivityStatus::Running),
                agent_turn_entry("turn-b-running", UiActivityStatus::Running),
            ],
            next_cursor: None,
        },
    );
    let one_finished = update(
        opened.model,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot: snapshot(2, &["thread-a"]),
                conversation: Some(UiConversationPage {
                    title: "Alice".to_owned(),
                    context: None,
                    row_id: "thread-a".to_owned(),
                    entries: vec![
                        actionable_entry("alice-message", message_id),
                        agent_turn_entry("turn-a-finished", UiActivityStatus::Succeeded),
                        agent_turn_entry("turn-b-running", UiActivityStatus::Running),
                    ],
                    next_cursor: None,
                }),
            },
        },
    )
    .expect("one terminal turn observed");
    assert!(one_finished.model.mailbox_draft().is_none());

    let all_finished = update(
        one_finished.model,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot: snapshot(3, &["thread-a"]),
                conversation: Some(UiConversationPage {
                    title: "Alice".to_owned(),
                    context: None,
                    row_id: "thread-a".to_owned(),
                    entries: vec![
                        actionable_entry("alice-message", message_id),
                        agent_turn_entry("turn-a-finished", UiActivityStatus::Succeeded),
                        agent_turn_entry("turn-b-finished", UiActivityStatus::Interrupted),
                    ],
                    next_cursor: None,
                }),
            },
        },
    )
    .expect("all terminal turns observed");
    assert!(matches!(
        all_finished.model.mailbox_draft(),
        Some(UiMailboxDraftPane::Loading {
            target: UiMailboxDraftTarget::Reply {
                message_id: selected_message,
            },
        }) if *selected_message == message_id
    ));
}

#[test]
fn project_reply_continues_the_exact_selected_conversation() {
    let project_id = [5; 32];
    let mut initial = snapshot(1, &["existing"]);
    initial.inbox_rows[0].conversation_target = Some(UiConversationTarget::Project {
        project_id,
        thread_id: [6; 32],
        root_message: [7; 32],
    });

    let continued = update(
        loaded_model(initial),
        UiEvent::Input(UiInput::Character('r')),
    )
    .expect("continue selected project conversation");
    assert!(matches!(
        open_draft_effect(&continued.effects).1,
        UiMailboxDraftTarget::Project {
            project_id: selected_project,
            thread_id: Some(thread_id),
        } if *selected_project == project_id && *thread_id == [6; 32]
    ));
}

#[test]
fn new_project_conversation_tracks_local_context_then_selects_authoritative_root() {
    let project_id = [5; 32];
    let mut initial = snapshot(1, &["existing"]);
    initial.inbox_rows[0].conversation_target = Some(UiConversationTarget::Project {
        project_id,
        thread_id: [6; 32],
        root_message: [7; 32],
    });
    initial.projects = vec![project(5, "release", "/work/release")];

    let opening = update(
        loaded_model(initial),
        UiEvent::Input(UiInput::Character('c')),
    )
    .expect("start a new project conversation");
    let (open_id, target) = open_draft_effect(&opening.effects);
    assert_eq!(
        target,
        &UiMailboxDraftTarget::Project {
            project_id,
            thread_id: None,
        }
    );
    assert_project_draft_context(&opening.model, project_id, "release", None);
    let loaded = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft: UiMailboxDraft {
                draft_id: [10; 32],
                target: target.clone(),
                content: "A separate topic".to_owned(),
                version: 1,
            },
        },
    )
    .expect("project draft loaded");
    let submitting = update(loaded.model, UiEvent::Input(UiInput::Activate))
        .expect("submit new project conversation");
    let send_id = submitting
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::SubmitMailboxCommand {
                id,
                action:
                    UiMailboxAction::Project {
                        project_id: selected_project,
                        thread_id: None,
                    },
                ..
            } if *selected_project == project_id => Some(*id),
            _ => None,
        })
        .expect("project mailbox command");
    let root_message = [11; 32];
    let committed = update(
        submitting.model,
        UiEvent::MailboxCommandCommitted {
            effect_id: send_id,
            revision: 2,
            message_id: Some(root_message),
        },
    )
    .expect("new project root committed");
    assert_project_draft_context(
        &committed.model,
        project_id,
        "release",
        Some("A separate topic"),
    );
    let refresh_id = snapshot_effect(&committed.effects);
    let mut refreshed = snapshot(2, &["existing", "new"]);
    refreshed.projects = vec![project(5, "release", "/work/release")];
    refreshed.inbox_rows[0].conversation_target = Some(UiConversationTarget::Project {
        project_id,
        thread_id: [6; 32],
        root_message: [7; 32],
    });
    refreshed.inbox_rows[1].conversation_target = Some(UiConversationTarget::Project {
        project_id,
        thread_id: [12; 32],
        root_message,
    });
    let selected = update(
        committed.model,
        UiEvent::SnapshotLoaded {
            effect_id: refresh_id,
            snapshot: refreshed,
        },
    )
    .expect("new project row selected");
    assert_eq!(selected.model.selected_row(), Some("new"));
    assert!(selected.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::ObserveConversation { row_id: Some(row_id) } if row_id == "new"
    )));
}

#[test]
fn direct_target_reselection_survives_authoritative_reorder() {
    let mut initial = snapshot(1, &["thread-a"]);
    initial.direct_targets = vec![direct_target("alpha", 1), direct_target("beta", 2)];
    let loaded = loaded_model(initial);
    let selecting =
        update(loaded, UiEvent::Input(UiInput::Character('d'))).expect("open target selector");
    let selected = update(selecting.model, UiEvent::Input(UiInput::NextItem)).expect("choose beta");
    let invalidated = update(selected.model, UiEvent::Invalidated { revision: 2 })
        .expect("reload while selecting");
    let snapshot_id = snapshot_effect(&invalidated.effects);
    let mut reordered = snapshot(2, &["thread-a"]);
    reordered.direct_targets = vec![direct_target("beta", 2), direct_target("alpha", 1)];
    let reloaded = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot: reordered,
        },
    )
    .expect("stable mailbox remains selected");
    assert!(matches!(
        reloaded.model.mailbox_modal(),
        Some(UiMailboxModal::SelectDirect { selected: Some((installation, mailbox)), .. })
            if *installation == [2; 32] && *mailbox == [12; 32]
    ));
    let opening = update(reloaded.model, UiEvent::Input(UiInput::Activate))
        .expect("open selected target draft");
    assert!(matches!(
        open_draft_effect(&opening.effects).1,
        UiMailboxDraftTarget::Direct { installation_id, mailbox_id }
            if *installation_id == [2; 32] && *mailbox_id == [12; 32]
    ));
}

#[test]
fn empty_direct_recipient_selection_is_inert_and_cancelable() {
    let loaded = loaded_model(snapshot(1, &[]));
    let opened =
        update(loaded, UiEvent::Input(UiInput::Character('d'))).expect("open recipient chooser");
    assert!(matches!(
        opened.model.mailbox_modal(),
        Some(UiMailboxModal::SelectDirect { targets, selected: None }) if targets.is_empty()
    ));

    for input in [UiInput::NextItem, UiInput::PreviousItem, UiInput::Activate] {
        let inert = update(opened.model.clone(), UiEvent::Input(input))
            .expect("unavailable recipient action is inert");
        assert!(inert.effects.is_empty());
        assert_eq!(inert.model, opened.model);
    }

    let closed = update(opened.model, UiEvent::Input(UiInput::Escape))
        .expect("close empty recipient chooser");
    assert!(closed.model.mailbox_modal().is_none());
    assert_eq!(redraw_count(&closed.effects), 1);
}

#[test]
fn direct_archive_and_restore_emit_only_their_typed_commands() {
    let mut source = snapshot(1, &["thread-a"]);
    source.direct_targets = vec![direct_target("builder", 5)];
    let loaded = loaded_model(source);
    let selecting = update(loaded, UiEvent::Input(UiInput::Character('d'))).expect("direct");
    let opening = update(selecting.model, UiEvent::Input(UiInput::Activate)).expect("target");
    let (open_id, target) = open_draft_effect(&opening.effects);
    let draft = UiMailboxDraft {
        draft_id: [6; 32],
        target: target.clone(),
        content: "direct content".to_owned(),
        version: 4,
    };
    let composing = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft: draft.clone(),
        },
    )
    .expect("direct draft");
    let direct = update(composing.model, UiEvent::Input(UiInput::Activate)).expect("direct submit");
    assert!(direct.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::SubmitMailboxCommand {
            draft: Some(submitted),
            action: UiMailboxAction::Direct {
                recipient_installation,
                recipient_mailbox,
            },
            ..
        } if submitted == &draft
            && *recipient_installation == [5; 32]
            && *recipient_mailbox == [15; 32]
    )));

    let open_message = opened_conversation(vec![actionable_entry("open", [8; 32])]);
    let archive_confirm =
        update(open_message, UiEvent::Input(UiInput::Character('a'))).expect("archive confirm");
    let archive =
        update(archive_confirm.model, UiEvent::Input(UiInput::Activate)).expect("archive submit");
    assert!(archive.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::SubmitMailboxCommand {
            draft: None,
            action: UiMailboxAction::Archive { target_message },
            ..
        } if *target_message == [8; 32]
    )));

    let mut archived = actionable_entry("archived", [9; 32]);
    archived.message_state = Some(UiMessageState::Archived);
    let archived_message = opened_conversation(vec![archived]);
    let restore_confirm =
        update(archived_message, UiEvent::Input(UiInput::Character('u'))).expect("restore confirm");
    let restore =
        update(restore_confirm.model, UiEvent::Input(UiInput::Activate)).expect("restore submit");
    assert!(restore.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::SubmitMailboxCommand {
            draft: None,
            action: UiMailboxAction::Restore { target_message },
            ..
        } if *target_message == [9; 32]
    )));
}

#[test]
fn escape_during_in_flight_autosave_waits_for_latest_text_before_closing() {
    let loaded = loaded_model(snapshot(1, &[]));
    let opening = update(loaded, UiEvent::Input(UiInput::Character('N'))).expect("note");
    let (open_id, _) = open_draft_effect(&opening.effects);
    let opened = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft: UiMailboxDraft {
                draft_id: [10; 32],
                target: UiMailboxDraftTarget::SelfNote,
                content: String::new(),
                version: 1,
            },
        },
    )
    .expect("draft");
    let first = update(opened.model, UiEvent::Input(UiInput::Character('a'))).expect("first edit");
    let timer = timer_effect(&first.effects, UiTimerKind::AutosaveDraft);
    let saving =
        update(first.model, UiEvent::TimerElapsed { effect_id: timer }).expect("first save starts");
    let (save_id, first_input) = save_draft_effect(&saving.effects);
    let newer = update(saving.model, UiEvent::Input(UiInput::Character('b')))
        .expect("edit while save is in flight");
    let closing =
        update(newer.model, UiEvent::Input(UiInput::Escape)).expect("close waits for latest save");
    assert!(matches!(
        closing.model.mailbox_draft(),
        Some(UiMailboxDraftPane::Editing { draft, closing: true, .. }) if draft.content == "ab"
    ));
    let follow_up = update(
        closing.model,
        UiEvent::DraftSaved {
            effect_id: save_id,
            draft: UiMailboxDraft {
                version: 2,
                ..first_input.clone()
            },
        },
    )
    .expect("old save triggers latest save");
    let (latest_id, latest) = save_draft_effect(&follow_up.effects);
    assert_eq!(latest.content, "ab");
    assert_eq!(latest.version, 2);
    let closed = update(
        follow_up.model,
        UiEvent::DraftSaved {
            effect_id: latest_id,
            draft: UiMailboxDraft {
                version: 3,
                ..latest.clone()
            },
        },
    )
    .expect("latest save closes");
    assert!(closed.model.mailbox_modal().is_none());
}

#[test]
fn optimistic_draft_conflict_preserves_local_text_and_adopts_current_version() {
    let loaded = loaded_model(snapshot(1, &[]));
    let opening = update(loaded, UiEvent::Input(UiInput::Character('N'))).expect("note");
    let (open_id, _) = open_draft_effect(&opening.effects);
    let opened = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft: UiMailboxDraft {
                draft_id: [11; 32],
                target: UiMailboxDraftTarget::SelfNote,
                content: "local".to_owned(),
                version: 1,
            },
        },
    )
    .expect("draft");
    let edited = update(opened.model, UiEvent::Input(UiInput::Character('!'))).expect("edit");
    let timer = timer_effect(&edited.effects, UiTimerKind::AutosaveDraft);
    let saving =
        update(edited.model, UiEvent::TimerElapsed { effect_id: timer }).expect("save starts");
    let (save_id, _) = save_draft_effect(&saving.effects);
    let conflicted = update(
        saving.model,
        UiEvent::DraftFailed {
            effect_id: save_id,
            failure: UiFailure {
                code: "draft_conflict".to_owned(),
                action: "edit the preserved text and retry against the current draft".to_owned(),
            },
            current: Some(UiMailboxDraft {
                draft_id: [11; 32],
                target: UiMailboxDraftTarget::SelfNote,
                content: "other writer".to_owned(),
                version: 7,
            }),
        },
    )
    .expect("conflict remains actionable");
    assert!(matches!(
        conflicted.model.mailbox_draft(),
        Some(UiMailboxDraftPane::Editing { draft, dirty: true, submitting: false, closing: false })
            if draft.content == "local!" && draft.version == 7
    ));
    assert_eq!(
        conflicted
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("draft_conflict")
    );
}

#[test]
fn agent_search_and_details_keep_stable_identity_across_reload_reconnect_and_resize() {
    let model = loaded_agents_model(1, &[agent(1, "alpha"), agent(2, "beta")]);
    let searching = update(model, UiEvent::Input(UiInput::Character('/'))).expect("search");
    let matched = update(
        searching.model,
        UiEvent::Input(UiInput::Paste("beta".to_owned())),
    )
    .expect("search query");
    assert_eq!(matched.model.selected_row(), Some(agent_row_id(2).as_str()));
    let invalidated = update(matched.model, UiEvent::Invalidated { revision: 2 }).expect("reload");
    let request = snapshot_effect(&invalidated.effects);
    let reloaded = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: request,
            snapshot: agents_snapshot(2, vec![agent(2, "beta"), agent(1, "alpha")]),
        },
    )
    .expect("authoritative reorder");
    assert_eq!(
        reloaded.model.selected_row(),
        Some(agent_row_id(2).as_str())
    );
    assert!(matches!(
        reloaded.model.agent_modal(),
        Some(UiAgentModal::Search { query }) if query == "beta"
    ));
    let details = update(reloaded.model, UiEvent::Input(UiInput::Activate)).expect("inspect");
    let resized = update(
        details.model,
        UiEvent::Resized(UiSize {
            width: 62,
            height: 17,
        }),
    )
    .expect("resize");
    let reconnecting = update(
        resized.model,
        UiEvent::ConnectionObserved {
            generation: 4,
            state: UiConnectionState::Reconnecting,
        },
    )
    .expect("reconnect");
    assert!(matches!(
        reconnecting.model.agent_modal(),
        Some(UiAgentModal::Details { agent, .. }) if agent.agent_id == [2; 32]
    ));
}

#[test]
fn open_agent_details_adopt_assignment_aware_status_from_a_new_snapshot() {
    let model = loaded_agents_model(1, &[agent(2, "builder")]);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("open agent details");
    let invalidated = update(details.model, UiEvent::Invalidated { revision: 2 }).expect("reload");
    let request = snapshot_effect(&invalidated.effects);
    let mut assigned = agent(2, "builder");
    assigned.status = UiAgentStatus::Assigned(UiAgentProjectAssignment {
        project_id: [7; 32],
        project_name: "release".to_owned(),
        assignment_id: [8; 32],
        provider: "codex".to_owned(),
        session: None,
        phase: UiAgentAssignmentPhase::SettingUp,
        blocked: None,
        cardinality_conflicted: false,
    });
    let reloaded = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: request,
            snapshot: agents_snapshot(2, vec![assigned]),
        },
    )
    .expect("authoritative assignment update");

    assert!(matches!(
        reloaded.model.agent_modal(),
        Some(UiAgentModal::Details {
            agent: UiAgent {
                status: UiAgentStatus::Assigned(assignment),
                ..
            },
            ..
        }) if assignment.project_name == "release"
            && assignment.phase == UiAgentAssignmentPhase::SettingUp
    ));
}

#[test]
fn agent_create_and_session_rename_emit_exact_typed_commands_and_preserve_failures() {
    let model = loaded_agents_model(1, &[agent(3, "builder")]);
    let create = update(model, UiEvent::Input(UiInput::Character('c'))).expect("create");
    let named = update(
        create.model,
        UiEvent::Input(UiInput::Paste("reviewer".to_owned())),
    )
    .expect("name");
    let submitted = update(named.model, UiEvent::Input(UiInput::Activate)).expect("submit");
    let (create_id, create_action) = agent_action_effect(&submitted.effects);
    assert_eq!(
        create_action,
        &UiAgentAction::Create {
            name: "reviewer".to_owned()
        }
    );
    let failed = update(
        submitted.model,
        UiEvent::AgentCommandFailed {
            effect_id: create_id,
            failure: UiFailure {
                code: "agent_command_failed".to_owned(),
                action: "correct the name and retry".to_owned(),
            },
        },
    )
    .expect("failure");
    assert!(matches!(
        failed.model.agent_modal(),
        Some(UiAgentModal::Create { name, submitting: false }) if name == "reviewer"
    ));

    let details_model = loaded_agents_model(1, &[agent(3, "builder")]);
    let details = update(details_model, UiEvent::Input(UiInput::Activate)).expect("details");
    let rename = update(details.model, UiEvent::Input(UiInput::Character('r'))).expect("rename");
    let cleared = update(rename.model, UiEvent::Input(UiInput::Backspace)).expect("clear old name");
    let renamed = update(
        cleared.model,
        UiEvent::Input(UiInput::Paste("live".to_owned())),
    )
    .expect("new name");
    let submitted =
        update(renamed.model, UiEvent::Input(UiInput::Activate)).expect("rename submit");
    assert!(matches!(
        agent_action_effect(&submitted.effects).1,
        UiAgentAction::RenameSession { agent_id, provider, session, display_name: Some(name) }
            if *agent_id == [3; 32] && provider == "codex" && session == "session-3" && name == "live"
    ));
}

#[test]
fn retirement_is_explicit_cancelable_and_force_is_part_of_the_typed_command() {
    let model = loaded_agents_model(1, &[agent(4, "worker")]);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let confirm = update(details.model, UiEvent::Input(UiInput::Character('x'))).expect("confirm");
    let cancelled = update(confirm.model, UiEvent::Input(UiInput::Escape)).expect("cancel");
    assert!(cancelled.model.agent_modal().is_none());
    assert!(
        !cancelled
            .effects
            .iter()
            .any(|effect| matches!(effect, UiEffect::SubmitAgentCommand { .. }))
    );

    let details =
        update(cancelled.model, UiEvent::Input(UiInput::Activate)).expect("details again");
    let confirm = update(details.model, UiEvent::Input(UiInput::Character('x'))).expect("confirm");
    let forced = update(confirm.model, UiEvent::Input(UiInput::Character('f'))).expect("force");
    let submitted = update(forced.model, UiEvent::Input(UiInput::Activate)).expect("retire");
    assert!(matches!(
        agent_action_effect(&submitted.effects).1,
        UiAgentAction::Retire { agent_id, force: true } if *agent_id == [4; 32]
    ));
}

#[test]
fn managed_session_start_confirms_switch_and_exact_resume_and_stop_emit_typed_commands() {
    let model = loaded_agents_model(1, &[agent(5, "runtime")]);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let confirm = update(details.model, UiEvent::Input(UiInput::Character('s')))
        .expect("the only provider is selected automatically");
    assert!(matches!(
        confirm.model.agent_modal(),
        Some(UiAgentModal::ConfirmManagedSession {
            action: UiManagedSessionAction::Start { agent_id, provider }, ..
        }) if *agent_id == [5; 32] && provider == "codex"
    ));
    let started = update(confirm.model, UiEvent::Input(UiInput::Activate)).expect("start");
    assert!(matches!(
        managed_session_effect(&started.effects).1,
        UiManagedSessionAction::Start { agent_id, provider }
            if *agent_id == [5; 32] && provider == "codex"
    ));

    let model = loaded_agents_model(1, &[agent(5, "runtime")]);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let resumed =
        update(details.model, UiEvent::Input(UiInput::Character('e'))).expect("exact resume");
    assert!(matches!(
        managed_session_effect(&resumed.effects).1,
        UiManagedSessionAction::Resume { agent_id, provider, session }
            if *agent_id == [5; 32] && provider == "codex" && session == "session-5"
    ));

    let model = loaded_agents_model(1, &[agent(5, "runtime")]);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let stopped = update(details.model, UiEvent::Input(UiInput::Character('t'))).expect("stop");
    assert!(matches!(
        managed_session_effect(&stopped.effects).1,
        UiManagedSessionAction::Stop { agent_id, provider }
            if *agent_id == [5; 32] && provider == "codex"
    ));
}

#[test]
fn managed_session_provider_choices_select_defaults_and_skip_unavailable_entries() {
    let target = agent(15, "chooser");
    let providers = vec![
        available_provider("alpha", "Alpha", false),
        available_provider("codex", "Codex", true),
        UiProvider {
            provider: "offline".to_owned(),
            name: "Offline service".to_owned(),
            available: false,
            configured_default: false,
        },
    ];
    let model = loaded_agents_model_with_providers(1, vec![target.clone()], providers);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let choosing = update(details.model, UiEvent::Input(UiInput::Character('s')))
        .expect("several providers require a choice");
    assert!(matches!(
        choosing.model.agent_modal(),
        Some(UiAgentModal::ManagedProvider { selected: Some(provider), .. })
            if provider == "codex"
    ));
    let choosing = update(choosing.model, UiEvent::Input(UiInput::PreviousItem))
        .expect("move to the previous available provider");
    assert!(matches!(
        choosing.model.agent_modal(),
        Some(UiAgentModal::ManagedProvider { selected: Some(provider), .. })
            if provider == "alpha"
    ));
    let ignored = update(
        choosing.model.clone(),
        UiEvent::Input(UiInput::Paste("offline".to_owned())),
    )
    .expect("provider names are not editable");
    assert_eq!(ignored.model, choosing.model);
    assert!(ignored.effects.is_empty());
    let confirm = update(choosing.model, UiEvent::Input(UiInput::Activate)).expect("choose alpha");
    assert!(matches!(
        confirm.model.agent_modal(),
        Some(UiAgentModal::ConfirmManagedSession {
            action: UiManagedSessionAction::Start { provider, .. }, ..
        }) if provider == "alpha"
    ));
}

#[test]
fn managed_session_provider_choices_explain_empty_and_ignore_unavailable_defaults() {
    let target = agent(15, "chooser");
    let model = loaded_agents_model_with_providers(1, vec![target.clone()], Vec::new());
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let empty = update(details.model, UiEvent::Input(UiInput::Character('s')))
        .expect("empty provider catalog opens guidance");
    assert!(matches!(
        empty.model.agent_modal(),
        Some(UiAgentModal::ManagedProvider { providers, selected: None, .. })
            if providers.is_empty()
    ));
    let inert = update(empty.model.clone(), UiEvent::Input(UiInput::Activate))
        .expect("no provider cannot submit");
    assert_eq!(inert.model, empty.model);
    assert!(
        inert
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitManagedSession { .. }))
    );

    let providers = vec![
        available_provider("alpha", "Alpha", false),
        UiProvider {
            provider: "removed".to_owned(),
            name: "Removed service".to_owned(),
            available: false,
            configured_default: true,
        },
    ];
    let model = loaded_agents_model_with_providers(1, vec![target.clone()], providers);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let automatic = update(details.model, UiEvent::Input(UiInput::Character('s')))
        .expect("one available provider ignores an unavailable default");
    assert!(matches!(
        automatic.model.agent_modal(),
        Some(UiAgentModal::ConfirmManagedSession {
            action: UiManagedSessionAction::Start { provider, .. }, ..
        }) if provider == "alpha"
    ));
}

#[test]
fn managed_session_provider_choice_recovers_when_the_catalog_changes() {
    let target = agent(15, "chooser");
    let providers = vec![
        available_provider("alpha", "Alpha", true),
        available_provider("codex", "Codex", false),
    ];
    let model = loaded_agents_model_with_providers(1, vec![target.clone()], providers);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let choosing =
        update(details.model, UiEvent::Input(UiInput::Character('s'))).expect("choose default");
    let invalidated = update(choosing.model, UiEvent::Invalidated { revision: 2 })
        .expect("provider catalog refresh");
    let effect_id = snapshot_effect(&invalidated.effects);
    let refreshed = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id,
            snapshot: agents_snapshot(2, vec![target]),
        },
    )
    .expect("stale provider selection is replaced");
    assert!(matches!(
        refreshed.model.agent_modal(),
        Some(UiAgentModal::ManagedProvider { selected: Some(provider), .. })
            if provider == "codex"
    ));
    assert_eq!(
        refreshed
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("provider_choice_stale")
    );
}

#[test]
fn managed_session_switch_cancel_stale_completion_and_actionable_outcomes_are_explicit() {
    let mut target = agent(6, "switcher");
    target.sessions.push(UiAgentSession {
        provider: "codex".to_owned(),
        session: "older-session".to_owned(),
        mailbox: None,
        conflicted: false,
        selected: false,
        name_resolved: true,
        display_name: Some("older".to_owned()),
    });
    let model = loaded_agents_model(1, &[target]);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let older = update(details.model, UiEvent::Input(UiInput::NextItem)).expect("older");
    let confirm = update(older.model, UiEvent::Input(UiInput::Character('e'))).expect("confirm");
    assert!(matches!(
        confirm.model.agent_modal(),
        Some(UiAgentModal::ConfirmManagedSession { .. })
    ));
    let cancelled = update(confirm.model, UiEvent::Input(UiInput::Escape)).expect("cancel");
    assert!(cancelled.model.agent_modal().is_none());

    let model = loaded_agents_model(1, &[agent(6, "switcher")]);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let pending = update(details.model, UiEvent::Input(UiInput::Character('e'))).expect("resume");
    let (effect_id, action) = managed_session_effect(&pending.effects);
    let reconnecting = update(
        pending.model,
        UiEvent::ConnectionObserved {
            generation: 7,
            state: UiConnectionState::Reconnecting,
        },
    )
    .expect("reconnect while managed operation is pending");
    let pending = update(
        reconnecting.model,
        UiEvent::Resized(UiSize {
            width: 61,
            height: 17,
        }),
    )
    .expect("resize while managed operation is pending");
    assert_eq!(pending.model.pending_managed_session(), Some(effect_id));
    assert!(matches!(
        pending.model.agent_modal(),
        Some(UiAgentModal::ManagingSession { .. })
    ));
    let stale_id = snapshot_effect(&started_model().effects);
    assert_ne!(stale_id, effect_id);
    let stale = update(
        pending.model.clone(),
        UiEvent::ManagedSessionCompleted {
            effect_id: stale_id,
            result: UiManagedSessionResult {
                action: action.clone(),
                operation_id: [7; 32],
                outcome: UiManagedSessionOutcome::Stopped,
            },
        },
    )
    .expect("stale completion");
    assert_eq!(stale.model, pending.model);

    let rejected = update(
        pending.model,
        UiEvent::ManagedSessionCompleted {
            effect_id,
            result: UiManagedSessionResult {
                action: action.clone(),
                operation_id: [8; 32],
                outcome: UiManagedSessionOutcome::Rejected {
                    category: "domain".to_owned(),
                    code: "managed_session_precondition".to_owned(),
                },
            },
        },
    )
    .expect("rejected");
    assert!(matches!(
        rejected.model.agent_modal(),
        Some(UiAgentModal::ManagedSessionOutcome {
            result: UiManagedSessionResult {
                outcome: UiManagedSessionOutcome::Rejected { code, .. }, ..
            }, ..
        }) if code == "managed_session_precondition"
    ));
    assert_eq!(
        rejected
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("managed_session_precondition")
    );
}

#[test]
fn managed_session_uncertainty_retains_operation_and_reconciliation_identity() {
    let model = loaded_agents_model(1, &[agent(6, "switcher")]);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let pending = update(details.model, UiEvent::Input(UiInput::Character('t'))).expect("stop");
    let (effect_id, action) = managed_session_effect(&pending.effects);
    let uncertain = update(
        pending.model,
        UiEvent::ManagedSessionCompleted {
            effect_id,
            result: UiManagedSessionResult {
                action: action.clone(),
                operation_id: [10; 32],
                outcome: UiManagedSessionOutcome::Uncertain {
                    reconciliation_id: [11; 32],
                },
            },
        },
    )
    .expect("uncertain");
    assert!(matches!(
        uncertain.model.agent_modal(),
        Some(UiAgentModal::ManagedSessionOutcome {
            result: UiManagedSessionResult {
                operation_id,
                outcome: UiManagedSessionOutcome::Uncertain {
                    reconciliation_id
                },
                ..
            },
            ..
        }) if *operation_id == [10; 32] && *reconciliation_id == [11; 32]
    ));
}

#[test]
fn managed_session_success_returns_to_the_refreshed_agent_with_a_transient_confirmation() {
    let target = agent(16, "runtime");
    let model = loaded_agents_model(1, std::slice::from_ref(&target));
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let pending = update(details.model, UiEvent::Input(UiInput::Character('e')))
        .expect("resume selected conversation");
    let (effect_id, action) = managed_session_effect(&pending.effects);
    let completed = update(
        pending.model,
        UiEvent::ManagedSessionCompleted {
            effect_id,
            result: UiManagedSessionResult {
                action: action.clone(),
                operation_id: [17; 32],
                outcome: UiManagedSessionOutcome::Ready {
                    session: "session-16".to_owned(),
                },
            },
        },
    )
    .expect("routine completion");
    assert!(completed.model.agent_modal().is_none());
    assert_eq!(
        completed.model.completion_notice(),
        Some("Agent conversation ready")
    );

    let snapshot_id = snapshot_effect(&completed.effects);
    let refreshed = update(
        completed.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot: agents_snapshot(2, vec![target]),
        },
    )
    .expect("refreshed agent context");
    assert!(matches!(
        refreshed.model.agent_modal(),
        Some(UiAgentModal::Details { agent, selected_session: Some((provider, session)) })
            if agent.agent_id == [16; 32] && provider == "codex" && session == "session-16"
    ));
}

#[test]
fn stopped_session_completion_survives_a_failed_refresh_and_returns_to_agent_details() {
    let target = agent(18, "runtime");
    let model = loaded_agents_model(1, std::slice::from_ref(&target));
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let pending = update(details.model, UiEvent::Input(UiInput::Character('t'))).expect("stop");
    let (effect_id, action) = managed_session_effect(&pending.effects);
    let completed = update(
        pending.model,
        UiEvent::ManagedSessionCompleted {
            effect_id,
            result: UiManagedSessionResult {
                action: action.clone(),
                operation_id: [19; 32],
                outcome: UiManagedSessionOutcome::Stopped,
            },
        },
    )
    .expect("stop completion");
    assert!(completed.model.agent_modal().is_none());
    assert_eq!(
        completed.model.completion_notice(),
        Some("Agent stopped; saved conversation kept")
    );
    let snapshot_id = snapshot_effect(&completed.effects);
    let failed = update(
        completed.model,
        UiEvent::SnapshotFailed {
            effect_id: snapshot_id,
            failure: UiFailure {
                code: "disconnected".to_owned(),
                action: "wait for HQ to reconnect".to_owned(),
            },
        },
    )
    .expect("refresh response lost");
    let retry_timer = timer_effect(&failed.effects, UiTimerKind::RetrySnapshot);
    let retrying = update(
        failed.model,
        UiEvent::TimerElapsed {
            effect_id: retry_timer,
        },
    )
    .expect("retry refresh");
    let retry_snapshot = snapshot_effect(&retrying.effects);
    let refreshed = update(
        retrying.model,
        UiEvent::SnapshotLoaded {
            effect_id: retry_snapshot,
            snapshot: agents_snapshot(2, vec![target]),
        },
    )
    .expect("reconnected context");
    assert!(matches!(
        refreshed.model.agent_modal(),
        Some(UiAgentModal::Details { agent, selected_session: Some((provider, session)) })
            if agent.agent_id == [18; 32] && provider == "codex" && session == "session-18"
    ));
}

#[test]
fn mailbox_navigation_workspace_survives_visiting_agent_session_management() {
    let mut source = snapshot(1, &["thread-a"]);
    let agent_source = agents_snapshot(1, vec![agent(9, "runtime")]);
    source.agent_rows = agent_source.agent_rows;
    source.agents = agent_source.agents;
    let mut model = materialized_transition(
        source,
        UiConversationPage {
            title: "Alice".to_owned(),
            context: None,
            row_id: "thread-a".to_owned(),
            entries: vec![entry("message-a", false)],
            next_cursor: None,
        },
    )
    .model;
    assert_eq!(model.conversation_anchor(), Some("message-a"));
    for _ in 0..3 {
        model = update(model, UiEvent::Input(UiInput::NextSection))
            .expect("next cached section")
            .model;
    }
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("agent details");
    model = update(details.model, UiEvent::Input(UiInput::Escape))
        .expect("close details")
        .model;
    for _ in 0..3 {
        model = update(model, UiEvent::Input(UiInput::PreviousSection))
            .expect("previous cached section")
            .model;
    }
    assert_eq!(model.selected_row(), Some("thread-a"));
    assert_eq!(model.conversation_anchor(), Some("message-a"));
    assert!(model.conversation().is_some());
}

#[test]
fn project_search_and_details_preserve_stable_identity_across_reload_and_resize() {
    let alpha = project(1, "alpha", "/work/alpha");
    let beta = project(2, "beta", "/work/beta");
    let mut model = loaded_projects_model(4, vec![alpha, beta.clone()]);

    model = update(model, UiEvent::Input(UiInput::Character('/')))
        .expect("open search")
        .model;
    model = update(model, UiEvent::Input(UiInput::Paste("beta".to_owned())))
        .expect("search")
        .model;
    assert_eq!(model.selected_row(), Some(agent_row_id(2).as_str()));
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("inspect")
        .model;
    let resized = update(
        model,
        UiEvent::Resized(UiSize {
            width: 73,
            height: 18,
        }),
    )
    .expect("resize");
    let invalidated = update(resized.model, UiEvent::Invalidated { revision: 5 }).expect("reload");
    let request = snapshot_effect(&invalidated.effects);
    let mut current = beta;
    current.name = "beta current".to_owned();
    let loaded = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: request,
            snapshot: projects_snapshot(5, vec![current, project(1, "alpha", "/work/alpha")]),
        },
    )
    .expect("authoritative reorder");
    assert_eq!(loaded.model.selected_row(), Some(agent_row_id(2).as_str()));
    assert_eq!(
        loaded
            .model
            .project_summary()
            .map(|project| project.name.as_str()),
        Some("beta current")
    );
}

#[test]
fn project_workspace_summary_preserves_typed_focus_across_resize_and_reload() {
    let mut target = project(2, "beta", "/work/beta");
    target.assignment = Some(UiProjectAssignment {
        assignment_id: [20; 32],
        agent_id: [21; 32],
        provider: "codex".to_owned(),
        session: Some("saved-session".to_owned()),
        phase: "runnable".to_owned(),
        thread_id: Some([22; 32]),
        launch_directory: Some("/work/beta".to_owned()),
        blocked: None,
        cardinality_conflicted: false,
        runnable: true,
    });
    let mut snapshot = projects_snapshot(4, vec![project(1, "alpha", "/work/alpha"), target]);
    snapshot.agents = vec![project_agent(21, [9; 32])];
    snapshot.inbox_rows = vec![project_conversation_row(2, 22, 23, "Alice")];
    let mut model = loaded_projects_snapshot(snapshot);
    model = update(model, UiEvent::Input(UiInput::NextItem))
        .expect("select beta")
        .model;

    let summary = model.project_summary().expect("selected project summary");
    assert_eq!(summary.name, "beta");
    assert_eq!(summary.conversations.open, 1);
    assert_eq!(summary.folders.len(), 1);
    assert_eq!(summary.assigned_agent.name.as_deref(), Some("agent-21"));

    model = update(model, UiEvent::Input(UiInput::MoveCursorRight))
        .expect("enter summary")
        .model;
    assert_eq!(
        model.project_workspace_level(),
        UiProjectWorkspaceLevel::Summary
    );
    model = update(model, UiEvent::Input(UiInput::NextItem))
        .expect("focus agent card")
        .model;
    assert_eq!(
        model.project_summary_focus(),
        Some(UiProjectSummaryFocus::AssignedAgent)
    );
    model = update(
        model,
        UiEvent::Resized(UiSize {
            width: 64,
            height: 18,
        }),
    )
    .expect("compact resize")
    .model;
    let invalidated = update(model, UiEvent::Invalidated { revision: 5 }).expect("reload");
    let effect_id = snapshot_effect(&invalidated.effects);
    let mut refreshed = projects_snapshot(
        5,
        vec![
            project(2, "beta renamed", "/work/beta"),
            project(1, "alpha", "/work/alpha"),
        ],
    );
    refreshed.agents = vec![project_agent(21, [9; 32])];
    refreshed.inbox_rows = vec![project_conversation_row(2, 22, 23, "Alice")];
    let loaded = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id,
            snapshot: refreshed,
        },
    )
    .expect("replacement snapshot");

    assert_eq!(loaded.model.selected_row(), Some(agent_row_id(2).as_str()));
    assert_eq!(
        loaded.model.project_workspace_level(),
        UiProjectWorkspaceLevel::Summary
    );
    assert_eq!(
        loaded.model.project_summary_focus(),
        Some(UiProjectSummaryFocus::AssignedAgent)
    );
    assert_eq!(
        loaded
            .model
            .project_summary()
            .map(|summary| summary.name.as_str()),
        Some("beta renamed")
    );
}

#[test]
fn project_primary_action_routes_zero_one_and_many_conversations_without_guessing() {
    let project = project(7, "routing", "/work/routing");

    let zero = update(
        loaded_projects_model(1, vec![project.clone()]),
        UiEvent::Input(UiInput::Activate),
    )
    .expect("start first conversation");
    assert_eq!(zero.model.section(), UiSection::Inbox);
    assert_eq!(
        zero.model.project_filter().map(|filter| filter.project_id),
        Some(project.project_id)
    );
    assert!(zero.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::OpenDraft {
            target: UiMailboxDraftTarget::Project {
                project_id,
                thread_id: None,
            },
            ..
        } if *project_id == project.project_id
    )));

    let first = project_conversation_row(7, 31, 41, "Alice");
    let mut one_snapshot = projects_snapshot(2, vec![project.clone()]);
    one_snapshot.inbox_rows = vec![first.clone()];
    let one = update(
        loaded_projects_snapshot(one_snapshot),
        UiEvent::Input(UiInput::Activate),
    )
    .expect("continue sole conversation");
    assert_eq!(one.model.section(), UiSection::Inbox);
    assert_eq!(one.model.selected_row(), Some(first.id.as_str()));
    assert!(one.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::ObserveConversation { row_id: Some(row_id) } if row_id == &first.id
    )));

    let second = project_conversation_row(7, 32, 42, "Bob");
    let mut many_snapshot = projects_snapshot(3, vec![project.clone()]);
    many_snapshot.inbox_rows = vec![first.clone(), second.clone()];
    let many = update(
        loaded_projects_snapshot(many_snapshot.clone()),
        UiEvent::Input(UiInput::Activate),
    )
    .expect("open filtered conversation list");
    assert_eq!(many.model.section(), UiSection::Inbox);
    assert_eq!(many.model.focus(), UiFocus::Content);
    assert_eq!(many.model.rows().expect("filtered rows").len(), 2);
    assert_eq!(
        many.model
            .project_filter()
            .map(|filter| filter.project_name.as_str()),
        Some("routing")
    );
    assert!(
        many.effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::LoadConversation { .. }))
    );

    let observed = update(
        many.model,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot: many_snapshot,
                conversation: Some(UiConversationPage {
                    title: first.title.clone(),
                    context: None,
                    row_id: first.id.clone(),
                    entries: vec![entry("project-message", false)],
                    next_cursor: None,
                }),
            },
        },
    )
    .expect("filtered conversation observed");
    let conversation = update(observed.model, UiEvent::Input(UiInput::MoveCursorRight))
        .expect("enter filtered conversation");
    let inbox = update(conversation.model, UiEvent::Input(UiInput::Escape))
        .expect("leave conversation before clearing filter");
    assert_eq!(inbox.model.focus(), UiFocus::Content);
    assert!(inbox.model.project_filter().is_some());

    let cleared =
        update(inbox.model, UiEvent::Input(UiInput::Escape)).expect("clear project filter");
    assert!(cleared.model.project_filter().is_none());
}

#[test]
#[allow(clippy::too_many_lines)]
fn both_project_creation_modes_emit_exact_typed_commands_and_cancel_without_effects() {
    let mut model = loaded_projects_model(1, Vec::new());
    model = update(model, UiEvent::Input(UiInput::Character('c')))
        .expect("creation chooser")
        .model;
    assert!(matches!(
        model.project_interaction(),
        Some(UiProjectInteraction::ChooseCreation {
            selected: UiProjectCreationChoice::ExistingFolder
        })
    ));
    model = update(model, UiEvent::Input(UiInput::PreviousItem))
        .expect("creation chooser clamps at its first item")
        .model;
    assert!(matches!(
        model.project_interaction(),
        Some(UiProjectInteraction::ChooseCreation {
            selected: UiProjectCreationChoice::ExistingFolder
        })
    ));
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("existing folder form")
        .model;
    model = update(
        model,
        UiEvent::Input(UiInput::Paste("/repo/existing".to_owned())),
    )
    .expect("path")
    .model;
    model = update(model, UiEvent::Input(UiInput::NextFocus))
        .expect("derived project name")
        .model;
    assert!(matches!(
        model.project_interaction(),
        Some(UiProjectInteraction::CreateExisting { name, .. }) if name == "existing"
    ));
    model = update(model, UiEvent::Input(UiInput::NextFocus))
        .expect("brief")
        .model;
    model = update(model, UiEvent::Input(UiInput::Paste("brief".to_owned())))
        .expect("brief text")
        .model;
    let submitted = update(model, UiEvent::Input(UiInput::Activate)).expect("submit existing");
    let (existing_id, existing_action) = project_effect(&submitted.effects);
    assert_eq!(
        existing_action,
        UiProjectAction::CreateExisting {
            name: "existing".to_owned(),
            brief: Some("brief".to_owned()),
            path: "/repo/existing".to_owned(),
        }
    );
    let failed = update(
        submitted.model,
        UiEvent::ProjectCommandFailed {
            effect_id: existing_id,
            failure: UiFailure {
                code: "path_changed".to_owned(),
                action: "inspect the current working tree".to_owned(),
            },
        },
    )
    .expect("recoverable failure");
    assert!(matches!(
        failed.model.project_interaction(),
        Some(UiProjectInteraction::CreateExisting { path, submitting: false, .. }) if path == "/repo/existing"
    ));
    let retry_model = failed.model.clone();
    let cancelled = update(failed.model, UiEvent::Input(UiInput::Escape)).expect("cancel");
    assert!(cancelled.model.project_interaction().is_none());
    assert!(
        cancelled
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitProjectCommand { .. }))
    );

    let committed = update(retry_model, UiEvent::Input(UiInput::Activate)).expect("retry create");
    let (create_id, create_action) = project_effect(&committed.effects);
    assert_eq!(
        create_action,
        UiProjectAction::CreateExisting {
            name: "existing".to_owned(),
            brief: Some("brief".to_owned()),
            path: "/repo/existing".to_owned(),
        }
    );
    let created = update(
        committed.model,
        UiEvent::ProjectCommandCompleted {
            effect_id: create_id,
            result: UiProjectResult {
                action: create_action.clone(),
                command_id: [37; 32],
                operation_id: [38; 32],
                project_id: [36; 32],
                runtime_state: None,
                runtime_code: None,
                outcome: UiProjectOutcome::Completed {
                    project_head: Some([39; 32]),
                },
            },
        },
    )
    .expect("creation completion");
    let creation_snapshot_id = snapshot_effect(&created.effects);
    assert!(created.model.project_interaction().is_none());
    assert_eq!(created.model.completion_notice(), Some("Project created"));
    let stale_refresh = update(
        created.model,
        UiEvent::SnapshotLoaded {
            effect_id: creation_snapshot_id,
            snapshot: projects_snapshot(1, Vec::new()),
        },
    )
    .expect("pre-commit refresh cannot strand completion");
    let followup_id = snapshot_effect(&stale_refresh.effects);
    let selected = update(
        stale_refresh.model,
        UiEvent::SnapshotLoaded {
            effect_id: followup_id,
            snapshot: projects_snapshot(2, vec![project(36, "existing", "/repo/existing")]),
        },
    )
    .expect("created project is selected");
    assert_eq!(
        selected.model.selected_row(),
        Some(agent_row_id(36).as_str())
    );
    assert!(selected.model.project_interaction().is_none());

    let mut model = update(cancelled.model, UiEvent::Input(UiInput::Character('c')))
        .expect("creation chooser")
        .model;
    model = update(model, UiEvent::Input(UiInput::NextItem))
        .expect("advanced worktree choice")
        .model;
    assert!(matches!(
        model.project_interaction(),
        Some(UiProjectInteraction::ChooseCreation {
            selected: UiProjectCreationChoice::IsolatedWorktree
        })
    ));
    let bounded = update(model.clone(), UiEvent::Input(UiInput::NextItem))
        .expect("creation chooser clamps at its last item");
    assert!(matches!(
        bounded.model.project_interaction(),
        Some(UiProjectInteraction::ChooseCreation {
            selected: UiProjectCreationChoice::IsolatedWorktree
        })
    ));
    let mut model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("worktree form")
        .model;
    for (index, value) in ["worktree", "", "/source", "/destination", "feature", "main"]
        .into_iter()
        .enumerate()
    {
        if !value.is_empty() {
            model = update(model, UiEvent::Input(UiInput::Paste(value.to_owned())))
                .expect("worktree field")
                .model;
        }
        if index < 5 {
            model = update(model, UiEvent::Input(UiInput::NextFocus))
                .expect("next worktree field")
                .model;
        }
    }
    let submitted = update(model, UiEvent::Input(UiInput::Activate)).expect("submit worktree");
    let (_, action) = project_effect(&submitted.effects);
    assert_eq!(
        action,
        UiProjectAction::CreateWorktree {
            name: "worktree".to_owned(),
            brief: None,
            source: "/source".to_owned(),
            destination: "/destination".to_owned(),
            branch: "feature".to_owned(),
            base: Some("main".to_owned()),
        }
    );
}

#[test]
fn existing_folder_creation_defaults_the_name_when_submitted_from_the_path_field() {
    let mut model = loaded_projects_model(1, Vec::new());
    model = update(model, UiEvent::Input(UiInput::Character('c')))
        .expect("creation chooser")
        .model;
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("existing folder form")
        .model;
    model = update(
        model,
        UiEvent::Input(UiInput::Paste("/repo/direct-submit".to_owned())),
    )
    .expect("folder path")
    .model;

    let submitted =
        update(model, UiEvent::Input(UiInput::Activate)).expect("submit from path field");
    assert_eq!(
        project_effect(&submitted.effects).1,
        UiProjectAction::CreateExisting {
            name: "direct-submit".to_owned(),
            brief: None,
            path: "/repo/direct-submit".to_owned(),
        }
    );
}

#[test]
fn forms_use_tab_unicode_safe_cursor_editing_and_current_user_path_expansion() {
    let mut model =
        loaded_projects_model(1, Vec::new()).with_home_directory(Some("/Users/example".to_owned()));
    model = update(model, UiEvent::Input(UiInput::Character('c')))
        .expect("creation chooser")
        .model;
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("existing form")
        .model;
    model = update(model, UiEvent::Input(UiInput::NextFocus))
        .expect("name field")
        .model;
    model = update(model, UiEvent::Input(UiInput::Paste("ac".to_owned())))
        .expect("name")
        .model;
    model = update(model, UiEvent::Input(UiInput::MoveCursorLeft))
        .expect("cursor left")
        .model;
    model = update(model, UiEvent::Input(UiInput::Character('é')))
        .expect("unicode insertion")
        .model;
    model = update(model, UiEvent::Input(UiInput::MoveCursorHome))
        .expect("cursor home")
        .model;
    model = update(model, UiEvent::Input(UiInput::Delete))
        .expect("delete at cursor")
        .model;
    model = update(model, UiEvent::Input(UiInput::MoveCursorEnd))
        .expect("cursor end")
        .model;
    model = update(model, UiEvent::Input(UiInput::Backspace))
        .expect("unicode-safe backspace")
        .model;
    assert!(matches!(
        model.project_interaction(),
        Some(UiProjectInteraction::CreateExisting { name, .. }) if name == "é"
    ));
    model = update(model, UiEvent::Input(UiInput::MoveCursorHome))
        .expect("cursor home before reconnect")
        .model;
    let invalidated = update(model, UiEvent::Invalidated { revision: 2 }).expect("reload");
    let request = snapshot_effect(&invalidated.effects);
    model = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: request,
            snapshot: projects_snapshot(2, Vec::new()),
        },
    )
    .expect("reconnected form")
    .model;
    model = update(model, UiEvent::Input(UiInput::Character('x')))
        .expect("insert at retained cursor")
        .model;
    assert!(matches!(
        model.project_interaction(),
        Some(UiProjectInteraction::CreateExisting { name, .. }) if name == "xé"
    ));

    model = update(model, UiEvent::Input(UiInput::NextFocus))
        .expect("brief field")
        .model;
    model = update(model, UiEvent::Input(UiInput::PreviousFocus))
        .expect("name field")
        .model;
    assert!(matches!(
        model.project_interaction(),
        Some(UiProjectInteraction::CreateExisting {
            field: UiProjectFormField::Name,
            ..
        })
    ));
    model = update(model, UiEvent::Input(UiInput::PreviousFocus))
        .expect("path field")
        .model;
    model = update(model, UiEvent::Input(UiInput::Paste("~/repo".to_owned())))
        .expect("home-relative path")
        .model;
    let submitted = update(model, UiEvent::Input(UiInput::Activate)).expect("submit");
    let (_, action) = project_effect(&submitted.effects);
    assert_eq!(
        action,
        UiProjectAction::CreateExisting {
            name: "xé".to_owned(),
            brief: None,
            path: "/Users/example/repo".to_owned(),
        }
    );
}

#[test]
fn resource_add_previews_authoritative_conflicts_before_mutation() {
    let target = project(9, "target", "/target");
    let mut model = open_project_folder_action(
        loaded_projects_model(1, vec![target.clone()]),
        0,
        UiProjectFolderAction::AddFolder,
    )
    .model;
    model = update(model, UiEvent::Input(UiInput::Paste("/shared".to_owned())))
        .expect("path")
        .model;
    model = update(model, UiEvent::Input(UiInput::NextFocus))
        .expect("primary field")
        .model;
    model = update(model, UiEvent::Input(UiInput::NextItem))
        .expect("primary toggle")
        .model;
    let previewing = update(model, UiEvent::Input(UiInput::Activate)).expect("preview");
    let (preview_id, preview_action) = project_effect(&previewing.effects);
    assert_eq!(
        preview_action,
        UiProjectAction::PreviewAddResource {
            project_id: target.project_id,
            path: "/shared".to_owned(),
            make_primary: true,
        }
    );
    let preview = update(
        previewing.model,
        UiEvent::ProjectCommandCompleted {
            effect_id: preview_id,
            result: UiProjectResult {
                action: preview_action,
                command_id: [4; 32],
                operation_id: [5; 32],
                project_id: target.project_id,
                runtime_state: None,
                runtime_code: None,
                outcome: UiProjectOutcome::ResourcePreview {
                    display_path: "/shared".to_owned(),
                    canonical_path: "/canonical/shared".to_owned(),
                    conflicts: vec![UiProjectResourceConflict {
                        project_id: [2; 32],
                        resource_id: [3; 32],
                        display_path: "/other".to_owned(),
                        canonical_path: "/canonical".to_owned(),
                        relationship: "descendant".to_owned(),
                    }],
                },
            },
        },
    )
    .expect("preview result");
    let blocked = update(preview.model, UiEvent::Input(UiInput::Activate)).expect("blocked");
    assert!(
        blocked
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitProjectCommand { .. }))
    );
    assert_eq!(
        blocked
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("project_resource_claim_conflict")
    );
}

#[test]
fn resource_edits_force_gate_selection_and_fresh_checks_use_exact_identities() {
    let mut target = project(10, "assigned", "/first");
    target.assignment = Some(UiProjectAssignment {
        assignment_id: [40; 32],
        agent_id: [41; 32],
        provider: "codex".to_owned(),
        session: Some("session".to_owned()),
        phase: "runnable".to_owned(),
        thread_id: Some([42; 32]),
        launch_directory: Some("/first".to_owned()),
        blocked: None,
        cardinality_conflicted: false,
        runnable: true,
    });
    target.resources.push(UiProjectResource {
        resource_id: [22; 32],
        display_path: "/second".to_owned(),
        canonical_path: "/second".to_owned(),
        health: "unknown".to_owned(),
        primary: false,
        active_claim: true,
        conflicting_projects: Vec::new(),
    });
    let model = open_project_folder_action(
        loaded_projects_model(1, vec![target.clone()]),
        1,
        UiProjectFolderAction::RemoveFolder,
    )
    .model;
    let gated = update(model, UiEvent::Input(UiInput::Activate)).expect("force gate");
    assert_eq!(
        gated
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("project_resource_remove_force_required")
    );
    let forced = update(gated.model, UiEvent::Input(UiInput::Character('f')))
        .expect("force toggle")
        .model;
    let removing = update(forced, UiEvent::Input(UiInput::Activate)).expect("remove");
    let (_, action) = project_effect(&removing.effects);
    assert_eq!(
        action,
        UiProjectAction::RemoveResource {
            project_id: target.project_id,
            resource_id: [22; 32],
            force: true,
        }
    );

    let checking = open_project_folder_action(
        loaded_projects_model(2, vec![target.clone()]),
        0,
        UiProjectFolderAction::CheckFolderNow,
    );
    let (check_id, check_action) = project_effect(&checking.effects);
    assert_eq!(
        check_action,
        UiProjectAction::CheckResources {
            project_id: target.project_id,
            resource_id: Some(target.resources[0].resource_id),
        }
    );
    let checked = update(
        checking.model,
        UiEvent::ProjectCommandCompleted {
            effect_id: check_id,
            result: UiProjectResult {
                action: check_action,
                command_id: [7; 32],
                operation_id: [8; 32],
                project_id: target.project_id,
                runtime_state: None,
                runtime_code: None,
                outcome: UiProjectOutcome::ResourceChecks {
                    checks: vec![UiProjectResourceCheck {
                        resource_id: target.resources[0].resource_id,
                        status: "accepted".to_owned(),
                        health: Some("healthy".to_owned()),
                        release: Some("clean".to_owned()),
                        observed_canonical_path: Some("/first".to_owned()),
                        details: None,
                        error_category: None,
                        error_code: None,
                        reconciliation_id: None,
                    }],
                },
            },
        },
    )
    .expect("fresh check");
    assert!(matches!(
        checked.model.project_interaction(),
        Some(UiProjectInteraction::Outcome { result })
            if matches!(result.outcome, UiProjectOutcome::ResourceChecks { .. })
    ));
}

#[test]
fn project_activation_uses_exact_project_thread_and_retained_directory() {
    let mut target = project(50, "activate", "/workspace/activate");
    target.threads.push(UiProjectThread {
        agent_id: [60; 32],
        provider: "codex".to_owned(),
        session: "durable-session".to_owned(),
        thread_id: [61; 32],
    });
    let model = loaded_projects_model_with_agents(
        1,
        vec![target.clone()],
        vec![project_agent(60, target.home)],
    );
    let mut model = open_project_management_action(model, UiProjectManagementAction::AssignAgent);
    model = update(model, UiEvent::Input(UiInput::NextFocus))
        .expect("mode field")
        .model;
    model = update(model, UiEvent::Input(UiInput::NextItem))
        .expect("exact mode")
        .model;
    let submitted = update(model, UiEvent::Input(UiInput::Activate)).expect("submit");
    assert!(submitted.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::SubmitProjectCommand {
            action: UiProjectAction::Activate {
                project_id,
                agent_id,
                provider,
                resume_session: Some(session),
                resume_thread: Some(thread),
                launch_directory,
            },
            ..
        } if *project_id == target.project_id
            && *agent_id == [60; 32]
            && provider == "codex"
            && session == "durable-session"
            && *thread == [61; 32]
            && launch_directory == "/workspace/activate"
    )));
}

#[test]
fn activation_target_and_edited_fields_survive_authoritative_reload() {
    let mut target = project(51, "retained activation", "/workspace/retained");
    target.threads.push(UiProjectThread {
        agent_id: [62; 32],
        provider: "codex".to_owned(),
        session: "retained-session".to_owned(),
        thread_id: [63; 32],
    });
    let agent = project_agent(62, target.home);
    let providers = vec![
        available_provider("alpha", "Alpha", false),
        available_provider("codex", "Codex", true),
    ];
    let model = loaded_projects_model_with_agents_and_providers(
        1,
        vec![target.clone()],
        vec![agent.clone()],
        providers.clone(),
    );
    let mut model = open_project_management_action(model, UiProjectManagementAction::AssignAgent);
    for _ in 0..2 {
        model = update(model, UiEvent::Input(UiInput::NextFocus))
            .expect("provider field")
            .model;
    }
    model = update(model, UiEvent::Input(UiInput::PreviousItem))
        .expect("provider choice")
        .model;
    model = update(model, UiEvent::Input(UiInput::NextFocus))
        .expect("directory field")
        .model;
    model = update(model, UiEvent::Input(UiInput::Paste("/child".to_owned())))
        .expect("directory edit")
        .model;
    let invalidated = update(model, UiEvent::Invalidated { revision: 2 }).expect("reload");
    let effect_id = snapshot_effect(&invalidated.effects);
    let mut snapshot = projects_snapshot(2, vec![target]);
    snapshot.agents = vec![agent];
    snapshot.providers = providers;
    let reloaded = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id,
            snapshot,
        },
    )
    .expect("authoritative reload")
    .model;
    let Some(UiProjectInteraction::Activate {
        agent_id,
        thread,
        provider,
        directory,
        ..
    }) = reloaded.project_interaction()
    else {
        panic!("activation remains open");
    };
    assert_eq!(*agent_id, Some([62; 32]));
    assert_eq!(thread.as_ref().map(|value| value.thread_id), Some([63; 32]));
    assert_eq!(provider, "alpha");
    assert_eq!(directory, "/workspace/retained/child");
}

#[test]
fn project_new_session_provider_is_a_typed_choice_and_empty_catalog_blocks_submission() {
    let target = project(52, "provider choice", "/workspace/provider-choice");
    let agent = project_agent(64, target.home);
    let providers = vec![
        available_provider("alpha", "Alpha", false),
        available_provider("codex", "Codex", true),
    ];
    let model = loaded_projects_model_with_agents_and_providers(
        1,
        vec![target.clone()],
        vec![agent.clone()],
        providers,
    );
    let mut model = open_project_management_action(model, UiProjectManagementAction::AssignAgent);
    for _ in 0..2 {
        model = update(model, UiEvent::Input(UiInput::NextFocus))
            .expect("provider field")
            .model;
    }
    let ignored = update(
        model.clone(),
        UiEvent::Input(UiInput::Paste("invented".to_owned())),
    )
    .expect("provider field does not accept text");
    assert_eq!(ignored.model, model);
    assert!(ignored.effects.is_empty());
    let selected = update(model, UiEvent::Input(UiInput::PreviousItem))
        .expect("choose alpha")
        .model;
    assert!(matches!(
        selected.project_interaction(),
        Some(UiProjectInteraction::Activate { provider, .. }) if provider == "alpha"
    ));

    let model =
        loaded_projects_model_with_agents_and_providers(1, vec![target], vec![agent], Vec::new());
    let model = open_project_management_action(model, UiProjectManagementAction::AssignAgent);
    let blocked = update(model, UiEvent::Input(UiInput::Activate))
        .expect("missing provider is explained before submission");
    assert!(
        blocked
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitProjectCommand { .. }))
    );
    assert!(matches!(
        blocked.model.project_interaction(),
        Some(UiProjectInteraction::Activate { provider, submitting: false, .. }) if provider.is_empty()
    ));
}

#[test]
fn successful_project_activation_does_not_open_a_message_dialog() {
    let target = project(53, "activation", "/workspace/activation");
    let agent = project_agent(65, target.home);
    let model = open_project_management_action(
        loaded_projects_model_with_agents(1, vec![target.clone()], vec![agent.clone()]),
        UiProjectManagementAction::AssignAgent,
    );
    let pending = update(model, UiEvent::Input(UiInput::Activate)).expect("activate project");
    let (effect_id, action) = project_effect(&pending.effects);
    let completed = update(
        pending.model,
        UiEvent::ProjectCommandCompleted {
            effect_id,
            result: UiProjectResult {
                action: action.clone(),
                command_id: [66; 32],
                operation_id: [67; 32],
                project_id: target.project_id,
                runtime_state: Some("ready".to_owned()),
                runtime_code: None,
                outcome: UiProjectOutcome::Completed {
                    project_head: Some([68; 32]),
                },
            },
        },
    )
    .expect("activation completion");
    assert_eq!(
        completed.model.completion_notice(),
        Some("Project work is ready")
    );
    let snapshot_id = snapshot_effect(&completed.effects);
    let mut refreshed_project = target;
    refreshed_project.assignment = Some(UiProjectAssignment {
        assignment_id: [69; 32],
        agent_id: agent.agent_id,
        provider: "codex".to_owned(),
        session: Some("session-65".to_owned()),
        phase: "runnable".to_owned(),
        thread_id: Some([70; 32]),
        launch_directory: Some("/workspace/activation".to_owned()),
        blocked: None,
        cardinality_conflicted: false,
        runnable: true,
    });
    let refreshed = update(
        completed.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot: projects_snapshot(2, vec![refreshed_project]),
        },
    )
    .expect("refresh project state");
    assert!(refreshed.model.project_interaction().is_none());
    assert!(refreshed.model.mailbox_draft().is_none());
}

#[test]
fn handoff_requires_confirmation_and_keeps_force_separate() {
    let mut target = project(70, "handoff", "/workspace/handoff");
    target.assignment = Some(UiProjectAssignment {
        assignment_id: [71; 32],
        agent_id: [72; 32],
        provider: "codex".to_owned(),
        session: Some("old-session".to_owned()),
        phase: "blocked".to_owned(),
        thread_id: Some([73; 32]),
        launch_directory: Some("/workspace/handoff".to_owned()),
        blocked: Some("runtime_stop_uncertain".to_owned()),
        cardinality_conflicted: false,
        runnable: false,
    });
    target.threads.push(UiProjectThread {
        agent_id: [80; 32],
        provider: "codex".to_owned(),
        session: "target-session".to_owned(),
        thread_id: [81; 32],
    });
    let model = loaded_projects_model_with_agents(
        1,
        vec![target.clone()],
        vec![
            project_agent(72, target.home),
            project_agent(80, target.home),
        ],
    );
    let model =
        open_project_management_action(model, UiProjectManagementAction::ChangeAssignedAgent);
    let blocked = update(model, UiEvent::Input(UiInput::Activate)).expect("confirmation gate");
    assert!(
        blocked
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitProjectCommand { .. }))
    );
    let confirmed = update(blocked.model, UiEvent::Input(UiInput::NextItem))
        .expect("confirm")
        .model;
    let force_field = update(confirmed, UiEvent::Input(UiInput::NextFocus))
        .expect("force field")
        .model;
    let forced = update(force_field, UiEvent::Input(UiInput::NextItem))
        .expect("force")
        .model;
    let submitted = update(forced, UiEvent::Input(UiInput::Activate)).expect("submit");
    assert!(submitted.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::SubmitProjectCommand {
            action: UiProjectAction::Handoff {
                project_id,
                agent_id,
                thread_id,
                force_takeover: true,
                ..
            },
            ..
        } if *project_id == target.project_id && *agent_id == [80; 32] && *thread_id == [81; 32]
    )));
}

#[test]
fn project_workspace_does_not_offer_manual_dispatch_without_typed_stalled_evidence() {
    let target = project(90, "dispatch", "/workspace/dispatch");
    let model = open_project_management(loaded_projects_model(1, vec![target]));
    let ignored = update(model, UiEvent::Input(UiInput::Character('d'))).expect("ignored shortcut");
    assert!(ignored.effects.iter().all(|effect| !matches!(
        effect,
        UiEffect::SubmitProjectCommand {
            action: UiProjectAction::DispatchPending { .. },
            ..
        }
    )));
}

#[test]
fn resource_add_retains_input_across_reload_and_preview_failure() {
    let target = project_with_second_resource();
    let mut model = open_project_folder_action(
        loaded_projects_model(1, vec![target.clone()]),
        0,
        UiProjectFolderAction::AddFolder,
    )
    .model;
    model = update(model, UiEvent::Input(UiInput::Paste("/added".to_owned())))
        .expect("add path")
        .model;

    let invalidated = update(model, UiEvent::Invalidated { revision: 2 }).expect("reload");
    let snapshot_id = snapshot_effect(&invalidated.effects);
    let mut refreshed = target.clone();
    refreshed.name = "resources-current".to_owned();
    model = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot: projects_snapshot(2, vec![refreshed]),
        },
    )
    .expect("current project")
    .model;
    assert!(matches!(
        model.project_interaction(),
        Some(UiProjectInteraction::AddResource { project, path, .. })
            if project.name == "resources-current" && path == "/added"
    ));

    let previewing = update(model, UiEvent::Input(UiInput::Activate)).expect("preview add");
    let (preview_id, preview_action) = project_effect(&previewing.effects);
    model = update(
        previewing.model,
        UiEvent::ProjectCommandFailed {
            effect_id: preview_id,
            failure: UiFailure {
                code: "resource_inspection_failed".to_owned(),
                action: "repair the path and retry".to_owned(),
            },
        },
    )
    .expect("preview failure")
    .model;
    assert!(matches!(
        model.project_interaction(),
        Some(UiProjectInteraction::AddResource { path, submitting: false, .. }) if path == "/added"
    ));
    assert_eq!(
        model.last_failure().map(|failure| failure.code.as_str()),
        Some("resource_inspection_failed")
    );

    let previewing = update(model, UiEvent::Input(UiInput::Activate)).expect("retry preview");
    let (preview_id, retried_action) = project_effect(&previewing.effects);
    assert_eq!(retried_action, preview_action);
    let previewed = update(
        previewing.model,
        UiEvent::ProjectCommandCompleted {
            effect_id: preview_id,
            result: UiProjectResult {
                action: retried_action,
                command_id: [40; 32],
                operation_id: [41; 32],
                project_id: target.project_id,
                runtime_state: None,
                runtime_code: None,
                outcome: UiProjectOutcome::ResourcePreview {
                    display_path: "/added".to_owned(),
                    canonical_path: "/canonical/added".to_owned(),
                    conflicts: Vec::new(),
                },
            },
        },
    )
    .expect("clean preview");
    let adding = update(previewed.model, UiEvent::Input(UiInput::Activate)).expect("add");
    assert_eq!(
        project_effect(&adding.effects).1,
        UiProjectAction::AddResource {
            project_id: target.project_id,
            path: "/added".to_owned(),
            make_primary: false,
        }
    );
}

#[test]
fn resource_replace_and_primary_are_exact_and_cancelable() {
    let target = project_with_second_resource();
    let cancelled = update(
        open_project_folder_action(
            loaded_projects_model(3, vec![target.clone()]),
            1,
            UiProjectFolderAction::ChangeFolderPath,
        )
        .model,
        UiEvent::Input(UiInput::Escape),
    )
    .expect("cancel replace");
    assert!(
        cancelled
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitProjectCommand { .. }))
    );
    assert!(cancelled.model.project_interaction().is_none());

    let mut model = open_project_folder_action(
        loaded_projects_model(3, vec![target.clone()]),
        1,
        UiProjectFolderAction::ChangeFolderPath,
    )
    .model;
    model = update(
        model,
        UiEvent::Input(UiInput::Paste("/replacement".to_owned())),
    )
    .expect("replacement path")
    .model;
    let replacing = update(model, UiEvent::Input(UiInput::Activate)).expect("replace preview");
    assert_eq!(
        project_effect(&replacing.effects).1,
        UiProjectAction::PreviewReplaceResource {
            project_id: target.project_id,
            resource_id: [33; 32],
            path: "/replacement".to_owned(),
        }
    );

    let primary = open_project_folder_action(
        loaded_projects_model(4, vec![target.clone()]),
        1,
        UiProjectFolderAction::UseAsWorkingFolder,
    );
    assert_eq!(
        project_effect(&primary.effects).1,
        UiProjectAction::SetPrimaryResource {
            project_id: target.project_id,
            resource_id: [33; 32],
        }
    );
}

#[test]
fn resource_check_failure_retains_exact_details_context() {
    let target = project_with_second_resource();
    let checking = open_project_folder_action(
        loaded_projects_model(5, vec![target.clone()]),
        0,
        UiProjectFolderAction::CheckAllFolders,
    );
    let (check_id, action) = project_effect(&checking.effects);
    assert_eq!(
        action,
        UiProjectAction::CheckResources {
            project_id: target.project_id,
            resource_id: None,
        }
    );
    let failed = update(
        checking.model,
        UiEvent::ProjectCommandFailed {
            effect_id: check_id,
            failure: UiFailure {
                code: "resource_check_unavailable".to_owned(),
                action: "retry after reconnect".to_owned(),
            },
        },
    )
    .expect("check failure");
    assert_eq!(
        failed.model.project_workspace_level(),
        UiProjectWorkspaceLevel::Folders
    );
    assert_eq!(
        failed
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("resource_check_unavailable")
    );
}

fn snapshot(revision: u64, ids: &[&str]) -> UiSnapshot {
    snapshot_for(UiSection::Inbox, revision, ids)
}

fn snapshot_for(section: UiSection, revision: u64, ids: &[&str]) -> UiSnapshot {
    let rows = ids
        .iter()
        .map(|id| UiRow {
            id: (*id).to_owned(),
            title: format!("{id} title"),
            detail: format!("{id} detail"),
            state: UiRowState::Open,
            kind: UiRowKind::Conversation,
            conversation_target: None,
        })
        .collect::<Vec<_>>();
    UiSnapshot {
        revision,
        human_state: UiHumanState::Ready,
        inbox_rows: if section == UiSection::Inbox {
            rows.clone()
        } else {
            Vec::new()
        },
        sent_rows: if section == UiSection::Sent {
            rows.clone()
        } else {
            Vec::new()
        },
        archived_rows: if section == UiSection::Archived {
            rows.clone()
        } else {
            Vec::new()
        },
        agent_rows: if section == UiSection::Agents {
            rows.clone()
        } else {
            Vec::new()
        },
        project_rows: if section == UiSection::Projects {
            rows
        } else {
            Vec::new()
        },
        direct_targets: Vec::new(),
        providers: Vec::new(),
        agents: Vec::new(),
        projects: Vec::new(),
    }
}

fn loaded_agents_model(revision: u64, agents: &[UiAgent]) -> UiModel {
    loaded_agents_model_with_providers(
        revision,
        agents.to_owned(),
        vec![available_provider("codex", "Codex", true)],
    )
}

fn loaded_agents_model_with_providers(
    revision: u64,
    agents: Vec<UiAgent>,
    providers: Vec<UiProvider>,
) -> UiModel {
    let mut snapshot = agents_snapshot(revision, agents);
    snapshot.providers = providers;
    let mut model = loaded_model(snapshot);
    for _ in 0..3 {
        model = update(model, UiEvent::Input(UiInput::MoveCursorRight))
            .expect("next cached section")
            .model;
    }
    model
}

fn agents_snapshot(revision: u64, agents: Vec<UiAgent>) -> UiSnapshot {
    let rows = agents
        .iter()
        .map(|agent| UiRow {
            id: agent_row_id(agent.agent_id[0]),
            title: agent.names.first().cloned().unwrap_or_default(),
            detail: "active".to_owned(),
            state: UiRowState::Open,
            kind: UiRowKind::Agent,
            conversation_target: None,
        })
        .collect();
    UiSnapshot {
        revision,
        human_state: UiHumanState::Ready,
        inbox_rows: Vec::new(),
        sent_rows: Vec::new(),
        archived_rows: Vec::new(),
        agent_rows: rows,
        project_rows: Vec::new(),
        direct_targets: Vec::new(),
        providers: vec![available_provider("codex", "Codex", true)],
        agents,
        projects: Vec::new(),
    }
}

fn loaded_projects_model(revision: u64, projects: Vec<UiProject>) -> UiModel {
    loaded_projects_snapshot(projects_snapshot(revision, projects))
}

fn loaded_projects_snapshot(snapshot: UiSnapshot) -> UiModel {
    let mut model = loaded_model(snapshot);
    for _ in 0..4 {
        model = update(model, UiEvent::Input(UiInput::MoveCursorRight))
            .expect("next cached section")
            .model;
    }
    model
}

fn loaded_projects_model_with_agents(
    revision: u64,
    projects: Vec<UiProject>,
    agents: Vec<UiAgent>,
) -> UiModel {
    loaded_projects_model_with_agents_and_providers(
        revision,
        projects,
        agents,
        vec![available_provider("codex", "Codex", true)],
    )
}

fn loaded_projects_model_with_agents_and_providers(
    revision: u64,
    projects: Vec<UiProject>,
    agents: Vec<UiAgent>,
    providers: Vec<UiProvider>,
) -> UiModel {
    let mut snapshot = projects_snapshot(revision, projects);
    let agent_source = agents_snapshot(revision, agents);
    snapshot.agent_rows = agent_source.agent_rows;
    snapshot.agents = agent_source.agents;
    snapshot.providers = providers;
    let mut model = loaded_model(snapshot);
    for _ in 0..4 {
        model = update(model, UiEvent::Input(UiInput::MoveCursorRight))
            .expect("next cached section")
            .model;
    }
    model
}

fn open_project_management(mut model: UiModel) -> UiModel {
    model = update(model, UiEvent::Input(UiInput::MoveCursorRight))
        .expect("open project summary")
        .model;
    for _ in 0..5 {
        if model.project_summary_focus() == Some(UiProjectSummaryFocus::Manage) {
            break;
        }
        model = update(model, UiEvent::Input(UiInput::NextItem))
            .expect("choose summary card")
            .model;
    }
    update(model, UiEvent::Input(UiInput::Activate))
        .expect("open project management")
        .model
}

fn open_project_management_action(model: UiModel, action: UiProjectManagementAction) -> UiModel {
    let mut model = open_project_management(model);
    for _ in 0..8 {
        if model.project_management_action() == Some(action) {
            return update(model, UiEvent::Input(UiInput::Activate))
                .expect("activate labeled project action")
                .model;
        }
        model = update(model, UiEvent::Input(UiInput::NextItem))
            .expect("choose project action")
            .model;
    }
    panic!("project action was not available: {action:?}");
}

fn open_project_folder_action(
    model: UiModel,
    folder_index: usize,
    action: UiProjectFolderAction,
) -> hq_tui::UiTransition {
    let mut model = open_project_management_action(model, UiProjectManagementAction::Folders);
    for _ in 0..folder_index {
        model = update(model, UiEvent::Input(UiInput::NextFocus))
            .expect("choose folder")
            .model;
    }
    for _ in 0..8 {
        if model.project_folder_action() == Some(action) {
            return update(model, UiEvent::Input(UiInput::Activate))
                .expect("activate labeled folder action");
        }
        model = update(model, UiEvent::Input(UiInput::NextItem))
            .expect("choose folder action")
            .model;
    }
    panic!("folder action was not available: {action:?}");
}

fn project_agent(byte: u8, home: [u8; 32]) -> UiAgent {
    UiAgent {
        agent_id: [byte; 32],
        names: vec![format!("agent-{byte}")],
        mailboxes: vec![UiAgentMailbox {
            installation_id: home,
            mailbox_id: [byte.saturating_add(1); 32],
        }],
        lifecycle: UiAgentLifecycle::Active,
        runnable: true,
        status: UiAgentStatus::Unassigned,
        sessions: vec![UiAgentSession {
            provider: "codex".to_owned(),
            session: format!("session-{byte}"),
            mailbox: None,
            conflicted: false,
            selected: true,
            name_resolved: true,
            display_name: None,
        }],
    }
}

fn projects_snapshot(revision: u64, projects: Vec<UiProject>) -> UiSnapshot {
    let rows = projects
        .iter()
        .map(|project| UiRow {
            id: agent_row_id(project.project_id[0]),
            title: project.name.clone(),
            detail: project.lifecycle.clone(),
            state: UiRowState::Open,
            kind: UiRowKind::Project,
            conversation_target: None,
        })
        .collect();
    UiSnapshot {
        revision,
        human_state: UiHumanState::Ready,
        inbox_rows: Vec::new(),
        sent_rows: Vec::new(),
        archived_rows: Vec::new(),
        agent_rows: Vec::new(),
        project_rows: rows,
        direct_targets: Vec::new(),
        providers: vec![available_provider("codex", "Codex", true)],
        agents: Vec::new(),
        projects,
    }
}

fn project_conversation_row(project: u8, thread: u8, root_message: u8, title: &str) -> UiRow {
    UiRow {
        id: format!("project:{}:{}", agent_row_id(project), agent_row_id(thread)),
        title: title.to_owned(),
        detail: "project conversation".to_owned(),
        state: UiRowState::Open,
        kind: UiRowKind::Conversation,
        conversation_target: Some(UiConversationTarget::Project {
            project_id: [project; 32],
            thread_id: [thread; 32],
            root_message: [root_message; 32],
        }),
    }
}

fn assert_project_draft_context(
    model: &UiModel,
    project_id: [u8; 32],
    project_name: &str,
    expected_body: Option<&str>,
) {
    assert_eq!(model.section(), UiSection::Inbox);
    assert_eq!(
        model
            .project_filter()
            .map(|filter| (filter.project_id, filter.project_name.as_str())),
        Some((project_id, project_name))
    );
    assert_eq!(
        model
            .selected_row_data()
            .map(|row| (row.title.as_str(), row.detail.as_str())),
        Some((project_name, "New project conversation"))
    );
    let conversation = model.conversation().expect("local project conversation");
    assert_eq!(conversation.title, project_name);
    assert_eq!(
        conversation.context.as_deref(),
        Some("New project conversation")
    );
    match expected_body {
        None => assert!(conversation.entries.is_empty()),
        Some(expected_body) => assert!(matches!(
            conversation.entries.as_slice(),
            [entry]
                if matches!(
                    &entry.presentation,
                    UiConversationEntryPresentation::Message {
                        author: UiConversationAuthor::You,
                        body,
                    } if body == expected_body
                )
        )),
    }
}

fn available_provider(provider: &str, name: &str, configured_default: bool) -> UiProvider {
    UiProvider {
        provider: provider.to_owned(),
        name: name.to_owned(),
        available: true,
        configured_default,
    }
}

fn project(byte: u8, name: &str, path: &str) -> UiProject {
    UiProject {
        project_id: [byte; 32],
        home: [9; 32],
        name: name.to_owned(),
        lifecycle: "open".to_owned(),
        archived: false,
        claimable: true,
        assignment: None,
        threads: Vec::new(),
        pending_inputs: Vec::new(),
        head: [byte.saturating_add(1); 32],
        input_sequence: 1,
        resources: vec![UiProjectResource {
            resource_id: [byte.saturating_add(2); 32],
            display_path: path.to_owned(),
            canonical_path: path.to_owned(),
            health: "clean".to_owned(),
            primary: true,
            active_claim: true,
            conflicting_projects: Vec::new(),
        }],
    }
}

fn project_with_second_resource() -> UiProject {
    let mut target = project(30, "resources", "/first");
    target.resources.push(UiProjectResource {
        resource_id: [33; 32],
        display_path: "/second".to_owned(),
        canonical_path: "/second".to_owned(),
        health: "unknown".to_owned(),
        primary: false,
        active_claim: true,
        conflicting_projects: Vec::new(),
    });
    target
}

fn project_effect(effects: &[UiEffect]) -> (hq_tui::EffectId, UiProjectAction) {
    effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::SubmitProjectCommand { id, action } => Some((*id, action.clone())),
            _ => None,
        })
        .expect("project effect")
}

fn agent(byte: u8, name: &str) -> UiAgent {
    UiAgent {
        agent_id: [byte; 32],
        names: vec![name.to_owned()],
        mailboxes: vec![UiAgentMailbox {
            installation_id: [9; 32],
            mailbox_id: [byte; 32],
        }],
        lifecycle: UiAgentLifecycle::Active,
        runnable: true,
        status: UiAgentStatus::Unassigned,
        sessions: vec![UiAgentSession {
            provider: "codex".to_owned(),
            session: format!("session-{byte}"),
            mailbox: None,
            conflicted: false,
            selected: true,
            name_resolved: true,
            display_name: Some("x".to_owned()),
        }],
    }
}

fn agent_row_id(byte: u8) -> String {
    format!("{byte:02x}").repeat(32)
}

fn started_model() -> hq_tui::UiTransition {
    update(
        UiModel::new(UiSize {
            width: 90,
            height: 24,
        }),
        UiEvent::Started,
    )
    .expect("startup transition")
}

fn loaded_model(snapshot: UiSnapshot) -> UiModel {
    loaded_transition(snapshot).model
}

fn loaded_transition(snapshot: UiSnapshot) -> hq_tui::UiTransition {
    let started = started_model();
    let id = snapshot_effect(&started.effects);
    update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: id,
            snapshot,
        },
    )
    .expect("snapshot loaded")
}

fn opened_conversation(entries: Vec<UiConversationEntry>) -> UiModel {
    let observed = materialized_transition(
        snapshot(1, &["thread-a"]),
        UiConversationPage {
            title: "Alice".to_owned(),
            context: None,
            row_id: "thread-a".to_owned(),
            entries,
            next_cursor: None,
        },
    );
    update(observed.model, UiEvent::Input(UiInput::Activate))
        .expect("open conversation")
        .model
}

fn observe_conversation_viewport(
    model: UiModel,
    entries: &[(&str, u16)],
    height: u16,
) -> hq_tui::UiTransition {
    update(
        model,
        UiEvent::ConversationViewportObserved {
            observation: UiConversationViewportObservation {
                conversation_id: "thread-a".to_owned(),
                width: 60,
                height,
                entries: entries
                    .iter()
                    .map(|(entry_id, height)| UiConversationEntryGeometry {
                        entry_id: (*entry_id).to_owned(),
                        height: *height,
                    })
                    .collect(),
            },
        },
    )
    .expect("conversation viewport observation applies")
}

fn materialized_transition(snapshot: UiSnapshot, page: UiConversationPage) -> hq_tui::UiTransition {
    let started = started_model();
    update(
        started.model,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot,
                conversation: Some(page),
            },
        },
    )
    .expect("materialized conversation")
}

fn snapshot_effect(effects: &[UiEffect]) -> hq_tui::EffectId {
    effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadSnapshot { id } => Some(*id),
            _ => None,
        })
        .expect("snapshot effect")
}

fn redraw_count(effects: &[UiEffect]) -> usize {
    effects
        .iter()
        .filter(|effect| matches!(effect, UiEffect::RequestRedraw))
        .count()
}

fn timer_effect(effects: &[UiEffect], expected: UiTimerKind) -> hq_tui::EffectId {
    effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::ScheduleTimer { id, kind, .. } if *kind == expected => Some(*id),
            _ => None,
        })
        .expect("timer effect")
}

fn conversation_effect(effects: &[UiEffect]) -> (hq_tui::EffectId, &str, Option<&str>) {
    effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadConversation { id, row_id, cursor } => {
                Some((*id, row_id.as_str(), cursor.as_deref()))
            }
            _ => None,
        })
        .expect("conversation effect")
}

fn open_draft_effect(effects: &[UiEffect]) -> (hq_tui::EffectId, &UiMailboxDraftTarget) {
    effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::OpenDraft { id, target } => Some((*id, target)),
            _ => None,
        })
        .expect("open draft effect")
}

fn save_draft_effect(effects: &[UiEffect]) -> (hq_tui::EffectId, &UiMailboxDraft) {
    effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::SaveDraft { id, draft } => Some((*id, draft)),
            _ => None,
        })
        .expect("save draft effect")
}

fn agent_action_effect(effects: &[UiEffect]) -> (hq_tui::EffectId, &UiAgentAction) {
    effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::SubmitAgentCommand { id, action } => Some((*id, action)),
            _ => None,
        })
        .expect("agent command effect")
}

fn managed_session_effect(effects: &[UiEffect]) -> (hq_tui::EffectId, &UiManagedSessionAction) {
    effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::SubmitManagedSession { id, action } => Some((*id, action)),
            _ => None,
        })
        .expect("managed-session effect")
}

fn entry(id: &str, activity: bool) -> UiConversationEntry {
    UiConversationEntry {
        id: id.to_owned(),
        presentation: if activity {
            UiConversationEntryPresentation::Activity {
                kind: UiConversationActivityKind::Progress,
                status: UiActivityStatus::Running,
                summary: format!("{id} summary"),
                detail: format!("{id} content"),
                truncated: false,
                completed: None,
            }
        } else {
            UiConversationEntryPresentation::Message {
                author: UiConversationAuthor::You,
                body: format!("{id} content"),
            }
        },
        message_state: (!activity).then_some(UiMessageState::Open),
        delivery: None,
        message_target: None,
        technical: if activity {
            vec![UiTechnicalSection::Activity {
                sequence: 2,
                source_installation: "installation".to_owned(),
                source_mailbox: "mailbox".to_owned(),
                provider: "provider".to_owned(),
                session: "session".to_owned(),
                operation: "operation".to_owned(),
                item: Some("item".to_owned()),
                logical_key: "progress".to_owned(),
                runtime: "runtime".to_owned(),
                occurred_at_unix_ms: 2,
                status: UiActivityStatus::Running,
                truncated: false,
            }]
        } else {
            Vec::new()
        },
    }
}

fn actionable_entry(id: &str, message_id: [u8; 32]) -> UiConversationEntry {
    UiConversationEntry {
        message_target: Some(UiMessageTarget {
            message_id,
            reply_allowed: true,
        }),
        ..entry(id, false)
    }
}

fn agent_turn_entry(id: &str, status: UiActivityStatus) -> UiConversationEntry {
    let mut value = entry(id, true);
    value.presentation = UiConversationEntryPresentation::Activity {
        kind: UiConversationActivityKind::AgentTurn,
        status,
        summary: format!("{id} summary"),
        detail: format!("{id} detail"),
        truncated: false,
        completed: None,
    };
    value
}

fn direct_target(label: &str, byte: u8) -> UiDirectTarget {
    UiDirectTarget {
        installation_id: [byte; 32],
        mailbox_id: [byte + 10; 32],
        label: label.to_owned(),
    }
}
