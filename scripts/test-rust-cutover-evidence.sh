#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
fixture_directory=$(mktemp -d "${TMPDIR:-/tmp}/hq-rust-cutover-evidence-test.XXXXXX")
revision=0123456789abcdef0123456789abcdef01234567

cleanup() {
  rm -rf "$fixture_directory"
}
trap cleanup EXIT

jq -cn --arg revision "$revision" \
  '{schema:"hq-rust-release-manifest-v1",revision:$revision,artifacts:[{version:"0.1.0"},{version:"0.1.0"},{version:"0.1.0"},{version:"0.1.0"}]}' \
  >"$fixture_directory/release-manifest.json"
jq -cn --arg revision "$revision" \
  '{schema:"hq-rust-recovery-manifest-v1",revision:$revision,rehearsals:[{},{},{},{}]}' \
  >"$fixture_directory/recovery-manifest.json"
jq -cn --arg revision "$revision" \
  '{schema:"hq-rust-controlled-failure-rehearsal-v1",version:"0.1.0",revision:$revision,rust_host:"x86_64-unknown-linux-gnu",operating_system:"Linux",architecture:"x86_64",relay_image:"rnostr/rnostr:v0.4.9@sha256:c022e4384f8fe1eb6023d497fe0c5cf9cd13d239f62160713546de4522f69214",release_candidate_startup:"passed",offline_catch_up:"passed",relay_loss_observed:"passed",relay_recovery:"passed",provider_transport_crash:"passed",provider_worker_ownership_released:true,ordered_drain:"passed",clean_shutdown:"passed",state_scope:"ephemeral",relay_scope:"controlled_pinned",production_identity_access:"prohibited"}' \
  >"$fixture_directory/controlled-failure.json"
jq -cn --arg revision "$revision" \
  '{schema:"hq-rust-cutover-rollback-rehearsal-v1",version:"0.1.0",revision:$revision,rust_host:"x86_64-unknown-linux-gnu",operating_system:"Linux",architecture:"x86_64",release_candidate_startup:"passed",release_candidate_clean_shutdown:"passed",release_candidate_state_scope:"ephemeral",archived_go_installation:"synthetic_read_only",archived_go_installation_unchanged:true,archived_go_process_started:false,archived_go_state_opened:false,rollback_selector:"switched_while_offline",unrelated_hq_processes:"preserved",production_identity_access:"prohibited"}' \
  >"$fixture_directory/cutover-rollback.json"

bundle="$fixture_directory/cutover-evidence.json"
"$repository_root/scripts/generate-rust-cutover-evidence.sh" \
  "$fixture_directory" "$revision" "$bundle"

jq -e '
  .acceptance_and_definition_clauses as $clauses |
  ($clauses | length) == ($clauses | unique | length)
' "$bundle" >/dev/null

jq '.cutover_authorization = "not_required"' "$bundle" >"$bundle.invalid"
if "$repository_root/scripts/verify-rust-cutover-evidence.sh" \
  "$bundle.invalid" "$fixture_directory" "$revision" >/dev/null 2>&1; then
  printf 'cutover evidence validator accepted a missing separate authorization\n' >&2
  exit 1
fi

jq '.acceptance_and_definition_clauses = .acceptance_and_definition_clauses[:-1]' \
  "$bundle" >"$bundle.invalid"
if "$repository_root/scripts/verify-rust-cutover-evidence.sh" \
  "$bundle.invalid" "$fixture_directory" "$revision" >/dev/null 2>&1; then
  printf 'cutover evidence validator accepted a missing definition-of-done clause\n' >&2
  exit 1
fi

jq '.activation = "performed"' "$bundle" >"$bundle.invalid"
if "$repository_root/scripts/verify-rust-cutover-evidence.sh" \
  "$bundle.invalid" "$fixture_directory" "$revision" >/dev/null 2>&1; then
  printf 'cutover evidence validator accepted an activated candidate\n' >&2
  exit 1
fi

printf 'Rust cutover evidence bundle tests passed.\n'
