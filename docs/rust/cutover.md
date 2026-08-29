# Rust v0.1.0 soak, cutover, and rollback

Status: operator checklist; every production activation requires explicit authorization

This procedure prepares a Rust release candidate without changing a live installation. It keeps
two decisions separate:

1. **Soak authorization** permits a new identity and new state directory on controlled relays only.
2. **Cutover authorization** is a later, separate decision that permits the verified Rust candidate
   to become the sole active product identity.

Completing repository qualification or a soak never implies cutover authorization.

## Evidence bundle

Use the combined `rust-release-candidate-<revision>` artifact from one successful `Rust release
candidate` workflow. Run these validators from the same revision:

```sh
scripts/verify-rust-release-matrix.sh "$EVIDENCE_DIRECTORY" "$REVISION" \
  "$AUDIT_DIRECTORY/release-manifest.json"
scripts/verify-rust-recovery-matrix.sh "$EVIDENCE_DIRECTORY" "$REVISION" \
  "$AUDIT_DIRECTORY/recovery-manifest.json"
scripts/verify-rust-controlled-failure.sh \
  "$EVIDENCE_DIRECTORY/controlled-failure.json" "$REVISION"
scripts/verify-rust-cutover-rollback.sh \
  "$EVIDENCE_DIRECTORY/cutover-rollback.json" "$REVISION"
scripts/verify-rust-cutover-evidence.sh \
  "$EVIDENCE_DIRECTORY/cutover-evidence.json" "$EVIDENCE_DIRECTORY" "$REVISION"
install -m 0600 "$EVIDENCE_DIRECTORY/controlled-failure.json" \
  "$AUDIT_DIRECTORY/controlled-failure.json"
install -m 0600 "$EVIDENCE_DIRECTORY/cutover-rollback.json" \
  "$AUDIT_DIRECTORY/cutover-rollback.json"
scripts/generate-rust-cutover-evidence.sh "$AUDIT_DIRECTORY" "$REVISION" \
  "$AUDIT_DIRECTORY/cutover-evidence.json"
cmp "$EVIDENCE_DIRECTORY/cutover-evidence.json" "$AUDIT_DIRECTORY/cutover-evidence.json"
```

The final `hq-rust-cutover-evidence-v1` record binds the four evidence inputs by SHA-256, lists all
11 acceptance-matrix rows and every definition-of-done clause, and records that soak and cutover
still require separate operator authorization. Validation is read-only except for new files in the
chosen audit directory.

## Pre-soak checklist

- Record the candidate revision, version, Rust host, archive checksum, workflow run, and evidence
  artifact retention date.
- Confirm `hq --output json version` reports the same version and full revision.
- Confirm the selected relay is controlled and uses no production identity or production state.
- Use a new absolute state root. Do not point Rust at a Go directory, key, database, log directory,
  or service definition.
- Inventory existing HQ processes by PID, full arguments, executable, and intended state root.
  Multiple state roots may legitimately have different owners; do not terminate them by name.
- Record the operator who may stop the soak and the quantitative observation window. Starting the
  soak remains blocked until the operator explicitly authorizes it.

After soak authorization, initialize only the controlled state root, add only controlled relays,
and start the candidate with the verified absolute path. Exercise the retained CLI/TUI workflows,
relay outage and catch-up, provider failure, restart, database repair, identity backup, and clean
shutdown. Record observations; do not reuse these credentials for production.

## Pre-cutover checklist

A soak result is input to this checklist, not permission to proceed.

- Obtain explicit cutover authorization naming the exact revision and installation identity.
- Revalidate the evidence bundle and candidate checksum after the soak.
- Confirm the authorized service definition names the exact absolute Rust executable and state root.
- Confirm the Rust identity backup is current, encrypted, mode `0600`, and stored separately.
- Stop the old installation through its own service manager and verify its exact PID is absent.
- Disable its automatic restart only if the cutover authorization explicitly includes that action.
- Archive its executable, key, database, logs, and service definition together. Make the archive
  read-only and record paths and metadata. Rust must never open any archived item.
- Re-inventory all HQ processes. Preserve every unrelated installation.
- Confirm the rollback owner, decision window, stop command, service-selection operation, and
  archived paths before starting Rust.

Only then may the authorized operator start Rust. Verify `daemon readiness`, identity metadata,
relay policy and catch-up, local account state, provider launch, and an end-to-end message. Rust
must be the sole active process for its production identity.

## Rollback boundaries

Rollback is a controlled installation selection, not a database conversion. Trigger it when the
authorized acceptance window observes identity mismatch, persistent readiness failure, unrepaired
database integrity failure, lost relay catch-up, provider lifecycle failure, or another declared
cutover criterion.

1. Stop the exact Rust installation with its verified binary and state root.
2. Verify its exact PID is absent and its state lock is released. Do not kill unrelated HQ daemons.
3. Preserve the Rust state and logs for diagnosis; do not copy them into the Go archive.
4. Point the operator-owned service selection back to the archived Go executable without opening
   its key or database.
5. Verify the archived paths and metadata still match the cutover record.
6. Starting or re-enabling the archived installation is a separate operator action. Never run it
   concurrently with Rust or with another process holding the same identity.

The automated rehearsal uses a synthetic read-only Go installation, starts only an isolated Rust
candidate, stops it cleanly, switches an offline selector, verifies the archive is unchanged, and
proves that unrelated target-directory HQ processes were preserved. It deliberately does not start
the synthetic Go executable or open its state.

## Recovery limits

Rust identity export/import restores only the installation identity. Relay-retained canonical
facts may be rebuilt, but local-only SQLite history has no node-replacement guarantee. A Go key or
database is never a Rust recovery input. See [recovery.md](recovery.md) for the normative boundary.
