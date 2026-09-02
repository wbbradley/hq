//! Neutral harness implementation over the private Codex protocol.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    io::Read,
    path::Path,
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use hq_domain::{
    BoundedVec, CommandDigest, ContentText, MessageId, OperationId, ProviderSessionId, ShortText,
};
use hq_harness::{
    HarnessCancellationOutcome, HarnessCapabilities, HarnessCapability, HarnessDrainOutcome,
    HarnessError, HarnessErrorClass, HarnessEvent, HarnessEventPoll, HarnessFactory,
    HarnessInstance, HarnessInstanceRequest, HarnessInteractiveAnswer, HarnessInteractiveRequest,
    HarnessInteractiveResponse, HarnessRequestChoice, HarnessRequestId, HarnessRequestKind,
    HarnessSession, HarnessSessionRequest, HarnessSubmission, HarnessSubmissionLookup,
    HarnessSubmissionOutcome, OpenedHarnessSession,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    CODEX_BASELINE_VERSION,
    normalize::Normalizer,
    process::{
        CodexDiagnosticSink, CodexLaunch, CodexLaunchResolver, CodexProcessControl,
        CodexProcessStarter, CodexWaitOutcome,
    },
    protocol::{
        ClientError, ClientErrorBody, ClientInfo, ClientNotification, ClientRequest, ClientResult,
        InitializeCapabilities, InitializeParams, TextInput, Thread, ThreadReadParams,
        ThreadResponse, ThreadResumeParams, ThreadStartParams, TurnInterruptParams, TurnResponse,
        TurnStartParams, TurnSteerParams, TurnSteerResponse, WireMessage,
    },
    transport::{JsonlTransport, TransportRead},
};

const CLIENT_NAME: &str = "hq";
const CLIENT_TITLE: &str = "HQ";
const STDERR_LINE_BYTES: usize = 16 * 1024;

/// Passive dependencies and bounds for independent Codex instances.
pub struct CodexFactoryConfig {
    /// Injectable child-process creation capability.
    pub starter: Arc<dyn CodexProcessStarter>,
    /// Provider-private launch policy resolver.
    pub resolver: Arc<dyn CodexLaunchResolver>,
    /// Private sink for bounded, untrusted provider diagnostics.
    pub diagnostics: Arc<dyn CodexDiagnosticSink>,
    /// Maximum wait for one correlated protocol response.
    pub call_timeout: Duration,
    /// Grace period before child termination escalates to kill.
    pub process_grace: Duration,
    /// Maximum parsed frames waiting between the reader and session owner.
    pub frame_capacity: usize,
}

impl fmt::Debug for CodexFactoryConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CodexFactoryConfig")
            .field("call_timeout", &self.call_timeout)
            .field("process_grace", &self.process_grace)
            .field("frame_capacity", &self.frame_capacity)
            .finish_non_exhaustive()
    }
}

/// Independently creates provider-owned app-server instances.
pub struct CodexFactory {
    config: CodexFactoryConfig,
}

impl fmt::Debug for CodexFactory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CodexFactory")
            .finish_non_exhaustive()
    }
}

impl CodexFactory {
    /// Validates and owns provider dependencies and bounds.
    pub fn new(config: CodexFactoryConfig) -> Result<Self, HarnessError> {
        if config.call_timeout.is_zero()
            || config.process_grace.is_zero()
            || config.frame_capacity == 0
        {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        }
        Ok(Self { config })
    }

    /// Declares the complete neutral behavior implemented by this adapter.
    pub fn capabilities() -> HarnessCapabilities {
        HarnessCapabilities {
            supported: [
                HarnessCapability::StartSessions,
                HarnessCapability::ResumeSessions,
                HarnessCapability::StableSubmissionIdempotency,
                HarnessCapability::SubmissionLookup,
                HarnessCapability::OperationCancellation,
                HarnessCapability::InteractiveRequests,
            ]
            .into_iter()
            .collect(),
        }
    }
}

impl HarnessFactory for CodexFactory {
    fn create_instance(
        &self,
        request: HarnessInstanceRequest,
    ) -> Result<Box<dyn HarnessInstance>, HarnessError> {
        let launch = self.config.resolver.resolve(&request)?;
        validate_launch(&launch)?;
        let pipes = self.config.starter.start(&launch, &request.environment)?;
        let mut transport = JsonlTransport::start(pipes.output, self.config.frame_capacity)?;
        transport.bind_input(pipes.input);
        let stderr = spawn_stderr_drain(pipes.errors, Arc::clone(&self.config.diagnostics))?;
        Ok(Box::new(CodexInstance {
            launch,
            transport,
            control: pipes.control,
            stderr: Some(stderr),
            call_timeout: self.config.call_timeout,
            process_grace: self.config.process_grace,
        }))
    }
}

struct CodexInstance {
    launch: CodexLaunch,
    transport: JsonlTransport,
    control: Arc<dyn CodexProcessControl>,
    stderr: Option<JoinHandle<()>>,
    call_timeout: Duration,
    process_grace: Duration,
}

impl HarnessInstance for CodexInstance {
    fn open_session(
        self: Box<Self>,
        request: HarnessSessionRequest,
    ) -> Result<OpenedHarnessSession, HarnessError> {
        let mut session = CodexSession::from_instance(*self);
        session.initialize()?;
        let acknowledged = match &request {
            HarnessSessionRequest::Start => session.start_thread()?,
            HarnessSessionRequest::Resume { session_id } => {
                match session.resume_thread(session_id.as_str()) {
                    Ok(acknowledged) => acknowledged,
                    Err(RpcFailure::Rejected) => {
                        return Err(HarnessError::new(HarnessErrorClass::SessionNotFound));
                    }
                    Err(error) => return Err(error.harness()),
                }
            }
        };
        let session_id = ProviderSessionId::new(acknowledged)
            .map_err(|_| HarnessError::new(HarnessErrorClass::ProtocolViolation))?;
        if let HarnessSessionRequest::Resume {
            session_id: expected,
        } = request
            && expected != session_id
        {
            session.stop_runtime()?;
            return Err(HarnessError::new(
                HarnessErrorClass::SessionIdentityMismatch,
            ));
        }
        session_id.as_str().clone_into(&mut session.thread_id);
        Ok(OpenedHarnessSession {
            session_id,
            session: Box::new(session),
        })
    }
}

