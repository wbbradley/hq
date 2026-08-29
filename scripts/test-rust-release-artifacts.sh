#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
matrix_file="$repository_root/qualification/platform-matrix.tsv"
fixture_directory=$(mktemp -d "${TMPDIR:-/tmp}/hq-rust-release-test.XXXXXX")
revision=0123456789abcdef0123456789abcdef01234567
version=0.1.0

cleanup() {
  rm -rf "$fixture_directory"
}
trap cleanup EXIT

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

write_target_fixture() {
  local operating_system=$1
  local architecture=$2
  local rust_host=$3
  local archive="hq-v${version}-${rust_host}.tar.gz"
  local manifest="hq-v${version}-${rust_host}.json"

  printf 'fixture for %s\n' "$rust_host" >"$fixture_directory/$archive"
  local digest
  digest=$(sha256_file "$fixture_directory/$archive")
  printf '%s  %s\n' "$digest" "$archive" >"$fixture_directory/$archive.sha256"
  printf '{"schema":"hq-rust-release-artifact-v1","version":"%s","revision":"%s","rust_host":"%s","operating_system":"%s","architecture":"%s","archive":"%s","archive_sha256":"%s","single_executable":true,"installed_lifecycle":"passed"}\n' \
    "$version" "$revision" "$rust_host" "$operating_system" "$architecture" "$archive" \
    "$digest" >"$fixture_directory/$manifest"
}

while IFS='|' read -r _evidence _runner operating_system architecture rust_host remainder; do
  [[ "$operating_system" == 'operating_system' ]] && continue
  [[ -z "${remainder:-}" ]]
  write_target_fixture "$operating_system" "$architecture" "$rust_host"
done <"$matrix_file"

aggregate="$fixture_directory/release-manifest.json"
"$repository_root/scripts/verify-rust-release-matrix.sh" \
  "$fixture_directory" "$revision" "$aggregate"
jq -e \
  '.schema == "hq-rust-release-manifest-v1" and .revision == $revision and (.artifacts | length == 4)' \
  --arg revision "$revision" "$aggregate" >/dev/null

printf 'corrupted\n' >>"$fixture_directory/hq-v${version}-x86_64-unknown-linux-gnu.tar.gz"
if "$repository_root/scripts/verify-rust-release-matrix.sh" \
  "$fixture_directory" "$revision" "$aggregate.invalid" >/dev/null 2>&1; then
  printf 'release validator accepted an archive with a mismatched checksum\n' >&2
  exit 1
fi

printf 'Rust release artifact validator tests passed.\n'
