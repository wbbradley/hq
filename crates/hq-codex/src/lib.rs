//! Private Codex app-server adapter behind HQ's provider-neutral harness contract.

mod adapter;
mod normalize;
mod operation_control;
mod process;
mod protocol;
mod transport;
mod transport_control;

pub use adapter::{CodexFactory, CodexFactoryConfig};
pub use process::{
    CodexDiagnosticSink, CodexLaunch, CodexLaunchResolver, CodexOperationalDiagnosticSink,
    CodexProcessControl, CodexProcessPipes, CodexProcessStarter, CodexWaitOutcome,
    DiscardCodexDiagnostics, ExecCodexProcessStarter, FixedCodexLaunchResolver,
};

/// Exact Codex CLI baseline that generated the checked-in app-server schema.
pub const CODEX_BASELINE_VERSION: &str = "0.150.1";

/// Neutral provider namespace registered by this adapter.
pub const CODEX_PROVIDER_ID: &str = "codex";

#[cfg(test)]
mod tests;

#[cfg(test)]
mod transport_control_tests;
