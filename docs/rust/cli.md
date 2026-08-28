# Rust CLI

HQ installs one `hq` executable. Global options precede the command:

```text
hq [--output human|json] [--state-root ABSOLUTE_PATH] <COMMAND>
```

The installed commands currently include `help`, `version`, `identity`, `config`,
`human create|show|select|invite|join|devices|revoke`,
`peer add|list|distrust`, `mailbox list|grant|revoke`, and
`relay add|list|remove|sync|status|repair`, and
`daemon run|status|readiness|stop|restart`. `daemon run` is the
internal foreground ownership role used by
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

Identity output has only the installation ID, signing public key, and public fingerprint.
Configuration output has the optional provider and the complete canonical relay list. Both are
passive data with public fields. Configuration setters replace one complete typed field, rebuild
the validated value, and the persistence adapter revalidates public fields again immediately before
the atomic write. Human, peer, mailbox, relay/health, route-history, and capability-history presentation records
are also passive public-field values; command enums and the live client capability remain closed
behavioral types.

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
persistence adapter because they must operate while the node is absent. Canonical administration,
including human account bootstrap, peer routes, mailbox capabilities, and relay health/repair, uses fresh snapshots plus
the reusable request,
mutation, and project methods rather than
opening implementation adapters directly.
