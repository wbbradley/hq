#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
matrix_file="$repository_root/qualification/platform-matrix.tsv"

fail() {
  printf 'Rust release publication preparation failed: %s\n' "$*" >&2
  exit 1
}

if (($# != 4)); then
  fail 'usage: scripts/prepare-rust-release-publication.sh CANDIDATE_DIRECTORY REVISION TAG OUTPUT_DIRECTORY'
fi

candidate_directory=$1
revision=$2
tag=$3
output_directory=$4
[[ "$candidate_directory" == /* && -d "$candidate_directory" && ! -L "$candidate_directory" ]] ||
  fail 'candidate directory must be an absolute existing directory'
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || fail 'revision must be a full lowercase Git SHA'
[[ "$tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?$ ]] ||
  fail 'tag must be a v-prefixed semantic version without build metadata'
[[ "$output_directory" == /* && ! -e "$output_directory" && ! -L "$output_directory" ]] ||
  fail 'output directory must be an absolute path that does not exist'
output_parent=$(dirname "$output_directory")
[[ -d "$output_parent" && ! -L "$output_parent" ]] || fail 'output parent must be an existing directory'

release_manifest="$candidate_directory/release-manifest.json"
[[ -f "$release_manifest" && ! -L "$release_manifest" ]] || fail 'release manifest is missing'
version=$(jq -er '.artifacts[0].version' "$release_manifest") || fail 'release version is missing'
[[ "v$version" == "$tag" ]] || fail "tag $tag does not match candidate version $version"

validation_directory=$(mktemp -d "${TMPDIR:-/tmp}/hq-rust-release-publication.XXXXXX")
expected_files="$validation_directory/expected-files"
actual_files="$validation_directory/actual-files"
cleanup() {
  rm -rf "$validation_directory"
}
trap cleanup EXIT

release_assets=()
: >"$expected_files"
while IFS='|' read -r _evidence _runner _operating_system _architecture rust_host remainder; do
  [[ "$rust_host" == 'rust_host' ]] && continue
  [[ -n "$rust_host" && -z "${remainder:-}" ]] || fail 'platform matrix contains an invalid row'
  archive="hq-v${version}-${rust_host}.tar.gz"
  release_target_manifest="hq-v${version}-${rust_host}.json"
  recovery_target="hq-recovery-${rust_host}.json"
  printf '%s\n' "$archive" "$archive.sha256" "$release_target_manifest" "$recovery_target" \
    >>"$expected_files"
  release_assets+=("$archive" "$archive.sha256")
done <"$matrix_file"

aggregate_evidence=(
  controlled-failure.json
  cutover-evidence.json
  cutover-rollback.json
  recovery-manifest.json
  release-manifest.json
)
printf '%s\n' "${aggregate_evidence[@]}" >>"$expected_files"
release_assets+=(
  release-manifest.json
  recovery-manifest.json
  controlled-failure.json
  cutover-rollback.json
  cutover-evidence.json
)

: >"$actual_files"
while IFS= read -r candidate_path; do
  [[ -f "$candidate_path" && ! -L "$candidate_path" ]] ||
    fail "candidate contains a non-regular entry: $(basename "$candidate_path")"
  basename "$candidate_path" >>"$actual_files"
done < <(find "$candidate_directory" -mindepth 1 -maxdepth 1 -print)
LC_ALL=C sort -o "$expected_files" "$expected_files"
LC_ALL=C sort -o "$actual_files" "$actual_files"
diff -u "$expected_files" "$actual_files" >/dev/null || fail 'candidate file set is incomplete or unexpected'

"$repository_root/scripts/verify-rust-release-matrix.sh" \
  "$candidate_directory" "$revision" "$validation_directory/release-manifest.json" >/dev/null
cmp -s "$release_manifest" "$validation_directory/release-manifest.json" ||
  fail 'release manifest does not match regenerated evidence'
"$repository_root/scripts/verify-rust-recovery-matrix.sh" \
  "$candidate_directory" "$revision" "$validation_directory/recovery-manifest.json" >/dev/null
cmp -s "$candidate_directory/recovery-manifest.json" \
  "$validation_directory/recovery-manifest.json" ||
  fail 'recovery manifest does not match regenerated evidence'
"$repository_root/scripts/verify-rust-controlled-failure.sh" \
  "$candidate_directory/controlled-failure.json" "$revision" >/dev/null
"$repository_root/scripts/verify-rust-cutover-rollback.sh" \
  "$candidate_directory/cutover-rollback.json" "$revision" >/dev/null
"$repository_root/scripts/verify-rust-cutover-evidence.sh" \
  "$candidate_directory/cutover-evidence.json" "$candidate_directory" "$revision" >/dev/null

jq -e --arg version "$version" \
  'all(.artifacts[]; .version == $version)' "$release_manifest" >/dev/null ||
  fail 'release artifacts disagree with the release version'
jq -e --arg version "$version" \
  'all(.rehearsals[]; .version == $version)' "$candidate_directory/recovery-manifest.json" \
  >/dev/null || fail 'recovery evidence disagrees with the release version'
for evidence in controlled-failure.json cutover-rollback.json cutover-evidence.json; do
  jq -e --arg version "$version" '.version == $version' "$candidate_directory/$evidence" \
    >/dev/null || fail "$evidence disagrees with the release version"
done

mkdir -m 700 "$output_directory"
for asset in "${release_assets[@]}"; do
  install -m 0644 "$candidate_directory/$asset" "$output_directory/$asset"
done

printf 'Prepared %d GitHub release assets for %s at revision %s.\n' \
  "${#release_assets[@]}" "$tag" "$revision"
