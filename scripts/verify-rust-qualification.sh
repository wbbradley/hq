#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
budget_file="$repository_root/qualification/budgets.env"
evidence_file="$repository_root/qualification/acceptance-evidence.tsv"
qualification_directory=$(mktemp -d "${TMPDIR:-/tmp}/hq-rust-qualification.XXXXXX")

cleanup() {
  rm -rf "$qualification_directory"
}
trap cleanup EXIT

fail() {
  printf 'rust qualification failed: %s\n' "$*" >&2
  exit 1
}

[[ -f "$budget_file" ]] || fail "missing qualification budget file"
[[ -f "$evidence_file" ]] || fail "missing acceptance evidence inventory"

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
HQ_QUALIFICATION_QUEUE_SHUTDOWN_MAX_MILLISECONDS
HQ_QUALIFICATION_RELEASE_BUILD_MAX_SECONDS
EOF
sed -n 's/=.*//p' "$budget_file" | sort -u >"$actual_budgets"
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

header=$(head -n 1 "$evidence_file")
[[ "$header" == 'area|evidence|purpose' ]] || fail "acceptance inventory has an unknown header"
: >"$actual_areas"
while IFS='|' read -r area evidence purpose remainder; do
  [[ "$area" == 'area' ]] && continue
  [[ -n "$area" && -n "$evidence" && -n "$purpose" && -z "${remainder:-}" ]] ||
    fail "acceptance inventory contains an incomplete row"
  [[ -f "$repository_root/$evidence" ]] || fail "evidence path does not resolve: $evidence"
  printf '%s\n' "$area" >>"$actual_areas"
done <"$evidence_file"
sort -u "$actual_areas" -o "$actual_areas"
diff -u "$expected_areas" "$actual_areas" || fail "acceptance inventory areas differ"

cd "$repository_root"
cargo test --locked -p hq-store --test qualification_budgets
cargo test --locked -p hq-tui --test qualification_budgets
cargo test --locked -p hq-node --test unix_qualification_budgets
cargo test --locked -p hq-node --test unix_session_registry \
  drain_completes_while_the_shared_event_queue_is_saturated -- --exact

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
printf 'qualification_schema=hq-rust-qualification-v1\n'
printf 'operating_system=%s\n' "$(uname -s)"
printf 'architecture=%s\n' "$(uname -m)"
printf 'rust_host=%s\n' "$rust_host"
printf 'git_revision=%s\n' "$git_revision"
printf 'release_build_seconds=%s\n' "$release_build_seconds"
grep -E '^[A-Z][A-Z0-9_]*=[0-9]+$' "$budget_file"
