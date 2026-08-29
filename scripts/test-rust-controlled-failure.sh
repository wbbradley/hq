#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
fixture_directory=$(mktemp -d "${TMPDIR:-/tmp}/hq-rust-controlled-failure-test.XXXXXX")
revision=0123456789abcdef0123456789abcdef01234567
fixture="$fixture_directory/controlled-failure.json"

cleanup() {
  rm -rf "$fixture_directory"
}
trap cleanup EXIT

jq -cn --arg revision "$revision" \
  '{schema:"hq-rust-controlled-failure-rehearsal-v1",version:"0.1.0",revision:$revision,rust_host:"x86_64-unknown-linux-gnu",operating_system:"Linux",architecture:"x86_64",relay_image:"rnostr/rnostr:v0.4.9@sha256:c022e4384f8fe1eb6023d497fe0c5cf9cd13d239f62160713546de4522f69214",release_candidate_startup:"passed",offline_catch_up:"passed",relay_loss_observed:"passed",relay_recovery:"passed",provider_transport_crash:"passed",provider_worker_ownership_released:true,ordered_drain:"passed",clean_shutdown:"passed",state_scope:"ephemeral",relay_scope:"controlled_pinned",production_identity_access:"prohibited"}' \
  >"$fixture"

"$repository_root/scripts/verify-rust-controlled-failure.sh" "$fixture" "$revision"

jq '.provider_worker_ownership_released = false' "$fixture" >"$fixture.invalid"
if "$repository_root/scripts/verify-rust-controlled-failure.sh" \
  "$fixture.invalid" "$revision" >/dev/null 2>&1; then
  printf 'controlled-failure validator accepted retained provider ownership\n' >&2
  exit 1
fi

jq '.relay_recovery = "skipped"' "$fixture" >"$fixture.invalid"
if "$repository_root/scripts/verify-rust-controlled-failure.sh" \
  "$fixture.invalid" "$revision" >/dev/null 2>&1; then
  printf 'controlled-failure validator accepted skipped relay recovery\n' >&2
  exit 1
fi

printf 'Rust controlled relay/provider failure validator tests passed.\n'