struct CodexSession {
    launch: CodexLaunch,
    transport: JsonlTransport,
    control: Arc<dyn CodexProcessControl>,
    stderr: Option<JoinHandle<()>>,
    call_timeout: Duration,
    process_grace: Duration,
    thread_id: String,
    next_call_id: u64,
    intake_open: bool,
    stopped: bool,
    compatibility_failed: bool,
    deferred_error: Option<HarnessErrorClass>,
    pending_submission_operation: Option<OperationId>,
    active_turn: Option<String>,
    operations: BTreeMap<String, OperationId>,
    submissions: BTreeMap<MessageId, SubmissionRecord>,
    events: VecDeque<HarnessEvent>,
    pending_requests: BTreeMap<HarnessRequestId, PendingRequest>,
    request_groups: BTreeMap<String, PendingGroup>,
    answered_requests: BTreeSet<HarnessRequestId>,
    cancelled_requests: BTreeSet<HarnessRequestId>,
    normalizer: Normalizer,
}

#[derive(Clone, Copy)]
struct SubmissionRecord {
    digest: CommandDigest,
    operation_id: OperationId,
}

struct PendingRequest {
    group: String,
    question_id: Option<String>,
    kind: PendingKind,
    choices: BTreeSet<String>,
    allow_text: bool,
}

#[derive(Clone, Copy)]
enum PendingKind {
    Question,
    Approval,
    Permission,
    McpForm,
    McpUrl,
}

struct PendingGroup {
    wire_id: Value,
    method: String,
    operation_id: OperationId,
    expected: usize,
    answers: BTreeMap<String, Value>,
    original: Value,
}

#[derive(Clone, Copy)]
enum RpcFailure {
    Rejected,
    Uncertain(HarnessErrorClass),
    Boundary(HarnessErrorClass),
}

impl RpcFailure {
    const fn harness(self) -> HarnessError {
        HarnessError::new(match self {
            Self::Rejected => HarnessErrorClass::Unavailable,
            Self::Uncertain(class) | Self::Boundary(class) => class,
        })
    }
}

impl CodexSession {
    fn from_instance(instance: CodexInstance) -> Self {
        Self {
            launch: instance.launch,
            transport: instance.transport,
            control: instance.control,
            stderr: instance.stderr,
            call_timeout: instance.call_timeout,
            process_grace: instance.process_grace,
            thread_id: String::new(),
            next_call_id: 0,
            intake_open: true,
            stopped: false,
            compatibility_failed: false,
            deferred_error: None,
            pending_submission_operation: None,
            active_turn: None,
            operations: BTreeMap::new(),
            submissions: BTreeMap::new(),
            events: VecDeque::new(),
            pending_requests: BTreeMap::new(),
            request_groups: BTreeMap::new(),
            answered_requests: BTreeSet::new(),
            cancelled_requests: BTreeSet::new(),
            normalizer: Normalizer::new(),
        }
    }

