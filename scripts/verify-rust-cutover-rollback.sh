#!/usr/bin/env bash

set -euo pipefail

fail() {
  printf 'Rust cutover rollback verification failed: %s\n' "$*" >&2
  exit 1
}

if (($# != 2)); then
  fail 'usage: scripts/verify-rust-cutover-rollback.sh EVIDENCE REVISION'
fi

evidence=$1
revision=$2
[[ "$evidence" == /* && -f "$evidence" && ! -L "$evidence" ]] ||
  fail 'evidence must be an absolute regular file'
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || fail 'revision must be a full lowercase Git SHA'

jq -e \
  '.schema == "hq-rust-cutover-rollback-rehearsal-v1" and
   .revision == $revision and
   .rust_host == "x86_64-unknown-linux-gnu" and
   .operating_system == "Linux" and
   .architecture == "x86_64" and
   .release_candidate_startup == "passed" and
   .release_candidate_clean_shutdown == "passed" and
   .release_candidate_state_scope == "ephemeral" and
   .archived_go_installation == "synthetic_read_only" and
   .archived_go_installation_unchanged == true and
   .archived_go_process_started == false and
   .archived_go_state_opened == false and
   .rollback_selector == "switched_while_offline" and
   .unrelated_hq_processes == "preserved" and
   .production_identity_access == "prohibited" and
   (.version | type == "string" and length > 0)' \
  --arg revision "$revision" "$evidence" >/dev/null || fail 'evidence contract is incomplete'

printf 'Verified Rust cutover rollback evidence for revision %s.\n' "$revision"
