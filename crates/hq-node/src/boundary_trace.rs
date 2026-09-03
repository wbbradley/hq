//! Privacy-safe, best-effort latency records for installed pipeline diagnosis.

use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    sync::{Arc, Mutex},
};

use hq_codex::CodexOperationalDiagnosticSink;
use hq_harness::{HarnessDiagnosticEvent, HarnessDiagnosticSink, HarnessDiagnosticTarget};
use nix::time::{ClockId, clock_gettime};
use serde::Serialize;

/// Opt-in absolute JSONL destination shared by installed HQ processes.
pub const BOUNDARY_TRACE_ENVIRONMENT: &str = "HQ_BOUNDARY_TRACE";

const TRACE_SCHEMA: &str = "hq.boundary.v1";
const MAX_RECORD_BYTES: usize = 2_048;
const MAX_TRACE_FILE_BYTES: u64 = 1_048_576;
const DIAGNOSTIC_DIRECTORY: &str = "diagnostics";
const DIAGNOSTIC_FILE: &str = "boundaries.jsonl";

/// Closed process roles that can own a latency boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryProcess {
    /// Foreground node daemon.
    Node,
    /// Interactive terminal client.
    Tui,
    /// One-shot or reconnecting local protocol client.
    Client,
}

/// Closed real-terminal phase vocabulary used in diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TuiTerminalPhase {
    /// Initial backend construction or terminal-mode activation.
    Activate,
    /// Terminal dimension observation.
    Size,
    /// Terminal or executor readiness polling.
    Poll,
    /// Frame rendering and flush.
    Draw,
    /// Terminal-mode restoration.
    Restore,
}

/// Stable privacy-safe classification of one terminal I/O failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TuiTerminalIoKind {
    /// The terminal resource was absent.
    NotFound,
    /// Access to the terminal resource was denied.
    PermissionDenied,
    /// A terminal connection was reset.
    ConnectionReset,
    /// A terminal stream closed while writing.
    BrokenPipe,
    /// The terminal operation would have blocked.
    WouldBlock,
    /// The terminal operation exceeded its deadline.
    TimedOut,
    /// The terminal operation was interrupted.
    Interrupted,
    /// The terminal operation is unsupported.
    Unsupported,
    /// The terminal stream ended before a complete operation.
    UnexpectedEof,
    /// Another operating-system I/O category occurred.
    Other,
}

/// Closed connection state vocabulary used in TUI diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TuiConnectionDiagnosticState {
    /// The first local connection is being established.
    Connecting,
    /// The subscribed connection is current.
    Ready,
    /// A later local connection generation is being established.
    Reconnecting,
    /// The local protocol versions are incompatible.
    Incompatible,
    /// No local connection is active.
    Disconnected,
}

/// Closed transport operation that caused a reconnect.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TuiReconnectOperation {
    /// Opening a local connection.
    Connect,
    /// Reading from the subscribed connection.
    Read,
    /// Writing to the local connection.
    Write,
}

/// Closed privacy-safe reconnect failure classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TuiReconnectFailureKind {
    /// The local endpoint was unavailable.
    Unavailable,
    /// The operating-system transport failed.
    Transport,
    /// Local framing or protocol decoding failed.
    Protocol,
}

/// Closed client failure reported independently of connection-state transitions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TuiClientFailureKind {
    /// The local protocol versions are incompatible.
    Incompatible,
    /// The local client could not complete its current operation.
    Unavailable,
}

/// Closed body-free event vocabulary for the interactive delivery pipeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryKind {
    /// Relay bytes were accepted for canonical decoding.
    RelayReceived,
    /// Canonical relay input committed to the store.
    RelayCommitted,
    /// A local canonical mutation committed to the store.
    StoreCommitted,
    /// Project reconciliation became ready.
    ProjectWoken,
    /// One durable project dispatch reached the runtime boundary.
    ProjectDispatched,
    /// A normalized instruction was submitted to Codex.
    CodexSubmitted,
    /// A normalized provider event reached the harness owner.
    ProviderEventReceived,
    /// A validated provider interaction entered the Codex adapter.
    ProviderInteractionReceived,
    /// A pending provider interaction became queryable.
    InteractionPublished,
    /// A local revision invalidation was published.
    LocalInvalidationPublished,
    /// A subscribed local connection emitted its invalidation.
    LocalInvalidationWritten,
    /// The TUI observation owner received an invalidation or interaction.
    TuiObservationReceived,
    /// The TUI observed a generation-scoped local connection transition.
    TuiConnectionObserved,
    /// The TUI client reported a generation-scoped workflow failure.
    TuiClientFailed,
    /// The TUI reducer applied the correlated observation.
    TuiModelUpdated,
    /// The first frame containing the correlated interaction was drawn.
    TuiDialogDrawn,
    /// The real terminal boundary failed with closed phase and OS evidence.
    TuiTerminalFailed,
    /// A harness worker-catalog lock wait crossed the reporting threshold.
    HarnessWorkerCatalogLock,
    /// A harness persistence-owner lock wait crossed the reporting threshold.
    HarnessPersistenceLock,
    /// One canonical harness persistence operation completed.
    HarnessPersistence,
    /// One bounded provider-event drain completed.
    HarnessReadyDrain,
    /// A live protocol response did not match its readiness artifact generation.
    StaleReadiness,
    /// Replaceable Codex notifications were coalesced before transport queue admission.
    CodexTransportCoalesced,
}