    fn initialize(&mut self) -> Result<(), HarnessError> {
        let _: Value = self
            .rpc(
                "initialize",
                InitializeParams {
                    client_info: ClientInfo {
                        name: CLIENT_NAME,
                        title: CLIENT_TITLE,
                        version: env!("CARGO_PKG_VERSION"),
                    },
                    capabilities: InitializeCapabilities {
                        experimental_api: true,
                    },
                },
            )
            .map_err(RpcFailure::harness)?;
        self.transport
            .write(&ClientNotification {
                method: "initialized",
                params: Value::Null,
            })
            .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))
    }

    fn start_thread(&mut self) -> Result<String, HarnessError> {
        let cwd = path_text(&self.launch.working_directory)?.to_owned();
        let permissive = self.launch.permissive;
        let model = self.launch.model.clone();
        let instructions = self.launch.developer_instructions.clone();
        let response: ThreadResponse = self
            .rpc(
                "thread/start",
                ThreadStartParams {
                    cwd: &cwd,
                    developer_instructions: &instructions,
                    model: model.as_deref(),
                    approval_policy: permissive.then_some("never"),
                    sandbox: permissive.then_some("danger-full-access"),
                },
            )
            .map_err(RpcFailure::harness)?;
        nonempty_thread(response.thread)
    }

    fn resume_thread(&mut self, thread_id: &str) -> Result<String, RpcFailure> {
        let cwd = path_text(&self.launch.working_directory)
            .map_err(|error| RpcFailure::Boundary(error.class))?
            .to_owned();
        let permissive = self.launch.permissive;
        let model = self.launch.model.clone();
        let response: ThreadResponse = self.rpc(
            "thread/resume",
            ThreadResumeParams {
                thread_id,
                cwd: &cwd,
                model: model.as_deref(),
                approval_policy: permissive.then_some("never"),
                sandbox: permissive.then_some("danger-full-access"),
            },
        )?;
        nonempty_thread(response.thread)
            .map_err(|_| RpcFailure::Boundary(HarnessErrorClass::ProtocolViolation))
    }

    fn rpc<T: Serialize, R: DeserializeOwned>(
        &mut self,
        method: &str,
        params: T,
    ) -> Result<R, RpcFailure> {
        self.next_call_id = self
            .next_call_id
            .checked_add(1)
            .ok_or(RpcFailure::Boundary(HarnessErrorClass::ProtocolViolation))?;
        let id = self.next_call_id;
        self.transport
            .write(&ClientRequest { method, id, params })
            .map_err(|_| RpcFailure::Uncertain(HarnessErrorClass::Unavailable))?;
        let deadline = Instant::now()
            .checked_add(self.call_timeout)
            .ok_or(RpcFailure::Boundary(HarnessErrorClass::InvalidInput))?;
        loop {
            let Some(wait) = deadline.checked_duration_since(Instant::now()) else {
                return Err(RpcFailure::Uncertain(HarnessErrorClass::Unavailable));
            };
            match self.transport.receive(wait) {
                TransportRead::Message(message) if message_id(&message) == Some(id) => {
                    if message.error.is_some() {
                        return Err(RpcFailure::Rejected);
                    }
                    return message
                        .result
                        .ok_or(RpcFailure::Boundary(HarnessErrorClass::ProtocolViolation))
                        .and_then(|result| {
                            serde_json::from_value(result).map_err(|_| {
                                RpcFailure::Boundary(HarnessErrorClass::ProtocolViolation)
                            })
                        });
                }
                TransportRead::Message(message) => self
                    .dispatch(message)
                    .map_err(|error| RpcFailure::Boundary(error.class))?,
                TransportRead::TimedOut => {
                    return Err(RpcFailure::Uncertain(HarnessErrorClass::Unavailable));
                }
                TransportRead::Closed => {
                    return Err(RpcFailure::Uncertain(HarnessErrorClass::TransportClosed));
                }
                TransportRead::Failed(failure) => {
                    return Err(RpcFailure::Boundary(failure.class()));
                }
            }
        }
    }

    fn dispatch(&mut self, message: WireMessage) -> Result<(), HarnessError> {
        match (message.id, message.method, message.params) {
            (Some(id), Some(method), params) => {
                self.server_request(id, &method, params.unwrap_or(Value::Null))
            }
            (None, Some(method), params) => {
                self.notification(&method, params.unwrap_or(Value::Null));
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn notification(&mut self, method: &str, params: Value) {
        if let Some((thread_id, turn_id)) = notification_context(&params)
            && thread_id == self.thread_id
            && !turn_id.is_empty()
            && !self.operations.contains_key(&turn_id)
        {
            let operation_id = self
                .pending_submission_operation
                .unwrap_or_else(|| provider_operation_id(&thread_id, &turn_id));
            self.operations.insert(turn_id, operation_id);
        }
        if let Some((thread_id, turn_id, status)) = notification_turn(method, &params)
            && thread_id == self.thread_id
            && !turn_id.is_empty()
        {
            if method == "turn/started"
                && !self.operations.contains_key(&turn_id)
                && let Some(operation_id) = self.pending_submission_operation
            {
                self.operations.insert(turn_id.clone(), operation_id);
            }
            if status == "inProgress" {
                self.active_turn = Some(turn_id.clone());
            } else if method == "turn/completed" && self.active_turn.as_deref() == Some(&turn_id) {
                self.active_turn = None;
            }
        }
        let events =
            self.normalizer
                .notification(method, params, &self.thread_id, &self.operations);
        self.events.extend(events);
    }

    fn server_request(
        &mut self,
        id: Value,
        method: &str,
        params: Value,
    ) -> Result<(), HarnessError> {
        if !self.intake_open {
            return self.fail_closed(id, method, &params);
        }
        if !matches!(
            method,
            "item/tool/requestUserInput"
                | "item/commandExecution/requestApproval"
                | "item/fileChange/requestApproval"
                | "item/permissions/requestApproval"
                | "mcpServer/elicitation/request"
        ) {
            self.transport.write(&ClientError {
                id,
                error: ClientErrorBody {
                    code: -32601,
                    message: "unsupported app-server method",
                },
            })?;
            self.compatibility_failed = true;
            return Err(HarnessError::new(HarnessErrorClass::CompatibilityMismatch));
        }
        let thread_id = string_field(&params, "threadId").unwrap_or_default();
        let turn_id = string_field(&params, "turnId").unwrap_or_default();
        if thread_id == self.thread_id && !turn_id.is_empty() {
            self.operations
                .entry(turn_id.clone())
                .or_insert_with(|| provider_operation_id(&thread_id, &turn_id));
            self.active_turn.get_or_insert_with(|| turn_id.clone());
        }
        let Some(operation_id) = self.operations.get(&turn_id).copied() else {
            self.fail_closed(id, method, &params)?;
            self.deferred_error = Some(HarnessErrorClass::InvalidInput);
            return Ok(());
        };
        if thread_id != self.thread_id {
            self.fail_closed(id, method, &params)?;
            self.deferred_error = Some(HarnessErrorClass::InvalidInput);
            return Ok(());
        }
        let group = wire_group_key(&id, method)?;
        if self.request_groups.contains_key(&group) {
            self.transport.write(&ClientError {
                id,
                error: ClientErrorBody {
                    code: -32600,
                    message: "duplicate app-server request",
                },
            })?;
            self.compatibility_failed = true;
            return Err(HarnessError::new(HarnessErrorClass::CompatibilityMismatch));
        }
        let Ok(requests) = self.normalize_request(method, &group, operation_id, &params) else {
            self.fail_closed(id, method, &params)?;
            self.deferred_error = Some(HarnessErrorClass::InvalidInput);
            return Ok(());
        };
        if requests.is_empty() {
            self.fail_closed(id, method, &params)?;
            return Ok(());
        }
        self.request_groups.insert(
            group,
            PendingGroup {
                wire_id: id,
                method: method.to_owned(),
                operation_id,
                expected: requests.len(),
                answers: BTreeMap::new(),
                original: params,
            },
        );
        self.events
            .extend(requests.into_iter().map(HarnessEvent::InteractiveRequest));
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn normalize_request(
        &mut self,
        method: &str,
        group: &str,
        operation_id: OperationId,
        params: &Value,
    ) -> Result<Vec<HarnessInteractiveRequest>, HarnessError> {
        if method.starts_with("item/")
            && string_field(params, "itemId").is_none_or(|value| value.is_empty())
        {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        }
        if method == "item/tool/requestUserInput" {
            let questions = params
                .get("questions")
                .and_then(Value::as_array)
                .ok_or_else(|| HarnessError::new(HarnessErrorClass::InvalidInput))?;
            if questions.is_empty() || questions.len() > 64 {
                return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
            }
            if questions.iter().any(|question| {
                question
                    .get("isSecret")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            }) {
                self.deferred_error = Some(HarnessErrorClass::SecretInputRejected);
                return Ok(Vec::new());
            }
            let mut normalized = Vec::with_capacity(questions.len());
            for (index, question) in questions.iter().enumerate() {
                let question_id = string_field(question, "id")
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| HarnessError::new(HarnessErrorClass::InvalidInput))?;
                let prompt = string_field(question, "question")
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| HarnessError::new(HarnessErrorClass::InvalidInput))?;
                let choices = question
                    .get("options")
                    .and_then(Value::as_array)
                    .map_or_else(Vec::new, |options| {
                        options
                            .iter()
                            .filter_map(|option| string_field(option, "label"))
                            .collect()
                    });
                let request_id = stable_request_id(group, index);
                let neutral_choices = choices
                    .iter()
                    .map(|choice| {
                        let value = bounded_short(choice)?;
                        Ok(HarnessRequestChoice {
                            value: value.clone(),
                            label: value,
                        })
                    })
                    .collect::<Result<Vec<_>, HarnessError>>()?;
                let allow_text = question
                    .get("isOther")
                    .and_then(Value::as_bool)
                    .unwrap_or(neutral_choices.is_empty());
                self.pending_requests.insert(
                    request_id,
                    PendingRequest {
                        group: group.to_owned(),
                        question_id: Some(question_id),
                        kind: PendingKind::Question,
                        choices: choices.into_iter().collect(),
                        allow_text,
                    },
                );
                normalized.push(HarnessInteractiveRequest {
                    request_id,
                    operation_id,
                    kind: HarnessRequestKind::Question,
                    prompt: bounded_content(&prompt)?,
                    choices: BoundedVec::new(neutral_choices)
                        .map_err(|_| HarnessError::new(HarnessErrorClass::InvalidInput))?,
                    allow_text,
                });
            }
            return Ok(normalized);
        }

        let (kind, pending_kind, prompt, choices, allow_text) = match method {
            "item/commandExecution/requestApproval" => {
                let mut choices = vec![
                    "accept".to_owned(),
                    "acceptForSession".to_owned(),
                    "decline".to_owned(),
                    "cancel".to_owned(),
                ];
                if params
                    .get("proposedExecpolicyAmendment")
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty())
                {
                    choices.push("acceptWithExecpolicyAmendment".to_owned());
                }
                if let Some(amendments) = params
                    .get("proposedNetworkPolicyAmendments")
                    .and_then(Value::as_array)
                {
                    choices.extend(
                        (1..=amendments.len())
                            .map(|index| format!("applyNetworkPolicyAmendment:{index}")),
                    );
                }
                (
                    HarnessRequestKind::CommandApproval,
                    PendingKind::Approval,
                    approval_prompt("Command approval", params),
                    choices,
                    false,
                )
            }
            "item/fileChange/requestApproval" => (
                HarnessRequestKind::FileApproval,
                PendingKind::Approval,
                approval_prompt("File-change approval", params),
                vec![
                    "accept".to_owned(),
                    "acceptForSession".to_owned(),
                    "decline".to_owned(),
                    "cancel".to_owned(),
                ],
                false,
            ),
            "item/permissions/requestApproval" => (
                HarnessRequestKind::Permission,
                PendingKind::Permission,
                approval_prompt("Permission approval", params),
                vec![
                    "grantTurn".to_owned(),
                    "grantSession".to_owned(),
                    "decline".to_owned(),
                ],
                false,
            ),
            "mcpServer/elicitation/request" => {
                let mode = string_field(params, "mode").unwrap_or_default();
                let message = string_field(params, "message").unwrap_or_default();
                if string_field(params, "serverName").is_none_or(|value| value.is_empty())
                    || message.trim().is_empty()
                {
                    return Ok(Vec::new());
                }
                if mode == "form" {
                    if schema_contains_secret(params.get("requestedSchema")) {
                        self.deferred_error = Some(HarnessErrorClass::SecretInputRejected);
                        return Ok(Vec::new());
                    }
                    if !valid_mcp_form_schema(params.get("requestedSchema")) {
                        return Ok(Vec::new());
                    }
                    (
                        HarnessRequestKind::McpForm,
                        PendingKind::McpForm,
                        message,
                        Vec::new(),
                        true,
                    )
                } else if mode == "url"
                    && string_field(params, "url").is_some_and(|value| !value.is_empty())
                    && string_field(params, "elicitationId").is_some_and(|value| !value.is_empty())
                {
                    (
                        HarnessRequestKind::McpUrl,
                        PendingKind::McpUrl,
                        format!(
                            "{}\nURL: {}",
                            message,
                            string_field(params, "url").unwrap_or_default()
                        ),
                        vec![
                            "accept".to_owned(),
                            "decline".to_owned(),
                            "cancel".to_owned(),
                        ],
                        false,
                    )
                } else {
                    return Ok(Vec::new());
                }
            }
            _ => return Ok(Vec::new()),
        };
        if prompt.trim().is_empty() {
            return Ok(Vec::new());
        }
        let request_id = stable_request_id(group, 0);
        if matches!(pending_kind, PendingKind::Permission)
            && !params.get("permissions").is_some_and(Value::is_object)
        {
            return Ok(Vec::new());
        }
        let choice_set = choices.iter().cloned().collect();
        let neutral_choices = choices
            .into_iter()
            .map(|value| {
                let value = bounded_short(&value)?;
                Ok(HarnessRequestChoice {
                    value: value.clone(),
                    label: value,
                })
            })
            .collect::<Result<Vec<_>, HarnessError>>()?;
        self.pending_requests.insert(
            request_id,
            PendingRequest {
                group: group.to_owned(),
                question_id: None,
                kind: pending_kind,
                choices: choice_set,
                allow_text,
            },
        );
        Ok(vec![HarnessInteractiveRequest {
            request_id,
            operation_id,
            kind,
            prompt: bounded_content(&prompt)?,
            choices: BoundedVec::new(neutral_choices)
                .map_err(|_| HarnessError::new(HarnessErrorClass::InvalidInput))?,
            allow_text,
        }])
    }

    fn fail_closed(&mut self, id: Value, method: &str, params: &Value) -> Result<(), HarnessError> {
        let result = fail_closed_result(method, params);
        self.transport.write(&ClientResult { id, result })
    }

    fn read_thread(&mut self) -> Result<Thread, HarnessError> {
        let thread_id = self.thread_id.clone();
        let response: ThreadResponse = self
            .rpc(
                "thread/read",
                ThreadReadParams {
                    thread_id: &thread_id,
                    include_turns: true,
                },
            )
            .map_err(RpcFailure::harness)?;
        if response.thread.id != self.thread_id {
            return Err(HarnessError::new(
                HarnessErrorClass::SessionIdentityMismatch,
            ));
        }
        self.active_turn = response
            .thread
            .turns
            .iter()
            .rev()
            .find(|turn| turn.status == "inProgress")
            .map(|turn| turn.id.clone());
        Ok(response.thread)
    }

    fn lookup_internal(
        &mut self,
        submission_id: MessageId,
        record: SubmissionRecord,
    ) -> Result<HarnessSubmissionLookup, HarnessError> {
        let client_id = message_hex(submission_id);
        let thread = self.read_thread()?;
        for turn in thread.turns {
            if turn
                .items
                .iter()
                .any(|item| item.kind == "userMessage" && item.client_id == client_id)
            {
                self.operations.insert(turn.id, record.operation_id);
                return Ok(HarnessSubmissionLookup::Accepted);
            }
        }
        Ok(HarnessSubmissionLookup::Missing)
    }

    fn cancel_pending_for_operation(
        &mut self,
        operation_id: OperationId,
    ) -> Result<bool, HarnessError> {
        let groups = self
            .request_groups
            .iter()
            .filter(|(_, group)| group.operation_id == operation_id)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for group_key in &groups {
            if let Some(group) = self.request_groups.remove(group_key) {
                self.fail_closed(group.wire_id, &group.method, &group.original)?;
            }
            let ids = self
                .pending_requests
                .iter()
                .filter(|(_, pending)| &pending.group == group_key)
                .map(|(id, _)| *id)
                .collect::<Vec<_>>();
            for id in ids {
                self.pending_requests.remove(&id);
                self.cancelled_requests.insert(id);
            }
        }
        Ok(!groups.is_empty())
    }

    fn stop_runtime(&mut self) -> Result<(), HarnessError> {
        if self.stopped {
            return Ok(());
        }
        self.intake_open = false;
        let groups = self.request_groups.keys().cloned().collect::<Vec<_>>();
        for key in groups {
            if let Some(group) = self.request_groups.remove(&key) {
                self.fail_closed(group.wire_id, &group.method, &group.original)?;
            }
        }
        self.pending_requests.clear();
        self.transport.close_input();
        if self.control.wait(self.process_grace)? == CodexWaitOutcome::Running {
            self.control.kill()?;
            if self.control.wait(self.process_grace)? == CodexWaitOutcome::Running {
                return Err(HarnessError::new(HarnessErrorClass::CleanupFailed));
            }
        }
        let deadline = Instant::now()
            .checked_add(self.process_grace)
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::CleanupFailed))?;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or(Duration::ZERO);
            match self.transport.receive(remaining) {
                TransportRead::Message(_) => {}
                TransportRead::Closed | TransportRead::Failed(_) | TransportRead::TimedOut => break,
            }
        }
        self.transport.join_reader()?;
        if let Some(stderr) = self.stderr.take() {
            stderr
                .join()
                .map_err(|_| HarnessError::new(HarnessErrorClass::CleanupFailed))?;
        }
        self.stopped = true;
        Ok(())
    }
}

