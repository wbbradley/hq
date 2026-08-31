# HQ canonical fact protocol v1

Status: normative

Protocol discriminator: `hq/canonical`

Version: `1` in an independent version space

Catalog families: `1`, `2`, `3`, `4`, `5`, `6`, `7`, `8`, `9`, `10`, `11`, `12`, `13`,
`14`, `15`, `16`, `17`, `18`, `19`, `20`, `21`, `22`, `23`, `24`, `25`, `26`, `27`, `28`,
`29`, `30`, `31`, `32`, `33`, `34`, `35`, `36`, `37`, `38`, `39`, `40`, `41`, `42`, `43`,
`44`, and `45`.

This specification defines the only canonical fact v1 byte representation. It does not reuse a
Rust domain struct, the frozen Go schema, a relay envelope, or a database representation. Exact
payload mappings are owned by [payload-mapping-v1.md](payload-mapping-v1.md), and trust states are
owned by [trust-transitions.md](trust-transitions.md).

## Named limits

All limits are inclusive. Byte lengths are UTF-8 octets, not Unicode scalar counts. A decoder
checks raw limits before allocation, decoded semantic limits after unescaping, and final encoded
limits after escaping.

| Name | Value | Applies to |
| --- | ---: | --- |
| `MAX_EVENT_BYTES` | 4,194,304 | Complete outer event JSON bytes |
| `MAX_CONTENT_BYTES` | 1,048,576 | Exact UTF-8 bytes in event `content` |
| `MAX_JSON_DEPTH` | 16 | Arrays and objects, with the top object at depth one |
| `MAX_OBJECT_MEMBERS` | 16 | Members in any one object |
| `MAX_COLLECTION_ITEMS` | 64 | Any array unless a smaller named limit applies |
| `MAX_PARENT_REFS` | 64 | `parents` entries |
| `MAX_AUTHORITY_REFS` | 8 | `auth` entries |
| `MAX_SHORT_TEXT_BYTES` | 128 | Decoded labels, names, keys, items, runtime strings, branches |
| `MAX_CONTENT_TEXT_BYTES` | 16,384 | Decoded message, brief, diagnostic, command, and reason text |
| `MAX_LOCATOR_TEXT_BYTES` | 4,096 | Decoded locator value |
| `MAX_PROVIDER_ID_BYTES` | 64 | Decoded provider namespace |
| `MAX_PROVIDER_SESSION_BYTES` | 256 | Decoded provider session identity |
| `MAX_RELAY_HINTS` | 8 | Relay locator entries |
| `MAX_RESOURCE_ITEMS` | 64 | Initial project resources |

A fixed-width hex value is exactly 64 lowercase ASCII hexadecimal characters representing 32
bytes. Text must also satisfy its semantic nonempty/slug/locator rules. A value within a wire limit
may still fail semantic conversion.

## Canonical content record

The event content is one UTF-8 JSON object with exactly these members in this order:

```text
{"p":P,"v":V,"f":F,"author":A,"time":T,"scope":S,"parents":R,"auth":H,"body":B}
```

- `p` is exactly `"hq/canonical"`.
- `v` is exactly integer `1`.
- `f` is one registered integer from 1 through 45 and selects exactly one body DTO.
- `author` is the author's installation ID as fixed-width hex.
- `time` is a nonnegative signed-64-bit Unix millisecond integer.
- `scope`, `parents`, and `auth` are the signed routing and causal envelope below.
- `body` is the exact family DTO in the mapping specification.

All nine members are required. `null` represents a present optional value; optional members are
never omitted. Unknown, duplicate, missing, or reordered members are non-canonical. A decoder must
retain the original content bytes and compare them byte-for-byte with the result of canonical
re-encoding before semantic conversion.

## Canonical JSON grammar

Only the JSON types explicitly selected by a DTO are accepted. There are no floating-point values,
arbitrary maps, or generic JSON values.

- No insignificant whitespace occurs outside strings. There is no byte-order mark, leading or
  trailing whitespace, comment, or trailing data.
- Integers use minimal base-10 ASCII. Zero is `0`; leading zeroes, `+`, `-0`, exponent notation,
  fractions, and integers outside the selected signed/unsigned width are rejected.
- Strings contain valid UTF-8 Unicode scalar values. Unpaired surrogates, invalid UTF-8, and U+0000
  are rejected. Unicode is not normalized; producers must preserve the validated scalar sequence.
