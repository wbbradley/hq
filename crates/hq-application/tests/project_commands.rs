//! Project command contract tests.

use hq_application::{
    ProjectCommandAction, ProjectCommandOutcome, ProjectCommandRequest, ProjectCommandStage,
    ProjectCreationRequest,
};
use hq_domain::{
    AccountId, AgentId, BoundedText, CommandDigest, CommandId, FactId, InstallationId, MailboxId,
    OperationId, ProjectId, ProviderId, ResourceId, ResourceLocator, ResourceScheme, ShortText,
    Timestamp,
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
        expected_head: Some(FactId::from_bytes(id(7))),
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

    let creation = ProjectCreationRequest {
        mailbox_id: MailboxId::from_bytes(id(10)),
        project_name: ShortText::new("existing")?,
        brief: None,
        resource_id: ResourceId::from_bytes(id(11)),
        resource: ResourceLocator::new(
            ResourceScheme::WorkingTree,
            BoundedText::new("/repo/existing")?,
        ),
    };
    assert_eq!(creation.project_name.as_str(), "existing");
    assert_eq!(creation.resource.value(), "/repo/existing");

    let resource_id = ResourceId::from_bytes(id(12));
    let desired = ResourceLocator::new(
        ResourceScheme::WorkingTree,
        BoundedText::new("/repo/desired")?,
    );
    let add = ProjectCommandAction::AddResource {
        resource_id,
        resource: desired.clone(),
        make_primary: true,
    };
    assert!(matches!(
        add,
        ProjectCommandAction::AddResource {
            resource_id: candidate,
            resource,
            make_primary: true,
        } if candidate == resource_id && resource == desired
    ));
    assert_eq!(
        ProjectCommandAction::SetPrimaryResource { resource_id },
        ProjectCommandAction::SetPrimaryResource { resource_id },
    );
    Ok(())
}