impl HarnessSession for CodexSession {
    fn register_event_notifier(
        &mut self,
        notifier: hq_harness::HarnessEventNotifier,
    ) -> Result<(), HarnessError> {
        self.transport.register_event_notifier(&notifier)
    }

    fn submit(
        &mut self,
        submission: HarnessSubmission,
    ) -> Result<HarnessSubmissionOutcome, HarnessError> {
        if !self.intake_open {
            return Err(HarnessError::new(HarnessErrorClass::IntakeClosed));
        }
        if let Some(existing) = self.submissions.get(&submission.submission_id)
            && existing.digest != submission.digest
        {
            return Err(HarnessError::new(
                HarnessErrorClass::SubmissionIdentityConflict,
            ));
        }
        let record = SubmissionRecord {
            digest: submission.digest,
            operation_id: submission.operation_id,
        };
        self.submissions.insert(submission.submission_id, record);
        let client_id = message_hex(submission.submission_id);
        let body = submission.body.as_str().to_owned();
        self.pending_submission_operation = Some(submission.operation_id);
        let result = if let Some(active_turn) = self.active_turn.clone() {
            let thread_id = self.thread_id.clone();
            let response = self.rpc::<_, TurnSteerResponse>(
                "turn/steer",
                TurnSteerParams {
                    thread_id: &thread_id,
                    expected_turn_id: &active_turn,
                    input: [TextInput {
                        kind: "text",
                        text: &body,
                    }],
                    client_user_message_id: client_id.clone(),
                },
            );
            match response {
                Ok(response) if response.turn_id == active_turn => {
                    self.operations.insert(active_turn, submission.operation_id);
                    Ok(HarnessSubmissionOutcome::Accepted)
                }
                Ok(_) => Err(HarnessError::new(HarnessErrorClass::ProtocolViolation)),
                Err(RpcFailure::Rejected) => {
                    match self.lookup_internal(submission.submission_id, record)? {
                        HarnessSubmissionLookup::Accepted => Ok(HarnessSubmissionOutcome::Accepted),
                        HarnessSubmissionLookup::Missing => {
                            self.start_submission(submission.operation_id, &body, client_id)
                        }
                    }
                }
                Err(RpcFailure::Uncertain(class)) => Ok(HarnessSubmissionOutcome::Uncertain(class)),
                Err(error) => Err(error.harness()),
            }
        } else {
            self.start_submission(submission.operation_id, &body, client_id)
        };
        self.pending_submission_operation = None;
        result
    }

