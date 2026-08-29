# Controlled relay and provider failure rehearsal

The manual Rust release-candidate workflow runs one destructive-by-design failure drill only on an
isolated Linux runner. It consumes the already packaged `x86_64-unknown-linux-gnu` executable and
requires its embedded revision to equal the checked-out source revision. The drill never uses a
standing HQ state directory, imported identity, provider credential, production relay, tag, or
release.

## Relay boundary

The runner creates a new identity under a short `/tmp/hq-rust-controlled-failure.*` state root and
starts the pinned rnostr v0.4.9 digest with a fresh data directory and a fixed loopback-only test
port. The container runs as the CI runner's UID and GID so only that runner owns the bind-mounted
data. Its temporary allow-list contains only the new installation and the two fixed interoperability keys.
The release executable reaches readiness, installs the exact relay policy, and remains ready while
the owned relay container is stopped.

The repository's opted-in interoperability contract publishes an authenticated, encrypted
kind-1059 wrapper, closes the publisher, catches the wrapper up from retained relay state, opens the
exact canonical bytes, and repeats after reconnect. The runner then stops the relay and proves both
that its endpoint is unreachable and that the HQ daemon continues serving lifecycle and relay-sync
requests. It restarts the same owned container and data directory, repeats the retained-event
contract, and verifies that the release node kept its enabled policy. This is evidence of bounded
loss survival and recovery, not a claim that a sync wake proves global causal completeness.

## Provider and drain boundary

Provider failure uses the deterministic scripted-provider seam at the same source revision. No
live Codex account or model quota is involved. The exact contracts prove that a transport-closed
poll is redacted and releases its sole worker lease, that response loss and partial persistence
reconcile before a forced teardown releases all worker ownership, and that node shutdown closes
admission before ordered component draining and final state-directory release.

The release daemon then stops cleanly and the offline identity command reacquires its state
directory. A trap scopes cleanup to the exact temporary state root and exact Docker container name.
It does not inspect or stop any unrelated HQ process.

## Evidence contract

`scripts/rehearse-rust-controlled-failure.sh BINARY OUTPUT REVISION` writes
`hq-rust-controlled-failure-rehearsal-v1` evidence only after every phase passes.
`scripts/verify-rust-controlled-failure.sh` rejects an incorrect revision, host, image digest,
skipped relay recovery, retained provider ownership, incomplete draining, non-ephemeral state, or
production-identity access. `scripts/test-rust-controlled-failure.sh` exercises valid and tampered
fixtures without Docker.

The workflow appends the evidence to the combined release-candidate artifact. This rehearsal does
not authorize a soak, cutover, tag, publication, or live service-manager change.
