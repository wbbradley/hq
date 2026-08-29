#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
contract="$repository_root/qualification/cutover-evidence.tsv"

fail() {
  printf 'Rust cutover evidence verification failed: %s\n' "$*" >&2
  exit 1
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

if (($# != 3)); then
  fail 'usage: scripts/verify-rust-cutover-evidence.sh BUNDLE EVIDENCE_DIRECTORY REVISION'
fi

bundle=$1
evidence_directory=$2
revision=$3
[[ "$bundle" == /* && -f "$bundle" && ! -L "$bundle" ]] || fail 'bundle must be an absolute regular file'
[[ "$evidence_directory" == /* && -d "$evidence_directory" && ! -L "$evidence_directory" ]] ||
  fail 'evidence directory must be an absolute existing directory'
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || fail 'revision must be a full lowercase Git SHA'

release_sha256=$(sha256_file "$evidence_directory/release-manifest.json")
recovery_sha256=$(sha256_file "$evidence_directory/recovery-manifest.json")
controlled_sha256=$(sha256_file "$evidence_directory/controlled-failure.json")
rollback_sha256=$(sha256_file "$evidence_directory/cutover-rollback.json")
contract_sha256=$(sha256_file "$contract")
clauses=$(tail -n +2 "$contract" | cut -d '|' -f 1 | LC_ALL=C sort -u |
  jq -Rsc 'split("\n") | map(select(length > 0))')

jq -e \
  '.schema == "hq-rust-cutover-evidence-v1" and
   .revision == $revision and
   (.version | type == "string" and length > 0) and
   .acceptance_and_definition_clauses == $clauses and
   .evidence_sha256.cutover_contract == $contract_sha256 and
   .evidence_sha256.release_manifest == $release_sha256 and
   .evidence_sha256.recovery_manifest == $recovery_sha256 and
   .evidence_sha256.controlled_failure == $controlled_sha256 and
   .evidence_sha256.cutover_rollback == $rollback_sha256 and
   .soak_authorization == "operator_required" and
   .cutover_authorization == "separate_operator_required" and
   .activation == "not_performed" and
   .production_identity_access == "prohibited"' \
  --arg revision "$revision" \
  --arg contract_sha256 "$contract_sha256" \
  --arg release_sha256 "$release_sha256" --arg recovery_sha256 "$recovery_sha256" \
  --arg controlled_sha256 "$controlled_sha256" --arg rollback_sha256 "$rollback_sha256" \
  --argjson clauses "$clauses" \
  "$bundle" >/dev/null || fail 'cutover evidence contract is incomplete or does not bind its inputs'

printf 'Verified Rust cutover evidence bundle for revision %s.\n' "$revision"
