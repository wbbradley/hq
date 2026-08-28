# ADR 0002: Rust identity backup boundary

Status: Accepted

Date: 2026-08-26

## Context

An installation identity combines a stable installation UUID with a secp256k1 root secret. Losing
either loses the authority needed to decrypt retained traffic and continue that installation.
The rewrite forbids reading Go identity or database files and forbids concurrent Go and Rust use of
one identity, but an intentional Rust-era backup and recovery workflow is still a product safety
requirement.

Database history, configuration, relay state, projections, provider sessions, and operational
ledgers have different confidentiality and recovery properties from the root identity. Treating a
copy of the state directory as an identity backup would blur those boundaries.

## Decision

The first complete Rust release includes identity initialization, public inspection, encrypted
export, and guarded import.

- An export is a new, versioned Rust-era backup package containing the installation UUID and a
  password-encrypted root secret. The encrypted secret uses the interoperable NIP-49 `ncryptsec`
  construction, but the surrounding package is not required to match the Go JSON backup.
- Export uses exclusive creation, restricted permissions, bounded encoding, cryptographically
  secure randomness, file and parent-directory durability, and cleanup of partial output.
- Import strictly decodes and decrypts the package, validates that the public key and installation
  UUID are well formed, refuses an existing identity or active node ownership, writes atomically
  with restricted permissions, and never opens or alters a Go key or database.
- Export and import require an explicit `--password-stdin` source. HQ consumes exactly one bounded
  UTF-8 line, zeroizes it, never accepts a password argument, and does not prompt on closed input.
- Public inspection exposes only the installation UUID, public key, and safe fingerprint/encoding.
  Secrets must not enter SQLite, canonical facts, local API results, diagnostics, logs, crash
  reports, or command history emitted by HQ.
- The normal product does not export or import canonical history, database state, configuration,
  provider credentials, environments, or runtime ledgers in the identity package. A future data
  migration or archival product would be a separate auditable tool and remains outside normal node
  startup.
- Running one imported identity from multiple active hosts, or from both Go and Rust, is
  unsupported. Local ownership checks prevent same-state concurrent use; documentation and the
  cutover procedure must make the distributed duplicate-identity hazard explicit.
- The Go `identity reset --yes` workflow is not carried into the normal Rust CLI. Destructive
  retirement of an installation is an operator archival/removal procedure outside the node and
  requires explicit authority; it must not masquerade as routine identity initialization.

The exact package fields, password KDF parameters, limits, and redaction test vectors are owned by
the installation-identity work package, which may strengthen this decision without adding Go
compatibility.

## Consequences

- Backup and restore are release gates rather than post-release operational polish.
- Restoring authority is intentionally separate from restoring message history or operational
  state.
- Tests must cover wrong passwords, corruption, permissions, partial writes, existing targets,
  active ownership, round trips, and redaction.
- Operators get a safer explicit archival procedure instead of a product command that recursively
  removes identity and state.
- This pre-release implementation requires no migration or storage-version bump: no Rust release or
  standing installation exists, and clean schema definitions may be changed in place.
