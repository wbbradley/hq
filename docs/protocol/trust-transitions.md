# HQ protocol trust transitions

Status: normative boundary model

This specification defines the only path from attacker-controlled event bytes to reducer input.
Each state is a distinct owned type. Naming a later state does not permit an implementation to
store an enum flag beside one mutable generic event and expose fields prematurely.

## State machine

```text
RawEventBytes
  -> ParsedOuterEvent
  -> CryptographicallyVerifiedEvent
       -> VerifiedUnsupportedRecord
       -> VerifiedSupportedRecord
            -> SemanticFact
                 -> ReducerAdmission
```

Any arrow may terminate in a typed failure. There is no arrow from a failed or unsupported value
to `SemanticFact`.

| State | Proven | Accessible data | Explicitly not proven |
| --- | --- | --- | --- |
| `RawEventBytes` | ingress supplied a bounded byte slice owner | exact received bytes and source-local diagnostics | UTF-8, JSON, identity, signature, protocol |
| `ParsedOuterEvent` | strict bounded outer JSON shape and primitive syntax | exact raw bytes plus typed candidate id/pubkey/time/kind/tags/content/signature slices | claimed ID, key validity, signature, content protocol |
| `CryptographicallyVerifiedEvent` | NIP-01 preimage reconstruction, SHA-256 ID equality, BIP-340 public-key/signature validity, HQ kind/tag/time carriage rules | exact outer bytes, exact reconstructed preimage bytes, exact content bytes, verified ID/key/time | supported discriminator/version/family, canonical content bytes, semantic validity, authority |
| `VerifiedUnsupportedRecord` | valid signed carriage plus a bounded inspectable but unsupported discriminator/version/family | exact verified bytes, ID/key, recognized dispatch fields, unsupported reason | DTO validity, canonical re-encoding, semantic payload, reducer eligibility |
| `VerifiedSupportedRecord` | supported namespace/version/family, strict bounded DTO parse, exact canonical re-encoding equality, namespace/scope/reference and intrinsic DTO validation | owned protocol DTO plus every exact verified byte sequence | historical parent availability, authorization, concurrency decision |
| `SemanticFact` | lossless owned conversion to one domain family; verified event ID/author/time/scope/causal mapping and all domain value invariants | only validated domain values, with provenance handle to retained verified bytes outside the pure value | projection, historical authority, acceptance |
| `ReducerAdmission` | complete reducer classified the fact against the causal batch | normalized accepted, unresolved, unauthorized, invalid, conflicted, or unsupported decision and blockers | external side effects, delivery, runtime truth |

`VerifiedUnsupportedRecord` never exposes `SemanticFact`. Neither do parse, hash, signature,
canonicalization, intrinsic-conversion, or bound failures. A `SemanticFact` is safe to inspect as a
validated claim but is not proof that its claim is historically authorized or projected.

## Required transition procedure

### Raw bytes to parsed outer event

1. Reject more than `MAX_EVENT_BYTES` before copying or parsing.
2. Validate UTF-8 and the exact seven-member outer object, member order, JSON grammar, fixed-width
   lowercase hex, integer ranges, empty tag array, and absence of trailing data.
3. Copy or retain slices into immutable ownership. A parser must not normalize content, replace
   invalid bytes, deduplicate members, or discard the original representation.

Failure classes at this transition are `event-too-large`, `outer-invalid-utf8`,
`outer-malformed-json`, `outer-duplicate-member`, `outer-unknown-member`, `outer-member-order`,
`outer-field-shape`, `outer-trailing-data`, and `content-too-large`.

### Parsed outer event to cryptographically verified event

1. Require provisional kind 6000 and exactly empty tags. Other kinds are `wrong-kind`; they are not
   HQ unsupported versions because their signing carriage contract is unknown.
2. Reconstruct exact NIP-01 ID preimage `[0,pubkey,created_at,kind,tags,content]` with the NIP-01
   escaping rules. Hash with SHA-256 and compare all 32 claimed ID bytes in constant-time where the
   crypto library supports it.
3. Parse the x-only public key and BIP-340 signature canonically and verify the signature over the
   recomputed event ID. Never verify the claimed ID without first recomputing it.
4. Preserve received event bytes, reconstructed preimage bytes, and exact decoded content bytes in
   immutable verified storage.

Failures are `wrong-kind`, `nonempty-tags`, `event-id-mismatch`, `invalid-public-key`,
`invalid-signature-encoding`, and `bad-signature`. All are cryptographic-boundary failures and
produce no verified record.

### Cryptographic verification to protocol dispatch

The dispatcher reads only enough content to bound and recognize `p`, `v`, and `f`; it does not
construct a domain payload.

- A supported triple enters strict DTO parsing.
- A well-formed recognized discriminator with an unknown nonnegative version becomes
  `VerifiedUnsupportedRecord(unsupported-version)`.
- A supported discriminator/version with a positive unknown family becomes
  `VerifiedUnsupportedRecord(unsupported-family)`.