    fn lookup_submission(
        &mut self,
        submission: &HarnessSubmission,
    ) -> Result<HarnessSubmissionLookup, HarnessError> {
        if let Some(record) = self.submissions.get(&submission.submission_id)
            && record.digest != submission.digest
        {
            return Err(HarnessError::new(
                HarnessErrorClass::SubmissionIdentityConflict,
            ));
        }
        let record = SubmissionRecord {
            digest: submission.digest,
            operation_id: submission.operation_id,
        };
        self.submissions.insert(submission.submission_id, record);
        self.lookup_internal(submission.submission_id, record)
    }

    fn cancel_operation(
        &mut self,
        operation_id: OperationId,
    ) -> Result<HarnessCancellationOutcome, HarnessError> {
        let cancelled_request = self.cancel_pending_for_operation(operation_id)?;
        let Some(turn_id) = self.active_turn.clone() else {
            return Ok(if cancelled_request {
                HarnessCancellationOutcome::Cancelled
            } else {
                HarnessCancellationOutcome::AlreadyFinished
            });
        };
        if self.operations.get(&turn_id).copied() != Some(operation_id) {
            return Ok(if cancelled_request {
                HarnessCancellationOutcome::Cancelled
            } else {
                HarnessCancellationOutcome::AlreadyFinished
            });
        }
        let thread_id = self.thread_id.clone();
        let result: Result<Value, RpcFailure> = self.rpc(
            "turn/interrupt",
            TurnInterruptParams {
                thread_id: &thread_id,
                turn_id: &turn_id,
            },
        );
        match result {
            Ok(_) => {
                self.active_turn = None;
                Ok(HarnessCancellationOutcome::Cancelled)
            }
            Err(RpcFailure::Rejected) => Ok(HarnessCancellationOutcome::Rejected(
                HarnessErrorClass::Unavailable,
            )),
            Err(RpcFailure::Uncertain(class)) => Ok(HarnessCancellationOutcome::Uncertain(class)),
            Err(error) => Err(error.harness()),
        }
    }

