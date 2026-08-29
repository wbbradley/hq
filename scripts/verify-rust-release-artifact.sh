#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
matrix_file="$repository_root/qualification/platform-matrix.tsv"

fail() {
  printf 'Rust release artifact verification failed: %s\n' "$*" >&2
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
  fail 'usage: scripts/verify-rust-release-artifact.sh ARTIFACT_DIRECTORY REVISION RUST_HOST'
fi

artifact_directory=$1
revision=$2
rust_host=$3
[[ "$artifact_directory" == /* && -d "$artifact_directory" && ! -L "$artifact_directory" ]] ||
  fail 'artifact directory must be an absolute existing directory'
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || fail 'revision must be a full lowercase Git SHA'

matrix_row=$(awk -F '|' -v host="$rust_host" '$5 == host { print; found = 1 } END { if (!found) exit 1 }' \
  "$matrix_file") || fail 'Rust host is not in the release matrix'
IFS='|' read -r _evidence _runner expected_os expected_architecture expected_host <<<"$matrix_row"
actual_os=$(uname -s)
actual_architecture=$(uname -m)
actual_host=$(rustc -vV | sed -n 's/^host: //p')
[[ "$actual_os" == "$expected_os" ]] || fail "expected operating system $expected_os, got $actual_os"
[[ "$actual_architecture" == "$expected_architecture" ]] ||
  fail "expected architecture $expected_architecture, got $actual_architecture"
[[ "$actual_host" == "$expected_host" ]] || fail "expected Rust host $expected_host, got $actual_host"

shopt -s nullglob
archives=("$artifact_directory"/hq-v*-"$rust_host".tar.gz)
shopt -u nullglob
((${#archives[@]} == 1)) || fail 'expected exactly one native archive'
archive_path=${archives[0]}
archive=$(basename "$archive_path")
checksum_path="$archive_path.sha256"
[[ -f "$checksum_path" && ! -L "$checksum_path" ]] || fail 'missing regular checksum file'
digest=$(sha256_file "$archive_path")
[[ $(cat "$checksum_path") == "$digest  $archive" ]] || fail 'archive checksum does not match'

install_directory=$(mktemp -d "${TMPDIR:-/tmp}/hq-rust-release-install.XXXXXX")
state_root=$(mktemp -d "${TMPDIR:-/tmp}/hq-rust-release-state.XXXXXX")
rm -rf "$state_root"
installed_binary="$install_directory/hq"
cleanup() {
  if [[ -x "$installed_binary" && -d "$state_root" ]]; then
    "$installed_binary" --state-root "$state_root" daemon stop >/dev/null 2>&1 || true
  fi
  rm -rf "$install_directory" "$state_root"
}
trap cleanup EXIT

[[ $(tar -tzf "$archive_path") == 'hq' ]] || fail 'archive must contain only the hq executable'
tar -xzf "$archive_path" -C "$install_directory"
[[ -f "$installed_binary" && -x "$installed_binary" && ! -L "$installed_binary" ]] ||
  fail 'installed hq is not an executable regular file'

version_record=$($installed_binary --output json version) || fail 'installed version command failed'
version=$(jq -er \
  'select(.schema == "hq-cli-output-v1" and .ok == true and .kind == "version" and .data.name == "hq") | .data.version' \
  <<<"$version_record") || fail 'installed binary returned invalid version metadata'
embedded_revision=$(jq -er '.data.commit' <<<"$version_record") ||
  fail 'installed binary has no embedded revision'
[[ "$embedded_revision" == "$revision" ]] || fail 'installed binary revision does not match'
[[ "$archive" == "hq-v${version}-${rust_host}.tar.gz" ]] ||
  fail 'archive name does not match installed version and host'

$installed_binary --state-root "$state_root" --output json identity init >/dev/null ||
  fail 'installed identity initialization failed'
$installed_binary --state-root "$state_root" --output json daemon readiness >/dev/null ||
  fail 'installed daemon readiness failed'
status_record=$($installed_binary --state-root "$state_root" --output json daemon status) ||
  fail 'installed daemon status failed'
jq -e '.schema == "hq-cli-output-v1" and .ok == true and .kind == "lifecycle" and .data.state == "ready"' \
  <<<"$status_record" >/dev/null || fail 'installed daemon did not report ready'
$installed_binary --state-root "$state_root" --output json daemon stop >/dev/null ||
  fail 'installed daemon clean shutdown failed'

manifest="hq-v${version}-${rust_host}.json"
manifest_path="$artifact_directory/$manifest"
[[ ! -e "$manifest_path" && ! -L "$manifest_path" ]] || fail "manifest already exists: $manifest"
temporary_manifest=$(mktemp "$artifact_directory/.hq-rust-release-manifest.XXXXXX")
jq -cn \
  --arg schema 'hq-rust-release-artifact-v1' \
  --arg version "$version" \
  --arg revision "$revision" \
  --arg rust_host "$rust_host" \
  --arg operating_system "$expected_os" \
  --arg architecture "$expected_architecture" \
  --arg archive "$archive" \
  --arg archive_sha256 "$digest" \
  '{schema:$schema,version:$version,revision:$revision,rust_host:$rust_host,operating_system:$operating_system,architecture:$architecture,archive:$archive,archive_sha256:$archive_sha256,single_executable:true,installed_lifecycle:"passed"}' \
  >"$temporary_manifest"
chmod 644 "$temporary_manifest"
ln "$temporary_manifest" "$manifest_path" || fail 'could not publish release manifest'
rm -f "$temporary_manifest"

printf '%s\n' "$manifest_path"
