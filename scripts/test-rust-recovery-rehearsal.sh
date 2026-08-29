#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
matrix_file="$repository_root/qualification/platform-matrix.tsv"
fixture_directory=$(mktemp -d "${TMPDIR:-/tmp}/hq-rust-recovery-test.XXXXXX")
revision=0123456789abcdef0123456789abcdef01234567

cleanup() {
  rm -rf "$fixture_directory"
}
trap cleanup EXIT

write_recovery_fixture() {
  local operating_system=$1
  local architecture=$2
  local rust_host=$3
  jq -cn \
    --arg revision "$revision" --arg rust_host "$rust_host" \
    --arg operating_system "$operating_system" --arg architecture "$architecture" \
    '{schema:"hq-rust-recovery-rehearsal-v1",version:"0.1.0",revision:$revision,rust_host:$rust_host,operating_system:$operating_system,architecture:$architecture,identity_round_trip:"passed",identity_backup_scope:"identity_only",database_history_restore:"unsupported",database_repair:"passed",original_restart:"passed",node_replacement:"passed",clean_shutdown:"passed",go_state_access:"prohibited",go_state_unchanged:true}' \
    >"$fixture_directory/hq-recovery-$rust_host.json"
}

while IFS='|' read -r _evidence _runner operating_system architecture rust_host remainder; do
  [[ "$operating_system" == 'operating_system' ]] && continue
  [[ -z "${remainder:-}" ]]
  write_recovery_fixture "$operating_system" "$architecture" "$rust_host"
done <"$matrix_file"

aggregate="$fixture_directory/recovery-manifest.json"
"$repository_root/scripts/verify-rust-recovery-matrix.sh" \
  "$fixture_directory" "$revision" "$aggregate"
jq -e \
  '.schema == "hq-rust-recovery-manifest-v1" and .revision == $revision and (.rehearsals | length == 4)' \
  --arg revision "$revision" "$aggregate" >/dev/null

jq '.database_repair = "skipped"' \
  "$fixture_directory/hq-recovery-x86_64-unknown-linux-gnu.json" \
  >"$fixture_directory/invalid.json"
mv "$fixture_directory/invalid.json" \
  "$fixture_directory/hq-recovery-x86_64-unknown-linux-gnu.json"
if "$repository_root/scripts/verify-rust-recovery-matrix.sh" \
  "$fixture_directory" "$revision" "$aggregate.invalid" >/dev/null 2>&1; then
  printf 'recovery validator accepted incomplete database-repair evidence\n' >&2
  exit 1
fi

printf 'Rust recovery rehearsal validator tests passed.\n'
