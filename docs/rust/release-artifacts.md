# Rust release artifact contract

HQ's first Rust release is one executable on each of the four native targets in
`qualification/platform-matrix.tsv`. Release-candidate automation is deliberately separate from
release publication: `.github/workflows/release.yml` is manually dispatched, has read-only
repository permissions, and does not create a tag or GitHub release. A second manual workflow,
`.github/workflows/publish-release.yml`, is the only automated publication path.

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
`rust-release-candidate-<REVISION>` for 30 days. That combined artifact also contains the four-host
recovery manifest, controlled relay/provider failure record, synthetic archived-Go rollback record,
and `hq-rust-cutover-evidence-v1` bundle. The cutover bundle binds those inputs by SHA-256 and audits
all acceptance-matrix and definition-of-done clauses.

The scripts refuse existing output files. They never inspect a default state root, a Go key, or a
Go database. The release workflow does not install into a user path, use an existing identity, or
leave the temporary verification daemon running.

The supported candidate-install procedure is the checksum, extraction, absolute-path installation,
and embedded revision check in the repository README. A successful workflow makes artifacts
available for review; it does not install, tag, publish, soak, activate, or change a service.

## GitHub release publication

Run `Rust release candidate` from `main` and wait for every job to succeed. Then manually dispatch
`Publish Rust GitHub release` with that exact workflow run ID, the `v`-prefixed version tag, and the
desired prerelease setting. Publication performs these checks before creating external state:

- the selected run is a completed, successful dispatch of `.github/workflows/release.yml` from
  this repository's `main` branch;
- its exact source revision remains in `main` history and it has one unexpired combined candidate
  artifact;
- the requested tag equals `v` plus the version recorded by every candidate evidence record;
- regenerated release and recovery manifests equal the candidate manifests, every archive checksum
  is valid, and the controlled-failure, rollback, and cutover evidence remains complete; and
- neither the requested tag nor its GitHub release already exists.

The workflow creates the tag at the candidate revision and publishes a GitHub release containing
the four native archives, their checksum files, and the aggregate release, recovery, failure,
rollback, and cutover manifests. It refuses to update or replace an existing tag or release.
Publication is not soak, activation, production cutover, or permission to access an operator's
identity or state.

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
[33257580370](https://github.com/wbbradley/hq/actions/runs/33257580370) built and verified source
revision `f408702866faeeb2530ecedff4a25f9786bea8be` on 2026-08-29. All four native jobs, controlled
failure, synthetic rollback, and aggregate verifier passed. The native jobs took 111 seconds on
Linux ARM64, 139 seconds on Linux x86-64, 139 seconds on Apple Silicon, and 331 seconds on Intel
macOS, each below the release workflow and qualification limits.

The combined artifact is
`rust-release-candidate-f408702866faeeb2530ecedff4a25f9786bea8be`. An independent download to a
new temporary directory passed every release, recovery, controlled-failure, rollback, and cutover
validator. Freshly regenerated release, recovery, and cutover manifests were byte-for-byte equal to
the workflow outputs. The verified archive SHA-256 values are:

| Rust host | SHA-256 |
| --- | --- |
| `aarch64-apple-darwin` | `3d4df043bbb9ac5581f77f5d1691695f10fca2b955fc28501e191fb04769287c` |
| `aarch64-unknown-linux-gnu` | `743204746ad4bfe0114b32d019f4203e48f13209acf5e4cc7490c22ecea967b7` |
| `x86_64-apple-darwin` | `cf1565790f3802c238572c53214d65a3a3c0678b3bec13bc1e99575852c695d4` |
| `x86_64-unknown-linux-gnu` | `63a2378e4ce4970784910d67b1db5380341b9537ad295096a19eb25cb55f16d7` |

This attests release-candidate artifact production only. It is not a tag, published release,
production soak, or cutover authorization.
