# Rust release artifact contract

HQ's first Rust release is one executable on each of the four native targets in
`qualification/platform-matrix.tsv`. Release-candidate automation is deliberately separate from
release publication: `.github/workflows/release.yml` is manually dispatched, has read-only
repository permissions, and does not create a tag or GitHub release.

## Per-target evidence

Each native runner builds `hq-node`'s `hq` binary with `HQ_BUILD_COMMIT` set to the complete source
revision. `scripts/package-rust-release.sh` accepts only a native release host from the acceptance
matrix, checks the binary's version record and embedded revision, and emits:

- `hq-v<VERSION>-<RUST_HOST>.tar.gz`, containing only the executable `hq`;
- the matching `.tar.gz.sha256`, in portable SHA-256 checksum-file form; and
- after installation verification, `hq-v<VERSION>-<RUST_HOST>.json`.

`scripts/verify-rust-release-artifact.sh` runs on the artifact's native host. It verifies the host
OS and architecture, archive checksum and contents, extracts the executable into a temporary
installation directory, and checks the embedded version and revision. It then uses a newly created
temporary state root to initialize an identity, reach daemon readiness, observe ready status, and
request clean shutdown. Only after those checks pass does it atomically publish the target JSON
manifest with `single_executable: true` and `installed_lifecycle: "passed"`.

## Aggregate evidence

`scripts/verify-rust-release-matrix.sh` requires exactly one manifest for every supported native
target, one shared version and revision, matching archives and checksums, and successful installed
lifecycle evidence. It emits `release-manifest.json` with schema
`hq-rust-release-manifest-v1`. CI uploads the complete matrix as
`rust-release-candidate-<REVISION>` for 30 days.

The scripts refuse existing output files. They never inspect a default state root, a Go key, or a
Go database. The release workflow does not install into a user path, use an existing identity, or
leave the temporary verification daemon running.

## Local rehearsal

Builds must embed the exact source revision:

```sh
revision=$(git rev-parse HEAD)
artifact_directory=$(mktemp -d "${TMPDIR:-/tmp}/hq-rust-release.XXXXXX")
HQ_BUILD_COMMIT=$revision cargo build --locked --release -p hq-node --bin hq
scripts/package-rust-release.sh \
  "$PWD/target/release/hq" "$artifact_directory" "$revision" \
  "$(rustc -vV | sed -n 's/^host: //p')"
scripts/verify-rust-release-artifact.sh \
  "$artifact_directory" "$revision" "$(rustc -vV | sed -n 's/^host: //p')"
```

The synthetic aggregate validator contract is exercised with:

```sh
scripts/test-rust-release-artifacts.sh
```

## Recorded release-candidate evidence

GitHub Actions run
[33251594731](https://github.com/wbbradley/hq/actions/runs/33251594731) built and verified source
revision `af7625225c4b41bf86c12d148a53e87755ac6e1f` on 2026-08-29. All four native jobs and the
aggregate verifier passed. The build steps took 100 seconds on Linux ARM64, 134 seconds on Linux
x86-64, 165 seconds on Apple Silicon, and 457 seconds on Intel macOS, each below the 900-second
release-build acceptance limit.

The combined artifact is
`rust-release-candidate-af7625225c4b41bf86c12d148a53e87755ac6e1f`. An independent download to a
new temporary directory passed a fresh `verify-rust-release-matrix.sh` invocation. Its regenerated
aggregate manifest was byte-for-byte equal to the workflow-produced manifest. The verified archive
SHA-256 values are:

| Rust host | SHA-256 |
| --- | --- |
| `aarch64-apple-darwin` | `246e2b2e971cc8073ef728cb37c82abcacc0066f8beb4f6041b6bdc3c3ac1f94` |
| `aarch64-unknown-linux-gnu` | `1cf8b3d518bc4facde688829e9f6f6b0f7841a1cf16f47996c73d76cda1aea4c` |
| `x86_64-apple-darwin` | `abea57bdaf03453b9ff58c561093387a937941f0bcaacd05ed1661ef8db981d2` |
| `x86_64-unknown-linux-gnu` | `17006477999f3ee394a998d65c09b1436ffb18713200b12de442daa4bed07f66` |

This attests release-candidate artifact production only. It is not a tag, published release,
production soak, or cutover authorization.
