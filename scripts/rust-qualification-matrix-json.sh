#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
matrix_file="$repository_root/qualification/platform-matrix.tsv"

fail() {
  printf 'Rust qualification matrix generation failed: %s\n' "$*" >&2
  exit 1
}

[[ -f "$matrix_file" ]] || fail "missing platform matrix"
[[ $(head -n 1 "$matrix_file") == 'evidence|runner|operating_system|architecture|rust_host' ]] ||
  fail "platform matrix has an unknown header"

printf '{"include":['
separator=
row_count=0
while IFS='|' read -r evidence runner operating_system architecture rust_host remainder; do
  [[ "$evidence" == 'evidence' ]] && continue
  [[ -n "$evidence" && -n "$runner" && -n "$operating_system" && -n "$architecture" &&
    -n "$rust_host" && -z "${remainder:-}" ]] || fail "platform matrix contains an incomplete row"
  [[ "$evidence" =~ ^[a-z0-9_-]+\.env$ ]] || fail "unsafe evidence filename: $evidence"
  [[ "$runner" =~ ^[a-z0-9.-]+$ ]] || fail "unsafe runner label: $runner"
  [[ "$operating_system" =~ ^(Linux|Darwin)$ ]] || fail "unknown operating system: $operating_system"
  [[ "$architecture" =~ ^(x86_64|aarch64|arm64)$ ]] || fail "unknown architecture: $architecture"
  [[ "$rust_host" =~ ^[a-z0-9_-]+$ ]] || fail "unsafe Rust host: $rust_host"
  printf '%s{"evidence":"%s","runner":"%s","operating_system":"%s","architecture":"%s","rust_host":"%s"}' \
    "$separator" "$evidence" "$runner" "$operating_system" "$architecture" "$rust_host"
  separator=,
  row_count=$((row_count + 1))
done <"$matrix_file"
((row_count == 4)) || fail "platform matrix must contain exactly four native targets"
printf ']}\n'
