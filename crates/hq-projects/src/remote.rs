//! Durable control-plane routing around the home-authoritative project workflow.

use hq_application::{
    AgentRetirementOutcome, AgentRetirementRequest, ApplicationError, ApplicationErrorCode,
    ControlProjects, ProjectCommandOutcome, ProjectCommandRequest, ProjectCommandStage,
    RetireAgents,
};
use hq_domain::{
    CommandId, DomainError, ErrorCategory, ErrorCode, FactId, InstallationId, RemoteCommandResult,
    RuntimeObservation, Timestamp,
};

use crate::{ProjectWorkerPort, RepairLocalProjectWorkflows, project_command_request_digest};

/// Maximum remote commands inspected by one home recovery call.
pub const MAX_REMOTE_PROJECT_REPAIRS: usize = 1_024;

/// Complete durable control-plane progress for one exact remote command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteProjectCommandProgress {
    /// The active device authored the request and the home has not acknowledged it.
    Queued,
    /// The immutable home acknowledged the exact request at an observed project head.
    Received {
        /// Exact home-authored receipt fact.
        receipt_fact: FactId,
        /// Canonical project head observed by the home.
        received_head: Option<FactId>,
        /// Semantic receipt time supplied to the home worker.
        received_at: Timestamp,
    },
    /// The home authored one definite terminal control-plane result.
    Terminal {
        /// Exact home-authored receipt fact.
        receipt_fact: FactId,
        /// Canonical project head observed before execution.
        received_head: Option<FactId>,
        /// Semantic receipt time.
        received_at: Timestamp,
        /// Exact home-authored outcome fact.
        outcome_fact: FactId,
        /// Canonical commit or typed stable rejection.
        result: RemoteCommandResult,
        /// Definite or uncertain runtime truth reported by the workflow.
        runtime: Option<RuntimeObservation>,
    },
    /// Unequal request, receipt, or outcome values exist for the stable identity.
    Conflicted,
}

/// Passive exact remote command reconstructed from authoritative projections.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteProjectCommandRecord {
    /// Original typed request reconstructed from its strict body and control metadata.
    pub request: ProjectCommandRequest,
    /// Exact active-device-authored request fact.
    pub request_fact: FactId,
    /// Current durable control-plane progress.
    pub progress: RemoteProjectCommandProgress,
}

/// Result of authoring or replaying one exact remote control fact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteProjectFactOutcome {
    /// The exact fact is durably committed or an identical receipt was replayed.
    Committed,
    /// Canonical policy definitely rejected the fact plan.
    Rejected(DomainError),
    /// The fact may have committed and exact lookup/replay is required.
    Uncertain,
}

/// Durable remote-control fact and projection capability.
pub trait RemoteProjectCommandPort {
    /// Loads one exact remote command projection.
    fn command(
        &self,
        command_id: CommandId,
    ) -> Result<Option<RemoteProjectCommandRecord>, ApplicationError>;

    /// Loads bounded queued or received commands targeted at one immutable home.
    fn pending(
        &self,
        home: InstallationId,
        limit: usize,
    ) -> Result<Vec<RemoteProjectCommandRecord>, ApplicationError>;

    /// Authors or exactly replays an inert active-device request.
    fn author_request(
        &self,
        request: &ProjectCommandRequest,
    ) -> Result<RemoteProjectFactOutcome, ApplicationError>;

    /// Authors or exactly replays the home receipt at its observed head.
    fn author_receipt(
        &self,
        command_id: CommandId,
        received_at: Timestamp,
    ) -> Result<RemoteProjectFactOutcome, ApplicationError>;

    /// Authors or exactly replays one terminal home outcome.
    fn author_outcome(
        &self,
        command_id: CommandId,
        result: RemoteCommandResult,
        runtime: Option<RuntimeObservation>,
    ) -> Result<RemoteProjectFactOutcome, ApplicationError>;
}

/// Routes local-home commands directly and remote commands through durable control facts.
pub struct ProjectCommandRouter<L, R> {
    local_installation: InstallationId,
    local: L,
    remote: R,
}

