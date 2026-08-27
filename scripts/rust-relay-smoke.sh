#!/usr/bin/env bash
set -euo pipefail

if [[ "${HQ_RUN_CONTROLLED_RELAY_SMOKE:-}" != "1" ]]; then
  echo "Set HQ_RUN_CONTROLLED_RELAY_SMOKE=1 to run the pinned Rust relay smoke." >&2
  exit 2
fi

for command in cargo curl docker sed; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Missing required command: $command" >&2
    exit 2
  fi
done
if ! docker info >/dev/null 2>&1; then
  echo "Docker is installed but its daemon is unavailable." >&2
  exit 2
fi

repo_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
smoke_dir=$(mktemp -d "${TMPDIR:-/tmp}/hq-rust-relay-smoke.XXXXXX")
container="hq-rust-relay-smoke-$$"
port="${HQ_CONTROLLED_RELAY_PORT:-17448}"
relay_url="${HQ_CONTROLLED_RELAY_URL:-ws://127.0.0.1:${port}}"
first_key="79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
second_key="c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5"

cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  if [[ "${HQ_CONTROLLED_RELAY_KEEP:-}" == "1" ]]; then
    echo "Kept controlled relay state at $smoke_dir"
  else
    rm -rf -- "$smoke_dir"
  fi
}
trap cleanup EXIT

mkdir -p "$smoke_dir/data"
sed \
  -e "s/REPLACE_WITH_FIRST_INSTALLATION_PUBLIC_KEY/$first_key/g" \
  -e "s/REPLACE_WITH_SECOND_INSTALLATION_PUBLIC_KEY/$second_key/g" \
  "$repo_dir/deploy/rnostr/rnostr.toml.example" >"$smoke_dir/rnostr.toml"

docker run -d --name "$container" \
  -p "127.0.0.1:${port}:8080" \
  -v "$smoke_dir/rnostr.toml:/rnostr/config/rnostr.toml:ro" \
  -v "$smoke_dir/data:/rnostr/data" \
  rnostr/rnostr:v0.4.9@sha256:c022e4384f8fe1eb6023d497fe0c5cf9cd13d239f62160713546de4522f69214 >/dev/null

ready=0
for ((attempt = 0; attempt < 100; attempt++)); do
  if curl -fsS "http://127.0.0.1:${port}/" >/dev/null; then
    ready=1
    break
  fi
  sleep 0.1
done
if [[ "$ready" != "1" ]]; then
  echo "Controlled rnostr did not become ready; inspect: docker logs $container" >&2
  exit 1
fi

HQ_RUN_CONTROLLED_RELAY_SMOKE=1 \
HQ_CONTROLLED_RELAY_URL="$relay_url" \
  cargo test --manifest-path "$repo_dir/Cargo.toml" -p hq-relay \
    --test rnostr_interop -- --ignored --nocapture

echo "Rust controlled relay smoke passed."