/// Optional stable correlation identities; no field accepts arbitrary text.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BoundaryIds {
    /// Canonical fact identity.
    pub fact: Option<[u8; 32]>,
    /// Stable conversation message identity.
    pub message: Option<[u8; 32]>,
    /// Stable project dispatch identity.
    pub dispatch: Option<[u8; 32]>,
    /// Stable harness operation identity.
    pub operation: Option<[u8; 32]>,
    /// Stable provider request identity.
    pub provider_request: Option<[u8; 32]>,
    /// Boot-local local-API connection identity.
    pub connection: Option<[u8; 32]>,
    /// Local subscription generation.
    pub subscription_generation: Option<u64>,
    /// Pure-model effect identity.
    pub tui_effect: Option<u64>,
    /// Authoritative store revision.
    pub revision: Option<u64>,
    /// Monotonic operation duration.
    pub elapsed_ns: Option<u64>,
    /// Retained normalized values after a bounded drain.
    pub pending_values: Option<u64>,
    /// Highest retained normalized value count during a bounded drain.
    pub queue_high_water: Option<u64>,
    /// Replaceable values coalesced during a bounded drain.
    pub coalesced_values: Option<u64>,
    /// Provider values polled during a bounded drain.
    pub events_polled: Option<u64>,
    /// Exact terminal phase for a terminal-failure record.
    pub terminal_phase: Option<TuiTerminalPhase>,
    /// Stable terminal I/O category when an operating-system error was available.
    pub terminal_io_kind: Option<TuiTerminalIoKind>,
    /// Platform terminal error number when supplied by the operating system.
    pub terminal_os_code: Option<i32>,
    /// Closed state for a TUI connection observation.
    pub tui_connection_state: Option<TuiConnectionDiagnosticState>,
    /// Closed transport operation that caused a reconnect.
    pub tui_reconnect_operation: Option<TuiReconnectOperation>,
    /// Closed transport failure that caused a reconnect.
    pub tui_reconnect_failure_kind: Option<TuiReconnectFailureKind>,
    /// Closed client workflow failure.
    pub tui_client_failure_kind: Option<TuiClientFailureKind>,
}

/// Cloneable best-effort append sink; absence or failure never affects authority.
#[derive(Clone)]
pub struct BoundaryTrace {
    process: BoundaryProcess,
    writer: Option<Arc<Mutex<BoundedTraceWriter>>>,
}

struct BoundedTraceWriter {
    file: File,
}

impl BoundaryTrace {
    /// Opens the opt-in environment destination, otherwise returns a disabled sink.
    pub fn from_environment(process: BoundaryProcess) -> Self {
        let Some(path) = std::env::var_os(BOUNDARY_TRACE_ENVIRONMENT) else {
            return Self::disabled(process);
        };
        Self::open(Path::new(&path), process)
    }

    /// Opens the environment override or the default private bounded state diagnostic.
    pub fn from_state(state_root: &Path, process: BoundaryProcess) -> Self {
        if let Some(path) = std::env::var_os(BOUNDARY_TRACE_ENVIRONMENT) {
            return Self::open(Path::new(&path), process);
        }
        Self::open_state(state_root, process)
    }