    fn next_event(&mut self) -> Result<HarnessEventPoll, HarnessError> {
        if let Some(error) = self.deferred_error.take() {
            return Err(HarnessError::new(error));
        }
        if self.compatibility_failed {
            return Err(HarnessError::new(HarnessErrorClass::CompatibilityMismatch));
        }
        if let Some(event) = self.events.pop_front() {
            return Ok(HarnessEventPoll::Event(event));
        }
        loop {
            match self.transport.try_receive() {
                TransportRead::Message(message) => {
                    self.dispatch(message)?;
                }
                TransportRead::TimedOut => return Ok(HarnessEventPoll::Pending),
                TransportRead::Closed => {
                    return match self.control.wait(Duration::ZERO)? {
                        CodexWaitOutcome::ExitedSuccessfully => Ok(HarnessEventPoll::Closed),
                        CodexWaitOutcome::ExitedUnsuccessfully => {
                            Err(HarnessError::new(HarnessErrorClass::ProcessFailed))
                        }
                        CodexWaitOutcome::Running => {
                            Err(HarnessError::new(HarnessErrorClass::TransportClosed))
                        }
                    };
                }
                TransportRead::Failed(failure) => {
                    return Err(HarnessError::new(failure.class()));
                }
            }
            if let Some(error) = self.deferred_error.take() {
                return Err(HarnessError::new(error));
            }
            if self.compatibility_failed {
                return Err(HarnessError::new(HarnessErrorClass::CompatibilityMismatch));
            }
            if let Some(event) = self.events.pop_front() {
                return Ok(HarnessEventPoll::Event(event));
            }
        }
    }

    fn answer_interactive(&mut self, answer: HarnessInteractiveAnswer) -> Result<(), HarnessError> {
        if !self.intake_open {
            return Err(HarnessError::new(HarnessErrorClass::IntakeClosed));
        }
        let Some(pending) = self.pending_requests.remove(&answer.request_id) else {
            return Err(HarnessError::new(
                if self.answered_requests.contains(&answer.request_id) {
                    HarnessErrorClass::InteractiveAlreadyAnswered
                } else {
                    HarnessErrorClass::InvalidInput
                },
            ));
        };
        let value = validate_answer(&pending, answer.response)?;
        self.answered_requests.insert(answer.request_id);
        let Some(group) = self.request_groups.get_mut(&pending.group) else {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        };
        let key = pending.question_id.unwrap_or_else(|| "decision".to_owned());
        group.answers.insert(key, value);
        if group.answers.len() == group.expected {
            let group = self
                .request_groups
                .remove(&pending.group)
                .ok_or_else(|| HarnessError::new(HarnessErrorClass::InvalidInput))?;
            let result = match completed_group_result(&group) {
                Ok(result) => result,
                Err(error) => {
                    self.fail_closed(group.wire_id, &group.method, &group.original)?;
                    return Err(error);
                }
            };
            self.transport.write(&ClientResult {
                id: group.wire_id,
                result,
            })?;
        }
        Ok(())
    }

    fn stop_intake(&mut self) -> Result<(), HarnessError> {
        if !self.intake_open {
            return Ok(());
        }
        self.intake_open = false;
        let groups = self.request_groups.keys().cloned().collect::<Vec<_>>();
        for key in groups {
            if let Some(group) = self.request_groups.remove(&key) {
                self.fail_closed(group.wire_id, &group.method, &group.original)?;
            }
        }
        self.pending_requests.clear();
        Ok(())
    }

    fn drain(&mut self, wait: Duration) -> Result<HarnessDrainOutcome, HarnessError> {
        if self.events.is_empty() && self.pending_requests.is_empty() && !wait.is_zero() {
            match self.transport.receive(wait) {
                TransportRead::Message(message) => {
                    self.dispatch(message)?;
                }
                TransportRead::Failed(failure) => {
                    return Err(HarnessError::new(failure.class()));
                }
                TransportRead::TimedOut | TransportRead::Closed => {}
            }
        }
        loop {
            match self.transport.receive(Duration::ZERO) {
                TransportRead::Message(message) => {
                    self.dispatch(message)?;
                }
                TransportRead::Failed(failure) => {
                    return Err(HarnessError::new(failure.class()));
                }
                TransportRead::TimedOut | TransportRead::Closed => break,
            }
        }
        let event_count = self.events.len();
        let request_count = self.pending_requests.len();
        Ok(if event_count == 0 && request_count == 0 {
            HarnessDrainOutcome::Complete
        } else {
            HarnessDrainOutcome::Pending {
                event_count,
                request_count,
            }
        })
    }

    fn force_stop(&mut self) -> Result<(), HarnessError> {
        self.stop_runtime()
    }
}

