# Rust CLI

HQ installs one `hq` executable. Its command grammar and root/nested help are generated from one
private Clap command tree. Global options may appear before or after a subcommand:

```text
hq [OPTIONS] [COMMAND]
```

Run `hq --help`, `hq COMMAND --help`, or `hq help COMMAND [SUBCOMMAND]...` for authoritative
syntax. A bare non-terminal invocation also renders root help; the tiny executable adapter retains
the separate bare-terminal default that opens the TUI. `--output` accepts `human` or `json` and
defaults to `human`. `--state-root` accepts a raw operating-system path and is valid only for
commands that operate on installation state. The TUI accepts only human output.

Clap owns command and option names, positional cardinality, defaults, repeated values, required
arguments, conflicts, and relationships such as provider/session pairs, rename set/clear choices,
archive/all filters, assignment new/resume choices, and explicit `--yes` confirmation. The
state-free mapping layer then validates domain values such as IDs, names, durations, content,
relay endpoints, and absolute or existing paths before constructing the closed execution command.
Usage failures keep one stable redacted diagnostic contract: neither Clap's detailed error text nor
rejected argv values cross the human or JSON output boundary.

The installed commands currently include `help`, `version`, `agents`, `identity`, `config`,
`human create|show|select|invite|join|devices|revoke`,
`peer add|list|distrust`, `mailbox list|grant|revoke`, and
`relay add|list|remove|sync|status|repair`,
`agent list|show|create|current|select|rename|retire`, and
`harness start|resume|stop`, the complete `project` catalog/resource/creation/assignment/lifecycle
tree, the agent-side
`ask|send|wait|poll`, human-side `get|list|answer|cancel|archive|restore`, `mailboxes`, and
`daemon run|status|readiness|stop|restart`. `daemon run` is the
internal foreground ownership role used by
autostart and service managers. `daemon status` never starts a process; `readiness` may start one
candidate and waits for all concurrent candidates to converge on the sole state-directory owner.
The foreground role closes every non-standard inherited descriptor before runtime startup, so an
autostart caller never remains coupled to the long-lived daemon by an unrelated pipe or socket.
These commands never inspect terminal state or prompt.

Human `answer`, `archive`, and `restore` submit through the ordinary retry-safe mailbox-command
endpoint. The node, not the CLI snapshot, resolves the current question root, target fact, state
frontier, and local human authority inside the canonical transaction. This preserves exact replay
after response loss and prevents a stale client from supplying obsolete causal facts. `cancel`
retains its separate question-owner cancellation planner. The same endpoint also supports direct
agent messages and self-notes for the interactive client, while durable draft autosave/load/delete
remain local operational state rather than canonical facts.

The installed `hq tui` uses that same endpoint for `r` reply, `d` direct message, `n` self-note,
and confirmed `a` archive / `u` restore actions. Its local draft editor autosaves after bounded
input, saves the latest text before close or submit, retains stale targets for recovery, and keeps
modal/focus/selection state across authoritative reload, reconnect, and resize. The TUI never
recomputes CLI or node authority. Installed pseudoterminal coverage submits a self-note, verifies it
through `hq list --all`, restarts the daemon, and verifies the same canonical message again.

Agent details in the installed TUI use the ordinary managed-session endpoint for `s` explicit-
provider start, `e` exact resume, and `t` local-runtime stop. A fresh or switching target is never
derived from terminal text: the model emits an exact stable agent/provider/session command, and the
shared harness CLI workflow captures launch context and owns retry/reconciliation. The TUI renders
ready, stopped, rejected, and uncertain as distinct outcomes and does not equate durable selection
with runtime presence. Installed pseudoterminal coverage exercises explicit-provider start and a
typed rejection; installed CLI restart coverage exercises stop and stale exact resume.

