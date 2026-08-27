# Codex adapter v1

Status: implemented protocol boundary
Pinned Codex CLI: `@openai/codex` `0.150.1`
Provider namespace: `codex`

## Baseline evidence

HQ pins the app-server protocol emitted by the locally installed official Codex CLI:

```text
codex-cli 0.150.1
@openai/codex@0.150.1
codex app-server generate-json-schema --experimental --out <directory>
```

The generated combined v1 and v2 bundles, their SHA-256 digests, and representative wire fixtures
are checked in under `crates/hq-codex/testdata`. The bundles are compatibility evidence, not runtime
code generation. Updating Codex requires deliberately regenerating those files, updating the
manifest and this document, and passing the adapter characterization and conformance tests.

The transport choice follows the official [Codex App Server documentation][app-server]: default
stdio is newline-delimited JSON, while WebSocket support is experimental. HQ therefore owns one
`codex app-server --listen stdio://` child per logical instance and does not offer a WebSocket mode.

[app-server]: https://developers.openai.com/codex/app-server

## Wire lifecycle

The adapter performs exactly one `initialize` request followed by one `initialized` notification.
It then performs exactly one of:

- `thread/start`, including the resolved working directory and fresh-thread developer instructions;
- `thread/resume`, naming the exact durable provider session requested by HQ.

An empty acknowledgement fails as a crashed boundary. A different resume acknowledgement fails as
`SessionIdentityMismatch` and the child is stopped. A definite resume rejection is
`SessionNotFound`; HQ never silently starts a replacement.

HQ submits bounded text through `turn/start` while idle and `turn/steer` with the exact expected turn
while active. The stable 32-byte HQ message identity is encoded as lowercase hexadecimal
`clientUserMessageId`. `thread/read(includeTurns=true)` is the sole acceptance lookup and steering
race reconciliation mechanism. The supervisor remains authoritative for the immutable
identity/digest/body collision check because Codex history retains the client identity but not HQ's
digest. Exact active-operation cancellation uses `turn/interrupt`.

## Compatibility boundary

The private DTO subset consumes these methods:

- client requests: `initialize`, `thread/start`, `thread/resume`, `thread/read`, `turn/start`,
  `turn/steer`, and `turn/interrupt`;
- client notification: `initialized`;
- authoritative notifications: `turn/started`, `turn/completed`, `turn/plan/updated`,
  `turn/diff/updated`, `item/completed`, the bounded item delta notifications, and
  `item/mcpToolCall/progress`;
- server requests: `item/tool/requestUserInput`, command and file-change approvals,
  `item/permissions/requestApproval`, and form or URL `mcpServer/elicitation/request`.

Unknown additive object fields and unknown non-authoritative notifications are ignored. Envelope
shape, nonempty identities, thread correlation, the 4 MiB inclusive frame limit, authority-bearing
request values, and response identities remain closed. An unknown server request receives JSON-RPC
method-not-found and terminates the compatibility boundary. Malformed or secret-marked supported
requests receive their method-specific fail-closed response; secret values never become neutral
events.

## Normalization and bounds

Only completed agent messages become output. Their Codex phase determines `Update` versus
`FinalAnswer`; provider item identity produces a deterministic stable output identity. Turn state,
plans, diffs, bounded progress, and completed command/file/tool/web-search items become typed neutral
activity. Raw reasoning, token deltas, spinner state, and model payloads are ignored. Content is
UTF-8-boundary truncated to the neutral 16 KiB content limit, short identities to 128 bytes, and the
`truncated` bit records activity truncation.

Stdout is exclusively protocol data. One bounded reader owns it and accepts only complete JSONL
frames. Stderr is drained separately in bounded 16 KiB lines to a provider-private diagnostic sink;
neither stderr nor provider response prose can enter neutral errors or `Debug`. The launch child
receives exactly the validated copied environment after `env_clear`.

## Failure mapping and shutdown

- a correlated provider error before acceptance is a definite neutral rejection;
- timeout, write failure, EOF, or response loss after submission is `Uncertain(Unavailable)`;
- malformed frames, identities, or authoritative response shapes are `Crashed`;
- changed digest under one attempted stable identity is `SubmissionIdentityConflict`;
- duplicate interactive answers are `InteractiveAlreadyAnswered`;
- secret interactive input is `SecretInputRejected`.

`stop_intake` rejects new submissions and fail-closes outstanding requests. `drain` pumps already
accepted frames for its bounded wait and reports exact queued event/request counts. `force_stop` is
idempotent: close stdin, wait the configured grace period, issue checked kill if still running, wait
again, drain and join stdout, then join stderr. Sibling instances never share process ownership.

## Verification and installed smoke

The crate tests hash the pinned schema, characterize representative fixtures and framing failures,
exercise fake stdio/process behavior, and run all 14 reusable neutral harness scenarios through the
real adapter seam. The installed-provider smoke is deliberately opt-in because it starts an
authenticated Codex thread:

```text
HQ_CODEX_INSTALLED_SMOKE=1 cargo test -p hq-codex installed_codex_smoke -- --ignored --nocapture
```

The smoke requires the installed binary to report exactly `codex-cli 0.150.1`; version drift fails
before app-server launch. It performs initialize plus start and exact resume of the persisted thread
named by `HQ_CODEX_SMOKE_RESUME_SESSION` (otherwise selecting a bounded recent inactive rollout),
then stops both children without submitting a turn or invoking a model.
