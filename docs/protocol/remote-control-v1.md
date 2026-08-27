# HQ remote-control protocol v1

Status: normative

Protocol discriminator: `hq/control`

Version: `1` in an independent version space

Catalog families: `46`, `47`, and `48`.

Remote-control records are signed causal audit records. They never mutate project state and never
stand in for canonical project facts. This protocol shares the strict JSON and NIP-01 carriage
rules in [canonical-fact-v1.md](canonical-fact-v1.md), but it has its own discriminator, version
registry, family range, DTOs, and compatibility decisions.

## Record and bounds

The content object has exactly the common nine members and order:

```text
{"p":"hq/control","v":1,"f":F,"author":A,"time":T,"scope":S,"parents":R,"auth":H,"body":B}
```

`MAX_EVENT_BYTES`, `MAX_CONTENT_BYTES`, `MAX_JSON_DEPTH`, `MAX_OBJECT_MEMBERS`,
`MAX_COLLECTION_ITEMS`, `MAX_PARENT_REFS`, `MAX_AUTHORITY_REFS`, `MAX_SHORT_TEXT_BYTES`,
`MAX_CONTENT_TEXT_BYTES`, `MAX_LOCATOR_TEXT_BYTES`, `MAX_PROVIDER_ID_BYTES`,
`MAX_PROVIDER_SESSION_BYTES`, `MAX_RELAY_HINTS`, and `MAX_RESOURCE_ITEMS` have exactly the values
and measurement rules defined by canonical fact v1. The complete strict JSON grammar, optional
`null` rule, lowercase fixed-width hex rule, nested operation DTO, NIP-01 event construction, kind
`6000`, empty tags, authored-time agreement, retained bytes, event ID, and BIP-340 verification are
also identical. Sharing those primitives does not couple the protocol version spaces.

The only valid scope is:

```text
["control",account-id,target-home-installation-id]
```

The account and target home are fixed-width hex. For family 46, body `target_home` must equal the
scope target. Families 47 and 48 are signed by the target home's exact root key: content `author`
equals the scope target and the verified outer public key must be the historically valid project
home key. Family 46 is signed by the requesting human installation; it may differ from the target
home but must cite exact active-human authority for the scoped account.

## Typed cross-namespace references

A parent is `[namespace,fact-id]`, sorted uniquely by namespace byte and decoded ID. Namespace `c`
means canonical fact event ID; namespace `r` means remote-control event ID. Both are permitted in
control records. A bare hex string is never enough to choose a namespace.

An authority is `[role,namespace,fact-id]`, sorted uniquely by the same closed role vocabulary as
canonical v1. Every exact reference must occur in `parents`. The following namespace constraints
apply before semantic construction:

- `active-human`, `project-home`, `assignment`, `dispatch`, `output-binding`, and
  `account-membership` cite `c`;
- `request` cites `r` and must identify the exact family-46 request;
- other roles are rejected for v1 control records.

A family's required historical roles are specified in the semantic catalog. Extra known roles that
the family does not permit are rejected. The decoder validates representation and intrinsic
agreement; the reducer validates that cited facts exist, have the required family and subject, and
were historically authoritative.

Canonical records may never cite `r`. This one-way dependency keeps control workflow evolution out
of canonical project authority while allowing receipts and outcomes to cite requests.

## Family DTOs

Every object has exactly these members in order. Field names are normative wire vocabulary and are
independent of Rust field names.

### Family 46: request

```text
{"command":hex,"digest":hex,"project":hex,"target_home":hex,"expected_head":hex,"operation":{"provider":text,"session":text,"id":hex},"body":text}
```

This maps to `RemoteProjectCommandRequested`. `command` is the stable retry identity; `digest`
identifies the exact request input; `expected_head` is a canonical fact reference encoded as a
fixed-width ID in the body and must also be represented by the required causal parent set when the
semantic catalog requires it. `body` is inert bounded command content, not executable JSON and not
a canonical project mutation.

### Family 47: receipt

```text
{"command":hex,"digest":hex,"project":hex,"received_head":hex,"received_at":milliseconds}
```

This maps to `RemoteProjectCommandReceipt`. It must cite the exact family-46 request in namespace
`r` and the observed project head in namespace `c`. A receipt proves home verification and receipt
at the stated head; it does not prove commitment or external execution.

### Family 48: outcome

```text
{"command":hex,"digest":hex,"project":hex,"result":result,"runtime":runtime-or-null}
```

`result` is exactly one of:

```text
{"state":"committed","head":hex}
{"state":"rejected","code":text}
```

`runtime` uses canonical v1's runtime-observation DTO. A committed outcome cites its canonical head
with namespace `c` and cites request/receipt control parents with namespace `r`. A rejected outcome
does not invent a canonical transition. An uncertain runtime observation records uncertainty about
an external effect; it does not make the signed command outcome itself ambiguous.

The exhaustive row-by-row mapping and required intrinsic relations live in
[payload-mapping-v1.md](payload-mapping-v1.md).

## Unsupported and malformed input

After valid outer kind-6000 identity and signature verification:

- exact `hq/control`, version 1, and family 46 through 48 continue to strict DTO parsing;
- another nonnegative integer control version becomes `VerifiedUnsupportedRecord` with reason
  `unsupported-version`;
- version 1 with another positive family becomes `VerifiedUnsupportedRecord` with reason
  `unsupported-family`;
- family 1 through 45 under `hq/control`, family 46 through 48 under `hq/canonical`, or an invalid
  reference namespace is `namespace-confusion`, not a future extension;
- malformed prefix fields, non-canonical bytes, scope disagreement, or impossible DTO values fail
  verification/conversion and are not retained as supported semantic content.

Unsupported records preserve the exact cryptographically verified outer event and content bytes.
They expose no `SemanticFact`, are not reducible, and cannot advance a remote command view. See
[trust-transitions.md](trust-transitions.md) for the complete transition and failure taxonomy.

## Exact vector

[remote-command-v1.json](vectors/remote-command-v1.json) is the exact family-46 vector. It uses the
same public fixture key and `nak 0.20.2` verification path as the canonical vector, but a different
discriminator, family, timestamp, content, event ID, and signature. It demonstrates a typed
canonical parent and active-human authority without implying that the synthetic parent is
historically sufficient; reducer authority is deliberately outside a wire vector.

[adversarial-v1.json](vectors/adversarial-v1.json) covers namespace confusion, wrong families,
scope/author disagreement, request mismatch, tampering, unsupported versions, and common strict
JSON failures.
