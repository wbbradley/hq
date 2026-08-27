# Rust protocol trust boundary

`hq-protocol` owns the transition from bounded untrusted bytes to cryptographically verified HQ
event content. Its signed-event path follows the wire contracts in `docs/protocol/` and deliberately
stops before DTO decoding or semantic conversion.

The trust states are disjoint owners:

1. `RawEventBytes` bounds and retains exact attacker-controlled bytes.
2. `ParsedOuterEvent` accepts only the canonical seven-member HQ NIP-01 event shape, but treats its
   identity, key, signature, and decoded content as untrusted.
3. `CryptographicallyVerifiedEvent` exists only after reconstructing the exact preimage, checking
   the SHA-256 ID, and verifying BIP-340 over that raw 32-byte ID.
4. Prefix dispatch produces either `SupportedContentBytes` or `VerifiedUnsupportedRecord`.
   Unsupported protocol, version, and family values never acquire a supported-content type.

The boundary uses `k256` 0.14 for pure-Rust secp256k1 Schnorr operations and `sha2` 0.11 for the
event ID. Signing requires caller-supplied 32-byte BIP-340 auxiliary randomness. Signer values do
not implement `Clone` or `Debug`, and protocol failures expose only a closed redacted class.
The isolated fuzz workspace adds `libfuzzer-sys`; dependency policy narrowly permits its vendored
LLVM runtime's OSI-approved NCSA license without adding NCSA to the workspace-wide allowlist.

## Fuzzing

The committed raw-byte target drives every reachable parse, verification, and dispatch transition.
Its corpus starts with the published canonical signed-event vector so the initial run reaches the
deepest boundary; the harness removes only the repository line terminator from text corpus files.

Install and run the pinned smoke gate:

```sh
rustup toolchain install nightly-2026-08-26 --profile minimal
cargo install cargo-fuzz --locked --version 0.12.0
scripts/verify-rust-protocol-fuzz.sh
```

For a longer AddressSanitizer run, keep the same pinned toolchain and target but replace the smoke
limit:

```sh
cargo +nightly-2026-08-26 fuzz run signed_event \
  --fuzz-dir crates/hq-protocol/fuzz -- -max_total_time=3600 -max_len=4194304 -timeout=10
```

Generated corpus and crash artifacts are diagnostic inputs; minimize and promote every useful
regression into a named deterministic test before committing it.
