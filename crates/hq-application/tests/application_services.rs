//! Transport-independent application service contracts.

#![allow(clippy::expect_used)]

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    error::Error,
};

use hq_application::{
    AgentSessionRequest, AgentSessionResult, Application, ApplicationError, ApplicationErrorClass,
    ApplicationErrorCode, ApplicationPorts, AuthoritativeSnapshot, CommitFacts, ConfigureRelays,
    ControlMailbox, DomainSnapshot, EffectOutcome, EffectRequest, FactMutation, FactPlan,
    InspectResource, MutationAttempt, MutationDecision, MutationOutcome, MutationReceipt,
    ObserveRevisions, PreparedSubscription, ProjectCommandOutcome, ProjectCommandRequest,
    PublishWake, QueryDomain, RelayAccess, RelayAuthentication, RelayConfiguration,
    ResourceInspectionRequest, ResourceInspectionResult, SessionControl, SubscriptionRequest,
    SubscriptionTopic, SynchronizationRequest, WakeDisposition,
};
use hq_domain::{
    BoundedSet, BoundedText, CausalReferences, CommandDigest, CommandId, DomainError,
    EncryptionPublicKey, ErrorCategory, ErrorCode, FactScope, InstallationId, MAX_FACT_AUTHORITIES,
    MAX_FACT_PARENTS, OperationId, Page, PageCursor, ResourceLocator, ResourceScheme, Revision,
    SemanticPayload, SigningPublicKey, Timestamp,
};
use hq_reducer::ConversationKey;

#[derive(Default)]
struct ScriptedPorts {
    trace: RefCell<Vec<&'static str>>,
    decisions: Cell<usize>,
    receipts: RefCell<BTreeMap<CommandId, (CommandDigest, MutationReceipt)>>,
    wake_error: Cell<bool>,
    query_error: Cell<bool>,
}

impl QueryDomain for ScriptedPorts {
    fn authoritative_snapshot(&self) -> Result<AuthoritativeSnapshot, ApplicationError> {
        self.trace.borrow_mut().push("query");
        if self.query_error.get() {
            return Err(ApplicationError::new(
                ApplicationErrorCode::AdapterUnavailable,
            ));
        }
        Ok(AuthoritativeSnapshot::new(
            Revision::new(7),
            DomainSnapshot::empty(),
        ))
    }

    fn conversation_entries(
        &self,
        _key: &ConversationKey,
        _limit: usize,
        _cursor: Option<&PageCursor>,
    ) -> Result<Page<hq_application::ConversationEntry>, ApplicationError> {
        Ok(Page::new(Vec::new(), None))
    }

    fn state_health(&self) -> Result<hq_application::StateHealth, ApplicationError> {
        Ok(scripted_health())
    }

    fn repair_state(
        &self,
        operation_id: OperationId,
    ) -> Result<hq_application::StateRepairReport, ApplicationError> {
        let health = scripted_health();
        Ok(hq_application::StateRepairReport {
            operation_id,
            revision: health.revision,
            domains: health.domains,
        })
    }
}

fn scripted_health() -> hq_application::StateHealth {
    hq_application::StateHealth {
        revision: Revision::new(7),
        domains: [
            hq_application::HealthDomain::Authority,
            hq_application::HealthDomain::Conversation,
            hq_application::HealthDomain::Agent,
            hq_application::HealthDomain::Project,
        ]
        .into_iter()
        .map(|domain| hq_application::DomainHealth {
            domain,
            projected: 0,
            unresolved: 0,
            unauthorized: 0,
            conflicted: 0,
            invalid: 0,
            unsupported: 0,
            conflicts: 0,
        })
        .collect(),
    }
}

impl CommitFacts for ScriptedPorts {
    fn commit_facts(&self, request: FactMutation) -> Result<MutationAttempt, ApplicationError> {
        let (command_id, request_digest, decide) = request.into_parts();
        if let Some((retained_digest, receipt)) = self.receipts.borrow().get(&command_id) {
            if *retained_digest != request_digest {
                return Err(ApplicationError::new(
                    ApplicationErrorCode::CommandIdentityConflict,
                ));
            }
            return Ok(MutationAttempt::Completed(receipt.clone()));
        }

        self.decisions.set(self.decisions.get() + 1);
        let outcome = match decide(&DomainSnapshot::empty()) {
            MutationDecision::Commit(_) => MutationOutcome::Committed,
            MutationDecision::Reject(error) => MutationOutcome::Rejected(error),
        };
        let receipt = MutationReceipt::new(command_id, request_digest, Revision::new(8), outcome);
        self.receipts
            .borrow_mut()
            .insert(command_id, (request_digest, receipt.clone()));
        Ok(MutationAttempt::Completed(receipt))
    }
}

impl PublishWake for ScriptedPorts {
    fn publish_wake(&self, _revision: Revision) -> Result<WakeDisposition, ApplicationError> {
        self.trace.borrow_mut().push("wake");
        if self.wake_error.get() {
            Err(ApplicationError::new(
                ApplicationErrorCode::AdapterUnavailable,
            ))
        } else {
            Ok(WakeDisposition::Scheduled)
        }
    }
}

