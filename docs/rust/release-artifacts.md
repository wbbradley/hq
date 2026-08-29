# Rust release artifact contract

HQ's first Rust release is one executable on each of the four native targets in
`qualification/platform-matrix.tsv`. `.github/workflows/release.yml` is manually dispatched from
`main` with a version tag. One run builds, verifies, tags, and publishes the GitHub release.

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
`hq-rust-release-manifest-v1`. The workflow uploads the complete matrix as
`rust-release-bundle-<REVISION>` for 30 days. That combined artifact also contains the four-host
recovery manifest, controlled relay/provider failure record, synthetic archived-Go rollback record,
and `hq-rust-cutover-evidence-v1` bundle. The cutover bundle binds those inputs by SHA-256 and audits
all acceptance-matrix and definition-of-done clauses.

The scripts refuse existing output files. They never inspect a default state root, a Go key, or a
Go database. The release workflow does not install into a user path, use an existing identity, or
leave the temporary verification daemon running.

The supported install procedure is the checksum, extraction, absolute-path installation, and
embedded revision check in the repository README. A successful workflow publishes artifacts; it
does not install them, soak, activate, change a service, or access an operator's identity or state.

## GitHub release publication

Manually dispatch `Rust GitHub release` from `main` with the `v`-prefixed version tag and desired
prerelease setting. The workflow performs these checks before creating external state:

- the workflow runs from this repository's `main` branch;
- the requested tag equals `v` plus the workspace and built-artifact version;
- regenerated release and recovery manifests equal the built manifests, every archive checksum
  is valid, and the controlled-failure, rollback, and cutover evidence remains complete; and
- neither the requested tag nor its GitHub release already exists.

The final job creates the tag at the workflow revision and publishes a GitHub release containing
the four native archives, their checksum files, and the aggregate release, recovery, failure,
rollback, and cutover manifests. It refuses to update or replace an existing tag or release.
Publication is not soak, activation, or production cutover.

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
