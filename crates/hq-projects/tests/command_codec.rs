//! Strict remote project-command body codec contract.

#![allow(clippy::expect_used)]

use hq_application::ProjectCommandAction;
use hq_domain::{AgentId, BoundedText, ProviderId, ResourceLocator, ResourceScheme, ThreadId};
use hq_projects::{decode_project_command_action, encode_project_command_action};

fn locator(path: &str) -> ResourceLocator {
    ResourceLocator::new(
        ResourceScheme::WorkingTree,
        BoundedText::new(path).expect("path validates"),
    )
}

#[test]
fn action_body_round_trips_canonically_without_behavioral_text_parsing() {
    let action = ProjectCommandAction::Handoff {
        agent_id: AgentId::from_bytes([1; 32]),
        provider: ProviderId::new("provider").expect("provider validates"),
        resume_session: None,
        thread_id: ThreadId::from_bytes([2; 32]),
        launch_directory: locator("/repo/worktree"),
        force_takeover: true,
    };

    let encoded = encode_project_command_action(&action).expect("action encodes");
    assert_eq!(
        decode_project_command_action(&encoded).expect("action decodes"),
        action
    );
    assert_eq!(
        encode_project_command_action(
            &decode_project_command_action(&encoded).expect("canonical action decodes")
        )
        .expect("canonical action re-encodes"),
        encoded
    );
}

#[test]
fn unknown_version_and_noncanonical_json_are_rejected() {
    let unknown =
        hq_domain::ContentText::new("hq-project-command-v2:{}").expect("bounded unknown body");
    assert!(decode_project_command_action(&unknown).is_err());

    let noncanonical = hq_domain::ContentText::new(
        "hq-project-command-v1:{\"action\":\"close\",\"force\":false }",
    )
    .expect("bounded noncanonical body");
    assert!(decode_project_command_action(&noncanonical).is_err());
}