impl ConfigureRelays for ScriptedPorts {
    fn configure_relay(
        &self,
        request: &EffectRequest<RelayConfiguration>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        Ok(EffectOutcome::Uncertain(request.operation_id))
    }

    fn synchronize(
        &self,
        _request: &EffectRequest<SynchronizationRequest>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        Ok(EffectOutcome::Rejected(DomainError::new(
            ErrorCategory::Unresolved,
            ErrorCode::new("relay_not_ready").expect("fixture code is valid"),
        )))
    }

    fn relay_status(&self) -> Result<hq_application::RelayStatus, ApplicationError> {
        Ok(hq_application::RelayStatus {
            policies: Vec::new(),
            queued: 0,
            prepared: 0,
            uncertain: 0,
            rejected: 0,
            accepted: 0,
            staged: 0,
            quarantined: 0,
            truncated: false,
        })
    }
}

impl hq_application::ControlHarness for ScriptedPorts {
    fn control_harness(
        &self,
        request: &EffectRequest<AgentSessionRequest>,
    ) -> Result<EffectOutcome<AgentSessionResult>, ApplicationError> {
        match &request.body.control {
            SessionControl::Stop => Ok(EffectOutcome::Accepted(AgentSessionResult::Stopped)),
            SessionControl::Start | SessionControl::Resume { .. } => {
                Ok(EffectOutcome::Uncertain(request.operation_id))
            }
        }
    }
}

impl InspectResource for ScriptedPorts {
    fn inspect_resource(
        &self,
        request: &EffectRequest<ResourceInspectionRequest>,
    ) -> Result<EffectOutcome<ResourceInspectionResult>, ApplicationError> {
        Ok(EffectOutcome::Uncertain(request.operation_id))
    }
}

impl hq_application::ControlProjects for ScriptedPorts {
    fn control_project(
        &self,
        _request: ProjectCommandRequest,
    ) -> Result<ProjectCommandOutcome, ApplicationError> {
        Err(ApplicationError::new(
            ApplicationErrorCode::AdapterUnavailable,
        ))
    }
}

impl hq_application::RetireAgents for ScriptedPorts {
    fn retire_agent(
        &self,
        _request: hq_application::AgentRetirementRequest,
    ) -> Result<hq_application::AgentRetirementOutcome, ApplicationError> {
        Err(ApplicationError::new(
            ApplicationErrorCode::AdapterUnavailable,
        ))
    }
}

impl ObserveRevisions for ScriptedPorts {
    fn register_subscription(
        &self,
        _request: &SubscriptionRequest,
    ) -> Result<(), ApplicationError> {
        self.trace.borrow_mut().push("register");
        Ok(())
    }

    fn activate_subscription(&self, _operation_id: OperationId) -> Result<(), ApplicationError> {
        self.trace.borrow_mut().push("activate");
        Ok(())
    }

    fn cancel_subscription(&self, _operation_id: OperationId) -> Result<(), ApplicationError> {
        self.trace.borrow_mut().push("cancel");
        Ok(())
    }
}

impl ApplicationPorts for ScriptedPorts {}
impl ControlMailbox for ScriptedPorts {}

#[test]
fn mutation_replay_is_exact_and_changed_digest_conflicts() -> Result<(), Box<dyn Error>> {
    let ports = ScriptedPorts::default();
    ports.wake_error.set(true);
    let application = Application::new(ports);
    let command_id = CommandId::from_bytes([1; 32]);
    let digest = CommandDigest::from_bytes([2; 32]);
    let rejection = DomainError::new(
        ErrorCategory::Unauthorized,
        ErrorCode::new("authority_missing")?,
    );

    let first = application.execute_mutation(FactMutation::new(command_id, digest, move |_| {
        MutationDecision::reject(rejection)
    }))?;
    let second = application.execute_mutation(FactMutation::new(command_id, digest, |_| {
        MutationDecision::reject(DomainError::new(
            ErrorCategory::InvariantViolation,
            ErrorCode::new("must_not_decide").expect("fixture code is valid"),
        ))
    }))?;

    assert_eq!(first.attempt(), second.attempt());
    assert_eq!(application.ports().decisions.get(), 1);
    assert!(first.wake().is_none(), "rejections schedule no relay work");

    let conflict = application
        .execute_mutation(FactMutation::new(
            command_id,
            CommandDigest::from_bytes([3; 32]),
            |_| {
                MutationDecision::reject(DomainError::new(
                    ErrorCategory::InvariantViolation,
                    ErrorCode::new("must_not_decide").expect("fixture code is valid"),
                ))
            },
        ))
        .expect_err("changed digest conflicts");
    assert_eq!(conflict.class(), ApplicationErrorClass::Conflict);
    Ok(())
}