    fn open_state(state_root: &Path, process: BoundaryProcess) -> Self {
        if !state_root.is_absolute() {
            return Self::disabled(process);
        }
        let directory = state_root.join(DIAGNOSTIC_DIRECTORY);
        if std::fs::create_dir_all(&directory).is_err()
            || std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).is_err()
        {
            return Self::disabled(process);
        }
        Self::open(&directory.join(DIAGNOSTIC_FILE), process)
    }

    /// Opens one absolute append-only destination, degrading safely when invalid or unavailable.
    pub fn open(path: &Path, process: BoundaryProcess) -> Self {
        if !path.is_absolute() {
            return Self::disabled(process);
        }
        let writer = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .ok()
            .and_then(|file| {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                    .ok()
                    .map(|()| file)
            })
            .map(|file| Arc::new(Mutex::new(BoundedTraceWriter { file })));
        Self { process, writer }
    }

    /// Constructs a disabled sink with the supplied process role.
    pub const fn disabled(process: BoundaryProcess) -> Self {
        Self {
            process,
            writer: None,
        }
    }

    /// Appends one bounded structured record when tracing is enabled.
    pub fn record(&self, kind: BoundaryKind, ids: BoundaryIds) {
        let Some(writer) = &self.writer else {
            return;
        };
        let Some(monotonic_ns) = monotonic_nanoseconds() else {
            return;
        };
        let record = EncodedBoundaryRecord::new(self.process, kind, monotonic_ns, &ids);
        let Ok(mut bytes) = serde_json::to_vec(&record) else {
            return;
        };
        bytes.push(b'\n');
        if bytes.len() > MAX_RECORD_BYTES {
            return;
        }
        if let Ok(mut writer) = writer.lock() {
            writer.write(&bytes);
        }
    }
}

impl BoundedTraceWriter {
    fn write(&mut self, bytes: &[u8]) {
        let bytes_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let current = self.file.metadata().map(|metadata| metadata.len()).ok();
        if current.is_some_and(|length| length.saturating_add(bytes_len) > MAX_TRACE_FILE_BYTES)
            && self.file.set_len(0).is_err()
        {
            return;
        }
        let _ = self.file.write_all(bytes);
    }
}

impl HarnessDiagnosticSink for BoundaryTrace {
    fn record(&self, event: HarnessDiagnosticEvent) {
        let kind = match event.target {
            HarnessDiagnosticTarget::WorkerCatalogLock => BoundaryKind::HarnessWorkerCatalogLock,
            HarnessDiagnosticTarget::PersistenceLock => BoundaryKind::HarnessPersistenceLock,
            HarnessDiagnosticTarget::Persistence => BoundaryKind::HarnessPersistence,
            HarnessDiagnosticTarget::ReadyDrain => BoundaryKind::HarnessReadyDrain,
        };
        BoundaryTrace::record(
            self,
            kind,
            BoundaryIds {
                elapsed_ns: Some(duration_nanoseconds(event.elapsed)),
                pending_values: Some(saturating_u64(event.pending_values)),
                queue_high_water: Some(saturating_u64(event.queue_high_water)),
                coalesced_values: Some(saturating_u64(event.coalesced_values)),
                events_polled: Some(saturating_u64(event.events_polled)),
                ..BoundaryIds::default()
            },
        );
    }
}

impl CodexOperationalDiagnosticSink for BoundaryTrace {
    fn transport_coalesced(&self, count: usize) {
        self.record(
            BoundaryKind::CodexTransportCoalesced,
            BoundaryIds {
                coalesced_values: Some(saturating_u64(count)),
                ..BoundaryIds::default()
            },
        );
    }

    fn interaction_received(&self, operation_id: [u8; 32], request_id: [u8; 32]) {
        self.record(
            BoundaryKind::ProviderInteractionReceived,
            BoundaryIds {
                operation: Some(operation_id),
                provider_request: Some(request_id),
                ..BoundaryIds::default()
            },
        );
    }
}

impl std::fmt::Debug for BoundaryTrace {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BoundaryTrace")
            .field("process", &self.process)
            .field("enabled", &self.writer.is_some())
            .finish()
    }
}

#[derive(Serialize)]
struct EncodedBoundaryRecord {
    schema: &'static str,
    process: BoundaryProcess,
    kind: BoundaryKind,
    monotonic_ns: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    fact_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dispatch_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    connection_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subscription_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tui_effect_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    elapsed_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_values: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    queue_high_water: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    coalesced_values: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    events_polled: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_phase: Option<TuiTerminalPhase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_io_kind: Option<TuiTerminalIoKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_os_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tui_connection_state: Option<TuiConnectionDiagnosticState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tui_reconnect_operation: Option<TuiReconnectOperation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tui_reconnect_failure_kind: Option<TuiReconnectFailureKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tui_client_failure_kind: Option<TuiClientFailureKind>,
}

