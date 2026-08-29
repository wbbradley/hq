#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
scan_root=$repository_root
fixture_mode=0

fail() {
  printf 'Rust Go-independence verification failed: %s\n' "$*" >&2
  exit 1
}

if (($# == 2)) && [[ $1 == '--scan-fixture' ]]; then
  scan_root=$2
  fixture_mode=1
elif (($# != 0)); then
  fail 'usage: scripts/verify-rust-go-independence.sh [--scan-fixture ROOT]'
fi

[[ "$scan_root" == /* && -d "$scan_root" && ! -L "$scan_root" ]] ||
  fail 'scan root must be an absolute existing directory'

required_inputs=(
  Cargo.toml
  Cargo.lock
  crates/hq-node/Cargo.toml
  crates/hq-node/src/identity/paths.rs
  .github/workflows/release.yml
  deploy
  scripts/package-rust-release.sh
)
for input in "${required_inputs[@]}"; do
  [[ -e "$scan_root/$input" && ! -L "$scan_root/$input" ]] || fail "missing normal-operation input: $input"
done

manifests=()
while IFS= read -r input; do manifests+=("$input"); done < <(
  find "$scan_root/crates" -mindepth 2 -maxdepth 2 -type f -name Cargo.toml | sort
)
production_sources=()
while IFS= read -r input; do production_sources+=("$input"); done < <(
  find "$scan_root/crates" -type f -path '*/src/*.rs' | sort
)
build_scripts=()
while IFS= read -r input; do build_scripts+=("$input"); done < <(
  find "$scan_root/crates" -type f -name build.rs | sort
)
deployment_inputs=()
while IFS= read -r input; do deployment_inputs+=("$input"); done < <(
  find "$scan_root/deploy" -type f | sort
)

((${#manifests[@]} > 0)) || fail 'no Rust crate manifests found'
((${#production_sources[@]} > 0)) || fail 'no Rust production sources found'
((${#deployment_inputs[@]} > 0)) || fail 'no deployment inputs found'
((${#build_scripts[@]} == 0)) || fail "custom build scripts are not allowed: ${build_scripts[*]}"

manifest_inputs=("$scan_root/Cargo.toml" "$scan_root/Cargo.lock" "${manifests[@]}")
if grep -Einq \
  '(^|[^[:alnum:]_])(go/bin|cmd/hq|go\.mod|go\.sum|\.go([^[:alnum:]_]|$)|build[[:space:]]*=[[:space:]]*"build\.rs")' \
  "${manifest_inputs[@]}"; then
  fail 'Cargo inputs reference Go code or a custom build bridge'
fi

if grep -Einq \
  '(Command::new\([[:space:]]*"go"|include(_bytes|_str)?!\([^)]*\.go|go/bin|cmd/hq|internal/|hq\.db|schema[[:space:]_-]*33|local[[:space:]_-]*wire[[:space:]_-]*7)' \
  "${production_sources[@]}"; then
  fail 'Rust production sources reference a Go executable, source tree, or legacy state format'
fi

operational_inputs=(
  "$scan_root/.github/workflows/release.yml"
  "$scan_root/scripts/package-rust-release.sh"
  "${deployment_inputs[@]}"
)
if grep -Einq \
  '(^|[^[:alnum:]_])(setup-go|go[[:space:]]+(build|run|install|test)|go/bin|cmd/hq|go\.mod|go\.sum|[^/[:space:]]+\.go([^[:alnum:]_]|$)|hq\.db)' \
  "${operational_inputs[@]}"; then
  fail 'release, packaging, or deployment inputs reference Go code, tooling, or state'
fi

paths_source="$scan_root/crates/hq-node/src/identity/paths.rs"
for rust_state_name in identity.v1 local-config.v1.json hq.sqlite3 node.lock; do
  grep -Fq "\"$rust_state_name\"" "$paths_source" ||
    fail "Rust state path contract is missing $rust_state_name"
done

grep -Eq "tar[[:space:]].*-C[[:space:]]+\"?\\\$stage_directory\"?[[:space:]]+hq([[:space:]]|\$)" \
  "$scan_root/scripts/package-rust-release.sh" ||
  fail 'release packaging does not contain the sole hq executable'

if ((fixture_mode == 0)); then
  metadata=$(cargo metadata --locked --no-deps --format-version 1 --manifest-path "$scan_root/Cargo.toml") ||
    fail 'Cargo metadata could not be resolved'
  jq -e --arg root "$scan_root/crates/" '
    all(.packages[];
      (.manifest_path | startswith($root)) and
      all(.targets[]; (.kind | index("custom-build") | not))
    )
  ' <<<"$metadata" >/dev/null ||
    fail 'a workspace package is outside crates/ or owns a custom build target'
fi

printf 'Rust normal-operation Go independence verified.\n'
