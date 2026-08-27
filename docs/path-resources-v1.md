# Path-resource identity and observation contract v1

Status: first-release adapter contract

`hq-resources` owns read-only filesystem and Git observation. Pure project mutation and claim
policy remain in the inward domain/reducer packages. The adapter never creates, moves, rewrites, or
deletes a path, worktree, branch, or repository, and its reports never grant lifecycle authority.

## Durable identity

A path resource is qualified by an immutable installation home and stable resource ID. Its passive
fact value has public `display_locator`, `canonical_locator`, and `health` fields. The display value
is the normalized absolute UTF-8 spelling selected by the human. The canonical value is immutable
identity used for claim aggregates and conflict checks. Both use the working-tree scheme.

Input is rejected before observation when it is relative, non-UTF-8, contains NUL, or exceeds the
4,096-byte locator bound. On the supported Linux and macOS targets, normalization removes `.` and
lexically resolves `..` without escaping the root.

For a missing selection, the adapter walks upward with bounded work until the nearest observable
existing ancestor, resolves that ancestor through symlinks, then appends the missing suffix. A
missing path therefore retains a deterministic reservation identity. Permission denial, broken
links, non-directories, and unknown observation failures remain distinct closed conditions and
never look healthy.

Revalidation resolves the recorded display spelling again. If its current canonical result differs
from the recorded canonical locator, the report is degraded with `IdentityChanged`; it exposes the
new observation but does not rewrite identity. Inspection reports include home and resource ID.

## Claims, primary selection, and launch directories

Only canonical working-tree locators participate in component-aware equal/ancestor/descendant
comparison. Overlap conflicts only across distinct projects in the same home; overlap within one
project is allowed. Conflict reports retain both sides' project/resource IDs and display/canonical
locators. Persisted active claims remain an advisory ledger, not an operating-system lock.

Primary selection accepts one explicit selected resource or deterministically uses the first
human-selected resource. Launch validation checks exactly the supplied absolute directory. A
healthy directory is either claimed or explicitly outside claims; an unhealthy directory is
unavailable. The adapter never substitutes or relocates to a claimed path.

## Git release observation

Release assessment first revalidates path identity. Non-Git paths are `NotApplicable`; unhealthy,
changed-identity, inaccessible, malformed, subprocess, malformed-output, or decoding failures are
`Unknown`. Dirty and unknown results require an explicit force decision; clean and not-applicable
results proceed. Force is evidence of human acceptance and does not alter files.

Git execution is read-only, synchronous, and bounded by wall time and inclusive stdout size.
stdin and stderr are discarded. Only closed change classes and a count are retained—never file
paths, contents, environment, command diagnostics, or stderr. Porcelain v1 `-z` parsing validates
records and handles rename/copy source records structurally. Worktree top level and absolute common
Git directory are separate identities, so linked worktrees remain distinct path claims even when
they share repository maintenance data.

## Effects and audit boundary

`PathSystem` and `GitRunner` are injectable capabilities; their standard implementations are the
only filesystem/process boundary. Requests and reports are passive public-field values. The
adapter's owned capabilities remain private because replacing or bypassing them would violate its
bounds. Every decision retains sufficient typed identity and classification for a caller to record
an auditable canonical observation without persisting raw operating-system data.
