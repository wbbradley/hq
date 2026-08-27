//! Exact coverage tests for the normative semantic fact catalog.

use std::collections::BTreeSet;

use hq_domain::{FactKind, ProtocolClass, RetentionClass};

const EXPECTED_FACTS: [(&str, &str); 48] = [
    ("FCT-001", "InstallationDeclared"),
    ("FCT-002", "MailboxCreated"),
    ("FCT-003", "MailboxSessionBound"),
    ("FCT-004", "MailboxContextRecorded"),
    ("FCT-005", "PeerRouteSet"),
    ("FCT-006", "PeerRouteBlocked"),
    ("FCT-007", "MailboxAccessGranted"),
    ("FCT-008", "MailboxAccessRevoked"),
    ("FCT-009", "MailboxActionObserved"),
    ("FCT-010", "HumanAccountCreated"),
    ("FCT-011", "HumanAccountSelected"),
    ("FCT-012", "HumanDeviceGranted"),
    ("FCT-013", "HumanDeviceAccepted"),
    ("FCT-014", "HumanDeviceRevoked"),
    ("FCT-015", "QuestionAsked"),
    ("FCT-016", "AsynchronousMessageSent"),
    ("FCT-017", "AnswerGiven"),
    ("FCT-018", "ThreadCancelled"),
    ("FCT-019", "MessageArchived"),
    ("FCT-020", "MessageRestored"),
    ("FCT-021", "MessageRejected"),
    ("FCT-022", "HarnessActivityRecorded"),
    ("FCT-023", "AgentNameClaimed"),
    ("FCT-024", "AgentRetired"),
    ("FCT-025", "ProviderSessionSelected"),
    ("FCT-026", "ProviderSessionRenamed"),
    ("FCT-027", "ProjectCreated"),
    ("FCT-028", "ProjectOpened"),
    ("FCT-029", "ProjectClosingStarted"),
    ("FCT-030", "ProjectClosed"),
    ("FCT-031", "ProjectArchived"),
    ("FCT-032", "ProjectUnarchived"),
    ("FCT-033", "ProjectMetadataUpdated"),
    ("FCT-034", "ProjectResourceAdded"),
    ("FCT-035", "ProjectResourceRemoved"),
    ("FCT-036", "ProjectResourceReplaced"),
    ("FCT-037", "ProjectPrimaryResourceChanged"),
    ("FCT-038", "ProjectResourceHealthObserved"),
    ("FCT-039", "ProjectAssignmentConfiguring"),
    ("FCT-040", "ProjectAssignmentRunnable"),
    ("FCT-041", "ProjectAssignmentBlocked"),
    ("FCT-042", "ProjectAssignmentEnded"),
    ("FCT-043", "ProjectInputAccepted"),
    ("FCT-044", "ProjectInputDispatched"),
    ("FCT-045", "ProjectOutputRecorded"),
    ("FCT-046", "RemoteProjectCommandRequested"),
    ("FCT-047", "RemoteProjectCommandReceipt"),
    ("FCT-048", "RemoteProjectCommandOutcome"),
];

#[test]
fn code_catalog_matches_every_normative_family_exactly() {
    let actual = FactKind::ALL
        .iter()
        .map(|kind| (kind.catalog_id(), kind.name()))
        .collect::<Vec<_>>();

    assert_eq!(actual, EXPECTED_FACTS);
    assert_eq!(
        actual.iter().copied().collect::<BTreeSet<_>>().len(),
        EXPECTED_FACTS.len()
    );
}

#[test]
fn markdown_catalog_and_code_catalog_are_bidirectionally_equal() {
    let markdown = include_str!("../../../docs/rust/semantic-fact-catalog.md");
    let documented = markdown
        .lines()
        .filter(|line| line.starts_with("| FCT-"))
        .map(|line| {
            let mut columns = line.split('|').map(str::trim);
            let _ = columns.next();
            (
                columns.next().unwrap_or_default(),
                columns.next().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(documented, EXPECTED_FACTS);
}

#[test]
fn protocol_and_retention_classes_preserve_remote_control_isolation() {
    for kind in FactKind::ALL {
        if kind.catalog_id() >= "FCT-046" {
            assert_eq!(kind.protocol_class(), ProtocolClass::RemoteControl);
            assert_eq!(kind.retention_class(), RetentionClass::ControlPermanent);
        } else {
            assert_eq!(kind.protocol_class(), ProtocolClass::Canonical);
            if kind == FactKind::HarnessActivityRecorded {
                assert_eq!(
                    kind.retention_class(),
                    RetentionClass::CanonicalCompactedView
                );
            } else {
                assert_eq!(kind.retention_class(), RetentionClass::CanonicalPermanent);
            }
        }
    }
}
