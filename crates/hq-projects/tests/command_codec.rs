//! Strict remote project-command body codec contract.

#![allow(clippy::expect_used)]

use std::num::NonZeroU64;

use hq_application::ProjectCommandAction;
use hq_domain::{
    AccountId, AgentId, AssignmentBinding, AssignmentId, BoundedText, CommandDigest, CommandId,
    ContentText, DispatchId, ErrorCode, FactId, InstallationId, MessageId, ProjectId,
    ProjectResource, ProviderId, ProviderSessionId, ResourceHealth, ResourceId, ResourceLocator,
    ResourceScheme, RuntimeObservation, ThreadId, Timestamp,
};
use hq_projects::{
    CanonicalProjectMutation, CanonicalProjectMutationAction, PendingProjectInput,
    decode_canonical_project_mutation, decode_project_command_action,
    encode_canonical_project_mutation, encode_project_command_action,
};

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

#[test]
fn in_flight_canonical_mutation_round_trips_with_complete_dispatch_attribution() {
    let mutation = CanonicalProjectMutation {
        command_id: CommandId::from_bytes([10; 32]),
        request_digest: CommandDigest::from_bytes([11; 32]),
        account_id: AccountId::from_bytes([12; 32]),
        project_id: ProjectId::from_bytes([13; 32]),
        home: InstallationId::from_bytes([14; 32]),
        expected_head: FactId::from_bytes([15; 32]),
        issued_at: Timestamp::from_unix_millis(16),
        action: CanonicalProjectMutationAction::RecordDispatch {
            input: PendingProjectInput {
                message_id: MessageId::from_bytes([17; 32]),
                input_fact_id: FactId::from_bytes([18; 32]),
                accepted_fact: FactId::from_bytes([19; 32]),
                sequence: NonZeroU64::new(20).expect("nonzero"),
                thread_id: ThreadId::from_bytes([21; 32]),
                body: ContentText::new("exact body").expect("body"),
            },
            dispatch_id: DispatchId::from_bytes([22; 32]),
            binding: AssignmentBinding {
                assignment_id: AssignmentId::from_bytes([23; 32]),
                agent_id: AgentId::from_bytes([24; 32]),
                provider: ProviderId::new("provider").expect("provider"),
                session: ProviderSessionId::new("session").expect("session"),
            },
            thread_id: ThreadId::from_bytes([25; 32]),
        },
    };

    let encoded = encode_canonical_project_mutation(&mutation).expect("mutation encodes");
    assert_eq!(
        decode_canonical_project_mutation(&encoded).expect("mutation decodes"),
        mutation
    );
    let mut changed = encoded;
    changed.push(b' ');
    assert!(decode_canonical_project_mutation(&changed).is_err());
}

#[test]
fn in_flight_resource_mutations_round_trip_exactly() {
    let resource = ProjectResource {
        resource_id: ResourceId::from_bytes([31; 32]),
        display_locator: locator("/human/repo"),
        canonical_locator: locator("/canonical/repo"),
        health: ResourceHealth::Degraded,
    };
    for action in [
        CanonicalProjectMutationAction::AddResource {
            resource: resource.clone(),
            make_primary: true,
        },
        CanonicalProjectMutationAction::RemoveResource {
            resource_id: resource.resource_id,
            force: true,
        },
        CanonicalProjectMutationAction::ReplaceResource {
            old_resource_id: ResourceId::from_bytes([32; 32]),
            new_resource: resource.clone(),
        },
    ] {
        let mutation = CanonicalProjectMutation {
            command_id: CommandId::from_bytes([33; 32]),
            request_digest: CommandDigest::from_bytes([34; 32]),
            account_id: AccountId::from_bytes([35; 32]),
            project_id: ProjectId::from_bytes([36; 32]),
            home: InstallationId::from_bytes([37; 32]),
            expected_head: FactId::from_bytes([38; 32]),
            issued_at: Timestamp::from_unix_millis(39),
            action,
        };
        let encoded = encode_canonical_project_mutation(&mutation).expect("mutation encodes");
        assert_eq!(
            decode_canonical_project_mutation(&encoded).expect("mutation decodes"),
            mutation
        );
    }
}

#[test]
fn in_flight_close_and_archive_mutations_round_trip_exactly() {
    let failure = ErrorCode::new("runtime-stop-failed").expect("error code");
    let uncertain = ErrorCode::new("runtime-stop-unknown").expect("error code");
    for action in [
        CanonicalProjectMutationAction::BlockAssignment {
            assignment_id: AssignmentId::from_bytes([40; 32]),
            cause: ErrorCode::new("runtime-stop-failed").expect("error code"),
        },
        CanonicalProjectMutationAction::EndAssignment {
            assignment_id: AssignmentId::from_bytes([41; 32]),
            forced: true,
            runtime: Some(RuntimeObservation::Failed(failure)),
        },
        CanonicalProjectMutationAction::FinishClosing {
            forced: true,
            runtime: Some(RuntimeObservation::Uncertain(uncertain)),
        },
        CanonicalProjectMutationAction::Archive,
        CanonicalProjectMutationAction::Unarchive,
        CanonicalProjectMutationAction::RetireAgent {
            agent_id: AgentId::from_bytes([49; 32]),
        },
    ] {
        let mutation = CanonicalProjectMutation {
            command_id: CommandId::from_bytes([42; 32]),
            request_digest: CommandDigest::from_bytes([43; 32]),
            account_id: AccountId::from_bytes([44; 32]),
            project_id: ProjectId::from_bytes([45; 32]),
            home: InstallationId::from_bytes([46; 32]),
            expected_head: FactId::from_bytes([47; 32]),
            issued_at: Timestamp::from_unix_millis(48),
            action,
        };
        let encoded = encode_canonical_project_mutation(&mutation).expect("mutation encodes");
        assert_eq!(
            decode_canonical_project_mutation(&encoded).expect("mutation decodes"),
            mutation
        );
    }
}