`identity init|show|export|import` are deliberately offline operations. `config get|set|themes`
uses the running daemon when present and otherwise acquires exclusive offline ownership. If daemon
startup races the offline probe, configuration retries through the daemon rather than surfacing the
state-lock error. Initialization and import never overwrite an identity. Export and import require
an absolute package path and the literal `--password-stdin`; exactly one bounded UTF-8 password line
is then consumed from stdin, normalized and zeroized. The secret is never accepted as argv, echoed,
retained in a diagnostic, or sent over the local API. A closed, oversized, malformed, or multiline
input fails explicitly. Other commands do not read stdin.

`config get` includes the optional startup TUI theme in human and JSON output. `config themes`
lists the reserved bundled selectors plus discovered user files, marks the active choice, retains
scheme name and author, and reports invalid definitions without requiring the user to guess a
name. `config set theme NAME_OR_ABSOLUTE_PATH` validates and persists a selection;
`config set theme none` restores automatic `gruvbox-dark-medium`/`NO_COLOR` behavior. Named files
are found in `$XDG_CONFIG_HOME/hq/themes` or `~/.config/hq/themes`. Resolution happens before raw
mode or the alternate screen, so a missing or invalid selected file produces an ordinary actionable
terminal diagnostic. See [TUI themes](../tui-themes.md) for built-ins, Base16 import, and the native
role schema.

`config set codex.yolo true` makes newly launched managed Codex processes use approval policy
`never` and sandbox mode `danger-full-access`; set it to `false` to restore Codex defaults. Because
the launch resolver reads committed configuration for every new process, no daemon restart is
required. `config set codex.model MODEL|none` follows the same future-process boundary. Existing
processes and durable session/thread bindings do not change. The TUI Config page uses the same
field-specific daemon API, reloads on entry or refresh, and applies an acknowledged theme change
immediately. Only enable Codex yolo inside an externally secured environment.

`human create [LABEL]` starts or connects to the node, reconciles the reserved human mailbox,
authors the installation's deterministic but separately namespaced creator-account identity when
absent, and selects it. Repeating the command, racing an identical creator command, losing a
response, or restarting converges from a fresh authoritative snapshot without another fact.
Changed immutable creator data, conflicting roots, or ambiguous authority fail closed. `human show`
renders all projected accounts plus the complete local selection candidates and unique active
selection. `human select ACCOUNT_ID` requires the exact creator root or one currently active
membership acceptance and cites the complete prior selection frontier.

`human invite INSTALLATION_ID SIGNING_KEY ABSOLUTE_PATH [--label LABEL] [--relay URL]...`
requires the uniquely selected creator account. It derives a deterministic grant identity from the
exact target, signed metadata, and complete current membership frontier; authors or reconciles that
grant; requests its exact transitive canonical ancestry from the node; re-verifies the result; and
creates one new mode-`0600` regular file without overwrite. The path is never rendered. A changed
frontier produces a distinct regrant identity.

`human join ABSOLUTE_PATH` reads one bounded regular non-symlink file, requires its unique canonical
pairing encoding, verifies every embedded signature and the complete grant ancestry offline,
requires the exact local installation and signing key named by the grant, and reduces the evidence
before importing it. It then reconciles the reserved human mailbox, idempotently imports the exact
events, authors the target-key acceptance through a pure plan, and selects the account. A lost
ordinary import response may be retried once because exact evidence ingestion is idempotent;
re-running the command after any interruption converges without another revision.

`human devices` inspects the uniquely selected account and renders the permanent creator plus every
projected non-creator membership in installation-ID order. Each member retains every creator grant,
acceptance, revoke, and causal-frontier fact plus every observed signing key; output never chooses a
historical grant. The closed presentation state is `creator`, `pending`, `active`, `revoked`,
`conflicted`, or `incomplete`. Multiple current grant lineages are conflicted, while an internally
unsupported projection is incomplete and cannot be used for mutation planning.

