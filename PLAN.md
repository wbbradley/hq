# HQ

## Next Up

### Keep the daemon responsive under long provider activity streams

Prevent replaceable Codex progress and output-delta traffic from turning into an unbounded sequence
of canonical activity mutations or monopolizing the harness, store, and local API. Provider events
must be admitted and coalesced before persistence, persistence must not run while holding supervisor
catalog locks, and every drain pass must have an explicit work/time bound while preserving durable
FIFO output, interaction ordering, checkpoint recovery, and terminal worker cleanup.

Add deterministic flood and blocked-persistence regressions proving that replaceable updates remain
bounded, pending-interaction queries and unrelated local API sessions stay responsive, and a ready
daemon continues answering liveness probes. Readiness must test the advertised generation rather
than trusting a live PID/socket artifact alone. Add default-on, bounded, mode-0600, privacy-safe
diagnostics for queue high-water marks, coalescing, drain/store/lock latency, stale readiness, and
the exact TUI terminal phase plus OS error kind/code; never record message, command, output,
environment, prompt, or secret bodies. Installed qualification must exercise a long fake-Codex tool
stream, reconnect a second client during it, and verify bounded facts, memory, latency, and clean
shutdown/restart.

### Smaller hand-added todos (each need some in-depth analysis)
