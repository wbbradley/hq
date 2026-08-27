# Validated domain primitives

Status: Active implementation contract

`hq-domain` owns semantic vocabulary that is valid before protocol encoding, persistence, or
runtime coordination. Constructors reject invalid values and all representations remain private.
The crate performs no I/O, reads no clock or randomness, and has no third-party dependency.

## Identity and key boundary

Fact, installation, mailbox, account, agent, project, message, resource, command, receipt, and
operation identities are distinct 32-byte newtypes. Signing and encryption public keys are also
distinct 32-byte values. Byte access supports cryptography and canonical encoding in outer crates;
the domain crate does not define text syntax, hashes, signatures, or wire bytes. It intentionally
contains no secret-key type: secret custody belongs to the identity adapter and must not acquire
ordinary cloning, debugging, serialization, or persistence behavior by accident.

`Eq` and lexicographic `Ord` make opaque values usable in deterministic sets and the specified
presentation tuple. That order is never a semantic winner rule for concurrent facts. A compile-fail
doctest proves that identifier types cannot be interchanged.

## Validated values

- `BoundedText<N>` owns non-empty UTF-8 with an inclusive encoded-byte limit and applies no hidden
  Unicode normalization.
- `BoundedVec<T, N>` owns at most `N` items. `BoundedSet<T, N>` rejects duplicate or oversized
  input, permits the empty set needed by root facts, and exposes deterministic iteration.
  `NonEmptyBoundedSet<T, N>` adds the non-empty invariant for values that require one member.
- `Timestamp` is an explicitly supplied signed Unix-millisecond value. `Revision` is an explicitly
  supplied monotonic local projection revision. Neither consults an ambient clock.
- `InstallationAddress` and `MailboxAddress` preserve identity and key roles rather than flattening
  them into strings.
- `ProviderId`, `ProviderSessionId`, and `OperationCorrelation` keep provider, session, and
  operation namespaces distinct while remaining neutral to any provider implementation.
- `ResourceLocator` pairs a semantic scheme with bounded opaque canonical text. Validation does not
  stat, resolve, open, or otherwise interpret an external resource.

`CausalReferences` uses a bounded parent set that may be empty for a root fact. Every authority
reference has an explicit semantic role, must cite a member of that parent set, and may occur at
most once per role. This enforces the shape needed to prevent unrelated-parent authority attacks;
reducers still decide whether the cited parent is usable and authoritative at the action's causal
point.

## Envelopes and errors

`Command<T>` carries a stable command identity, explicit issue time, and typed body.
`Outcome<T>` separates committed values from typed domain rejection. `Page<T>` carries query-owned
items and an opaque continuation cursor, while `VersionedView<T>` pairs a rebuildable projection
with its local revision. These are domain/application shapes, not RPC or database DTOs.

`DomainError` has a policy-facing category and bounded stable code. Human prose, HTTP/RPC status,
SQL details, retry timing, and presentation formatting belong outside the pure domain boundary.

`SemanticFact` now replaces the temporary text-only fact shape. `InMemoryFrame` remains only as a
workspace boundary demonstration and constructs a real `InstallationDeclared` fixture after
validation. The complete payload and deterministic test-support contract is documented in
`semantic-facts.md`.
