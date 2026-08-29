#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
matrix_file="$repository_root/qualification/platform-matrix.tsv"
budget_file="$repository_root/qualification/budgets.env"
validation_directory=$(mktemp -d "${TMPDIR:-/tmp}/hq-rust-matrix.XXXXXX")

cleanup() {
  rm -rf "$validation_directory"
}
trap cleanup EXIT

fail() {
  printf 'Rust qualification matrix verification failed: %s\n' "$*" >&2
  exit 1
}

if (($# != 2)); then
  fail "usage: scripts/verify-rust-qualification-matrix.sh EVIDENCE_DIRECTORY GIT_REVISION"
fi

evidence_directory=$1
expected_revision=$2
[[ -d "$evidence_directory" && ! -L "$evidence_directory" ]] ||
  fail "evidence directory is missing or is a symbolic link"
[[ "$expected_revision" =~ ^[0-9a-f]{40}$ ]] || fail "expected revision is not a full Git SHA"
[[ -f "$matrix_file" ]] || fail "missing platform matrix"
[[ -f "$budget_file" ]] || fail "missing qualification budgets"
[[ $(head -n 1 "$matrix_file") == 'evidence|runner|operating_system|architecture|rust_host' ]] ||
  fail "platform matrix has an unknown header"

expected_keys="$validation_directory/expected-keys"
actual_keys="$validation_directory/actual-keys"
expected_files="$validation_directory/expected-files"
actual_files="$validation_directory/actual-files"
seen_runners="$validation_directory/seen-runners"
seen_hosts="$validation_directory/seen-hosts"
: >"$expected_files"
: >"$seen_runners"
: >"$seen_hosts"
cat >"$expected_keys" <<'EOF'
architecture
git_revision
operating_system
qualification_schema
release_build_seconds
rust_host
EOF
sed -n 's/=.*//p' "$budget_file" >>"$expected_keys"
LC_ALL=C sort -u "$expected_keys" -o "$expected_keys"

field() {
  local key=$1
  local record=$2
  sed -n "s/^${key}=//p" "$record"
}

printf '| Runner | Operating system | Architecture | Rust host | Release build |\n'
printf '| --- | --- | --- | --- | ---: |\n'
row_count=0
while IFS='|' read -r evidence runner operating_system architecture rust_host remainder; do
  [[ "$evidence" == 'evidence' ]] && continue
  [[ -n "$evidence" && -n "$runner" && -n "$operating_system" && -n "$architecture" &&
    -n "$rust_host" && -z "${remainder:-}" ]] || fail "platform matrix contains an incomplete row"
  [[ "$evidence" =~ ^[a-z0-9_-]+\.env$ ]] || fail "unsafe evidence filename: $evidence"
  printf '%s\n' "$evidence" >>"$expected_files"
  printf '%s\n' "$runner" >>"$seen_runners"
  printf '%s\n' "$rust_host" >>"$seen_hosts"
  record="$evidence_directory/$evidence"
  [[ -f "$record" && ! -L "$record" ]] || fail "missing regular evidence file: $evidence"
  grep -Eq '^[A-Za-z_][A-Za-z0-9_]*=[A-Za-z0-9._-]+$' "$record" ||
    fail "malformed evidence record: $evidence"
  sed 's/=.*//' "$record" | LC_ALL=C sort >"$actual_keys"
  diff -u "$expected_keys" "$actual_keys" >/dev/null || fail "unknown or missing keys in $evidence"
  [[ $(field qualification_schema "$record") == 'hq-rust-qualification-v1' ]] ||
    fail "unknown qualification schema in $evidence"
  [[ $(field operating_system "$record") == "$operating_system" ]] ||
    fail "operating system mismatch in $evidence"
  [[ $(field architecture "$record") == "$architecture" ]] ||
    fail "architecture mismatch in $evidence"
  [[ $(field rust_host "$record") == "$rust_host" ]] || fail "Rust host mismatch in $evidence"
  [[ $(field git_revision "$record") == "$expected_revision" ]] ||
    fail "Git revision mismatch in $evidence"
  release_build_seconds=$(field release_build_seconds "$record")
  [[ "$release_build_seconds" =~ ^[0-9]+$ ]] || fail "release build was skipped in $evidence"
  while IFS='=' read -r budget value; do
    [[ -z "$budget" || "$budget" == \#* ]] && continue
    [[ $(field "$budget" "$record") == "$value" ]] || fail "budget mismatch for $budget in $evidence"
  done <"$budget_file"
  release_limit=$(field HQ_QUALIFICATION_RELEASE_BUILD_MAX_SECONDS "$record")
  ((release_build_seconds <= release_limit)) || fail "release build budget exceeded in $evidence"
  printf '| %s | %s | %s | %s | %ss |\n' \
    "$runner" "$operating_system" "$architecture" "$rust_host" "$release_build_seconds"
  row_count=$((row_count + 1))
done <"$matrix_file"

((row_count == 4)) || fail "platform matrix must contain exactly four native targets"
[[ $(LC_ALL=C sort "$expected_files" | uniq -d | wc -l | tr -d ' ') == 0 ]] ||
  fail "platform matrix contains duplicate evidence filenames"
[[ $(LC_ALL=C sort "$seen_runners" | uniq -d | wc -l | tr -d ' ') == 0 ]] ||
  fail "platform matrix contains duplicate runners"
[[ $(LC_ALL=C sort "$seen_hosts" | uniq -d | wc -l | tr -d ' ') == 0 ]] ||
  fail "platform matrix contains duplicate Rust hosts"
find "$evidence_directory" -maxdepth 1 -name '*.env' -exec basename {} \; | LC_ALL=C sort >"$actual_files"
LC_ALL=C sort "$expected_files" -o "$expected_files"
diff -u "$expected_files" "$actual_files" >/dev/null ||
  fail "evidence directory does not contain the exact native record set"
