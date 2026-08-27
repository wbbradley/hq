#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
workspace_manifest="$repository_root/Cargo.toml"

fail() {
  printf 'rust architecture violation: %s\n' "$*" >&2
  exit 1
}

[[ -f "$workspace_manifest" ]] || fail "missing Cargo.toml workspace manifest"

expected_crates=(
  hq-application
  hq-codex
  hq-domain
  hq-harness
  hq-local-api
  hq-node
  hq-protocol
  hq-reducer
  hq-relay
  hq-store
  hq-testkit
  hq-tui
)

actual_crates=()
while IFS= read -r crate_directory; do
  actual_crates+=("${crate_directory##*/}")
done < <(find "$repository_root/crates" -mindepth 1 -maxdepth 1 -type d -name 'hq-*' | sort)

[[ "${actual_crates[*]}" == "${expected_crates[*]}" ]] ||
  fail "crate inventory differs: expected '${expected_crates[*]}', found '${actual_crates[*]}'"

for crate in "${expected_crates[@]}"; do
  manifest="$repository_root/crates/$crate/Cargo.toml"
  [[ -f "$manifest" ]] || fail "missing manifest for $crate"
  grep -Fq "\"crates/$crate\"" "$workspace_manifest" ||
    fail "$crate is not an explicit workspace member"
  grep -Eq '^\[lints\][[:space:]]*$' "$manifest" || fail "$crate does not inherit workspace lints"
  grep -Eq '^workspace[[:space:]]*=[[:space:]]*true[[:space:]]*$' "$manifest" ||
    fail "$crate does not enable workspace lints"
done

allowed_internal_dependency() {
  case "$1:$2" in
    hq-reducer:hq-domain | \
      hq-protocol:hq-domain | \
      hq-application:hq-domain | hq-application:hq-reducer | \
      hq-store:hq-domain | hq-store:hq-reducer | hq-store:hq-protocol | \
      hq-store:hq-application | \
      hq-local-api:hq-domain | hq-local-api:hq-protocol | hq-local-api:hq-application | \
      hq-relay:hq-domain | hq-relay:hq-protocol | hq-relay:hq-application | \
      hq-harness:hq-domain | hq-harness:hq-application | \
      hq-codex:hq-domain | hq-codex:hq-application | hq-codex:hq-harness | \
      hq-tui:hq-domain | hq-tui:hq-application | \
      hq-node:hq-domain | hq-node:hq-reducer | hq-node:hq-protocol | \
      hq-node:hq-application | hq-node:hq-store | hq-node:hq-local-api | \
      hq-node:hq-relay | hq-node:hq-harness | hq-node:hq-codex | \
      hq-node:hq-tui | hq-node:hq-testkit | \
      hq-testkit:hq-domain | hq-testkit:hq-reducer | hq-testkit:hq-protocol | \
      hq-testkit:hq-application)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

for crate in "${expected_crates[@]}"; do
  manifest="$repository_root/crates/$crate/Cargo.toml"
  while IFS= read -r dependency; do
    if ! allowed_internal_dependency "$crate" "$dependency"; then
      fail "$crate may not depend directly on $dependency"
    fi
  done < <(
    awk '
      /^\[/ { in_dependencies = ($0 ~ /dependencies/) }
      in_dependencies && /^hq-[a-z-]+(\.workspace)?[[:space:]]*=/ {
        dependency = $0
        sub(/[[:space:]]*=.*/, "", dependency)
        sub(/\.workspace$/, "", dependency)
        print dependency
      }
    ' "$manifest"
  )
done

grep -Eq '^hq-harness(\.workspace)?[[:space:]]*=' "$repository_root/crates/hq-codex/Cargo.toml" ||
  fail "hq-codex must depend on the neutral hq-harness contract"

if grep -ERiq --include='Cargo.toml' --include='*.rs' '(^|[^a-z])(codex)([^a-z]|$)' \
  "$repository_root/crates/hq-harness"; then
  fail "hq-harness contains Codex-specific vocabulary"
fi

for core_crate in hq-domain hq-reducer; do
  if grep -ERiq --include='Cargo.toml' --include='*.rs' \
    '(tokio|rusqlite|sqlite|nostr|ratatui|std::fs|std::process|codex|claude|anthropic|openai)' \
    "$repository_root/crates/$core_crate"; then
    fail "$core_crate contains a forbidden runtime, adapter, filesystem, process, or provider-specific reference"
  fi
done

binary_manifests=()
while IFS= read -r manifest; do
  binary_manifests+=("$manifest")
done < <(grep -El '^name[[:space:]]*=[[:space:]]*"hq"[[:space:]]*$' \
  "$repository_root"/crates/hq-*/Cargo.toml || true)

[[ "${#binary_manifests[@]}" -eq 1 ]] || fail "expected exactly one hq binary declaration"
[[ "${binary_manifests[0]}" == "$repository_root/crates/hq-node/Cargo.toml" ]] ||
  fail "the hq binary must be owned by hq-node"

printf 'Rust workspace architecture verified.\n'
