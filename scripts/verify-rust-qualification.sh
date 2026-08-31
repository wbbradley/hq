#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
budget_file="$repository_root/qualification/budgets.env"
evidence_file=${HQ_QUALIFICATION_EVIDENCE_FILE:-"$repository_root/qualification/acceptance-evidence.tsv"}
evidence_output=${HQ_QUALIFICATION_EVIDENCE_OUTPUT:-}
qualification_directory=$(mktemp -d "${TMPDIR:-/tmp}/hq-rust-qualification.XXXXXX")
evidence_output_temporary=

cleanup() {
  if [[ -n "$evidence_output_temporary" ]]; then
    rm -f "$evidence_output_temporary"
  fi
  rm -rf "$qualification_directory"
}
trap cleanup EXIT

fail() {
  printf 'rust qualification failed: %s\n' "$*" >&2
  exit 1
}

[[ -f "$budget_file" ]] || fail "missing qualification budget file"
[[ -f "$evidence_file" ]] || fail "missing acceptance evidence inventory"
if (($# > 1)) || [[ ${1:-} != '' && ${1:-} != '--validate-only' ]]; then
  fail "usage: scripts/verify-rust-qualification.sh [--validate-only]"
fi
if [[ -n "$evidence_output" ]]; then
  [[ "$evidence_output" == /* ]] || fail "evidence output path must be absolute"
  evidence_output_parent=$(dirname "$evidence_output")
  [[ -d "$evidence_output_parent" ]] || fail "evidence output parent does not exist"
  [[ ! -e "$evidence_output" && ! -L "$evidence_output" ]] ||
    fail "evidence output already exists"
fi

if grep -Ev '^(#.*|[A-Z][A-Z0-9_]*=[0-9]+|[[:space:]]*)$' "$budget_file" >/dev/null; then
  fail "budget file contains a non-numeric or non-variable entry"
fi

expected_budgets="$qualification_directory/expected-budgets"
actual_budgets="$qualification_directory/actual-budgets"
cat >"$expected_budgets" <<'EOF'
HQ_QUALIFICATION_ACTIVE_RESIDENT_MEMORY_MAX_KIBIBYTES
HQ_QUALIFICATION_COLD_READINESS_MAX_MILLISECONDS
HQ_QUALIFICATION_FULL_REBUILD_MAX_MILLISECONDS
HQ_QUALIFICATION_GRACEFUL_SHUTDOWN_MAX_MILLISECONDS
HQ_QUALIFICATION_IDLE_RESIDENT_MEMORY_MAX_KIBIBYTES
HQ_QUALIFICATION_INVALIDATION_REDRAW_MAX_MILLISECONDS
HQ_QUALIFICATION_LATE_PARENT_FANOUT_MAX_MILLISECONDS
HQ_QUALIFICATION_LATER_PAGE_BATCH_MAX_MILLISECONDS
HQ_QUALIFICATION_MAXIMUM_MARKDOWN_PAGE_REDRAW_MAX_MILLISECONDS
HQ_QUALIFICATION_QUEUE_SHUTDOWN_MAX_MILLISECONDS
HQ_QUALIFICATION_RELEASE_BUILD_MAX_SECONDS
EOF
LC_ALL=C sort -u "$expected_budgets" -o "$expected_budgets"
sed -n 's/=.*//p' "$budget_file" | LC_ALL=C sort -u >"$actual_budgets"
diff -u "$expected_budgets" "$actual_budgets" || fail "qualification budget variables differ"

set -a
# shellcheck source=/dev/null
source "$budget_file"
set +a

expected_areas="$qualification_directory/expected-areas"
actual_areas="$qualification_directory/actual-areas"
cat >"$expected_areas" <<'EOF'
Authorization
Canonical protocol
Domain/algebra
Harness
Local API/node
Persistence
Projects
Queries
Relay
Security/operations
TUI/CLI
EOF
LC_ALL=C sort -u "$expected_areas" -o "$expected_areas"

header=$(head -n 1 "$evidence_file")
[[ "$header" == 'area|evidence|proof|purpose' ]] || fail "acceptance inventory has an unknown header"
: >"$actual_areas"
evidence_keys="$qualification_directory/evidence-keys"
: >"$evidence_keys"
while IFS='|' read -r area evidence proof purpose remainder; do
  [[ "$area" == 'area' ]] && continue
  [[ -n "$area" && -n "$evidence" && -n "$proof" && -n "$purpose" && -z "${remainder:-}" ]] ||
    fail "acceptance inventory contains an incomplete row"
  [[ -f "$repository_root/$evidence" ]] || fail "evidence path does not resolve: $evidence"
  git -C "$repository_root" ls-files --error-unmatch -- "$evidence" >/dev/null 2>&1 ||
    fail "evidence path is not tracked: $evidence"
  case "$proof" in
    test:*)
      test_name=${proof#test:}
      [[ "$test_name" =~ ^[a-z][a-z0-9_]*$ ]] || fail "invalid test selector: $proof"
      grep -Eq "^[[:space:]]*(pub[[:space:]]+)?(async[[:space:]]+)?fn[[:space:]]+${test_name}[[:space:]]*\\(" \
        "$repository_root/$evidence" || fail "test selector does not resolve: $evidence#$test_name"
      ;;
    command)
      [[ -x "$repository_root/$evidence" ]] || fail "command evidence is not executable: $evidence"
      ;;
    configuration)
      case "$evidence" in
        qualification/budgets.env | qualification/platform-matrix.tsv) ;;
        *) fail "unknown configuration evidence: $evidence" ;;
      esac
      ;;
    *)
      fail "unknown evidence proof: $proof"
      ;;
  esac
  printf '%s\n' "$area" >>"$actual_areas"
  printf '%s|%s\n' "$evidence" "$proof" >>"$evidence_keys"