#[test]
fn committed_mutation_remains_committed_when_post_commit_wake_fails() -> Result<(), Box<dyn Error>>
{
    let ports = ScriptedPorts::default();
    ports.wake_error.set(true);
    let application = Application::new(ports);
    let causal =
        CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(BoundedSet::new([])?, [])?;
    let plan = FactPlan::new(
        InstallationId::from_bytes([4; 32]),
        Timestamp::from_unix_millis(5),
        FactScope::InstallationPrivate(InstallationId::from_bytes([4; 32])),
        causal,
        SemanticPayload::InstallationDeclared {
            installation_id: InstallationId::from_bytes([4; 32]),
            signing_key: SigningPublicKey::from_bytes([5; 32]),
            encryption_key: EncryptionPublicKey::from_bytes([6; 32]),
            label: None,
        },
        [7; 32],
    );

    let completion = application.execute_mutation(FactMutation::new(
        CommandId::from_bytes([4; 32]),
        CommandDigest::from_bytes([5; 32]),
        move |_| MutationDecision::commit(plan),
    ))?;

    let MutationAttempt::Completed(receipt) = completion.attempt() else {
        return Err("scripted commit should complete".into());
    };
    assert_eq!(receipt.outcome(), &MutationOutcome::Committed);
    assert!(
        matches!(completion.wake(), Some(Err(error)) if error.class() == ApplicationErrorClass::Unavailable)
    );
    Ok(())
}

#[test]
fn external_use_cases_preserve_stable_uncertain_and_accepted_outcomes() -> Result<(), Box<dyn Error>>
{
    let application = Application::new(ScriptedPorts::default());
    let endpoint = ResourceLocator::new(
        ResourceScheme::Opaque,
        BoundedText::new("wss://relay.example")?,
    );
    let operation_id = OperationId::from_bytes([0x31; 32]);
    let relay = EffectRequest::new(
        operation_id,
        CommandDigest::from_bytes([0x32; 32]),
        Timestamp::from_unix_millis(33),
        RelayConfiguration::new(
            endpoint,
            RelayAccess::ReadWrite,
            RelayAuthentication::Required,
            true,
        ),
    );

    assert_eq!(
        application.configure_relay(&relay)?,
        EffectOutcome::Uncertain(operation_id)
    );

    let synchronization = EffectRequest::new(
        OperationId::from_bytes([0x35; 32]),
        CommandDigest::from_bytes([0x36; 32]),
        Timestamp::from_unix_millis(37),
        SynchronizationRequest::All,
    );
    assert!(matches!(
        application.synchronize(&synchronization)?,
        EffectOutcome::Rejected(error) if error.category() == ErrorCategory::Unresolved
    ));
    assert!(application.relay_status()?.policies.is_empty());
    assert_eq!(application.state_health()?.domains.len(), 4);
    let repair_operation = OperationId::from_bytes([0x39; 32]);
    assert_eq!(
        application.repair_state(repair_operation)?.operation_id,
        repair_operation
    );

    let stop = EffectRequest::new(
        OperationId::from_bytes([0x41; 32]),
        CommandDigest::from_bytes([0x42; 32]),
        Timestamp::from_unix_millis(43),
        AgentSessionRequest::new(
            hq_domain::AgentId::from_bytes([0x44; 32]),
            hq_domain::ProviderId::new("scripted")?,
            SessionControl::Stop,
            None,
        ),
    );
    assert_eq!(
        application.control_agent_session(&stop)?,
        EffectOutcome::Accepted(AgentSessionResult::Stopped)
    );
    Ok(())
}

#[test]
fn subscription_registration_precedes_snapshot_and_activation_is_explicit()
-> Result<(), Box<dyn Error>> {
    let application = Application::new(ScriptedPorts::default());
    let request =
        SubscriptionRequest::new(OperationId::from_bytes([9; 32]), [SubscriptionTopic::All])?;

    let prepared: PreparedSubscription = application.prepare_subscription(request)?;

    assert_eq!(&*application.ports().trace.borrow(), &["register", "query"]);
    assert_eq!(prepared.snapshot().revision(), Revision::new(7));
    application.activate_subscription(prepared.operation_id())?;
    assert_eq!(
        &*application.ports().trace.borrow(),
        &["register", "query", "activate"]
    );
    Ok(())
}

#[test]
fn failed_snapshot_cancels_pending_subscription() -> Result<(), Box<dyn Error>> {
    let ports = ScriptedPorts::default();
    ports.query_error.set(true);
    let application = Application::new(ports);
    let request = SubscriptionRequest::new(
        OperationId::from_bytes([8; 32]),
        [SubscriptionTopic::Conversation],
    )?;

    let error = application
        .prepare_subscription(request)
        .expect_err("query failure propagates");

    assert_eq!(error.class(), ApplicationErrorClass::Unavailable);
    assert_eq!(
        &*application.ports().trace.borrow(),
        &["register", "query", "cancel"]
    );
    Ok(())
}