impl CodexSession {
    fn start_submission(
        &mut self,
        operation_id: OperationId,
        body: &str,
        client_id: String,
    ) -> Result<HarnessSubmissionOutcome, HarnessError> {
        let thread_id = self.thread_id.clone();
        let response = self.rpc::<_, TurnResponse>(
            "turn/start",
            TurnStartParams {
                thread_id: &thread_id,
                input: [TextInput {
                    kind: "text",
                    text: body,
                }],
                client_user_message_id: client_id,
            },
        );
        match response {
            Ok(response) if !response.turn.id.is_empty() => {
                self.operations
                    .insert(response.turn.id.clone(), operation_id);
                if response.turn.status == "inProgress" || response.turn.status.is_empty() {
                    self.active_turn = Some(response.turn.id);
                }
                Ok(HarnessSubmissionOutcome::Accepted)
            }
            Ok(_) => Err(HarnessError::new(HarnessErrorClass::ProtocolViolation)),
            Err(RpcFailure::Rejected) => Ok(HarnessSubmissionOutcome::Rejected(
                HarnessErrorClass::Unavailable,
            )),
            Err(RpcFailure::Uncertain(class)) => Ok(HarnessSubmissionOutcome::Uncertain(class)),
            Err(error) => Err(error.harness()),
        }
    }
}

impl Drop for CodexSession {
    fn drop(&mut self) {
        let _ = self.stop_runtime();
    }
}

fn validate_launch(launch: &CodexLaunch) -> Result<(), HarnessError> {
    if launch.executable.as_os_str().is_empty()
        || launch.working_directory.as_os_str().is_empty()
        || launch.developer_instructions.is_empty()
    {
        return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
    }
    path_text(&launch.working_directory).map(|_| ())
}

fn path_text(path: &Path) -> Result<&str, HarnessError> {
    path.to_str()
        .ok_or_else(|| HarnessError::new(HarnessErrorClass::InvalidInput))
}

fn nonempty_thread(thread: Thread) -> Result<String, HarnessError> {
    if thread.id.is_empty() {
        Err(HarnessError::new(HarnessErrorClass::ProtocolViolation))
    } else {
        Ok(thread.id)
    }
}

fn message_id(message: &WireMessage) -> Option<u64> {
    message.id.as_ref().and_then(Value::as_u64)
}

fn notification_turn(method: &str, params: &Value) -> Option<(String, String, String)> {
    if !matches!(method, "turn/started" | "turn/completed") {
        return None;
    }
    let thread = string_field(params, "threadId")?;
    let turn = params.get("turn")?;
    Some((
        thread,
        string_field(turn, "id")?,
        string_field(turn, "status").unwrap_or_default(),
    ))
}

fn notification_context(params: &Value) -> Option<(String, String)> {
    let thread_id = string_field(params, "threadId")?;
    let turn_id = string_field(params, "turnId")
        .or_else(|| params.get("turn").and_then(|turn| string_field(turn, "id")))?;
    Some((thread_id, turn_id))
}

fn provider_operation_id(thread_id: &str, turn_id: &str) -> OperationId {
    let mut digest = Sha256::new();
    digest.update(b"hq.codex.operation.v1\0");
    digest.update(thread_id.as_bytes());
    digest.update(b"\0");
    digest.update(turn_id.as_bytes());
    OperationId::from_bytes(digest.finalize().into())
}

fn message_hex(id: MessageId) -> String {
    let mut output = String::with_capacity(64);
    for byte in id.as_bytes() {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn wire_group_key(id: &Value, method: &str) -> Result<String, HarnessError> {
    serde_json::to_string(&(method, id))
        .map_err(|_| HarnessError::new(HarnessErrorClass::InvalidInput))
}

fn stable_request_id(group: &str, index: usize) -> HarnessRequestId {
    let mut digest = Sha256::new();
    digest.update(b"hq.codex.request.v1\0");
    digest.update(group.as_bytes());
    digest.update(index.to_be_bytes());
    HarnessRequestId::from_bytes(digest.finalize().into())
}

fn bounded_short(value: &str) -> Result<ShortText, HarnessError> {
    ShortText::new(truncate(value, hq_domain::SHORT_TEXT_MAX_BYTES))
        .map_err(|_| HarnessError::new(HarnessErrorClass::InvalidInput))
}

fn bounded_content(value: &str) -> Result<ContentText, HarnessError> {
    ContentText::new(truncate(value, hq_domain::CONTENT_MAX_BYTES))
        .map_err(|_| HarnessError::new(HarnessErrorClass::InvalidInput))
}

fn truncate(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.to_owned();
    }
    let mut end = maximum;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn approval_prompt(label: &str, params: &Value) -> String {
    let detail = ["reason", "command", "cwd", "grantRoot"]
        .into_iter()
        .filter_map(|key| string_field(params, key))
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if detail.is_empty() {
        label.to_owned()
    } else {
        format!("{label}\n{detail}")
    }
}

fn schema_contains_secret(schema: Option<&Value>) -> bool {
    schema.is_some_and(|value| {
        let encoded = value.to_string().to_ascii_lowercase();
        encoded.contains("password") || encoded.contains("secret")
    })
}

fn valid_mcp_form_schema(schema: Option<&Value>) -> bool {
    let Some(schema) = schema.and_then(Value::as_object) else {
        return false;
    };
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return false;
    }
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return false;
    };
    if properties.len() > 64 || !properties.values().all(valid_mcp_property_schema) {
        return false;
    }
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .is_none_or(|values| {
            values.len() <= properties.len()
                && values.iter().all(|value| {
                    value
                        .as_str()
                        .is_some_and(|name| properties.contains_key(name))
                })
        });
    let additional = schema
        .get("additionalProperties")
        .and_then(Value::as_bool)
        .is_none_or(|allowed| !allowed);
    required && additional
}

fn valid_mcp_property_schema(value: &Value) -> bool {
    let Some(schema) = value.as_object() else {
        return false;
    };
    matches!(
        schema.get("type").and_then(Value::as_str),
        Some("string" | "number" | "integer" | "boolean")
    ) && schema
        .get("enum")
        .and_then(Value::as_array)
        .is_none_or(|values| !values.is_empty() && values.len() <= 64)
}