`human revoke INSTALLATION_ID` requires the selected account's exact creator installation. It
rejects the creator, non-creators, missing/conflicted/incomplete membership, and ambiguous grant
attribution. The pure plan cites the account root, exact attributed grant, and complete current
membership frontier. Repeating an already projected revoke is a no-op, while lost mutation
responses replay the same framed command identity. The admitted account-addressed revoke is always
queued directly to its named device before any separately requested peer-route block can prevent
ordinary routing.

`peer add INSTALLATION_ID SIGNING_KEY ENCRYPTION_KEY [--label LABEL] [--relay URL]...`
authors one local installation-private directional route. The signing key is the peer authority;
the encryption key and signed relay locators are transport metadata only. An exact current route is
reused. A changed route or recovery after a block cites the complete causal-maximal route frontier.
`peer list` exposes the derived `routable`, `blocked`, or `conflicted` state together with every
retained route, block, frontier identity, exact public key, label, and relay hint. It never chooses
a winner from a conflicted frontier.

`mailbox list` exposes every locally owned installation-qualified mailbox and complete capability
lineage. `mailbox grant MAILBOX_ID PEER_INSTALLATION_ID` requires exactly one current routable peer
candidate, binds its exact signing key, and cites the exact local mailbox creation fact. An exact
active capability is reused; regrant after revocation cites every retained revoke maximum and
creates a distinct stable grant identity. `mailbox revoke MAILBOX_ID PEER_INSTALLATION_ID` revokes
the one exact active grant with its complete retained support and is a no-op for an already revoked
lineage. Missing ownership, route conflict, and ambiguous capability history fail closed.

`peer distrust INSTALLATION_ID` first revokes every active locally owned mailbox capability for the
peer, one committed revision at a time, and only then authors a full-frontier route block. Repeating
an already blocked state is a no-op. A later explicit `peer add` may recover the route by citing the
block frontier, but it does not silently reactivate old capabilities; mailbox access requires a new
lineage-complete grant.

`relay add URL [--access read|write|read-write] [--auth
disabled|on-challenge|required]` installs or changes one exact durable `ws`/`wss` policy. The
defaults are `read-write` and `on-challenge`. Repeating an equal desired policy is an `unchanged`
no-op; a change advances its positive generation. `relay remove URL` disables the policy without
erasing delivery history, and repeating removal is also unchanged. At most 256 policies are
accepted. Invalid or credential-bearing URLs fail with redacted usage output.

`relay sync [URL]` sends a coalescible prompt wake for all policies or one validated enabled policy;
an absent or disabled target returns a typed rejected outcome.
`relay list` and `relay status` render the bounded policy set, enabled state and generation, queued
and prepared work, accepted/rejected/uncertain attempts, staging, quarantine, and an explicit
truncation flag. `relay repair` is the deliberate all-domain rebuild operation: it derives a stable
audit identity from the observed revision, reverifies immutable evidence, atomically replaces only
rebuildable indexes, and reports health for authority, conversation, agent, and project domains.
Policy and sync effects preserve exact operation/request identities across response loss; policy
loss is first reconciled from status before an exact retry. Repair retries the same idempotent
operation identity.

`agent create NAME [--mailbox MAILBOX_ID]` creates one deterministic installation-local agent
mailbox or adopts one existing local agent mailbox, then authors the permanent lowercase name
claim through pure application planners. Repeating the command after success, response loss, or a
partial mailbox-only commit converges from a fresh authoritative snapshot. A conflicting or retired
reservation, remote mailbox, non-agent mailbox, or incompatible partial state fails closed.
`agent list` retains active, conflicted, and retired catalog rows; `agent show NAME|AGENT_ID`
requires one unambiguous identity.

