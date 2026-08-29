#!/usr/bin/env bash

set -euo pipefail

fail() {
  printf 'Rust controlled-failure verification failed: %s\n' "$*" >&2
  exit 1
}

if (($# != 2)); then
  fail 'usage: scripts/verify-rust-controlled-failure.sh EVIDENCE REVISION'
fi

evidence=$1
revision=$2
[[ "$evidence" == /* && -f "$evidence" && ! -L "$evidence" ]] ||
  fail 'evidence must be an absolute regular file'
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || fail 'revision must be a full lowercase Git SHA'

jq -e \
  '.schema == "hq-rust-controlled-failure-rehearsal-v1" and
   .revision == $revision and
   .rust_host == "x86_64-unknown-linux-gnu" and
   .operating_system == "Linux" and
   .architecture == "x86_64" and
   .relay_image == "rnostr/rnostr:v0.4.9@sha256:c022e4384f8fe1eb6023d497fe0c5cf9cd13d239f62160713546de4522f69214" and
   .release_candidate_startup == "passed" and
   .offline_catch_up == "passed" and
   .relay_loss_observed == "passed" and
   .relay_recovery == "passed" and
   .provider_transport_crash == "passed" and
   .provider_worker_ownership_released == true and
   .ordered_drain == "passed" and
   .clean_shutdown == "passed" and
   .state_scope == "ephemeral" and
   .relay_scope == "controlled_pinned" and
   .production_identity_access == "prohibited" and
   (.version | type == "string")' \
  --arg revision "$revision" "$evidence" >/dev/null || fail 'evidence contract is incomplete'

printf 'Verified Rust controlled relay/provider failure evidence for revision %s.\n' "$revision"
