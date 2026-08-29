#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
fixture_directory=$(mktemp -d "${TMPDIR:-/tmp}/hq-rust-cutover-rollback-test.XXXXXX")
revision=0123456789abcdef0123456789abcdef01234567
fixture="$fixture_directory/cutover-rollback.json"

cleanup() {
  rm -rf "$fixture_directory"
}
trap cleanup EXIT

jq -cn --arg revision "$revision" \
  '{schema:"hq-rust-cutover-rollback-rehearsal-v1",version:"0.1.0",revision:$revision,rust_host:"x86_64-unknown-linux-gnu",operating_system:"Linux",architecture:"x86_64",release_candidate_startup:"passed",release_candidate_clean_shutdown:"passed",release_candidate_state_scope:"ephemeral",archived_go_installation:"synthetic_read_only",archived_go_installation_unchanged:true,archived_go_process_started:false,archived_go_state_opened:false,rollback_selector:"switched_while_offline",unrelated_hq_processes:"preserved",production_identity_access:"prohibited"}' \
  >"$fixture"

"$repository_root/scripts/verify-rust-cutover-rollback.sh" "$fixture" "$revision"

jq '.archived_go_process_started = true' "$fixture" >"$fixture.invalid"
if "$repository_root/scripts/verify-rust-cutover-rollback.sh" \
  "$fixture.invalid" "$revision" >/dev/null 2>&1; then
  printf 'cutover rollback validator accepted a started archived Go process\n' >&2
  exit 1
fi

jq '.unrelated_hq_processes = "terminated"' "$fixture" >"$fixture.invalid"
if "$repository_root/scripts/verify-rust-cutover-rollback.sh" \
  "$fixture.invalid" "$revision" >/dev/null 2>&1; then
  printf 'cutover rollback validator accepted unrelated HQ process termination\n' >&2
  exit 1
fi

jq '.archived_go_installation_unchanged = false' "$fixture" >"$fixture.invalid"
if "$repository_root/scripts/verify-rust-cutover-rollback.sh" \
  "$fixture.invalid" "$revision" >/dev/null 2>&1; then
  printf 'cutover rollback validator accepted a changed Go archive\n' >&2
  exit 1
fi

printf 'Rust cutover rollback validator tests passed.\n'