`agent current` resolves exactly one Codex, Claude Code, Pi, or explicit
`HQ_PROVIDER`/`HQ_SESSION` environment identity to its immutable local mailbox binding. Built-in
and custom identities are one combined ambiguity set: multiple or partial sources fail with a
redacted diagnostic. `agent select NAME|AGENT_ID [--provider PROVIDER --session SESSION]
[--dir PATH]` cites the exact name claim, immutable binding, matching typed repository context, and
complete prior selection frontier. `agent rename NAME|AGENT_ID DISPLAY_NAME` and `--clear` cite the
exact claim/binding and complete rename frontier; rename never selects or starts a runtime. Without
an explicit pair, selection uses the one current provider environment, while rename may use that
environment or the unique durable selection.

`agent retire NAME|AGENT_ID --yes [--force]` is a node-owned local API workflow. It binds the exact
active claim and rejects stale, conflicted, retired, wrong-home, inactive-human, or multiply
assigned state. An idle agent commits one transaction-consistent installation-private retirement.
An assigned agent instead enters the owning project's durable saga, quiesces its runtime, ends the
assignment, and only then retires. Definite or uncertain graceful-stop failure keeps the assignment
blocked and HQ authority intact; only an explicit `--force` retry may revoke it, and output exposes
`failed` or `uncertain` runtime truth and its stable code. Exact command frames replay across local
response loss, nonterminal sagas repair after restart, and catalog history retains the retired name.
Managed runtime start/resume/stop remains a separate node-owned workflow.

`harness start --agent NAME|AGENT_ID --provider PROVIDER [--dir PATH]` and `harness resume
--agent NAME|AGENT_ID --provider PROVIDER --session SESSION [--dir PATH]` resolve exactly one active
named agent, canonicalize the supplied directory (or caller working directory), and copy the complete
caller environment at the local-API boundary. Environment names remain UTF-8 while values are
preserved as binary sensitive data. `resume` names one exact provider session and never falls back to
start. `harness stop --agent NAME|AGENT_ID --provider PROVIDER` stops only the local runtime and does
not erase durable history. Every invocation generates one operation identity and derives its digest
from that identity, timestamp, agent, provider, action, absolute directory, and complete environment;
the reconnecting client replays the exact framed request after response loss. Output is a passive
public-field view with `ready`, `stopped`, `rejected`, or `uncertain` status. Rejection exits 1;
uncertainty exits 3 and exposes both operation and reconciliation identities without launch inputs.

`project list` and `project show PROJECT_ID` start or connect to the node and derive their complete
result from one fresh authoritative local-API snapshot. Projects are ordered by identity and expose
their immutable home, lifecycle, archive state, canonical head, accepted input sequence, desired
resources, resource health, primary flag, advisory claims, and every overlapping project. Accepted
inputs join dispatches by message identity, and dispatches join retained outputs by dispatch
identity; records with unavailable attribution are counted explicitly instead of being assigned to
a guessed project. Changed duplicate identities and project-owned records without a project fail
closed. Remote commands retain their command/request/operation attribution and structured queued,
received, terminal, or conflicted progress, including receipt/head/outcome facts, committed or
rejected result, and exact runtime success/failure/uncertainty. Both human and JSON output are
deterministic, and the same catalog survives a node restart without a separate CLI cache.

`project resource list PROJECT_ID` and `project resource show PROJECT_ID RESOURCE_ID` select desired
resources from that same fresh snapshot. They preserve the stable resource identity and expose both
the normalized display locator and immutable canonical locator, primary selection, projected
health, active advisory claim, and every conflicting project. `project check PROJECT_ID
[RESOURCE_ID]` then crosses the existing read-only resource-inspection port once per selected
resource in stable identity order. It reports the freshly observed canonical locator, health,
clean/dirty/unknown/not-applicable release state, checked time, rejection, response loss, and
reconciliation identity without changing the filesystem, Git, desired membership, or claims.
Because resource namespaces and adapters are co-located with the immutable home in v1, a check for
a project homed on another installation fails closed instead of inspecting the caller's local path.
The unshipped local API v1 is evolved in place with no compatibility shape or storage migration.

