# HQ human pairing invitation v1

Status: normative for the Rust first release.

Owner: `hq-protocol::pairing`.

## Artifact and trust model

A pairing invitation is public signed evidence, not a capability-bearing secret. It contains no
root secret, local selection, receipt, revision, database row, runtime path, or relay credential.
Possession alone grants nothing: the exact named installation key must author a canonical
`HumanDeviceAccepted` fact before membership becomes active.

The compact canonical JSON object has exactly these fields in this order:

```json
{"schema":"hq-human-pairing-invitation-v1","grant_fact":"<lowercase-hex-32>","facts":["<exact signed event JSON>"]}
```

`facts` is sorted by verified fact ID and contains exactly the named `HumanDeviceGranted` fact's
transitive parent closure. Every event is the exact canonical outer event bytes represented as a
JSON string. Duplicate, unsupported, malformed, noncanonical, missing, or extraneous evidence is
rejected. Re-encoding the verified value must reproduce the complete artifact byte for byte.

The artifact is at most 1,048,576 bytes and 64 facts. The target installation ID and signing public
key, account ID, grant ID, optional label, and at most eight relay hints come only from the verified
grant payload. Relay hints aid discovery but grant no authority.

## Join and cancellation

Join first verifies artifact encoding, signatures, exact target binding, complete ancestry, and
ordinary authority reduction without network access. Only then may a local adapter import evidence
and ask the application planner to author acceptance. Exact evidence import, acceptance, and local
selection are idempotent.

Version 1 has no wall-clock expiry field. HQ has no trusted distributed clock and an unsigned local
expiry would not revoke canonical authority. The creator cancels an unused or accepted invitation
with the ordinary frontier-complete device revocation fact. A later regrant must cite the resulting
membership frontier and receives a distinct deterministic grant identity. This is the initial
clean-sheet format: HQ has not shipped, so there is no migration, compatibility decoder, or storage
version transition.