impl<L, R> ProjectCommandRouter<L, R> {
    /// Constructs a router around one local workflow and one durable remote-control port.
    pub const fn new(local_installation: InstallationId, local: L, remote: R) -> Self {
        Self {
            local_installation,
            local,
            remote,
        }
    }
}

impl<L, R> ProjectCommandRouter<L, R>
where
    L: ControlProjects,
    R: RemoteProjectCommandPort,
{
    /// Drives a bounded deterministic set of queued or received commands for this home.
    pub fn repair_remote(
        &self,
        received_at: Timestamp,
        limit: usize,
    ) -> Result<Vec<ProjectCommandOutcome>, ApplicationError> {
        if limit == 0 || limit > MAX_REMOTE_PROJECT_REPAIRS {
            return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest));
        }
        self.remote
            .pending(self.local_installation, limit)?
            .into_iter()
            .map(|record| self.advance_remote(&record, received_at))
            .collect()
    }

    fn advance_remote(
        &self,
        record: &RemoteProjectCommandRecord,
        received_at: Timestamp,
    ) -> Result<ProjectCommandOutcome, ApplicationError> {
        if record.request.home != self.local_installation {
            return Ok(rejected(
                &record.request,
                ErrorCategory::Unauthorized,
                "project_remote_wrong_home",
                None,
            ));
        }
        match &record.progress {
            RemoteProjectCommandProgress::Queued => {
                match self
                    .remote
                    .author_receipt(record.request.command_id, received_at)?
                {
                    RemoteProjectFactOutcome::Committed => {}
                    RemoteProjectFactOutcome::Rejected(error) => {
                        return Ok(ProjectCommandOutcome::Rejected {
                            operation_id: record.request.operation_id,
                            error,
                            runtime: None,
                        });
                    }
                    RemoteProjectFactOutcome::Uncertain => {
                        return Ok(reconcilable(
                            &record.request,
                            ProjectCommandStage::ReceivedAtHome,
                            "project_remote_receipt_unknown",
                        ));
                    }
                }
            }
            RemoteProjectCommandProgress::Received { .. } => {}
            RemoteProjectCommandProgress::Terminal { .. }
            | RemoteProjectCommandProgress::Conflicted => {
                return Ok(outcome_from_record(record));
            }
        }
        let outcome = self.local.control_project(record.request.clone())?;
        let (result, runtime) = match &outcome {
            ProjectCommandOutcome::Completed {
                project_head,
                runtime,
                ..
            } => (
                RemoteCommandResult::Committed(*project_head),
                runtime.clone(),
            ),
            ProjectCommandOutcome::Rejected { error, runtime, .. } => (
                RemoteCommandResult::Rejected(error.code().clone()),
                runtime.clone(),
            ),
            ProjectCommandOutcome::Accepted { .. }
            | ProjectCommandOutcome::Running { .. }
            | ProjectCommandOutcome::Reconcilable { .. } => return Ok(outcome),
        };
        match self
            .remote
            .author_outcome(record.request.command_id, result, runtime)?
        {
            RemoteProjectFactOutcome::Committed => Ok(outcome),
            RemoteProjectFactOutcome::Rejected(error) => Ok(ProjectCommandOutcome::Rejected {
                operation_id: record.request.operation_id,
                error,
                runtime: None,
            }),
            RemoteProjectFactOutcome::Uncertain => Ok(reconcilable(
                &record.request,
                ProjectCommandStage::ReceivedAtHome,
                "project_remote_outcome_unknown",
            )),
        }
    }
}

impl<L, R> ControlProjects for ProjectCommandRouter<L, R>
where
    L: ControlProjects,
    R: RemoteProjectCommandPort,
{
    fn control_project(
        &self,
        request: ProjectCommandRequest,
    ) -> Result<ProjectCommandOutcome, ApplicationError> {
        if project_command_request_digest(&request).ok() != Some(request.request_digest) {
            return Ok(rejected(
                &request,
                ErrorCategory::InvalidInput,
                "project_command_digest_mismatch",
                None,
            ));
        }
        if request.home == self.local_installation {
            return self.local.control_project(request);
        }
        if let Some(existing) = self.remote.command(request.command_id)? {
            return Ok(if existing.request == request {
                outcome_from_record(&existing)
            } else {
                rejected(
                    &request,
                    ErrorCategory::Conflict,
                    "project_remote_command_identity_conflict",
                    None,
                )
            });
        }
        match self.remote.author_request(&request)? {
            RemoteProjectFactOutcome::Committed => Ok(ProjectCommandOutcome::Accepted {
                operation_id: request.operation_id,
                stage: ProjectCommandStage::AwaitingHome,
            }),
            RemoteProjectFactOutcome::Rejected(error) => Ok(ProjectCommandOutcome::Rejected {
                operation_id: request.operation_id,
                error,
                runtime: None,
            }),
            RemoteProjectFactOutcome::Uncertain => Ok(reconcilable(
                &request,
                ProjectCommandStage::AwaitingHome,
                "project_remote_request_unknown",
            )),
        }
    }
}