done <"$evidence_file"
LC_ALL=C sort -u "$actual_areas" -o "$actual_areas"
diff -u "$expected_areas" "$actual_areas" || fail "acceptance inventory areas differ"
duplicate_evidence=$(LC_ALL=C sort "$evidence_keys" | uniq -d)
[[ -z "$duplicate_evidence" ]] || fail "duplicate evidence proof: $duplicate_evidence"

if [[ ${1:-} == '--validate-only' ]]; then
  printf 'Rust qualification evidence verified.\n'
  exit 0
fi

cd "$repository_root"
cargo test --locked -p hq-store --test qualification_budgets -- --test-threads=1
cargo test --locked -p hq-tui --test qualification_budgets -- --test-threads=1
cargo test --locked -p hq-node --test unix_qualification_budgets -- --test-threads=1
cargo test --locked -p hq-node --test unix_session_registry \
  drain_completes_while_the_shared_event_queue_is_saturated -- --exact --test-threads=1

release_build_seconds=skipped
if [[ "${HQ_QUALIFICATION_SKIP_RELEASE_BUILD:-0}" != 1 ]]; then
  release_target="$qualification_directory/release-target"
  release_started=$SECONDS
  CARGO_TARGET_DIR="$release_target" cargo build --locked --release -p hq-node --bin hq
  release_build_seconds=$((SECONDS - release_started))
  if ((release_build_seconds > HQ_QUALIFICATION_RELEASE_BUILD_MAX_SECONDS)); then
    fail "release build took ${release_build_seconds}s, exceeding ${HQ_QUALIFICATION_RELEASE_BUILD_MAX_SECONDS}s"
  fi
fi

rust_host=$(rustc -vV | sed -n 's/^host: //p')
git_revision=$(git rev-parse HEAD)

emit_evidence() {
  printf 'qualification_schema=hq-rust-qualification-v1\n'
  printf 'operating_system=%s\n' "$(uname -s)"
  printf 'architecture=%s\n' "$(uname -m)"
  printf 'rust_host=%s\n' "$rust_host"
  printf 'git_revision=%s\n' "$git_revision"
  printf 'release_build_seconds=%s\n' "$release_build_seconds"
  grep -E '^[A-Z][A-Z0-9_]*=[0-9]+$' "$budget_file"
}

if [[ -z "$evidence_output" ]]; then
  emit_evidence
else
  umask 077
  evidence_output_temporary=$(mktemp "$evidence_output_parent/.hq-rust-qualification-evidence.XXXXXX")
  emit_evidence | tee "$evidence_output_temporary"
  chmod 600 "$evidence_output_temporary"
  ln "$evidence_output_temporary" "$evidence_output" || fail "could not publish evidence output"
  rm -f "$evidence_output_temporary"
  evidence_output_temporary=
fi
