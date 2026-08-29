#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
matrix_file="$repository_root/qualification/platform-matrix.tsv"

fail() {
  printf 'Rust release matrix verification failed: %s\n' "$*" >&2
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
  fail 'usage: scripts/verify-rust-release-matrix.sh ARTIFACT_DIRECTORY REVISION OUTPUT_MANIFEST'
fi

artifact_directory=$1
revision=$2
output_manifest=$3
[[ "$artifact_directory" == /* && -d "$artifact_directory" && ! -L "$artifact_directory" ]] ||
  fail 'artifact directory must be an absolute existing directory'
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || fail 'revision must be a full lowercase Git SHA'
[[ "$output_manifest" == /* ]] || fail 'output manifest path must be absolute'
[[ -d $(dirname "$output_manifest") ]] || fail 'output manifest parent does not exist'
[[ ! -e "$output_manifest" && ! -L "$output_manifest" ]] || fail 'output manifest already exists'

temporary_output=$(mktemp "$(dirname "$output_manifest")/.hq-rust-release-aggregate.XXXXXX")
cleanup() {
  rm -f "$temporary_output"
}
trap cleanup EXIT

target_count=0
release_version=
ordered_manifests=()
while IFS='|' read -r _evidence _runner operating_system architecture rust_host remainder; do
  [[ "$operating_system" == 'operating_system' ]] && continue
  [[ -z "${remainder:-}" ]] || fail 'platform matrix contains an invalid row'
  shopt -s nullglob
  manifests=("$artifact_directory"/hq-v*-"$rust_host".json)
  shopt -u nullglob
  ((${#manifests[@]} == 1)) || fail "expected one manifest for $rust_host"
  manifest=${manifests[0]}
  jq -e \
    '.schema == "hq-rust-release-artifact-v1" and .revision == $revision and .rust_host == $rust_host and .operating_system == $operating_system and .architecture == $architecture and .single_executable == true and .installed_lifecycle == "passed" and (.version | type == "string") and (.archive | type == "string") and (.archive_sha256 | test("^[0-9a-f]{64}$"))' \
    --arg revision "$revision" --arg rust_host "$rust_host" \
    --arg operating_system "$operating_system" --arg architecture "$architecture" \
    "$manifest" >/dev/null || fail "invalid manifest for $rust_host"
  version=$(jq -er '.version' "$manifest")
  if [[ -z "$release_version" ]]; then
    release_version=$version
  fi
  [[ "$version" == "$release_version" ]] || fail 'target manifests disagree on release version'
  archive=$(jq -er '.archive' "$manifest")
  [[ "$archive" == "hq-v${version}-${rust_host}.tar.gz" ]] ||
    fail "unsafe archive name for $rust_host"
  archive_path="$artifact_directory/$archive"
  checksum_path="$archive_path.sha256"
  [[ -f "$archive_path" && ! -L "$archive_path" ]] || fail "missing archive for $rust_host"
  [[ -f "$checksum_path" && ! -L "$checksum_path" ]] || fail "missing checksum for $rust_host"
  digest=$(sha256_file "$archive_path")
  [[ "$digest" == $(jq -er '.archive_sha256' "$manifest") ]] ||
    fail "manifest checksum mismatch for $rust_host"
  [[ $(cat "$checksum_path") == "$digest  $archive" ]] ||
    fail "checksum file mismatch for $rust_host"
  ordered_manifests[${#ordered_manifests[@]}]=$manifest
  target_count=$((target_count + 1))
done <"$matrix_file"
((target_count == 4)) || fail 'release matrix must contain exactly four targets'

shopt -s nullglob
all_manifests=("$artifact_directory"/hq-v*.json)
shopt -u nullglob
((${#all_manifests[@]} == 4)) || fail 'artifact directory contains an unexpected target manifest'

jq -sc --arg schema 'hq-rust-release-manifest-v1' --arg revision "$revision" \
  '{schema:$schema,revision:$revision,artifacts:(sort_by(.rust_host))}' \
  "${ordered_manifests[@]}" >"$temporary_output"
chmod 644 "$temporary_output"
ln "$temporary_output" "$output_manifest" || fail 'could not publish aggregate release manifest'
rm -f "$temporary_output"

printf 'Verified %d Rust release artifacts for revision %s.\n' "$target_count" "$revision"
