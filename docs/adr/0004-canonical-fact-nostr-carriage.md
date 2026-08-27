# ADR 0004: Canonical fact Nostr carriage

Status: Accepted

Date: 2026-08-27

## Context

HQ needs an immutable signed event boundary for canonical facts and remote-control records. The Go
implementation's event kinds and encodings are frozen scenario evidence, not compatibility
requirements. Choosing a wire value by copying them would silently couple the clean-sheet Rust
system to a schema that this rewrite explicitly does not preserve.

Nostr standards and the kind registry are living documents. This decision was checked against:

- [NIP-01 at NIPs revision
  `dabfcb2aaecf4fa374eda8b1232ab303a03f60ba`](https://github.com/nostr-protocol/nips/blob/dabfcb2aaecf4fa374eda8b1232ab303a03f60ba/01.md),
  including its event object, event-ID serialization, BIP-340 signature, and kind-behavior ranges;
- [the registry-of-kinds at revision
  `1159ee2f92af3d1b78f888528dcfb260a78baf80`](https://github.com/nostr-protocol/registry-of-kinds/tree/1159ee2f92af3d1b78f888528dcfb260a78baf80);
  and
- [BIP-340](https://bips.xyz/340)'s x-only public-key and tagged-hash Schnorr verification rules.

At those revisions, kind `6000` is in NIP-01's regular-event range and does not appear in the
registry. The registry is explicitly non-exhaustive, so absence is not proof of global uniqueness.

## Decision

HQ provisionally carries both canonical fact v1 and remote-control v1 in NIP-01 regular event kind
`6000`. Events have an empty tag array. The exact canonical protocol record is the event `content`;
audience and causal information are signed inside that content rather than duplicated in mutable or
unencrypted routing tags. A later encrypted-envelope specification may wrap the same verified event
using NIP-59 without changing its identity.

Kind `6000` is provisional and unregistered. It is suitable for private development and controlled
interoperation only. Public interoperability requires checking the then-current registry,
coordinating or registering a kind, and recording a superseding ADR. Implementations must not imply
that this repository owns the number.

The carriage kind is not a content protocol version. `hq/canonical` and `hq/control` have independent
version spaces, registries, and DTOs. A future kind migration changes only the outer carriage rule;
it must not reinterpret or silently change canonical v1 content bytes. Conversely, a new content
version does not acquire a new kind merely because its DTO changes.

The normative details are split by responsibility:

- [canonical-fact-v1.md](../protocol/canonical-fact-v1.md) defines canonical records and exact
  NIP-01 construction;
- [remote-control-v1.md](../protocol/remote-control-v1.md) defines the separate control namespace;
- [payload-mapping-v1.md](../protocol/payload-mapping-v1.md) owns every DTO-to-semantic mapping; and
- [trust-transitions.md](../protocol/trust-transitions.md) defines verification states and failures.

## Consequences

- No Rust domain type, Go struct, relay wrapper, or database row is a wire schema.
- The content protocol can be tested and versioned independently of relay transport and encryption.
- Exact content and event bytes must survive verification because identity and canonicality depend
  on them.
- Unknown supported-kind content can be retained as cryptographically verified but unsupported
  without exposing a semantic fact.
- Before public use, kind collision risk remains an explicit release blocker rather than an
  accidental compatibility promise.
