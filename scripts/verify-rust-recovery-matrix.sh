#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
matrix_file="$repository_root/qualification/platform-matrix.tsv"

fail() {
  printf 'Rust recovery matrix verification failed: %s\n' "$*" >&2
  exit 1
}

if (($# != 3)); then
  fail 'usage: scripts/verify-rust-recovery-matrix.sh EVIDENCE_DIRECTORY REVISION OUTPUT_MANIFEST'
fi

evidence_directory=$1
revision=$2
output_manifest=$3
[[ "$evidence_directory" == /* && -d "$evidence_directory" && ! -L "$evidence_directory" ]] ||
  fail 'evidence directory must be an absolute existing directory'
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || fail 'revision must be a full lowercase Git SHA'
[[ "$output_manifest" == /* ]] || fail 'output manifest path must be absolute'
output_parent=$(dirname "$output_manifest")
[[ -d "$output_parent" && ! -L "$output_parent" ]] || fail 'output parent does not exist'
[[ ! -e "$output_manifest" && ! -L "$output_manifest" ]] || fail 'output manifest already exists'

temporary_output=$(mktemp "$output_parent/.hq-rust-recovery-aggregate.XXXXXX")
cleanup() {
  rm -f "$temporary_output"
}
trap cleanup EXIT

rehearsals=()
recovery_version=
target_count=0
while IFS='|' read -r _evidence _runner operating_system architecture rust_host remainder; do
  [[ "$operating_system" == 'operating_system' ]] && continue
  [[ -z "${remainder:-}" ]] || fail 'platform matrix contains an invalid row'
  rehearsal="$evidence_directory/hq-recovery-$rust_host.json"
  [[ -f "$rehearsal" && ! -L "$rehearsal" ]] || fail "missing recovery evidence for $rust_host"
  jq -e \
    '.schema == "hq-rust-recovery-rehearsal-v1" and .revision == $revision and .rust_host == $rust_host and .operating_system == $operating_system and .architecture == $architecture and .identity_round_trip == "passed" and .identity_backup_scope == "identity_only" and .database_history_restore == "unsupported" and .database_repair == "passed" and .original_restart == "passed" and .node_replacement == "passed" and .clean_shutdown == "passed" and .go_state_access == "prohibited" and .go_state_unchanged == true and (.version | type == "string")' \
    --arg revision "$revision" --arg rust_host "$rust_host" \
    --arg operating_system "$operating_system" --arg architecture "$architecture" \
    "$rehearsal" >/dev/null || fail "invalid recovery evidence for $rust_host"
  version=$(jq -er '.version' "$rehearsal")
  if [[ -z "$recovery_version" ]]; then
    recovery_version=$version
  fi
  [[ "$version" == "$recovery_version" ]] || fail 'recovery targets disagree on release version'
  rehearsals[${#rehearsals[@]}]=$rehearsal
  target_count=$((target_count + 1))
done <"$matrix_file"
((target_count == 4)) || fail 'recovery matrix must contain exactly four targets'

shopt -s nullglob
all_rehearsals=("$evidence_directory"/hq-recovery-*.json)
shopt -u nullglob
((${#all_rehearsals[@]} == 4)) || fail 'evidence directory contains an unexpected recovery target'

jq -sc --arg revision "$revision" \
  '{schema:"hq-rust-recovery-manifest-v1",revision:$revision,rehearsals:(sort_by(.rust_host))}' \
  "${rehearsals[@]}" >"$temporary_output"
chmod 600 "$temporary_output"
ln "$temporary_output" "$output_manifest" || fail 'could not publish aggregate recovery manifest'
rm -f "$temporary_output"

printf 'Verified %d Rust recovery rehearsals for revision %s.\n' "$target_count" "$revision"
