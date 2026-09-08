//! Bounded independent cancellation dispatch with stable request identity and observable results.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};

use hq_application::{
    AgentCancellationRequest, AgentCancellationState, ApplicationError, ApplicationErrorCode,
    RuntimeFailureReason,
};
use hq_domain::OperationId;

const MAX_REQUESTS: usize = 1_024;
const MAX_ACTIVE: usize = 8;

struct Record {
    request: AgentCancellationRequest,
    state: AgentCancellationState,
}

#[derive(Default)]
struct Jobs {
    closed: bool,
    draining: bool,
    records: BTreeMap<OperationId, Record>,
    active: BTreeMap<OperationId, JoinHandle<()>>,
}

#[derive(Clone, Default)]
pub(crate) struct AgentCancellationJobs(Arc<Mutex<Jobs>>);

impl AgentCancellationJobs {
    pub(crate) fn submit(
        &self,
        request: AgentCancellationRequest,
        execute: impl FnOnce() -> AgentCancellationState + Send + 'static,
    ) -> Result<AgentCancellationState, ApplicationError> {
        let mut jobs = self.0.lock().map_err(|_| unavailable())?;
        reap(&mut jobs);
        if jobs.closed {
            return Err(unavailable());
        }
        if let Some(record) = jobs.records.get(&request.request_id) {
            check_identity(record, &request)?;
            if jobs.active.contains_key(&request.request_id)
                || !matches!(record.state, AgentCancellationState::Uncertain(_))
            {
                return Ok(record.state.clone());
            }
        } else if jobs.records.len() >= MAX_REQUESTS {
            return Err(ApplicationError::new(ApplicationErrorCode::IntakeFull));
        }
        if jobs.active.len() >= MAX_ACTIVE {
            return Err(ApplicationError::new(ApplicationErrorCode::IntakeFull));
        }
        let id = request.request_id;
        let completion = Arc::clone(&self.0);
        let task = thread::Builder::new()
            .name("hq-agent-cancel".to_owned())
            .spawn(move || {
                let state = execute();
                if let Ok(mut jobs) = completion.lock()
                    && let Some(record) = jobs.records.get_mut(&id)
                {
                    record.state = state;
                }
            })
            .map_err(|_| unavailable())?;
        jobs.records.insert(
            id,
            Record {
                request,
                state: AgentCancellationState::Queued,
            },
        );
        jobs.active.insert(id, task);
        Ok(AgentCancellationState::Queued)
    }

    pub(crate) fn observe(
        &self,
        request: &AgentCancellationRequest,
    ) -> Result<AgentCancellationState, ApplicationError> {
        let mut jobs = self.0.lock().map_err(|_| unavailable())?;
        reap(&mut jobs);
        let record = jobs
            .records
            .get(&request.request_id)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCode::InvalidRequest))?;
        check_identity(record, request)?;
        Ok(record.state.clone())
    }

    pub(crate) fn restart(&self) -> Result<(), ApplicationError> {
        let mut jobs = self.0.lock().map_err(|_| unavailable())?;
        if (!jobs.closed && !jobs.records.is_empty()) || jobs.draining || !jobs.active.is_empty() {
            return Err(unavailable());
        }
        *jobs = Jobs::default();
        Ok(())
    }

    pub(crate) fn shutdown(&self) -> Result<(), ApplicationError> {
        let tasks = {
            let mut jobs = self.0.lock().map_err(|_| unavailable())?;
            if jobs.draining {
                return Err(unavailable());
            }
            jobs.closed = true;
            jobs.draining = true;
            std::mem::take(&mut jobs.active)
        };
        let mut failed = false;
        for (_, task) in tasks {
            failed |= task.join().is_err();
        }
        self.0.lock().map_err(|_| unavailable())?.draining = false;
        if failed { Err(unavailable()) } else { Ok(()) }
    }
}

fn check_identity(
    record: &Record,
    request: &AgentCancellationRequest,
) -> Result<(), ApplicationError> {
    if record.request == *request {
        Ok(())
    } else {
        Err(ApplicationError::new(
            ApplicationErrorCode::CommandIdentityConflict,
        ))
    }
}