`project resource add PROJECT_ID --path ABSOLUTE_PATH [--primary]`, `remove PROJECT_ID RESOURCE_ID
[--force]`, `replace PROJECT_ID RESOURCE_ID --path ABSOLUTE_PATH`, and `primary PROJECT_ID
RESOURCE_ID` use the same durable expected-head command path as lifecycle control. Add and replace
allocate stable resource identities from the command operation and carry only normalized display
locators; the immutable home performs canonical identification before its serialized mutation.
Replace is atomic, and primary selection changes the future launch default without reordering
membership. Remove requires force only while assigned. None of these commands deletes or modifies
paths, repositories, worktrees, branches, or files, including across close and archive.

`project create NAME --path ABSOLUTE_PATH [--brief TEXT] [--home INSTALLATION_ID]` creates an
initially open project over one existing directory. The caller sends only its normalized absolute
spelling; the selected authoritative home resolves the canonical filesystem identity and requires a
healthy directory before committing the project and its primary advisory claim. The default home is
the local installation; an explicit home must be the account creator or exactly one active member.
Command, workflow, project, mailbox, and resource identities are stable for one exact framed
request, while changing any request content changes its digest. The create workflow has no prior
project head, replays byte-for-byte after response loss, and rejects overlapping concurrent claims.
Output exposes accepted, running, completed, rejected, or reconcilable workflow truth; rejection
exits 1 and reconcilable uncertainty exits 3. Creation and catalog state survive restart without client-side
state. Because HQ has no shipped installations, this capability extends local API v1 in place and
adds no storage migration or compatibility shape.

`project worktree NAME --source ABSOLUTE_PATH --destination ABSOLUTE_PATH --branch BRANCH
[--create-branch BASE] [--brief TEXT] [--home INSTALLATION_ID]` reserves and provisions one exact
Git worktree before creating its project. With `--create-branch`, the home validates the base as an
exact commit and creates `BRANCH` from it. Without that option, `BRANCH` must already exist. Source
and destination are normalized lexically by the client but observed only on the selected home, so
remote-home requests never inspect the caller's filesystem namespace. Command, operation, project,
mailbox, and resource identities remain stable through replay.

The home checkpoints reservation, Git intent/result, resource identification, and canonical
creation independently. A lost Git success response is repaired by reconciling the common
repository, destination, and branch; HQ does not issue a second mutation once exact created state
is visible. Rejected and reconcilable output includes a typed `worktree_may_exist` warning with the
exact destination and branch whenever Git may have retained external state. HQ never prunes,
resets, removes, or otherwise compensates that worktree or branch automatically.

`project send PROJECT_ID [MESSAGE]` authors an ordinary asynchronous account message whose typed
project ID and direct recipient name the project's immutable mailbox. With no argument it consumes
one bounded UTF-8 body from stdin. The selected local human account must match the project account;
ambiguous membership or project identity fails closed. The authoritative home assigns each usable
message the next contiguous input sequence independently of lifecycle or assignment, so work sent
to a closed, archived, or unassigned project remains pending rather than reopening or dispatching
it. Message and acceptance mutations retain deterministic identities across response loss. Human
output is exactly `project=ID message=ID`; JSON adds the same IDs to `hq-cli-output-v1`.

`project open PROJECT_ID`, `project close PROJECT_ID --yes [--force]`, `project archive
PROJECT_ID`, and `project unarchive PROJECT_ID` submit through the same stable project-command
builder. Each command resolves the selected active account, immutable project home, and exact
canonical head from one fresh authoritative snapshot; a delayed command therefore rejects as stale
instead of applying to a newer project state. Close always requires `--yes`. `--force` is a
separate authorization for dirty/unknown release or failed/uncertain runtime cessation and never
acts as confirmation. Archive gracefully closes an open project before hiding it; unarchive
restores presentation while leaving the project closed, and `open` reacquires its advisory claims.
All four commands preserve accepted inputs and desired resources and render accepted, running,
completed, rejected, or reconcilable workflow truth with runtime failure/uncertainty details.
Exact requests replay after local response loss, and durable workflow checkpoints repair after
restart. This extends local API v1 in place because no HQ installation has shipped.

