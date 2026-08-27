use std::{fs, path::PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{CODEX_BASELINE_VERSION, protocol::WireMessage};

#[test]
fn pinned_schema_manifest_matches_exact_bundles() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/schema")
        .join(format!("v{CODEX_BASELINE_VERSION}"));
    let manifest: Value = serde_json::from_slice(&fs::read(root.join("manifest.json"))?)?;
    assert_eq!(
        manifest.get("version").and_then(Value::as_str),
        Some(CODEX_BASELINE_VERSION)
    );
    for file in [
        "codex_app_server_protocol.schemas.json",
        "codex_app_server_protocol.v2.schemas.json",
    ] {
        let bytes = fs::read(root.join(file))?;
        let actual = hex_digest(&bytes);
        let expected = manifest
            .get("artifacts")
            .and_then(|files| files.get(file))
            .and_then(Value::as_str);
        assert_eq!(expected, Some(actual.as_str()));
    }
    Ok(())
}

#[test]
fn pinned_schema_contains_every_consumed_method() -> Result<(), Box<dyn std::error::Error>> {
    let schema = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/schema")
            .join(format!("v{CODEX_BASELINE_VERSION}"))
            .join("codex_app_server_protocol.schemas.json"),
    )?;
    for method in [
        "initialize",
        "initialized",
        "thread/start",
        "thread/resume",
        "thread/read",
        "turn/start",
        "turn/steer",
        "turn/interrupt",
        "turn/started",
        "turn/completed",
        "turn/plan/updated",
        "turn/diff/updated",
        "item/completed",
        "item/tool/requestUserInput",
        "item/commandExecution/requestApproval",
        "item/fileChange/requestApproval",
        "item/permissions/requestApproval",
        "mcpServer/elicitation/request",
    ] {
        assert!(
            schema.contains(&format!("\"{method}\"")),
            "missing {method}"
        );
    }
    Ok(())
}