- `"`, `\\`, backspace, form feed, newline, carriage return, and tab use the short escapes `\"`,
  `\\`, `\b`, `\f`, `\n`, `\r`, and `\t`. Other U+0001 through U+001F controls use one lowercase
  `\u00xx` escape. Solidus is never escaped. All other scalars, including non-ASCII scalars, are
  emitted directly as UTF-8. Any different but semantically equal escape spelling is rejected by
  re-encoding equality.
- Objects use the member order stated by their DTO. Arrays preserve DTO order. Set-shaped arrays
  use the explicit sorting rules below. Duplicate set values are rejected rather than collapsed.
- Nesting and member/count limits are checked while parsing. A parser may not first build an
  unbounded generic value.

## Signed scope

`scope` is exactly one of these tagged arrays:

| Form | Semantic value | Intrinsic agreement |
| --- | --- | --- |
| `["local",installation]` | installation-private | `installation == author` |
| `["peer",installation,mailbox]` | peer-addressed mailbox | payload sender/recipient rules for its family agree with this exact mailbox |
| `["account",account]` | account-addressed | payload account/project audience agrees and any direct recipient rule is satisfied |

All IDs are fixed-width hex. A canonical record cannot use the `control` scope. Scope is signed
inside content and is not inferred from Nostr tags, relay recipients, encryption wrappers, or the
payload alone. An audience/author contradiction is an intrinsic semantic failure.

## Causal references and authority

Each parent is `["c",fact-id]`, where `c` means a canonical-event ID. A canonical v1 record may not
cite the remote-control namespace. `parents` is a unique set encoded in ascending order by namespace
byte and then decoded 32-byte ID. Its maximum is `MAX_PARENT_REFS`.

Each authority is `[role,"c",fact-id]`. Roles use only this closed vocabulary:

```text
account-creator
account-membership
active-human
assignment
device-grant
dispatch
local-installation
mailbox-grant
mailbox-owner
output-binding
previous-state
project-home
request
```

`auth` is sorted by the UTF-8 role string, roles are unique, and every authority's exact namespace
and ID pair must also occur in `parents`. Its maximum is `MAX_AUTHORITY_REFS`. Unknown roles,
duplicate roles, duplicate parents, wrong ordering, an authority absent from parents, or a role not
allowed by the selected family fail before construction of a semantic fact. Historical authority
validity remains a reducer decision; the decoder proves only exact typed representation and
intrinsic envelope agreement.

## Nested DTO vocabulary

Nested objects have exactly the listed members in order.

| DTO | Canonical JSON shape |
| --- | --- |
| installation address | `{"installation":hex,"signing":hex}` |
| mailbox address | `{"installation":hex,"mailbox":hex}` |
| locator | `{"scheme":enum,"value":text}` |
| repository context | `{"directory":locator,"repository":locator-or-null,"worktree":locator-or-null,"branch":text-or-null}` |
| operation correlation | `{"provider":text,"session":text,"id":hex}` |
| message | `{"id":hex,"sender":mailbox-address,"recipient":mailbox-address-or-null,"body":text,"purpose":enum,"presentation":enum,"correlation":operation-or-null,"project":hex-or-null}` |
| resource | `{"id":hex,"display":locator,"canonical":locator,"health":enum}` |
| assignment binding | `{"assignment":hex,"agent":hex,"provider":text,"session":text}` |

Closed enum strings are:

- mailbox kind: `human`, `agent`;
- locator scheme: `git`, `worktree`, `container`, `opaque`;
- message purpose: `question`, `asynchronous`, `project-output`;
- presentation: `message`, `final-answer`, `status`;
- activity kind: `status`, `agent-turn`, `progress`, `plan`, `diff`, `completed-item`;
- activity status: `{"state":"snapshot"}`, `{"state":"running"}`,
  `{"state":"succeeded"}`, `{"state":"failed","code":text}`,
  `{"state":"interrupted"}`;
- runtime observation: `{"state":"succeeded"}`, `{"state":"failed","code":text}`,
  `{"state":"uncertain","code":text}`;
- resource health: `unknown`, `healthy`, `degraded`, `unavailable`;
- initial project state: `open`, `closed`.

Error codes are nonempty `MAX_SHORT_TEXT_BYTES` strings from the domain error-code registry. A
positive sequence is a JSON integer in 1 through 18,446,744,073,709,551,615. Ordered relay/resource
arrays preserve their declared order and reject exact duplicate identities.

## NIP-01 event construction