`project activate PROJECT_ID --agent NAME|AGENT_ID --provider PROVIDER --new-session [--thread
THREAD_ID] [--dir ABSOLUTE_PATH]` starts a new provider session. The optional thread must be an
authoritative historical thread for that exact project and agent. `project activate PROJECT_ID
--agent NAME|AGENT_ID --provider PROVIDER --session SESSION --thread THREAD_ID [--dir
ABSOLUTE_PATH]` instead resumes an exact authoritative agent/provider/session/project/thread tuple
and never falls back to a new session. The agent mailbox must belong to the immutable project home.
Without `--dir`, the CLI uses the sole authoritative primary resource; the home still validates the
launch directory and current claims. `project dispatch PROJECT_ID` reconciles and drains all pending
accepted inputs in sequence through the same stable expected-head command path. Catalog output
includes the current assignment and deduplicated historical thread bindings as passive public-field
records. These additions evolve the unshipped local API v1 in place without a migration, compatibility
shape, or second stored representation.

`project handoff PROJECT_ID --agent NAME|AGENT_ID --provider PROVIDER (--new-session | --session
SESSION) --thread THREAD_ID [--dir ABSOLUTE_PATH] --yes [--force]` resolves the current assignment
and exact target history from the same authoritative snapshot. Handoff always requires explicit
confirmation. `--force` is independent takeover authority used only after normal quiescence is
blocked or uncertain; it does not replace `--yes`. The stable project-command path preserves stale
head rejection, response-loss replay, restart repair, and complete workflow/runtime rendering.

`agents [messaging|retry|synchronization|delivery|causality|administration]` is installed guidance
for agents. It explains stable retry identity, explicit sync, at-least-once completion, inert
dependency-incomplete history, and the boundary that humans own identity, authority, durable
selection, and retirement administration.

Identity output has only the installation ID, signing public key, and public fingerprint.
Configuration output has the optional provider, theme, and Codex
defaults (yolo and optional model). Identity and configuration are
passive data with public fields. Configuration setters replace one typed field through the current
exclusive owner and return the complete committed value. Relay policy is changed only through
`hq relay add/remove`. Human, peer, mailbox, relay/health, route-history, capability-history,
named-agent, session, project-catalog, and project-operation presentation records are also passive
public-field values; command enums
and the live client capability remain closed behavioral types. The clean unshipped local API v1
contract carries exact agent claims, mailboxes, immutable binding facts, and selection/rename
candidates and frontiers in place; there is no compatibility accessor, migration, or version bump.

The installed TUI Agents section uses the same catalog and command workflows. `/` searches stable
agent, name, provider, session, and display-name identities; Enter inspects durable history; `c`
creates; `r` renames or clears one exact selected session; and `x` opens permanent retirement
confirmation, with `f` as an explicit force toggle. These actions add no TUI-specific authority,
storage record, local-API version, migration, or compatibility facade.

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
semantic operation. Ordinary requests are never replayed after response loss. Exact mutation,
project-command, named-agent-retirement, and managed-session frames retain their stable identities
and replay byte-for-byte. Managed-session calls return explicit uncertainty for caller-visible
reconciliation rather than waiting for a fabricated definite result. Snapshot-oriented clients may request a fresh view
after negotiation; command-only clients do not issue an unsolicited snapshot.

CLI production code has no canonical storage, signer, relay, resource, harness-provider, or SQLite
access. The identity/configuration commands cross only the private state-ownership and identity
persistence adapter because they must operate while the node is absent. Canonical administration,
including human account bootstrap, peer routes, mailbox capabilities, named-agent catalog/session
metadata, and relay health/repair, uses fresh snapshots plus
the reusable request,
mutation, and project methods rather than
opening implementation adapters directly.