impl EncodedBoundaryRecord {
    fn new(
        process: BoundaryProcess,
        kind: BoundaryKind,
        monotonic_ns: u64,
        ids: &BoundaryIds,
    ) -> Self {
        Self {
            schema: TRACE_SCHEMA,
            process,
            kind,
            monotonic_ns,
            fact_id: ids.fact.map(hex_identity),
            message_id: ids.message.map(hex_identity),
            dispatch_id: ids.dispatch.map(hex_identity),
            operation_id: ids.operation.map(hex_identity),
            provider_request_id: ids.provider_request.map(hex_identity),
            connection_id: ids.connection.map(hex_identity),
            subscription_generation: ids.subscription_generation,
            tui_effect_id: ids.tui_effect,
            revision: ids.revision,
            elapsed_ns: ids.elapsed_ns,
            pending_values: ids.pending_values,
            queue_high_water: ids.queue_high_water,
            coalesced_values: ids.coalesced_values,
            events_polled: ids.events_polled,
            terminal_phase: ids.terminal_phase,
            terminal_io_kind: ids.terminal_io_kind,
            terminal_os_code: ids.terminal_os_code,
            tui_connection_state: ids.tui_connection_state,
            tui_reconnect_operation: ids.tui_reconnect_operation,
            tui_reconnect_failure_kind: ids.tui_reconnect_failure_kind,
            tui_client_failure_kind: ids.tui_client_failure_kind,
        }
    }
}

fn duration_nanoseconds(duration: std::time::Duration) -> u64 {
    duration.as_nanos().try_into().unwrap_or(u64::MAX)
}

fn saturating_u64(value: usize) -> u64 {
    value.try_into().unwrap_or(u64::MAX)
}

fn monotonic_nanoseconds() -> Option<u64> {
    let time = clock_gettime(ClockId::CLOCK_MONOTONIC).ok()?;
    let seconds = u64::try_from(time.tv_sec()).ok()?;
    let nanoseconds = u64::try_from(time.tv_nsec()).ok()?;
    seconds.checked_mul(1_000_000_000)?.checked_add(nanoseconds)
}

