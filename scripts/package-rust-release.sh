#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
matrix_file="$repository_root/qualification/platform-matrix.tsv"

fail() {
  printf 'Rust release packaging failed: %s\n' "$*" >&2
  exit 1
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

if (($# != 4)); then
  fail 'usage: scripts/package-rust-release.sh BINARY OUTPUT_DIRECTORY REVISION RUST_HOST'
fi

binary=$1
output_directory=$2
revision=$3
rust_host=$4

[[ "$binary" == /* && -f "$binary" && -x "$binary" && ! -L "$binary" ]] ||
  fail 'binary must be an absolute executable regular file'
[[ "$output_directory" == /* && -d "$output_directory" && ! -L "$output_directory" ]] ||
  fail 'output directory must be an absolute existing directory'
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || fail 'revision must be a full lowercase Git SHA'
grep -Fq "|$rust_host" "$matrix_file" || fail 'Rust host is not in the release matrix'

actual_host=$(rustc -vV | sed -n 's/^host: //p')
[[ "$actual_host" == "$rust_host" ]] ||
  fail "native Rust host $actual_host does not match requested host $rust_host"

version_record=$($binary --output json version) || fail 'binary version command failed'
version=$(jq -er \
  'select(.schema == "hq-cli-output-v1" and .ok == true and .kind == "version" and .data.name == "hq") | .data.version' \
  <<<"$version_record") || fail 'binary returned invalid version metadata'
embedded_revision=$(jq -er '.data.commit' <<<"$version_record") ||
  fail 'binary has no embedded release revision'
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]] ||
  fail 'binary version is not safe for an artifact name'
[[ "$embedded_revision" == "$revision" ]] ||
  fail 'binary embedded revision does not match requested revision'

archive="hq-v${version}-${rust_host}.tar.gz"
checksum="$archive.sha256"
[[ ! -e "$output_directory/$archive" && ! -L "$output_directory/$archive" ]] ||
  fail "archive already exists: $archive"
[[ ! -e "$output_directory/$checksum" && ! -L "$output_directory/$checksum" ]] ||
  fail "checksum already exists: $checksum"

stage_directory=$(mktemp -d "${TMPDIR:-/tmp}/hq-rust-release-stage.XXXXXX")
temporary_archive=$(mktemp "$output_directory/.hq-rust-release-archive.XXXXXX")
temporary_checksum=$(mktemp "$output_directory/.hq-rust-release-checksum.XXXXXX")
cleanup() {
  rm -rf "$stage_directory"
  rm -f "$temporary_archive" "$temporary_checksum"
}
trap cleanup EXIT

cp "$binary" "$stage_directory/hq"
chmod 755 "$stage_directory/hq"
tar -czf "$temporary_archive" -C "$stage_directory" hq
digest=$(sha256_file "$temporary_archive")
printf '%s  %s\n' "$digest" "$archive" >"$temporary_checksum"
chmod 644 "$temporary_archive" "$temporary_checksum"
ln "$temporary_archive" "$output_directory/$archive" || fail 'could not publish archive'
ln "$temporary_checksum" "$output_directory/$checksum" || fail 'could not publish checksum'

printf '%s\n' "$output_directory/$archive"
