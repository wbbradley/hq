//! Project command contract tests.

use hq_application::{
    ProjectCommandAction, ProjectCommandOutcome, ProjectCommandRequest, ProjectCommandStage,
};
use hq_domain::{
    AccountId, AgentId, BoundedText, CommandDigest, CommandId, FactId, InstallationId, OperationId,
    ProjectId, ProviderId, ResourceLocator, ResourceScheme, Timestamp,
};

fn id(byte: u8) -> [u8; 32] {
    [byte; 32]
}

#[test]
fn project_command_values_are_plain_public_data() -> Result<(), Box<dyn std::error::Error>> {
    let request = ProjectCommandRequest {
        command_id: CommandId::from_bytes(id(1)),
        operation_id: OperationId::from_bytes(id(2)),
        request_digest: CommandDigest::from_bytes(id(3)),
        account_id: AccountId::from_bytes(id(4)),
        project_id: ProjectId::from_bytes(id(5)),
        home: InstallationId::from_bytes(id(6)),
        expected_head: FactId::from_bytes(id(7)),
        issued_at: Timestamp::from_unix_millis(8),
        action: ProjectCommandAction::Activate {
            agent_id: AgentId::from_bytes(id(9)),
            provider: ProviderId::new("provider")?,
            resume_session: None,
            resume_thread: None,
            launch_directory: ResourceLocator::new(
                ResourceScheme::WorkingTree,
                BoundedText::new("/repo")?,
            ),
        },
    };

    assert_eq!(request.project_id, ProjectId::from_bytes(id(5)));
    assert!(matches!(
        request.action,
        ProjectCommandAction::Activate {
            resume_session: None,
            ..
        }
    ));

    let outcome = ProjectCommandOutcome::Running {
        operation_id: request.operation_id,
        stage: ProjectCommandStage::ValidatingResources,
    };
    assert!(matches!(
        outcome,
        ProjectCommandOutcome::Running {
            stage: ProjectCommandStage::ValidatingResources,
            ..
        }
    ));
    Ok(())
}