fn hex_identity(bytes: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn trace_is_bounded_structured_and_contains_only_closed_identifiers() {
        let directory = std::env::temp_dir().join(format!(
            "hq-boundary-trace-{}-{}",
            std::process::id(),
            monotonic_nanoseconds().expect("monotonic clock")
        ));
        std::fs::create_dir(&directory).expect("trace directory");
        let path = directory.join("boundaries.jsonl");
        let trace = BoundaryTrace::open(&path, BoundaryProcess::Node);
        trace.record(
            BoundaryKind::InteractionPublished,
            BoundaryIds {
                operation: Some([0xab; 32]),
                provider_request: Some([0xcd; 32]),
                revision: Some(7),
                ..BoundaryIds::default()
            },
        );
        let bytes = std::fs::read(&path).expect("trace bytes");
        assert!(bytes.len() <= MAX_RECORD_BYTES);
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("trace JSON");
        assert_eq!(value["schema"], TRACE_SCHEMA);
        assert_eq!(value["process"], "node");
        assert_eq!(value["kind"], "interaction_published");
        assert_eq!(value["revision"], 7);
        assert_eq!(value["operation_id"], "ab".repeat(32));
        assert_eq!(value["provider_request_id"], "cd".repeat(32));
        assert!(value.get("message").is_none());
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(directory);
    }

    #[test]
    fn tui_connection_diagnostics_distinguish_state_cause_and_client_failure() {
        let directory = std::env::temp_dir().join(format!(
            "hq-boundary-connection-{}-{}",
            std::process::id(),
            monotonic_nanoseconds().expect("monotonic clock")
        ));
        std::fs::create_dir(&directory).expect("trace directory");
        let path = directory.join("boundaries.jsonl");
        let trace = BoundaryTrace::open(&path, BoundaryProcess::Tui);
        trace.record(
            BoundaryKind::TuiConnectionObserved,
            BoundaryIds {
                subscription_generation: Some(2),
                tui_connection_state: Some(TuiConnectionDiagnosticState::Reconnecting),
                tui_reconnect_operation: Some(TuiReconnectOperation::Read),
                tui_reconnect_failure_kind: Some(TuiReconnectFailureKind::Protocol),
                ..BoundaryIds::default()
            },
        );
        trace.record(
            BoundaryKind::TuiClientFailed,
            BoundaryIds {
                subscription_generation: Some(2),
                tui_client_failure_kind: Some(TuiClientFailureKind::Unavailable),
                ..BoundaryIds::default()
            },
        );

        let records = std::fs::read_to_string(&path).expect("trace bytes");
        let values = records
            .lines()
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<Result<Vec<_>, _>>()
            .expect("connection diagnostics");
        assert_eq!(values[0]["kind"], "tui_connection_observed");
        assert_eq!(values[0]["subscription_generation"], 2);
        assert_eq!(values[0]["tui_connection_state"], "reconnecting");
        assert_eq!(values[0]["tui_reconnect_operation"], "read");
        assert_eq!(values[0]["tui_reconnect_failure_kind"], "protocol");
        assert_eq!(values[1]["kind"], "tui_client_failed");
        assert_eq!(values[1]["tui_client_failure_kind"], "unavailable");
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(directory);
    }

    #[test]
    fn codex_interaction_ingress_records_only_correlated_identities() {
        let directory = std::env::temp_dir().join(format!(
            "hq-boundary-interaction-{}-{}",
            std::process::id(),
            monotonic_nanoseconds().expect("monotonic clock")
        ));
        std::fs::create_dir(&directory).expect("trace directory");
        let path = directory.join("boundaries.jsonl");
        let trace = BoundaryTrace::open(&path, BoundaryProcess::Node);
        CodexOperationalDiagnosticSink::interaction_received(&trace, [0xab; 32], [0xcd; 32]);

        let bytes = std::fs::read(&path).expect("trace bytes");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("trace JSON");
        assert_eq!(value["kind"], "provider_interaction_received");
        assert_eq!(value["operation_id"], "ab".repeat(32));
        assert_eq!(value["provider_request_id"], "cd".repeat(32));
        assert_eq!(value.as_object().expect("record object").len(), 6);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(directory);
    }

    #[test]
    fn disabled_and_invalid_sinks_are_inert() {
        BoundaryTrace::disabled(BoundaryProcess::Tui)
            .record(BoundaryKind::TuiModelUpdated, BoundaryIds::default());
        BoundaryTrace::open(Path::new("relative.jsonl"), BoundaryProcess::Node)
            .record(BoundaryKind::ProjectWoken, BoundaryIds::default());
    }

    #[test]
    fn independent_append_owners_emit_complete_jsonl_records() {
        let directory = std::env::temp_dir().join(format!(
            "hq-boundary-append-{}-{}",
            std::process::id(),
            monotonic_nanoseconds().expect("monotonic clock")
        ));
        std::fs::create_dir(&directory).expect("trace directory");
        let path = directory.join("boundaries.jsonl");
        let node = BoundaryTrace::open(&path, BoundaryProcess::Node);
        let tui = BoundaryTrace::open(&path, BoundaryProcess::Tui);
        let node_thread = std::thread::spawn(move || {
            for revision in 1..=64 {
                node.record(
                    BoundaryKind::LocalInvalidationPublished,
                    BoundaryIds {
                        revision: Some(revision),
                        ..BoundaryIds::default()
                    },
                );
            }
        });
        let tui_thread = std::thread::spawn(move || {
            for revision in 1..=64 {
                tui.record(
                    BoundaryKind::TuiObservationReceived,
                    BoundaryIds {
                        revision: Some(revision),
                        ..BoundaryIds::default()
                    },
                );
            }
        });
        node_thread.join().expect("node append owner");
        tui_thread.join().expect("TUI append owner");
        let trace = std::fs::read_to_string(&path).expect("trace bytes");
        let records = trace
            .lines()
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<Result<Vec<_>, _>>()
            .expect("every append remains complete JSON");
        assert_eq!(records.len(), 128);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(directory);
    }

    #[test]
    fn default_state_trace_is_private_and_bounded() {
        let state = std::env::temp_dir().join(format!(
            "hq-boundary-state-{}-{}",
            std::process::id(),
            monotonic_nanoseconds().expect("monotonic clock")
        ));
        let trace = BoundaryTrace::open_state(&state, BoundaryProcess::Node);
        for revision in 0..10_000 {
            trace.record(
                BoundaryKind::HarnessReadyDrain,
                BoundaryIds {
                    operation: Some([0xab; 32]),
                    provider_request: Some([0xcd; 32]),
                    revision: Some(revision),
                    elapsed_ns: Some(1_000),
                    pending_values: Some(64),
                    queue_high_water: Some(64),
                    coalesced_values: Some(63),
                    events_polled: Some(64),
                    ..BoundaryIds::default()
                },
            );
        }
        let directory = state.join(DIAGNOSTIC_DIRECTORY);
        let path = directory.join(DIAGNOSTIC_FILE);
        let metadata = std::fs::metadata(&path).expect("trace metadata");
        assert!(metadata.len() <= MAX_TRACE_FILE_BYTES);
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::metadata(&directory)
                .expect("diagnostic directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let contents = std::fs::read_to_string(&path).expect("trace reads");
        assert!(
            contents
                .lines()
                .all(|line| serde_json::from_str::<serde_json::Value>(line).is_ok())
        );
        let _ = std::fs::remove_dir_all(state);
    }
}
