# Rust CLI

HQ installs one `hq` executable. Global options precede the command:

```text
hq [--output human|json] [--state-root ABSOLUTE_PATH] <COMMAND>
```

The installed commands currently include `help`, `version`, `identity`, `config`, and `daemon
run|status|readiness|stop|restart`. `daemon run` is the internal foreground ownership role used by
autostart and service managers. `daemon status` never starts a process; `readiness` may start one
candidate and waits for all concurrent candidates to converge on the sole state-directory owner.
These commands never inspect terminal state or prompt.

`identity init|show|export|import` and `config get|set` are deliberately offline operations. They
acquire the same exclusive state owner as the node and refuse a live owner instead of reading or
writing behind it. Initialization and import never overwrite an identity. Export and import require
an absolute package path and the literal `--password-stdin`; exactly one bounded UTF-8 password line
is then consumed from stdin, normalized and zeroized. The secret is never accepted as argv, echoed,
retained in a diagnostic, or sent over the local API. A closed, oversized, malformed, or multiline
input fails explicitly. Other commands do not read stdin.

Identity output has only the installation ID, signing public key, and public fingerprint.
Configuration output has the optional provider and the complete canonical relay list. Both are
passive data with public fields. Configuration setters replace one complete typed field, rebuild
the validated value, and the persistence adapter revalidates public fields again immediately before
the atomic write.

Human output is concise newline-terminated text. JSON output is exactly one newline-terminated
object with schema `hq-cli-output-v1`, an `ok` boolean, a stable `kind`, and typed `data`. Errors use
the same envelope on stderr and contain only stable class, code, and redacted message fields.
Arguments or filesystem inputs are never echoed into diagnostics.

Exit statuses are stable classes:

| Status | Class | Meaning |
| ---: | --- | --- |
| 0 | success | The command completed and stdout contains its record. |
| 1 | failure | Valid command execution failed. |
| 2 | usage | Arguments or caller-supplied paths were invalid. |
| 3 | unavailable | A compatible local node could not be reached or made ready. |

The reusable command client first crosses `NodeClientCoordinator` for bounded readiness, then owns
one Unix transport and `hq-local-api::ReconnectingClient`. The transport performs bounded strict
length-prefixed reads and complete writes. The runner allows one response-producing write at a
time, renegotiates each connection, caps attempts and wall time, and correlates errors with their
semantic operation. Ordinary requests are never replayed after response loss. Exact mutation and
project command frames retain their stable identities and replay byte-for-byte until a definite
typed result or the explicit bound is reached. Snapshot-oriented clients may request a fresh view
after negotiation; command-only clients do not issue an unsolicited snapshot.

CLI production code has no canonical storage, signer, relay, resource, harness-provider, or SQLite
access. The identity/configuration commands cross only the private state-ownership and identity
persistence adapter because they must operate while the node is absent. Canonical administration
and later command families use the reusable request, mutation, and project methods rather than
opening implementation adapters directly.