fn reap(jobs: &mut Jobs) {
    let finished = jobs
        .active
        .iter()
        .filter(|(_, task)| task.is_finished())
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    for id in finished {
        if let Some(task) = jobs.active.remove(&id)
            && task.join().is_err()
            && let Some(record) = jobs.records.get_mut(&id)
        {
            record.state = AgentCancellationState::Uncertain(RuntimeFailureReason::Unavailable);
        }
    }
}

fn unavailable() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCode::AdapterUnavailable)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use hq_application::{
        AgentOperationQuery, AgentOperationScope, AgentOperationTarget, ConversationKey,
        RuntimeGenerationId, RuntimeWorkerOwner,
    };
    use hq_domain::{
        AccountId, AgentId, InstallationId, MailboxAddress, MailboxId, ProviderId,
        ProviderSessionId,
    };
    use std::{
        num::NonZeroU64,
        sync::mpsc,
        time::{Duration, Instant},
    };

    fn request(id: u8) -> AgentCancellationRequest {
        let home = InstallationId::from_bytes([1; 32]);
        let mailbox = MailboxAddress::new(home, MailboxId::from_bytes([2; 32]));
        let provider = ProviderId::new("test").expect("provider");
        let session = ProviderSessionId::new("session").expect("session");
        AgentCancellationRequest {
            request_id: OperationId::from_bytes([id; 32]),
            target: AgentOperationTarget {
                query: AgentOperationQuery {
                    account_id: AccountId::from_bytes([3; 32]),
                    home,
                    conversation: ConversationKey::ProviderSession {
                        counterparty: mailbox,
                        provider: provider.clone(),
                        session: session.clone(),
                    },
                    tracked_operation: None,
                },
                scope: AgentOperationScope {
                    agent_id: AgentId::from_bytes([4; 32]),
                    mailbox,
                    provider,
                    session,
                    project: None,
                },
                generation: RuntimeGenerationId::from_bytes([5; 32]).expect("generation"),
                owner: RuntimeWorkerOwner::from_bytes([6; 32]).expect("owner"),
                operation_id: OperationId::from_bytes([7; 32]),
                sequence: NonZeroU64::MIN,
            },
        }
    }

    fn settled(
        jobs: &AgentCancellationJobs,
        request: &AgentCancellationRequest,
    ) -> AgentCancellationState {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let state = jobs.observe(request).expect("observe");
            if state != AgentCancellationState::Queued {
                return state;
            }
            assert!(Instant::now() < deadline, "cancellation did not settle");
            thread::yield_now();
        }
    }

    #[test]
    fn admission_and_observation_do_not_wait_for_control_and_replay_is_exact() {
        let jobs = AgentCancellationJobs::default();
        let input = request(10);
        let (release, gate) = mpsc::channel();
        let (started, running) = mpsc::channel();
        assert_eq!(
            jobs.submit(input.clone(), move || {
                started.send(()).expect("started");
                gate.recv_timeout(Duration::from_secs(3)).expect("release");
                AgentCancellationState::Requested
            })
            .expect("admit"),
            AgentCancellationState::Queued
        );
        running
            .recv_timeout(Duration::from_secs(3))
            .expect("running");
        assert_eq!(
            jobs.observe(&input).expect("observe blocked job"),
            AgentCancellationState::Queued
        );
        assert_eq!(
            jobs.submit(input.clone(), || AgentCancellationState::AlreadyFinished)
                .expect("replay"),
            AgentCancellationState::Queued
        );
        let mut changed = input.clone();
        changed.target.operation_id = OperationId::from_bytes([8; 32]);
        assert!(
            jobs.submit(changed.clone(), || AgentCancellationState::AlreadyFinished)
                .is_err()
        );
        assert!(jobs.observe(&changed).is_err());
        release.send(()).expect("release");
        assert_eq!(settled(&jobs, &input), AgentCancellationState::Requested);
        assert_eq!(
            jobs.submit(input, || AgentCancellationState::AlreadyFinished)
                .expect("terminal replay"),
            AgentCancellationState::Requested
        );
        jobs.shutdown().expect("shutdown");
        assert!(
            jobs.submit(request(11), || AgentCancellationState::Requested)
                .is_err()
        );
    }

    #[test]
    fn uncertain_retry_waits_for_prior_dispatch_to_exit() {
        let jobs = AgentCancellationJobs::default();
        let input = request(13);
        let (release, gate) = mpsc::channel();
        jobs.submit(input.clone(), move || {
            gate.recv_timeout(Duration::from_secs(3)).expect("release");
            AgentCancellationState::Uncertain(RuntimeFailureReason::Unavailable)
        })
        .expect("submit");
        // Model the interval after a worker publishes its outcome but before its thread exits.
        jobs.0
            .lock()
            .expect("jobs")
            .records
            .get_mut(&input.request_id)
            .expect("record")
            .state = AgentCancellationState::Uncertain(RuntimeFailureReason::Unavailable);
        assert_eq!(
            jobs.submit(input.clone(), || AgentCancellationState::Requested)
                .expect("in-flight retry"),
            AgentCancellationState::Uncertain(RuntimeFailureReason::Unavailable)
        );
        assert_eq!(jobs.0.lock().expect("jobs").active.len(), 1);
        release.send(()).expect("release");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let mut state = jobs.0.lock().expect("jobs");
            reap(&mut state);
            if state.active.is_empty() {
                break;
            }
            drop(state);
            assert!(Instant::now() < deadline, "worker failed to exit");
            thread::yield_now();
        }
        assert_eq!(
            jobs.submit(input.clone(), || AgentCancellationState::Requested)
                .expect("retry"),
            AgentCancellationState::Queued
        );
        assert_eq!(settled(&jobs, &input), AgentCancellationState::Requested);
        jobs.shutdown().expect("shutdown");
    }

    #[test]
    fn active_dispatch_is_bounded_and_completed_replays_need_no_slot() {
        let jobs = AgentCancellationJobs::default();
        let completed = request(20);
        jobs.submit(completed.clone(), || AgentCancellationState::Requested)
            .expect("submit");
        assert_eq!(
            settled(&jobs, &completed),
            AgentCancellationState::Requested
        );
        // Reap deterministically before filling the capacity.
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let mut state = jobs.0.lock().expect("jobs");
            reap(&mut state);
            if state.active.is_empty() {
                break;
            }
            drop(state);
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        let mut releases = Vec::new();
        for id in 30..30 + u8::try_from(MAX_ACTIVE).expect("bounded capacity") {
            let (release, gate) = mpsc::channel();
            releases.push(release);
            jobs.submit(request(id), move || {
                gate.recv_timeout(Duration::from_secs(3)).expect("release");
                AgentCancellationState::Requested
            })
            .expect("capacity");
        }
        assert!(
            jobs.submit(request(50), || AgentCancellationState::Requested)
                .is_err()
        );
        assert_eq!(
            jobs.submit(completed, || AgentCancellationState::AlreadyFinished)
                .expect("replay at capacity"),
            AgentCancellationState::Requested
        );
        for release in releases {
            release.send(()).expect("release");
        }
        jobs.shutdown().expect("shutdown");
    }

    #[test]
    fn restart_reopens_only_after_shutdown_and_discards_old_generation_records() {
        let jobs = AgentCancellationJobs::default();
        let input = request(12);
        jobs.submit(input.clone(), || AgentCancellationState::Requested)
            .expect("submit");
        assert_eq!(settled(&jobs, &input), AgentCancellationState::Requested);
        assert!(jobs.restart().is_err(), "a live generation cannot be reset");
        jobs.shutdown().expect("shutdown");
        jobs.restart().expect("restart");
        assert!(jobs.observe(&input).is_err());
        jobs.submit(input.clone(), || AgentCancellationState::AlreadyFinished)
            .expect("new generation");
        assert_eq!(
            settled(&jobs, &input),
            AgentCancellationState::AlreadyFinished
        );
        jobs.shutdown().expect("shutdown");
    }
}
