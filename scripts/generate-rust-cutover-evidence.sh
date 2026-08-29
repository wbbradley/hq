#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
contract="$repository_root/qualification/cutover-evidence.tsv"

fail() {
  printf 'Rust cutover evidence generation failed: %s\n' "$*" >&2
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
  fail 'usage: scripts/generate-rust-cutover-evidence.sh EVIDENCE_DIRECTORY REVISION OUTPUT'
fi

evidence_directory=$1
revision=$2
output=$3
[[ "$evidence_directory" == /* && -d "$evidence_directory" && ! -L "$evidence_directory" ]] ||
  fail 'evidence directory must be an absolute existing directory'
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || fail 'revision must be a full lowercase Git SHA'
[[ "$output" == /* ]] || fail 'output path must be absolute'
output_parent=$(dirname "$output")
[[ -d "$output_parent" && ! -L "$output_parent" ]] || fail 'output parent does not exist'
[[ ! -e "$output" && ! -L "$output" ]] || fail 'output already exists'

"$repository_root/scripts/verify-rust-qualification.sh" --validate-only >/dev/null

release_manifest="$evidence_directory/release-manifest.json"
recovery_manifest="$evidence_directory/recovery-manifest.json"
controlled_failure="$evidence_directory/controlled-failure.json"
cutover_rollback="$evidence_directory/cutover-rollback.json"
for evidence in "$release_manifest" "$recovery_manifest" "$controlled_failure" "$cutover_rollback"; do
  [[ -f "$evidence" && ! -L "$evidence" ]] || fail "missing regular evidence file: $evidence"
done

jq -e '.schema == "hq-rust-release-manifest-v1" and .revision == $revision and (.artifacts | length == 4)' \
  --arg revision "$revision" "$release_manifest" >/dev/null || fail 'release manifest is invalid'
jq -e '.schema == "hq-rust-recovery-manifest-v1" and .revision == $revision and (.rehearsals | length == 4)' \
  --arg revision "$revision" "$recovery_manifest" >/dev/null || fail 'recovery manifest is invalid'
"$repository_root/scripts/verify-rust-controlled-failure.sh" "$controlled_failure" "$revision" >/dev/null
"$repository_root/scripts/verify-rust-cutover-rollback.sh" "$cutover_rollback" "$revision" >/dev/null

version=$(jq -er '.artifacts[0].version' "$release_manifest") || fail 'release version is missing'
jq -e --arg version "$version" 'all(.artifacts[]; .version == $version)' \
  "$release_manifest" >/dev/null || fail 'release artifacts disagree on version'

expected_clauses=$(mktemp "$output_parent/.hq-rust-cutover-expected.XXXXXX")
actual_clauses=$(mktemp "$output_parent/.hq-rust-cutover-actual.XXXXXX")
temporary_output=$(mktemp "$output_parent/.hq-rust-cutover-evidence.XXXXXX")
cleanup() {
  rm -f "$expected_clauses" "$actual_clauses" "$temporary_output"
}
trap cleanup EXIT

cat >"$expected_clauses" <<'EOF'
acceptance:Authorization
acceptance:Canonical protocol
acceptance:Domain/algebra
acceptance:Harness
acceptance:Local API/node
acceptance:Persistence
acceptance:Projects
acceptance:Queries
acceptance:Relay
acceptance:Security/operations
acceptance:TUI/CLI
definition:atomicity
definition:causal-authority
definition:convergence
definition:domain-state-transitions
definition:durable-and-external-recovery
definition:go-independent-normal-operation
definition:lifecycle-ownership
definition:reviewed-requirements-and-algebra
definition:rust-era-protocol-specifications
EOF

header=$(head -n 1 "$contract")
[[ "$header" == 'clause|evidence|proof|purpose' ]] || fail 'cutover contract has an unknown header'
: >"$actual_clauses"
while IFS='|' read -r clause evidence proof purpose remainder; do
  [[ "$clause" == 'clause' ]] && continue
  [[ -n "$clause" && -n "$evidence" && -n "$proof" && -n "$purpose" && -z "${remainder:-}" ]] ||
    fail 'cutover contract contains an incomplete row'
  [[ -f "$repository_root/$evidence" ]] || fail "cutover evidence path does not resolve: $evidence"
  git -C "$repository_root" ls-files --error-unmatch -- "$evidence" >/dev/null 2>&1 ||
    fail "cutover evidence path is not tracked: $evidence"
  case "$proof" in
    acceptance-area)
      area=${clause#acceptance:}
      [[ "$clause" == acceptance:* ]] || fail "acceptance proof has invalid clause: $clause"
      awk -F '|' -v area="$area" '$1 == area { found = 1 } END { exit !found }' \
        "$repository_root/qualification/acceptance-evidence.tsv" ||
        fail "acceptance area does not resolve: $area"
      ;;
    test:*)
      test_name=${proof#test:}
      [[ "$test_name" =~ ^[a-z][a-z0-9_]*$ ]] || fail "invalid test selector: $proof"
      grep -Eq "^[[:space:]]*(pub[[:space:]]+)?(async[[:space:]]+)?fn[[:space:]]+${test_name}[[:space:]]*\\(" \
        "$repository_root/$evidence" || fail "test selector does not resolve: $evidence#$test_name"
      ;;
    command)
      [[ -x "$repository_root/$evidence" ]] || fail "command evidence is not executable: $evidence"
      ;;
    specification) ;;
    *) fail "unknown cutover proof: $proof" ;;
  esac
  printf '%s\n' "$clause" >>"$actual_clauses"
done <"$contract"
LC_ALL=C sort "$expected_clauses" -o "$expected_clauses"
LC_ALL=C sort "$actual_clauses" -o "$actual_clauses"
diff -u "$expected_clauses" "$actual_clauses" || fail 'cutover clauses differ'

clauses=$(jq -Rsc 'split("\n") | map(select(length > 0))' <"$actual_clauses")
jq -cn \
  --arg version "$version" --arg revision "$revision" \
  --arg release_sha256 "$(sha256_file "$release_manifest")" \
  --arg recovery_sha256 "$(sha256_file "$recovery_manifest")" \
  --arg controlled_sha256 "$(sha256_file "$controlled_failure")" \
  --arg rollback_sha256 "$(sha256_file "$cutover_rollback")" \
  --arg contract_sha256 "$(sha256_file "$contract")" \
  --argjson clauses "$clauses" \
  '{schema:"hq-rust-cutover-evidence-v1",version:$version,revision:$revision,acceptance_and_definition_clauses:$clauses,evidence_sha256:{cutover_contract:$contract_sha256,release_manifest:$release_sha256,recovery_manifest:$recovery_sha256,controlled_failure:$controlled_sha256,cutover_rollback:$rollback_sha256},soak_authorization:"operator_required",cutover_authorization:"separate_operator_required",activation:"not_performed",production_identity_access:"prohibited"}' \
  >"$temporary_output"
chmod 600 "$temporary_output"
ln "$temporary_output" "$output" || fail 'could not publish cutover evidence'
rm -f "$temporary_output"
temporary_output=

"$repository_root/scripts/verify-rust-cutover-evidence.sh" "$output" "$evidence_directory" "$revision"
