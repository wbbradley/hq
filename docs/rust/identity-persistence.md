# Rust installation identity and local configuration v1

This note fixes the first Rust release's local identity boundary. It implements ADR 0002 and
behavior-ledger requirements IDN-001 through IDN-007. None of these files is a canonical fact,
remote protocol, SQLite schema, local API message, or Go migration format.

## State layout and ownership

An explicit absolute state root, or `$XDG_STATE_HOME/hq` with `$HOME/.local/state/hq` as fallback,
contains these stable paths:

| Path | Purpose | Unix mode |
| --- | --- | ---: |
| `identity.v1` | installation identity and root secret | `0600` |
| `local-config.v1.json` | unsigned relay/provider defaults | `0600` |
| `hq.sqlite3` | reserved input to the Rust store owner | store-owned |
| `node.lock` | process-lifetime exclusive state ownership | `0600` |

The state root is `0700`. HQ rejects the state root, identity, configuration, and lock when the
artifact itself is a symbolic link or has a broader/different private mode. `node.lock` uses an
exclusive standard-library file lock whose open handle is owned for the complete
`StateDirectoryOwner` lifetime. This prevents two local Rust processes from owning one state root;
it cannot prevent the same imported identity from running on multiple hosts.

Private writes create an unpredictable same-directory `0600` temporary file with `create_new`,
write it completely, sync it, publish it atomically, remove only that attempt's temporary name,
and sync the parent directory. Identity and backup publication use an atomic hard-link
no-overwrite operation. Configuration replacement uses atomic rename. A crash before publication
leaves the final path absent or at its prior complete value; unrelated abandoned temporary files
are never interpreted as state.

## Identity file v1

`identity.v1` is exactly 73 bytes:

| Offset | Length | Value |
| ---: | ---: | --- |
| 0 | 8 | ASCII `HQIDV1`, followed by two zero bytes |
| 8 | 1 | format version `1` |
| 9 | 32 | nonzero opaque installation identity |
| 41 | 32 | valid nonzero secp256k1 secret scalar |

There is no stored public-key field to disagree with the secret. Loading derives the x-only public
key with the same BIP-340 implementation used by the signed-event boundary. Public inspection
returns only the installation identity, public key, and the first eight public-key bytes as a
lowercase hexadecimal fingerprint. The identity owner is non-cloneable and non-serializable; its
secret and intermediate plaintext buffers are zeroized on drop.

The safe `PublicIdentity` value exposes those three passive fields directly. Root identity,
password, signer, and state-ownership types remain opaque because they hold secrets or enforce live
capability invariants. The offline CLI acquires exclusive state ownership for every identity
operation; export/import require explicit bounded password input from stdin and never accept the
password in argv.

## Encrypted backup package v1

An export is exact canonical JSON in this member order:

```json
{"format":"hq-identity-backup","version":1,"installation":"<64 lowercase hex>","public_key":"<64 lowercase hex>","ncryptsec":"<NIP-49 value>"}
```

The package is bounded to 4096 bytes and created exclusively as `0600`. It contains no database,
history, configuration, relay/provider state, credentials, runtime ledger, or Go data. Import
requires exclusive state ownership and an absent `identity.v1`, verifies canonical package bytes,
decrypts NIP-49, derives the public key, and requires equality with the package public key before
publishing the identity.

NIP-49 uses classic Bech32 with HRP `ncryptsec`, version `2`, NFKC-normalized nonempty passwords of
at most 1024 UTF-8 bytes, scrypt `r=8` and `p=1`, a random 16-byte salt, a random 24-byte
XChaCha20-Poly1305 nonce, and the one-byte key-security value as associated data. Exports use
`log_n=16` (64 MiB). Imports accept only `log_n` 16 through 18 before performing the KDF, accept
defined security bytes 0 through 2, require the exact 91-byte decoded layout and canonical lowercase
Bech32 spelling, and authenticate before treating the 32 plaintext bytes as a secret scalar. The
official NIP-49 decryption vector pins interoperability.

## Unsigned local configuration v1

Absent configuration means empty explicit defaults. A present file is bounded to 65536 bytes and
is exact canonical JSON in this member order:

```json
{"version":1,"default_provider":"provider","theme":"gruvbox-dark-medium"}
```

`default_provider` is required and may be `null`. `theme` is an optional bounded built-in/user name
or absolute theme-file path. It is omitted when unset; an explicit `null` is noncanonical. The optional provider
uses the domain's nonempty 64-byte `ProviderId` bound. Configuration has no conversion into a
semantic payload and is replaced independently from identity and canonical history. Codex defaults
contain a boolean yolo policy and an optional bounded single-line model selector.

The daemon owns one serialized configuration manager for the lifetime of its state-directory lock.
Each update replaces one typed field, rebuilds and validates the complete value, atomically persists
it, and only then publishes the in-memory snapshot. A failed write leaves both views unchanged.
Offline commands mint the same capability only when no daemon owns the directory. Relay membership
is not duplicated here; `hq relay add/remove` owns its complete access and authentication policy.

## First-start declaration

Identity creation and canonical installation declaration are separate durable boundaries. Before
any foreground component starts or readiness is published, the exclusively owning node checks the
authoritative store. An empty clean store is given exactly one `InstallationDeclared` fact through
the ordinary application mutation and store-signing path. Its stable command identity makes the
commit replay-safe. A reopened store must already project the owned installation with exactly the
derived signing and encryption keys; absence or disagreement fails startup without replacing
either identity.

This is reconciliation of a new installation, not schema evolution. HQ has no shipped Rust release
or standing installations, so this work changes no schema, migration path, legacy reader, or
storage version.