impl<L, R> RetireAgents for ProjectCommandRouter<L, R>
where
    L: RetireAgents,
{
    fn retire_agent(
        &self,
        request: AgentRetirementRequest,
    ) -> Result<AgentRetirementOutcome, ApplicationError> {
        if request.home != self.local_installation {
            return Ok(AgentRetirementOutcome::Rejected {
                operation_id: request.operation_id,
                error: DomainError::new(
                    ErrorCategory::Unauthorized,
                    stable_code("agent_retirement_wrong_home"),
                ),
                runtime: None,
            });
        }
        self.local.retire_agent(request)
    }
}

impl<L, R> ProjectWorkerPort for ProjectCommandRouter<L, R>
where
    L: RepairLocalProjectWorkflows,
    R: RemoteProjectCommandPort,
{
    fn repair_pending(
        &self,
        received_at: Timestamp,
        limit: usize,
    ) -> Result<Vec<ProjectCommandOutcome>, ApplicationError> {
        let mut outcomes = self.local.repair_local(limit)?;
        match self.repair_remote(received_at, limit) {
            Ok(remote) => outcomes.extend(remote),
            Err(error) if error.code() == ApplicationErrorCode::AdapterUnavailable => {}
            Err(error) => return Err(error),
        }
        Ok(outcomes)
    }
}

fn outcome_from_record(record: &RemoteProjectCommandRecord) -> ProjectCommandOutcome {
    match &record.progress {
        RemoteProjectCommandProgress::Queued => ProjectCommandOutcome::Accepted {
            operation_id: record.request.operation_id,
            stage: ProjectCommandStage::AwaitingHome,
        },
        RemoteProjectCommandProgress::Received { .. } => ProjectCommandOutcome::Running {
            operation_id: record.request.operation_id,
            stage: ProjectCommandStage::ReceivedAtHome,
        },
        RemoteProjectCommandProgress::Terminal {
            result, runtime, ..
        } => match result {
            RemoteCommandResult::Committed(project_head) => ProjectCommandOutcome::Completed {
                operation_id: record.request.operation_id,
                project_head: *project_head,
                runtime: runtime.clone(),
            },
            RemoteCommandResult::Rejected(code) => ProjectCommandOutcome::Rejected {
                operation_id: record.request.operation_id,
                error: DomainError::new(ErrorCategory::Conflict, code.clone()),
                runtime: runtime.clone(),
            },
        },
        RemoteProjectCommandProgress::Conflicted => rejected(
            &record.request,
            ErrorCategory::Conflict,
            "project_remote_command_conflicted",
            None,
        ),
    }
}

fn rejected(
    request: &ProjectCommandRequest,
    category: ErrorCategory,
    code: &str,
    runtime: Option<RuntimeObservation>,
) -> ProjectCommandOutcome {
    ProjectCommandOutcome::Rejected {
        operation_id: request.operation_id,
        error: DomainError::new(category, stable_code(code)),
        runtime,
    }
}

fn reconcilable(
    request: &ProjectCommandRequest,
    stage: ProjectCommandStage,
    code: &str,
) -> ProjectCommandOutcome {
    ProjectCommandOutcome::Reconcilable {
        operation_id: request.operation_id,
        stage,
        error: DomainError::new(ErrorCategory::Unresolved, stable_code(code)),
    }
}

fn stable_code(code: &str) -> ErrorCode {
    ErrorCode::new(code).unwrap_or_else(|_| unreachable!("static project error codes are valid"))
}
