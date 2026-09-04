# HQ

## Next Up

### Remove yanked wNAF from the k256 dependency graph

HQ's workspace and protocol-fuzz lockfiles resolve `k256` 0.14.0 to yanked `wnaf` 0.14.0, so
locked builds emit a registry warning and dependency-policy checks retain a withdrawn release.
`k256` 0.14.0 remains the current stable release and permits `wnaf` 0.14.1; upstream yanked 0.14.0
to require corrected scalar-representation endianness bounds rather than because HQ selected an
obsolete `k256` API.

- Refresh the `k256` dependency resolution in both `Cargo.lock` and
  `crates/hq-protocol/fuzz/Cargo.lock` so they select the latest compatible, non-yanked `wnaf`
  release (0.14.1 at task creation). Use a targeted lockfile update: do not unlock unrelated
  dependencies, add an unnecessary direct `wnaf` dependency, switch to an unpublished Git
  revision, or enable additional `k256` default features.
- Keep the workspace's explicit `k256` feature contract (`default-features = false`, `ecdh`, and
  `schnorr`) and confirm the resolved graph remains compatible with Rust 1.98. Preserve the exact
  BIP-340 event signing/verification and NIP-44 ECDH/envelope behavior exercised by `hq-protocol`
  and `hq-relay`.
- Verify both dependency graphs contain no yanked `wnaf` 0.14.0, ordinary locked workspace builds
  and the locked `hq-node` installation no longer print the warning, the protocol fuzz workspace
  resolves under its checked-in lockfile, and `scripts/verify-rust-dependencies.sh` accepts both
  graphs. Run the protocol signed-event/vector tests, relay NIP-44/envelope tests, and the full
  workspace suite to catch cryptographic or feature-resolution regressions.

Dependencies: the current RustCrypto `k256` 0.14.x dependency graph and the separately locked
protocol-fuzz workspace.

Completion condition: every checked-in lockfile resolves `k256` through non-yanked `wnaf` 0.14.1
or a newer compatible stable release, locked builds are warning-free, and HQ's BIP-340 and NIP-44
compatibility tests remain unchanged and passing.