- A well-formed unknown discriminator may become
  `VerifiedUnsupportedRecord(unsupported-protocol)` under the bounded retention policy.
- A family from the other HQ namespace is `namespace-confusion`, a malformed supported-kind event,
  not an extension.

An unsupported record keeps exact bytes so a future implementation can re-evaluate it, but storage
must bound unsupported count and bytes independently from canonical fact retention. It cannot
enter dependency indexes, authorization, or reduction.

### Dispatch to verified supported record

1. Parse the complete DTO using streaming depth/member/count/allocation limits.
2. Reject missing, duplicate, unknown, or reordered members; invalid integers, UTF-8, escapes,
   enums, fixed widths, optionals, collection order/uniqueness, and decoded semantic bounds.
3. Validate protocol/family range, author/scope agreement, family/body agreement, typed parent
   namespace, unique/sorted parents, unique/sorted roles, role applicability, and every authority's
   presence in parents.
4. Canonically encode the owned protocol DTO and reject unless its bytes equal the retained content
   bytes exactly. Check `MAX_CONTENT_BYTES` again on the final escaped form.
5. Validate the relation between content millisecond time and outer event seconds.

Failures use `content-*` shape classes, `decoded-bound`, `encoded-bound`, `invalid-hex`,
`unknown-enum`, `noncanonical-bytes`, `namespace-confusion`, `authority-not-parent`,
`duplicate-authority-role`, `scope-author-mismatch`, `family-body-mismatch`, and
`authored-time-mismatch`.

### Verified supported record to semantic fact

Conversion consumes the protocol DTO and creates owned validated domain values. It may fail only
with stable intrinsic conversion classes: `domain-value-invalid`, `payload-invariant`,
`scope-payload-mismatch`, or `author-subject-mismatch`. It performs no clock lookup, database query,
relay lookup, key discovery, reducer call, or I/O.

Event ID becomes fact ID; content author plus verified public key becomes installation address;
content time becomes authored time; signed scope becomes domain scope; typed refs become bounded
causal references; and the mapped body becomes exactly one semantic payload. The protocol DTO is
dropped only after conversion succeeds, while immutable verified bytes remain addressable for
audit/storage.

### Semantic fact to reducer admission

Reducer admission is the first stage allowed to consult the complete fact batch and derived causal
reachability. Missing parents are unresolved; present but historically insufficient authority is
unauthorized; intrinsic catalog violations discovered from referenced facts are invalid; concurrent
aggregate ambiguity is conflicted; supported semantic facts outside a reducer capability are
unsupported. No timestamp, relay receipt, signature validity, or parse order selects a winner.

Reducer rejection does not retroactively make a signature invalid. Exact canonical facts and their
normalized decisions are retained according to the semantic catalog; operational rejected-input
retention remains separate and bounded.

## Failure ownership and diagnostics

| Failure family | Owner | Retry/reclassification rule |
| --- | --- | --- |
| ingress size, UTF-8, JSON, primitive shape | protocol parser | permanent for exact bytes |
| event ID, key, signature, kind/tags | cryptographic carriage | permanent for exact event |
| unsupported protocol/version/family | dispatch registry | may become supported only after a software upgrade; exact bytes unchanged |
| canonical bytes, bounds, namespace/scope/DTO invariant | protocol conversion | permanent for exact event under this version |
| missing causal parents | reducer | may resolve when exact parents arrive |
| historical authority or subject mismatch | reducer | deterministic from complete batch; may retract only as specified by causal reduction |
| local storage/I/O/resource exhaustion | adapter/operations | not a semantic decision and must not be serialized as one |

Diagnostics may include stable failure class, stage, bounded field path, observed count/length, and
expected limit. They must not log secret keys, decrypted envelope plaintext outside explicit secure
debug policy, or unbounded attacker input. Cryptographic failures should avoid distinctions that
would create a signing or key oracle at an unauthenticated remote boundary.

## Preservation and replay invariants

- Verification is a pure function of exact event bytes, pinned protocol registry, and crypto
  implementation; it never depends on arrival order or ambient time.
- Re-encoding is a validation operation, not a replacement for retained bytes.
- A verified event cannot change state in place. Advancing creates a new typed owner that contains
  or references the prior immutable evidence.
- Replaying identical bytes yields the identical event ID, trust branch, DTO, semantic fact, and
  reducer input. Same claimed ID with unequal bytes cannot pass event-ID verification.
- Unsupported input is namespace-isolated and bounded. It cannot masquerade as a missing parent,
  authority fact, canonical fact, or control record.
- Domain and reducer APIs do not accept raw JSON, Nostr events, protocol DTOs, or an unchecked
  boolean such as `verified=true`.

Exact positive vectors are
[canonical-installation-v1.json](vectors/canonical-installation-v1.json) and
[remote-command-v1.json](vectors/remote-command-v1.json). Exact negative cases and their last
permitted states are in [adversarial-v1.json](vectors/adversarial-v1.json).