Canonical v1 uses provisional regular event kind `6000` selected by
[ADR 0004](../adr/0004-canonical-fact-nostr-carriage.md). The exact outer event JSON has these seven
members in order and no others:

```text
{"id":I,"pubkey":K,"created_at":C,"kind":6000,"tags":[],"content":Q,"sig":G}
```

- `Q` is the canonical content byte sequence represented as a JSON string using NIP-01 escaping.
- `K` is the x-only 32-byte BIP-340 public key as lowercase hex. It becomes the signing-key part of
  the semantic installation address and must agree with payload declarations that name that key.
- `C` is a nonnegative Unix-seconds integer equal to Euclidean floor division `time / 1000`.
- Tags are exactly the empty array. A missing tag array, any tag, or a different kind is not a
  canonical v1 event.
- The event-ID preimage is the whitespace-free UTF-8 NIP-01 serialization
  `[0,K,C,6000,[],Q]`. `I` is lowercase hex SHA-256 of exactly those bytes and becomes the canonical
  `FactId`.
- `G` is a 64-byte lowercase-hex BIP-340 Schnorr signature of the 32-byte event ID under `K`.
  Verification includes canonical x-only public-key parsing and signature range checks.

The receiver bounds and parses the outer event without trusting any field, reconstructs and checks
the event ID, verifies the signature and public key, and only then dispatches retained content
bytes. It preserves the exact verified outer bytes, the exact event-ID preimage bytes, and exact
content bytes. Encoding the outer event deterministically does not make domain DTOs wire schemas;
the protocol layer owns all representations.

## Version and family dispatch

After outer cryptographic verification, dispatch examines only bounded top-level prefix fields.

- Exact discriminator `hq/canonical`, version 1, and family 1 through 45 continue to strict DTO
  parsing and canonical-byte equality.
- A syntactically inspectable `hq/canonical` record with another nonnegative integer version is a
  verified unsupported version.
- Version 1 with a positive integer family outside 1 through 45 is a verified unsupported family.
- Another well-formed discriminator is a verified unsupported protocol when permitted by local
  retention policy.
- A non-integer/negative discriminator field, a canonical/control family-range mismatch, or an
  uninspectable prefix is malformed, not unsupported.

Unsupported records retain exact verified event and content bytes plus the recognized discriminator,
version, and family when available. They never expose a semantic payload or enter reduction.

## Typed local authoring

`CanonicalEventPlan` is the production authoring boundary for local canonical and control records.
It owns only typed domain author, millisecond time, scope, bounded causal references, and one of the
48 semantic payload variants. Protocol DTO types remain private. The protocol implementation maps
every semantic family exhaustively to its v1 body, applies the canonical namespace and sorting rules,
encodes the exact content record, and signs it with caller-supplied BIP-340 auxiliary randomness.

Authoring does not create a privileged trust state. The resulting event reruns ordinary prefix
dispatch, complete DTO decoding and byte-equality verification, and intrinsic semantic conversion.
A signer/author mismatch, negative or inconsistent time, invalid scope/payload relationship, or
other impossible plan therefore fails before storage. The verified event ID remains the sole fact
identity; IDs, time, and signing randomness are explicit inputs and no ambient source is consulted.

## Intrinsic semantic conversion

Only a cryptographically verified, supported, byte-canonical record can convert to a semantic fact.
Conversion validates every fixed width, bound, enum, optional, cross-field invariant, scope,
author, family, and typed reference. The resulting semantic ID is the verified NIP-01 event ID;
semantic author is `(author, pubkey)`; authored time is `time`; and causal references come only
from `parents` and `auth`.

Missing historical parents, stale authority, concurrency, and authorization are not decoder
failures. They are reducer outcomes. Conversely, malformed bytes, an invalid signature, a scope
contradiction, or an impossible family body can never be represented as `SemanticFact` merely so a
reducer can reject it.

## Exact vector

[canonical-installation-v1.json](vectors/canonical-installation-v1.json) is the normative positive
vector. It records content bytes, the NIP-01 event-ID preimage, event ID, public key, signature,
complete outer event, and semantic mapping. The signature was generated and independently verified
with `nak 0.20.2`; its public key also matches BIP-340's standard secret-key-one x-only public key.
The secret key exists only to make this public test fixture reproducible and is never a deployment
key.

The negative corpus is [adversarial-v1.json](vectors/adversarial-v1.json). Every case states the
stage and stable failure class; implementations may add diagnostic detail but may not admit a case
at a later trust state.