#[test]
fn representative_wire_fixtures_decode_with_additive_tolerance()
-> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(format!("v{CODEX_BASELINE_VERSION}"));
    for file in [
        "initialize-request.json",
        "thread-start-response.json",
        "turn-start-response.json",
        "thread-read-accepted.json",
        "item-completed-agent-message.json",
        "request-user-input.json",
    ] {
        let mut value: Value = serde_json::from_slice(&fs::read(root.join(file))?)?;
        value
            .as_object_mut()
            .ok_or("fixture envelope was not an object")?
            .insert("futureAdditiveField".to_owned(), Value::Bool(true));
        let _: WireMessage = serde_json::from_value(value)?;
    }
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(unix)]
mod adapter {
    use std::{
        collections::{BTreeSet, VecDeque},
        io::{self, BufRead, BufReader, Cursor, Write},
        num::NonZeroU64,
        os::unix::{ffi::OsStrExt, net::UnixStream},
        path::PathBuf,
        process::Command,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    use hq_domain::{
        ActivityKind, ActivityStatus, AgentId, CommandDigest, ContentText, MessageId, OperationId,
        ProviderId, ProviderSessionId, ShortText,
    };
    use hq_harness::{
        HarnessActivity, HarnessCancellationOutcome, HarnessCapabilities, HarnessCapability,
        HarnessDrainOutcome, HarnessError, HarnessErrorClass, HarnessEvent, HarnessEventPoll,
        HarnessFactory, HarnessInstance, HarnessInstanceRequest, HarnessInteractiveAnswer,
        HarnessInteractiveResponse, HarnessOutput, HarnessOutputKind, HarnessSession,
        HarnessSessionRequest, HarnessSubmission, HarnessSubmissionLookup,
        HarnessSubmissionOutcome, OpenedHarnessSession,
    };
    use hq_testkit::{
        HarnessConformanceFailure, HarnessConformanceFixture, HarnessConformanceObservation,
        HarnessConformanceScenario, HarnessConformanceSubject, HarnessConformanceTrace,
        run_harness_conformance,
    };
    use serde::{Serialize, de::DeserializeOwned};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    use crate::{
        CodexDiagnosticSink, CodexFactory, CodexFactoryConfig, CodexLaunch, CodexProcessControl,
        CodexProcessPipes, CodexProcessStarter, CodexWaitOutcome, ExecCodexProcessStarter,
        FixedCodexLaunchResolver,
    };
    use crate::{
        protocol::{
            ClientInfo, ClientNotification, ClientRequest, InitializeCapabilities,
            InitializeParams, ThreadResponse, ThreadResumeParams, ThreadStartParams,
        },
        transport::{JsonlTransport, TransportRead},
    };

    #[derive(Clone)]
    #[allow(clippy::struct_excessive_bools)]
    struct ServerSpec {
        thread_id: String,
        resume_ack: Option<String>,
        lose_turn_response: bool,
        history_accepts: bool,
        emit_events: bool,
        emit_after_read: bool,
        initialize_error: bool,
        late_response_on_initialize: bool,
        resume_error: bool,
        post_open: Vec<Value>,
        corrupt_after_open: bool,
        stderr: Vec<u8>,
    }

    impl Default for ServerSpec {
        fn default() -> Self {
            Self {
                thread_id: "thr-test".to_owned(),
                resume_ack: None,
                lose_turn_response: false,
                history_accepts: false,
                emit_events: false,
                emit_after_read: false,
                initialize_error: false,
                late_response_on_initialize: false,
                resume_error: false,
                post_open: Vec::new(),
                corrupt_after_open: false,
                stderr: Vec::new(),
            }
        }
    }

    struct FakeStarter {
        specs: Mutex<VecDeque<ServerSpec>>,
        observed: Arc<Mutex<Vec<Value>>>,
        controls: Mutex<Vec<Arc<FakeControl>>>,
    }

    impl FakeStarter {
        fn new(specs: impl IntoIterator<Item = ServerSpec>) -> Self {
            Self {
                specs: Mutex::new(specs.into_iter().collect()),
                observed: Arc::new(Mutex::new(Vec::new())),
                controls: Mutex::new(Vec::new()),
            }
        }
    }

    impl CodexProcessStarter for FakeStarter {
        fn start(
            &self,
            _launch: &CodexLaunch,
            _environment: &hq_harness::HarnessEnvironment,
        ) -> Result<CodexProcessPipes, HarnessError> {
            let spec = self
                .specs
                .lock()
                .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?
                .pop_front()
                .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?;
            let stderr = spec.stderr.clone();
            let (client_input, server_input) = UnixStream::pair()
                .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?;
            let (server_output, client_output) = UnixStream::pair()
                .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?;
            let control = Arc::new(FakeControl::default());
            self.controls
                .lock()
                .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?
                .push(Arc::clone(&control));
            let observed = Arc::clone(&self.observed);
            let server_control = Arc::clone(&control);
            thread::Builder::new()
                .name("hq-codex-fake-server".to_owned())
                .spawn(move || {
                    run_server(spec, server_input, server_output, &observed);
                    server_control.exited.store(true, Ordering::SeqCst);
                })
                .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?;
            Ok(CodexProcessPipes {
                input: Box::new(client_input),
                output: Box::new(client_output),
                errors: Box::new(Cursor::new(stderr)),
                control,
            })
        }
    }

    #[derive(Default)]
    struct FakeControl {
        exited: AtomicBool,
        kills: AtomicUsize,
    }

    impl CodexProcessControl for FakeControl {
        fn wait(&self, wait: Duration) -> Result<CodexWaitOutcome, HarnessError> {
            let deadline = Instant::now()
                .checked_add(wait)
                .ok_or_else(|| HarnessError::new(HarnessErrorClass::InvalidInput))?;
            loop {
                if self.exited.load(Ordering::SeqCst) {
                    return Ok(CodexWaitOutcome::ExitedSuccessfully);
                }
                if Instant::now() >= deadline {
                    return Ok(CodexWaitOutcome::Running);
                }
                thread::yield_now();
            }
        }

        fn kill(&self) -> Result<(), HarnessError> {
            self.kills.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingSink {
        lines: Mutex<Vec<String>>,
    }

    impl CodexDiagnosticSink for RecordingSink {
        fn line(&self, line: &str) {
            if let Ok(mut lines) = self.lines.lock() {
                lines.push(line.to_owned());
            }
        }
    }

    #[test]
    fn start_submit_normalize_and_answer_request() -> Result<(), Box<dyn std::error::Error>> {
        let spec = ServerSpec {
            emit_events: true,
            stderr: b"private diagnostic\n".to_vec(),
            ..ServerSpec::default()
        };
        let starter = Arc::new(FakeStarter::new([spec]));
        let sink = Arc::new(RecordingSink::default());
        let factory = factory(Arc::clone(&starter), Arc::clone(&sink))?;
        let instance = factory.create_instance(instance_request())?;
        let mut opened = instance.open_session(HarnessSessionRequest::Start)?;
        assert_eq!(opened.session_id.as_str(), "thr-test");
        let submission = submission()?;
        assert_eq!(
            opened.session.submit(submission.clone())?,
            HarnessSubmissionOutcome::Accepted
        );

        assert!(matches!(
            opened.session.poll_event(Duration::from_secs(1))?,
            HarnessEventPoll::Event(HarnessEvent::Activity(_))
        ));
        let output = opened.session.poll_event(Duration::from_secs(1))?;
        assert!(matches!(
            output,
            HarnessEventPoll::Event(HarnessEvent::Output(ref output))
                if output.kind == HarnessOutputKind::FinalAnswer
                    && output.operation_id == submission.operation_id
                    && output.body.as_str() == "finished"
        ));
        let HarnessEventPoll::Event(HarnessEvent::InteractiveRequest(request)) =
            opened.session.poll_event(Duration::from_secs(1))?
        else {
            return Err("expected interactive request".into());
        };
        opened
            .session
            .answer_interactive(HarnessInteractiveAnswer {
                request_id: request.request_id,
                response: HarnessInteractiveResponse::Choice(hq_domain::ShortText::new("approve")?),
            })?;
        let duplicate = opened
            .session
            .answer_interactive(HarnessInteractiveAnswer {
                request_id: request.request_id,
                response: HarnessInteractiveResponse::Cancelled,
            })
            .err()
            .ok_or("duplicate answer was accepted")?;
        assert_eq!(
            duplicate.class,
            HarnessErrorClass::InteractiveAlreadyAnswered
        );
        wait_for_methodless_response(&starter.observed, 40)?;
        opened.session.force_stop()?;
        assert!(
            sink.lines
                .lock()
                .map_err(|_| "diagnostic sink was poisoned")?
                .iter()
                .all(|line| line == "private diagnostic")
        );
        Ok(())
    }

    #[test]
    fn late_unrelated_response_cannot_satisfy_a_new_call() -> Result<(), Box<dyn std::error::Error>>
    {
        let spec = ServerSpec {
            late_response_on_initialize: true,
            ..ServerSpec::default()
        };
        let factory = factory(
            Arc::new(FakeStarter::new([spec])),
            Arc::new(RecordingSink::default()),
        )?;
        let mut opened = factory
            .create_instance(instance_request())?
            .open_session(HarnessSessionRequest::Start)?;
        assert_eq!(opened.session_id.as_str(), "thr-test");
        opened.session.force_stop()?;
        Ok(())
    }

    #[test]
    fn active_submission_uses_exact_turn_steering() -> Result<(), Box<dyn std::error::Error>> {
        let starter = Arc::new(FakeStarter::new([ServerSpec::default()]));
        let factory = factory(Arc::clone(&starter), Arc::new(RecordingSink::default()))?;
        let mut opened = factory
            .create_instance(instance_request())?
            .open_session(HarnessSessionRequest::Start)?;
        assert_eq!(
            opened.session.submit(submission()?)?,
            HarnessSubmissionOutcome::Accepted
        );
        let steered = HarnessSubmission {
            submission_id: MessageId::from_bytes([7; 32]),
            digest: CommandDigest::from_bytes([8; 32]),
            operation_id: OperationId::from_bytes([9; 32]),
            body: ContentText::new("steered input")?,
        };
        assert_eq!(
            opened.session.submit(steered)?,
            HarnessSubmissionOutcome::Accepted
        );
        let observed = starter
            .observed
            .lock()
            .map_err(|_| "observed request list was poisoned")?;
        assert_eq!(count_method(&observed, "turn/start"), 1);
        assert_eq!(count_method(&observed, "turn/steer"), 1);
        assert!(observed.iter().any(|value| {
            value.get("method").and_then(Value::as_str) == Some("turn/steer")
                && value
                    .get("params")
                    .and_then(|params| params.get("expectedTurnId"))
                    .and_then(Value::as_str)
                    == Some("turn-test")
        }));
        drop(observed);
        opened.session.force_stop()?;
        Ok(())
    }

    #[test]
    fn response_loss_reconciles_by_stable_client_identity() -> Result<(), Box<dyn std::error::Error>>
    {
        let spec = ServerSpec {
            lose_turn_response: true,
            history_accepts: true,
            ..ServerSpec::default()
        };
        let starter = Arc::new(FakeStarter::new([spec]));
        let sink = Arc::new(RecordingSink::default());
        let factory = factory(Arc::clone(&starter), sink)?;
        let mut opened = factory
            .create_instance(instance_request())?
            .open_session(HarnessSessionRequest::Start)?;
        let submission = submission()?;
        assert!(matches!(
            opened.session.submit(submission.clone())?,
            HarnessSubmissionOutcome::Uncertain(HarnessErrorClass::Unavailable)
        ));
        assert_eq!(
            opened.session.lookup_submission(&submission)?,
            HarnessSubmissionLookup::Accepted
        );
        let observed = starter
            .observed
            .lock()
            .map_err(|_| "observed request list was poisoned")?;
        assert_eq!(count_method(&observed, "turn/start"), 1);
        assert_eq!(count_method(&observed, "thread/read"), 1);
        drop(observed);
        opened.session.force_stop()?;
        Ok(())
    }

    #[test]
    fn fresh_adapter_lookup_restores_durable_operation_correlation()
    -> Result<(), Box<dyn std::error::Error>> {
        let spec = ServerSpec {
            history_accepts: true,
            emit_after_read: true,
            ..ServerSpec::default()
        };
        let starter = Arc::new(FakeStarter::new([spec]));
        let factory = factory(starter, Arc::new(RecordingSink::default()))?;
        let mut opened = factory
            .create_instance(instance_request())?
            .open_session(HarnessSessionRequest::Start)?;
        let submission = submission()?;
        assert_eq!(
            opened.session.lookup_submission(&submission)?,
            HarnessSubmissionLookup::Accepted
        );
        assert!(matches!(
            opened.session.poll_event(Duration::from_secs(1))?,
            HarnessEventPoll::Event(HarnessEvent::Output(ref output))
                if output.operation_id == submission.operation_id
                    && output.body.as_str() == "recovered output"
        ));
        opened.session.force_stop()?;
        Ok(())
    }

    #[test]
    fn approval_permission_and_mcp_requests_answer_with_closed_shapes()
    -> Result<(), Box<dyn std::error::Error>> {
        let spec = ServerSpec {
            post_open: supported_request_frames(),
            ..ServerSpec::default()
        };
        let starter = Arc::new(FakeStarter::new([spec]));
        let factory = factory(Arc::clone(&starter), Arc::new(RecordingSink::default()))?;
        let mut opened = factory
            .create_instance(instance_request())?
            .open_session(HarnessSessionRequest::Start)?;
        for (id, response) in [
            (70, HarnessInteractiveResponse::Approval(true)),
            (
                71,
                HarnessInteractiveResponse::Choice(ShortText::new("acceptForSession")?),
            ),
            (
                72,
                HarnessInteractiveResponse::Choice(ShortText::new("grantSession")?),
            ),
            (
                73,
                HarnessInteractiveResponse::Choice(ShortText::new("decline")?),
            ),
            (
                74,
                HarnessInteractiveResponse::Text(ContentText::new("{\"name\":\"HQ\"}")?),
            ),
        ] {
            let HarnessEventPoll::Event(HarnessEvent::InteractiveRequest(request)) =
                opened.session.poll_event(Duration::from_secs(1))?
            else {
                return Err(format!("expected interactive request {id}").into());
            };
            opened
                .session
                .answer_interactive(HarnessInteractiveAnswer {
                    request_id: request.request_id,
                    response,
                })?;
            wait_for_methodless_response(&starter.observed, id)?;
        }
        let observed = starter
            .observed
            .lock()
            .map_err(|_| "observed request list was poisoned")?;
        assert_eq!(
            response_result(&observed, 70),
            Some(&json!({"decision":"accept"}))
        );
        assert_eq!(
            response_result(&observed, 71),
            Some(&json!({"decision":"acceptForSession"}))
        );
        assert_eq!(
            response_result(&observed, 72),
            Some(&json!({"permissions":{"network":true},"scope":"session"}))
        );
        assert_eq!(
            response_result(&observed, 73),
            Some(&json!({"action":"decline","content":null}))
        );
        assert_eq!(
            response_result(&observed, 74),
            Some(&json!({"action":"accept","content":{"name":"HQ"}}))
        );
        drop(observed);
        opened.session.force_stop()?;
        Ok(())
    }

    #[test]
    fn unknown_server_request_fails_the_compatibility_boundary()
    -> Result<(), Box<dyn std::error::Error>> {
        let spec = ServerSpec {
            post_open: vec![json!({
                "id": 99,
                "method": "future/authorityRequest",
                "params": {"threadId":"thr-test","turnId":"turn-open"}
            })],
            ..ServerSpec::default()
        };
        let starter = Arc::new(FakeStarter::new([spec]));
        let factory = factory(Arc::clone(&starter), Arc::new(RecordingSink::default()))?;
        let mut opened = factory
            .create_instance(instance_request())?
            .open_session(HarnessSessionRequest::Start)?;
        let error = opened
            .session
            .poll_event(Duration::from_secs(1))
            .err()
            .ok_or("unknown server request was ignored")?;
        assert_eq!(error.class, HarnessErrorClass::CompatibilityMismatch);
        wait_for_methodless_response(&starter.observed, 99)?;
        assert!(
            starter
                .observed
                .lock()
                .map_err(|_| "observed request list was poisoned")?
                .iter()
                .any(|value| {
                    value.get("id").and_then(Value::as_u64) == Some(99)
                        && value
                            .get("error")
                            .and_then(|error| error.get("code"))
                            .and_then(Value::as_i64)
                            == Some(-32601)
                })
        );
        opened.session.force_stop()?;
        Ok(())
    }

    #[test]
    fn exact_resume_identity_is_enforced() -> Result<(), Box<dyn std::error::Error>> {
        let matching = ServerSpec {
            resume_ack: Some("existing-thread".to_owned()),
            ..ServerSpec::default()
        };
        let mismatching = ServerSpec {
            resume_ack: Some("different-thread".to_owned()),
            ..ServerSpec::default()
        };
        let starter = Arc::new(FakeStarter::new([matching, mismatching]));
        let sink = Arc::new(RecordingSink::default());
        let factory = factory(Arc::clone(&starter), sink)?;
        let requested = ProviderSessionId::new("existing-thread")?;
        let mut opened = factory.create_instance(instance_request())?.open_session(
            HarnessSessionRequest::Resume {
                session_id: requested.clone(),
            },
        )?;
        assert_eq!(opened.session_id, requested);
        opened.session.force_stop()?;

        let error = factory
            .create_instance(instance_request())?
            .open_session(HarnessSessionRequest::Resume {
                session_id: ProviderSessionId::new("existing-thread")?,
            })
            .err()
            .ok_or("mismatched resume was accepted")?;
        assert_eq!(error.class, HarnessErrorClass::SessionIdentityMismatch);
        Ok(())
    }

    #[test]
    fn provider_diagnostics_never_enter_neutral_errors() -> Result<(), Box<dyn std::error::Error>> {
        let spec = ServerSpec {
            initialize_error: true,
            stderr: b"stderr-secret\n".to_vec(),
            ..ServerSpec::default()
        };
        let starter = Arc::new(FakeStarter::new([spec]));
        let sink = Arc::new(RecordingSink::default());
        let factory = factory(starter, Arc::clone(&sink))?;
        let error = factory
            .create_instance(instance_request())?
            .open_session(HarnessSessionRequest::Start)
            .err()
            .ok_or("initialize error was accepted")?;
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("stderr-secret"));
        assert!(!rendered.contains("provider-secret"));
        Ok(())
    }

    #[test]
    #[ignore = "starts an authenticated installed Codex thread only when explicitly requested"]
    fn installed_codex_smoke() -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var("HQ_CODEX_INSTALLED_SMOKE").as_deref() != Ok("1") {
            return Err("set HQ_CODEX_INSTALLED_SMOKE=1 to authorize the installed smoke".into());
        }
        let executable =
            std::env::var_os("CODEX_BIN").map_or_else(|| PathBuf::from("codex"), PathBuf::from);
        let version = Command::new(&executable).arg("--version").output()?;
        if !version.status.success()
            || std::str::from_utf8(&version.stdout)?.trim() != "codex-cli 0.150.1"
        {
            return Err("installed Codex version does not match pinned 0.150.1".into());
        }
        let launch = CodexLaunch {
            executable,
            working_directory: std::env::current_dir()?,
            developer_instructions: "HQ installed adapter smoke; do not execute a turn.".to_owned(),
            model: None,
            permissive: false,
        };
        let cwd = launch
            .working_directory
            .to_str()
            .ok_or("installed smoke working directory is not UTF-8")?;
        let mut start_connection = installed_connection(&launch)?;
        let started: ThreadResponse = installed_call(
            &mut start_connection.transport,
            2,
            "thread/start",
            ThreadStartParams {
                cwd,
                developer_instructions: &launch.developer_instructions,
                model: None,
                approval_policy: Some("never"),
                sandbox: Some("danger-full-access"),
            },
        )?;
        if started.thread.id.is_empty() {
            return Err("installed Codex returned an empty started thread".into());
        }
        close_installed_connection(start_connection)?;
        let resume_id = match std::env::var("HQ_CODEX_SMOKE_RESUME_SESSION") {
            Ok(resume_id) => resume_id,
            Err(_) => persisted_resume_thread()?,
        };
        if resume_id.is_empty() {
            return Err("installed smoke resume identity is empty".into());
        }
        let mut resume_connection = installed_connection(&launch)?;
        let resumed: ThreadResponse = installed_call(
            &mut resume_connection.transport,
            2,
            "thread/resume",
            ThreadResumeParams {
                thread_id: &resume_id,
                cwd,
                model: None,
                approval_policy: Some("never"),
                sandbox: Some("danger-full-access"),
            },
        )?;
        if resumed.thread.id != resume_id {
            return Err("installed Codex changed the resumed thread identity".into());
        }
        close_installed_connection(resume_connection)?;
        Ok(())
    }

    struct InstalledConnection {
        transport: JsonlTransport,
        control: Arc<dyn CodexProcessControl>,
        stderr: thread::JoinHandle<()>,
    }

    fn installed_connection(
        launch: &CodexLaunch,
    ) -> Result<InstalledConnection, Box<dyn std::error::Error>> {
        let pipes = ExecCodexProcessStarter.start(launch, &copied_process_environment()?)?;
        let CodexProcessPipes {
            input,
            output,
            errors,
            control,
        } = pipes;
        let stderr = thread::spawn(move || {
            let mut errors = errors;
            let _ = io::copy(&mut errors, &mut io::sink());
        });
        let mut transport = JsonlTransport::start(output, 64)?;
        transport.bind_input(input);
        let _: Value = installed_call(
            &mut transport,
            1,
            "initialize",
            InitializeParams {
                client_info: ClientInfo {
                    name: "hq-smoke",
                    title: "HQ Codex adapter smoke",
                    version: "0.150.1",
                },
                capabilities: InitializeCapabilities {
                    experimental_api: true,
                },
            },
        )?;
        transport.write(&ClientNotification {
            method: "initialized",
            params: Value::Null,
        })?;
        Ok(InstalledConnection {
            transport,
            control,
            stderr,
        })
    }

    fn close_installed_connection(
        mut connection: InstalledConnection,
    ) -> Result<(), Box<dyn std::error::Error>> {
        connection.transport.close_input();
        if connection.control.wait(Duration::from_secs(3))? == CodexWaitOutcome::Running {
            connection.control.kill()?;
            let _ = connection.control.wait(Duration::from_secs(3))?;
        }
        while let TransportRead::Message(_) =
            connection.transport.receive(Duration::from_millis(20))
        {}
        connection.transport.join_reader()?;
        connection
            .stderr
            .join()
            .map_err(|_| "stderr drain thread failed")?;
        Ok(())
    }

    fn installed_call<T: Serialize, R: DeserializeOwned>(
        transport: &mut JsonlTransport,
        id: u64,
        method: &str,
        params: T,
    ) -> Result<R, Box<dyn std::error::Error>> {
        transport.write(&ClientRequest { method, id, params })?;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or("installed smoke deadline overflow")?;
        loop {
            let wait = deadline
                .checked_duration_since(Instant::now())
                .ok_or("installed Codex call timed out")?;
            match transport.receive(wait) {
                TransportRead::Message(message)
                    if message.id.as_ref().and_then(Value::as_u64) == Some(id) =>
                {
                    if let Some(error) = message.error {
                        return Err(format!(
                            "installed Codex returned protocol error code {}",
                            error.code
                        )
                        .into());
                    }
                    return serde_json::from_value(
                        message.result.ok_or("installed response omitted result")?,
                    )
                    .map_err(Into::into);
                }
                TransportRead::Message(_) => {}
                TransportRead::TimedOut => return Err("installed Codex call timed out".into()),
                TransportRead::Closed => return Err("installed Codex transport closed".into()),
                TransportRead::Failed(_) => {
                    return Err("installed Codex transport failed".into());
                }
            }
        }
    }

    fn copied_process_environment()
    -> Result<hq_harness::HarnessEnvironment, Box<dyn std::error::Error>> {
        let mut entries = Vec::new();
        for (name, value) in std::env::vars_os() {
            let name = name
                .to_str()
                .ok_or("installed smoke cannot copy a non-UTF-8 environment name")?;
            entries.push((name.to_owned(), value.as_bytes().to_vec()));
        }
        hq_harness::HarnessEnvironment::copy_from(
            entries
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_slice())),
        )
        .map_err(Into::into)
    }

    fn persisted_resume_thread() -> Result<String, Box<dyn std::error::Error>> {
        let sessions = std::env::var_os("CODEX_HOME").map_or_else(
            || {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".codex/sessions"))
            },
            |home| Some(PathBuf::from(home).join("sessions")),
        );
        let sessions = sessions.ok_or("cannot locate installed Codex sessions")?;
        let mut rollouts = Vec::new();
        collect_rollouts(&sessions, 0, &mut rollouts)?;
        rollouts.sort_unstable_by(|left, right| right.cmp(left));
        let current = std::env::var("CODEX_THREAD_ID").ok();
        rollouts
            .iter()
            .filter_map(|path| path.file_stem().and_then(|name| name.to_str()))
            .filter_map(rollout_thread_id)
            .find(|thread_id| current.as_deref() != Some(thread_id.as_str()))
            .ok_or_else(|| "set HQ_CODEX_SMOKE_RESUME_SESSION to a persisted thread".into())
    }

    fn collect_rollouts(
        directory: &std::path::Path,
        depth: usize,
        rollouts: &mut Vec<PathBuf>,
    ) -> Result<(), std::io::Error> {
        if depth > 6 || rollouts.len() >= 4096 {
            return Ok(());
        }
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                collect_rollouts(&entry.path(), depth + 1, rollouts)?;
            } else if file_type.is_file()
                && entry.path().extension().and_then(|value| value.to_str()) == Some("jsonl")
            {
                rollouts.push(entry.path());
            }
            if rollouts.len() >= 4096 {
                break;
            }
        }
        Ok(())
    }

    fn rollout_thread_id(stem: &str) -> Option<String> {
        let start = stem.len().checked_sub(36)?;
        let candidate = &stem[start..];
        let valid = candidate.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        });
        valid.then(|| candidate.to_owned())
    }

    #[test]
    fn real_adapter_seam_passes_reusable_harness_conformance()
    -> Result<(), Box<dyn std::error::Error>> {
        let report = run_harness_conformance(&CodexConformanceSubject)?;
        assert_eq!(report.scenarios, HarnessConformanceScenario::ALL);
        Ok(())
    }

    struct CodexConformanceSubject;

    impl HarnessConformanceSubject for CodexConformanceSubject {
        fn fixture(
            &self,
            scenario: HarnessConformanceScenario,
        ) -> Result<HarnessConformanceFixture, HarnessConformanceFailure> {
            let starter = Arc::new(FakeStarter::new(conformance_specs(scenario)));
            let inner = factory(starter, Arc::new(RecordingSink::default()))
                .map_err(|_| conformance_failure(scenario))?;
            let state = Arc::new(Mutex::new(Vec::new()));
            let capabilities = if scenario == HarnessConformanceScenario::UnsafeRegistration {
                HarnessCapabilities {
                    supported: BTreeSet::from([
                        HarnessCapability::StartSessions,
                        HarnessCapability::ResumeSessions,
                    ]),
                }
            } else {
                CodexFactory::capabilities()
            };
            Ok(HarnessConformanceFixture {
                provider_id: ProviderId::new("codex-conformance")
                    .map_err(|_| conformance_failure(scenario))?,
                capabilities,
                factory: Arc::new(TracingFactory {
                    inner,
                    state: Arc::clone(&state),
                }),
                trace: Arc::new(TracingTrace { scenario, state }),
                expected_output_activity: expected_codex_events(scenario)?,
            })
        }
    }

    fn conformance_specs(scenario: HarnessConformanceScenario) -> Vec<ServerSpec> {
        let mut spec = ServerSpec {
            thread_id: "scripted-session".to_owned(),
            ..ServerSpec::default()
        };
        match scenario {
            HarnessConformanceScenario::UnsafeRegistration => return Vec::new(),
            HarnessConformanceScenario::ResumedSession => {
                spec.resume_ack = Some("scripted-session".to_owned());
            }
            HarnessConformanceScenario::MissingResume => spec.resume_error = true,
            HarnessConformanceScenario::MismatchedResume => {
                spec.resume_ack = Some("different-session".to_owned());
            }
            HarnessConformanceScenario::ResponseLossAccepted => {
                spec.lose_turn_response = true;
                spec.history_accepts = true;
            }
            HarnessConformanceScenario::ResponseLossMissingRetry
            | HarnessConformanceScenario::ActiveOperationRace => {
                spec.lose_turn_response = true;
            }
            HarnessConformanceScenario::InteractiveRequest => {
                spec.post_open = vec![question_frame(50, false), question_frame(51, false)];
            }
            HarnessConformanceScenario::SecretRequestRejection => {
                spec.post_open = vec![question_frame(52, true)];
            }
            HarnessConformanceScenario::OutputActivityOrder => spec.post_open = output_frames(),
            HarnessConformanceScenario::CrashIsolation => {
                let first = ServerSpec {
                    thread_id: "scripted-session".to_owned(),
                    corrupt_after_open: true,
                    ..ServerSpec::default()
                };
                return vec![first, spec];
            }
            HarnessConformanceScenario::Teardown => {
                spec.post_open = vec![
                    json!({"method":"item/completed","params":{"threadId":"scripted-session","turnId":"turn-open","item":{"type":"agentMessage","id":"accepted","text":"accepted","phase":"commentary"}}}),
                    question_frame(60, false),
                ];
            }
            HarnessConformanceScenario::NewSession
            | HarnessConformanceScenario::ChangedInputCollision => {}
        }
        vec![spec]
    }

    fn question_frame(id: u64, secret: bool) -> Value {
        json!({
            "id": id,
            "method": "item/tool/requestUserInput",
            "params": {
                "threadId": "scripted-session",
                "turnId": "turn-open",
                "itemId": format!("question-{id}"),
                "questions": [{
                    "id": "decision",
                    "header": "Approval",
                    "question": "Allow this bounded action?",
                    "options": [{"label":"approve","description":"Approve"}],
                    "isSecret": secret
                }]
            }
        })
    }

    fn supported_request_frames() -> Vec<Value> {
        vec![
            json!({"id":70,"method":"item/commandExecution/requestApproval","params":{"threadId":"thr-test","turnId":"turn-open","itemId":"command","command":"echo hi","cwd":"/tmp","reason":"test"}}),
            json!({"id":71,"method":"item/fileChange/requestApproval","params":{"threadId":"thr-test","turnId":"turn-open","itemId":"file","reason":"test","grantRoot":"/tmp"}}),
            json!({"id":72,"method":"item/permissions/requestApproval","params":{"threadId":"thr-test","turnId":"turn-open","itemId":"permission","cwd":"/tmp","reason":"test","permissions":{"network":true}}}),
            json!({"id":73,"method":"mcpServer/elicitation/request","params":{"threadId":"thr-test","turnId":"turn-open","serverName":"example","mode":"url","message":"Open sign-in","url":"https://example.test/sign-in","elicitationId":"elicit-url"}}),
            json!({"id":74,"method":"mcpServer/elicitation/request","params":{"threadId":"thr-test","turnId":"turn-open","serverName":"example","mode":"form","message":"Enter a name","requestedSchema":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}}}),
        ]
    }

    fn output_frames() -> Vec<Value> {
        vec![
            json!({"method":"item/completed","params":{"threadId":"scripted-session","turnId":"turn-open","item":{"type":"agentMessage","id":"working","text":"working","phase":"commentary"}}}),
            json!({"method":"item/completed","params":{"threadId":"scripted-session","turnId":"turn-open","item":{"type":"commandExecution","id":"command","status":"completed","command":"echo hi","aggregatedOutput":"ok","exitCode":0}}}),
            json!({"method":"item/completed","params":{"threadId":"scripted-session","turnId":"turn-open","item":{"type":"agentMessage","id":"finished","text":"finished","phase":"final_answer"}}}),
        ]
    }

    fn expected_codex_events(
        scenario: HarnessConformanceScenario,
    ) -> Result<Vec<HarnessEvent>, HarnessConformanceFailure> {
        if scenario != HarnessConformanceScenario::OutputActivityOrder {
            return Ok(Vec::new());
        }
        let operation_id = derived_operation("scripted-session", "turn-open");
        Ok(vec![
            HarnessEvent::Output(HarnessOutput {
                output_id: derived_output("working"),
                operation_id,
                kind: HarnessOutputKind::Update,
                status: ActivityStatus::Running,
                body: ContentText::new("working").map_err(|_| conformance_failure(scenario))?,
            }),
            HarnessEvent::Activity(HarnessActivity {
                operation_id,
                item: Some(ShortText::new("command").map_err(|_| conformance_failure(scenario))?),
                kind: ActivityKind::CompletedItem,
                logical_key: ShortText::new("command")
                    .map_err(|_| conformance_failure(scenario))?,
                runtime: ShortText::new("codex").map_err(|_| conformance_failure(scenario))?,
                sequence: NonZeroU64::new(1).ok_or_else(|| conformance_failure(scenario))?,
                status: ActivityStatus::Succeeded,
                content: ContentText::new("echo hi\nok\nExit code: 0")
                    .map_err(|_| conformance_failure(scenario))?,
                truncated: false,
            }),
            HarnessEvent::Output(HarnessOutput {
                output_id: derived_output("finished"),
                operation_id,
                kind: HarnessOutputKind::FinalAnswer,
                status: ActivityStatus::Succeeded,
                body: ContentText::new("finished").map_err(|_| conformance_failure(scenario))?,
            }),
        ])
    }

    fn derived_operation(thread_id: &str, turn_id: &str) -> OperationId {
        let mut digest = Sha256::new();
        digest.update(b"hq.codex.operation.v1\0");
        digest.update(thread_id.as_bytes());
        digest.update(b"\0");
        digest.update(turn_id.as_bytes());
        OperationId::from_bytes(digest.finalize().into())
    }

    fn derived_output(item_id: &str) -> MessageId {
        let mut digest = Sha256::new();
        digest.update(b"hq.codex.output.v1\0");
        digest.update(item_id.as_bytes());
        MessageId::from_bytes(digest.finalize().into())
    }

    struct TracingFactory {
        inner: CodexFactory,
        state: Arc<Mutex<Vec<HarnessConformanceObservation>>>,
    }

    impl HarnessFactory for TracingFactory {
        fn create_instance(
            &self,
            request: HarnessInstanceRequest,
        ) -> Result<Box<dyn HarnessInstance>, HarnessError> {
            trace_record(&self.state, HarnessConformanceObservation::InstanceCreated)?;
            Ok(Box::new(TracingInstance {
                inner: self.inner.create_instance(request)?,
                state: Arc::clone(&self.state),
            }))
        }
    }

    struct TracingInstance {
        inner: Box<dyn HarnessInstance>,
        state: Arc<Mutex<Vec<HarnessConformanceObservation>>>,
    }

    impl HarnessInstance for TracingInstance {
        fn open_session(
            self: Box<Self>,
            request: HarnessSessionRequest,
        ) -> Result<OpenedHarnessSession, HarnessError> {
            match &request {
                HarnessSessionRequest::Start => {
                    trace_record(&self.state, HarnessConformanceObservation::SessionStarted)?;
                }
                HarnessSessionRequest::Resume { session_id } => trace_record(
                    &self.state,
                    HarnessConformanceObservation::SessionResumed(session_id.clone()),
                )?,
            }
            let state = Arc::clone(&self.state);
            match self.inner.open_session(request) {
                Ok(opened) => Ok(OpenedHarnessSession {
                    session_id: opened.session_id,
                    session: Box::new(TracingSession {
                        inner: opened.session,
                        state,
                        force_stopped: false,
                        crashed: false,
                    }),
                }),
                Err(error) => {
                    if error.class == HarnessErrorClass::SessionIdentityMismatch {
                        trace_record(&state, HarnessConformanceObservation::ForceStopped)?;
                    }
                    Err(error)
                }
            }
        }
    }

    struct TracingSession {
        inner: Box<dyn HarnessSession>,
        state: Arc<Mutex<Vec<HarnessConformanceObservation>>>,
        force_stopped: bool,
        crashed: bool,
    }

    impl HarnessSession for TracingSession {
        fn submit(
            &mut self,
            submission: HarnessSubmission,
        ) -> Result<HarnessSubmissionOutcome, HarnessError> {
            trace_record(
                &self.state,
                HarnessConformanceObservation::SubmissionAttempt {
                    submission_id: submission.submission_id,
                    digest: submission.digest,
                },
            )?;
            self.inner.submit(submission)
        }

        fn lookup_submission(
            &mut self,
            submission: &HarnessSubmission,
        ) -> Result<HarnessSubmissionLookup, HarnessError> {
            trace_record(
                &self.state,
                HarnessConformanceObservation::SubmissionLookup {
                    submission_id: submission.submission_id,
                    digest: submission.digest,
                },
            )?;
            self.inner.lookup_submission(submission)
        }

        fn cancel_operation(
            &mut self,
            operation_id: OperationId,
        ) -> Result<HarnessCancellationOutcome, HarnessError> {
            trace_record(
                &self.state,
                HarnessConformanceObservation::OperationCancelled(operation_id),
            )?;
            self.inner.cancel_operation(operation_id)
        }

        fn poll_event(&mut self, wait: Duration) -> Result<HarnessEventPoll, HarnessError> {
            match self.inner.poll_event(wait) {
                Err(error)
                    if matches!(
                        error.class,
                        HarnessErrorClass::Crashed
                            | HarnessErrorClass::ProtocolViolation
                            | HarnessErrorClass::TransportClosed
                            | HarnessErrorClass::ProcessFailed
                            | HarnessErrorClass::CompatibilityMismatch
                    ) =>
                {
                    if !self.crashed {
                        trace_record(&self.state, HarnessConformanceObservation::Crashed)?;
                        self.crashed = true;
                    }
                    Err(error)
                }
                result => result,
            }
        }

        fn answer_interactive(
            &mut self,
            answer: HarnessInteractiveAnswer,
        ) -> Result<(), HarnessError> {
            let request_id = answer.request_id;
            self.inner.answer_interactive(answer)?;
            trace_record(
                &self.state,
                HarnessConformanceObservation::InteractiveAnswered(request_id),
            )
        }

        fn stop_intake(&mut self) -> Result<(), HarnessError> {
            self.inner.stop_intake()?;
            trace_record(&self.state, HarnessConformanceObservation::IntakeStopped)
        }

        fn drain(&mut self, wait: Duration) -> Result<HarnessDrainOutcome, HarnessError> {
            let outcome = self.inner.drain(wait)?;
            trace_record(
                &self.state,
                HarnessConformanceObservation::DrainObserved(outcome),
            )?;
            Ok(outcome)
        }

        fn force_stop(&mut self) -> Result<(), HarnessError> {
            self.inner.force_stop()?;
            if !self.force_stopped {
                trace_record(&self.state, HarnessConformanceObservation::ForceStopped)?;
                self.force_stopped = true;
            }
            Ok(())
        }
    }

    struct TracingTrace {
        scenario: HarnessConformanceScenario,
        state: Arc<Mutex<Vec<HarnessConformanceObservation>>>,
    }

    impl HarnessConformanceTrace for TracingTrace {
        fn observations(
            &self,
        ) -> Result<Vec<HarnessConformanceObservation>, HarnessConformanceFailure> {
            self.state
                .lock()
                .map(|observations| observations.clone())
                .map_err(|_| conformance_failure(self.scenario))
        }
    }

    fn trace_record(
        state: &Arc<Mutex<Vec<HarnessConformanceObservation>>>,
        observation: HarnessConformanceObservation,
    ) -> Result<(), HarnessError> {
        state
            .lock()
            .map(|mut observations| observations.push(observation))
            .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))
    }

    const fn conformance_failure(
        scenario: HarnessConformanceScenario,
    ) -> HarnessConformanceFailure {
        HarnessConformanceFailure {
            scenario,
            check: "Codex conformance fixture failed",
        }
    }

    fn factory(
        starter: Arc<FakeStarter>,
        sink: Arc<RecordingSink>,
    ) -> Result<CodexFactory, HarnessError> {
        CodexFactory::new(CodexFactoryConfig {
            starter,
            resolver: Arc::new(FixedCodexLaunchResolver {
                launch: CodexLaunch {
                    executable: PathBuf::from("codex"),
                    working_directory: PathBuf::from("/tmp"),
                    developer_instructions: "test instructions".to_owned(),
                    model: None,
                    permissive: true,
                },
            }),
            diagnostics: sink,
            call_timeout: Duration::from_millis(40),
            process_grace: Duration::from_secs(1),
            frame_capacity: 16,
        })
    }

    fn instance_request() -> HarnessInstanceRequest {
        HarnessInstanceRequest {
            agent_id: AgentId::from_bytes([1; 32]),
            project_id: None,
            environment: hq_harness::HarnessEnvironment::default(),
        }
    }

    fn submission() -> Result<HarnessSubmission, Box<dyn std::error::Error>> {
        Ok(HarnessSubmission {
            submission_id: MessageId::from_bytes([4; 32]),
            digest: CommandDigest::from_bytes([5; 32]),
            operation_id: OperationId::from_bytes([6; 32]),
            body: ContentText::new("durable input")?,
        })
    }

    #[allow(clippy::too_many_lines)]
    fn run_server(
        mut spec: ServerSpec,
        input: UnixStream,
        mut output: UnixStream,
        observed: &Arc<Mutex<Vec<Value>>>,
    ) {
        let mut reader = BufReader::new(input);
        let mut line = String::new();
        let mut client_id = String::new();
        loop {
            line.clear();
            let Ok(count) = reader.read_line(&mut line) else {
                return;
            };
            if count == 0 {
                return;
            }
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                return;
            };
            if let Ok(mut messages) = observed.lock() {
                messages.push(message.clone());
            }
            let method = message.get("method").and_then(Value::as_str);
            match method {
                Some("initialize") => {
                    if spec.late_response_on_initialize {
                        write_frame(&mut output, &json!({"id":999,"result":{}}));
                    }
                    if spec.initialize_error {
                        write_frame(
                            &mut output,
                            &json!({"id": message.get("id"), "error": {"code": -32000, "message": "provider-secret"}}),
                        );
                    } else {
                        write_result(&mut output, &message, &json!({}));
                    }
                }
                Some("thread/start") => {
                    write_result(
                        &mut output,
                        &message,
                        &json!({"thread": {"id": spec.thread_id, "turns": []}}),
                    );
                    write_open_frames(&spec, &mut output);
                }
                Some("thread/resume") => {
                    if spec.resume_error {
                        write_frame(
                            &mut output,
                            &json!({"id":message.get("id"),"error":{"code":-32004,"message":"missing"}}),
                        );
                        continue;
                    }
                    let acknowledged = spec
                        .resume_ack
                        .as_deref()
                        .or_else(|| {
                            message
                                .get("params")
                                .and_then(|params| params.get("threadId"))
                                .and_then(Value::as_str)
                        })
                        .unwrap_or("missing");
                    write_result(
                        &mut output,
                        &message,
                        &json!({"thread": {"id": acknowledged, "turns": []}}),
                    );
                    write_open_frames(&spec, &mut output);
                }
                Some("turn/start") => {
                    client_id = message
                        .get("params")
                        .and_then(|params| params.get("clientUserMessageId"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    write_frame(
                        &mut output,
                        &json!({"method":"turn/started","params":{"threadId":spec.thread_id,"turn":{"id":"turn-test","status":"inProgress","items":[]}}}),
                    );
                    if spec.lose_turn_response {
                        spec.lose_turn_response = false;
                    } else {
                        write_result(
                            &mut output,
                            &message,
                            &json!({"turn":{"id":"turn-test","status":"inProgress","items":[]}}),
                        );
                        if spec.emit_events {
                            write_frame(
                                &mut output,
                                &json!({"method":"item/completed","params":{"threadId":spec.thread_id,"turnId":"turn-test","item":{"type":"agentMessage","id":"answer","text":"finished","phase":"final_answer"}}}),
                            );
                            write_frame(
                                &mut output,
                                &json!({"id":40,"method":"item/tool/requestUserInput","params":{"threadId":spec.thread_id,"turnId":"turn-test","itemId":"question","questions":[{"id":"scope","header":"Scope","question":"Proceed?","options":[{"label":"approve","description":"Continue"}]}]}}),
                            );
                        }
                    }
                }
                Some("thread/read") => {
                    let accepted_client_id = if client_id.is_empty() {
                        "04".repeat(32)
                    } else {
                        client_id.clone()
                    };
                    let turns = if spec.history_accepts {
                        json!([{"id":"turn-test","status":"inProgress","items":[{"type":"userMessage","id":"user","clientId":accepted_client_id}]}])
                    } else {
                        json!([])
                    };
                    write_result(
                        &mut output,
                        &message,
                        &json!({"thread":{"id":spec.thread_id,"turns":turns}}),
                    );
                    if spec.emit_after_read {
                        write_frame(
                            &mut output,
                            &json!({"method":"item/completed","params":{"threadId":spec.thread_id,"turnId":"turn-test","item":{"type":"agentMessage","id":"recovered","text":"recovered output","phase":"final_answer"}}}),
                        );
                    }
                }
                Some("turn/interrupt" | "turn/steer") => {
                    write_result(&mut output, &message, &json!({"turnId":"turn-test"}));
                }
                _ => {}
            }
        }
    }

    fn write_result(output: &mut UnixStream, request: &Value, result: &Value) {
        write_frame(
            output,
            &json!({"id":request.get("id").cloned().unwrap_or(Value::Null),"result":result}),
        );
    }

    fn write_open_frames(spec: &ServerSpec, output: &mut UnixStream) {
        for frame in &spec.post_open {
            write_frame(output, frame);
        }
        if spec.corrupt_after_open {
            let _ = output.write_all(b"not-json\n");
            let _ = output.flush();
        }
    }

    fn write_frame(output: &mut UnixStream, value: &Value) {
        if serde_json::to_writer(&mut *output, value).is_ok() {
            let _ = output.write_all(b"\n");
            let _ = output.flush();
        }
    }

    fn count_method(values: &[Value], method: &str) -> usize {
        values
            .iter()
            .filter(|value| value.get("method").and_then(Value::as_str) == Some(method))
            .count()
    }

    fn response_result(values: &[Value], id: u64) -> Option<&Value> {
        values
            .iter()
            .find(|value| {
                value.get("id").and_then(Value::as_u64) == Some(id) && value.get("method").is_none()
            })
            .and_then(|value| value.get("result"))
    }

    fn wait_for_methodless_response(
        observed: &Arc<Mutex<Vec<Value>>>,
        id: u64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(1))
            .ok_or("invalid deadline")?;
        while Instant::now() < deadline {
            if observed
                .lock()
                .map_err(|_| "observed request list was poisoned")?
                .iter()
                .any(|value| {
                    value.get("id").and_then(Value::as_u64) == Some(id)
                        && value.get("method").is_none()
                })
            {
                return Ok(());
            }
            thread::yield_now();
        }
        Err("server did not observe the correlated response".into())
    }
}