fn valid_mcp_form_answer(schema: Option<&Value>, answer: &Value) -> bool {
    if !valid_mcp_form_schema(schema) {
        return false;
    }
    let Some(schema) = schema.and_then(Value::as_object) else {
        return false;
    };
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return false;
    };
    let Some(answer) = answer.as_object() else {
        return false;
    };
    if !answer.keys().all(|name| properties.contains_key(name)) {
        return false;
    }
    if schema
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|required| {
            required
                .iter()
                .filter_map(Value::as_str)
                .any(|name| !answer.contains_key(name))
        })
    {
        return false;
    }
    answer.iter().all(|(name, value)| {
        properties
            .get(name)
            .is_some_and(|property| valid_mcp_property_value(property, value))
    })
}

fn valid_mcp_property_value(schema: &Value, value: &Value) -> bool {
    let expected = schema.get("type").and_then(Value::as_str);
    let typed = match expected {
        Some("string") => value.is_string(),
        Some("number") => value.is_number(),
        Some("integer") => value.as_i64().is_some() || value.as_u64().is_some(),
        Some("boolean") => value.is_boolean(),
        _ => false,
    };
    typed
        && schema
            .get("enum")
            .and_then(Value::as_array)
            .is_none_or(|choices| choices.contains(value))
}

fn fail_closed_result(method: &str, params: &Value) -> Value {
    match method {
        "item/tool/requestUserInput" => json!({"answers": {}}),
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            json!({"decision": "cancel"})
        }
        "item/permissions/requestApproval" => json!({"permissions": {}, "scope": "turn"}),
        "mcpServer/elicitation/request" => json!({"action": "cancel", "content": null}),
        _ => {
            let _ = params;
            Value::Null
        }
    }
}

fn validate_answer(
    pending: &PendingRequest,
    response: HarnessInteractiveResponse,
) -> Result<Value, HarnessError> {
    match (pending.kind, response) {
        (_, HarnessInteractiveResponse::Cancelled) => Ok(Value::String("cancel".to_owned())),
        (_, HarnessInteractiveResponse::Choice(value))
            if pending.choices.contains(value.as_str()) =>
        {
            Ok(Value::String(value.into_string()))
        }
        (PendingKind::Question, HarnessInteractiveResponse::Text(value)) if pending.allow_text => {
            Ok(Value::String(value.into_string()))
        }
        (
            PendingKind::Approval | PendingKind::McpUrl,
            HarnessInteractiveResponse::Approval(value),
        ) => Ok(Value::String(
            if value { "accept" } else { "decline" }.to_owned(),
        )),
        (PendingKind::Permission, HarnessInteractiveResponse::Approval(value)) => Ok(
            Value::String(if value { "grantTurn" } else { "decline" }.to_owned()),
        ),
        (PendingKind::McpForm, HarnessInteractiveResponse::Text(value)) => {
            let parsed: Value = serde_json::from_str(value.as_str())
                .map_err(|_| HarnessError::new(HarnessErrorClass::InvalidInput))?;
            if !parsed.is_object() {
                return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
            }
            Ok(parsed)
        }
        _ => Err(HarnessError::new(HarnessErrorClass::InvalidInput)),
    }
}

fn completed_group_result(group: &PendingGroup) -> Result<Value, HarnessError> {
    match group.method.as_str() {
        "item/tool/requestUserInput" => Ok(json!({
            "answers": group
                .answers
                .iter()
                .map(|(key, value)| (key.clone(), json!({"answers": [value]})))
                .collect::<serde_json::Map<String, Value>>()
        })),
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => Ok(json!({
            "decision": group.answers.get("decision").and_then(Value::as_str).unwrap_or("cancel")
        })),
        "item/permissions/requestApproval" => {
            let decision = group
                .answers
                .get("decision")
                .and_then(Value::as_str)
                .unwrap_or("decline");
            if decision == "decline" || decision == "cancel" {
                Ok(json!({"permissions": {}, "scope": "turn"}))
            } else {
                Ok(json!({
                    "permissions": group.original.get("permissions").cloned().unwrap_or_else(|| json!({})),
                    "scope": if decision == "grantSession" { "session" } else { "turn" }
                }))
            }
        }
        "mcpServer/elicitation/request" => {
            let mode = string_field(&group.original, "mode").unwrap_or_default();
            let answer = group
                .answers
                .get("decision")
                .cloned()
                .unwrap_or(Value::Null);
            if answer.as_str() == Some("cancel") {
                Ok(json!({"action": "cancel", "content": null}))
            } else if mode == "form"
                && valid_mcp_form_answer(group.original.get("requestedSchema"), &answer)
            {
                Ok(json!({"action": "accept", "content": answer}))
            } else if mode == "form" {
                Err(HarnessError::new(HarnessErrorClass::InvalidInput))
            } else {
                Ok(json!({
                    "action": answer.as_str().unwrap_or("cancel"),
                    "content": null
                }))
            }
        }
        _ => Err(HarnessError::new(HarnessErrorClass::InvalidInput)),
    }
}

fn spawn_stderr_drain(
    errors: Box<dyn Read + Send>,
    sink: Arc<dyn CodexDiagnosticSink>,
) -> Result<JoinHandle<()>, HarnessError> {
    thread::Builder::new()
        .name("hq-codex-stderr".to_owned())
        .spawn(move || drain_stderr(errors, &*sink))
        .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))
}

fn drain_stderr(mut errors: Box<dyn Read + Send>, sink: &dyn CodexDiagnosticSink) {
    let mut line = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 4096];
    loop {
        let count = match errors.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        for byte in &chunk[..count] {
            if *byte == b'\n' {
                emit_diagnostic(&line, sink);
                line.clear();
            } else if line.len() < STDERR_LINE_BYTES {
                line.push(*byte);
            }
        }
    }
    if !line.is_empty() {
        emit_diagnostic(&line, sink);
    }
}

fn emit_diagnostic(line: &[u8], sink: &dyn CodexDiagnosticSink) {
    if let Ok(line) = std::str::from_utf8(line) {
        sink.line(line);
    }
}

const _: &str = CODEX_BASELINE_VERSION;
