#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
matrix_file="$repository_root/qualification/platform-matrix.tsv"
budget_file="$repository_root/qualification/budgets.env"
fixture_directory=$(mktemp -d "${TMPDIR:-/tmp}/hq-rust-matrix-test.XXXXXX")
revision=0123456789abcdef0123456789abcdef01234567

cleanup() {
  rm -rf "$fixture_directory"
}
trap cleanup EXIT

matrix_json=$("$repository_root/scripts/rust-qualification-matrix-json.sh")
[[ $(printf '%s\n' "$matrix_json" | grep -o '"evidence"' | wc -l | tr -d ' ') == 4 ]]

write_record() {
  local evidence=$1
  local operating_system=$2
  local architecture=$3
  local rust_host=$4
  local git_revision=$5
  {
    printf 'qualification_schema=hq-rust-qualification-v1\n'
    printf 'operating_system=%s\n' "$operating_system"
    printf 'architecture=%s\n' "$architecture"
    printf 'rust_host=%s\n' "$rust_host"
    printf 'git_revision=%s\n' "$git_revision"
    printf 'release_build_seconds=1\n'
    grep -E '^[A-Z][A-Z0-9_]*=[0-9]+$' "$budget_file"
  } >"$fixture_directory/$evidence"
}

while IFS='|' read -r evidence _runner operating_system architecture rust_host remainder; do
  [[ "$evidence" == 'evidence' ]] && continue
  [[ -z "${remainder:-}" ]]
  write_record "$evidence" "$operating_system" "$architecture" "$rust_host" "$revision"
done <"$matrix_file"

"$repository_root/scripts/verify-rust-qualification-matrix.sh" "$fixture_directory" "$revision" \
  >/dev/null

write_record linux-x86_64.env Linux x86_64 x86_64-unknown-linux-gnu \
  fedcba9876543210fedcba9876543210fedcba98
if "$repository_root/scripts/verify-rust-qualification-matrix.sh" "$fixture_directory" "$revision" \
  >/dev/null 2>&1; then
  printf 'matrix validator accepted evidence from a different revision\n' >&2
  exit 1
fi

printf 'Rust qualification matrix validator tests passed.\n'
