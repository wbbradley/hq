#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
relay_image='rnostr/rnostr:v0.4.9@sha256:c022e4384f8fe1eb6023d497fe0c5cf9cd13d239f62160713546de4522f69214'
rust_host=x86_64-unknown-linux-gnu
first_key=79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798
second_key=c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5

fail() {
  printf 'Rust controlled relay/provider rehearsal failed: %s\n' "$*" >&2
  exit 1
}

if (($# != 3)); then
  fail 'usage: scripts/rehearse-rust-controlled-failure.sh BINARY OUTPUT REVISION'
fi

binary=$1
evidence_output=$2
revision=$3
[[ "$binary" == /* && -f "$binary" && -x "$binary" && ! -L "$binary" ]] ||
  fail 'binary must be an absolute executable regular file'
[[ "$evidence_output" == /* ]] || fail 'evidence output path must be absolute'
evidence_parent=$(dirname "$evidence_output")
[[ -d "$evidence_parent" && ! -L "$evidence_parent" ]] ||
  fail 'evidence output parent must be an existing regular directory'
[[ ! -e "$evidence_output" && ! -L "$evidence_output" ]] || fail 'evidence output already exists'
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || fail 'revision must be a full lowercase Git SHA'
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] ||
  fail 'controlled failure rehearsal requires Linux x86_64'
[[ $(rustc -vV | sed -n 's/^host: //p') == "$rust_host" ]] ||
  fail 'native Rust host does not match controlled failure target'

for command in cargo curl docker jq sed; do
  command -v "$command" >/dev/null 2>&1 || fail "missing required command: $command"
done
docker info >/dev/null 2>&1 || fail 'Docker is installed but its daemon is unavailable'

version_record=$($binary --output json version) || fail 'binary version command failed'
version=$(jq -er \
  'select(.schema == "hq-cli-output-v1" and .ok == true and .kind == "version" and .data.name == "hq") | .data.version' \
  <<<"$version_record") || fail 'binary returned invalid version metadata'
[[ $(jq -er '.data.commit' <<<"$version_record") == "$revision" ]] ||
  fail 'binary embedded revision does not match rehearsal revision'

rehearsal_root=$(mktemp -d /tmp/hq-rust-controlled-failure.XXXXXX)
case "$rehearsal_root" in
  /tmp/hq-rust-controlled-failure.*) ;;
  *) fail 'temporary rehearsal root escaped its fixed namespace' ;;
esac
state_root="$rehearsal_root/state"
controlled_home="$rehearsal_root/home"
relay_data="$rehearsal_root/relay-data"
relay_config="$rehearsal_root/rnostr.toml"
container="hq-rust-controlled-failure-$$"
container_user="$(id -u):$(id -g)"
relay_port=${HQ_CONTROLLED_RELAY_PORT:-17448}
[[ "$relay_port" =~ ^[0-9]+$ && "$relay_port" -ge 1024 && "$relay_port" -le 65535 ]] ||
  fail 'controlled relay port must be an unprivileged TCP port'
relay_url="ws://127.0.0.1:$relay_port"
temporary_evidence=

hq_command() {
  HOME="$controlled_home" "$binary" --state-root "$state_root" --output json "$@"
}

wait_for_relay() {
  local relay_port=$1
  local attempt=0
  while ((attempt < 150)); do
    if curl -fsS "http://127.0.0.1:$relay_port/" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
    attempt=$((attempt + 1))
  done
  return 1
}

wait_for_offline_ownership() {
  local attempt=0
  while ((attempt < 100)); do
    if hq_command identity show >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.05
    attempt=$((attempt + 1))
  done
  return 1
}

cleanup() {
  if [[ -d "$state_root" ]]; then
    hq_command daemon stop >/dev/null 2>&1 || true
  fi
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -rf "$rehearsal_root"
  if [[ -n "$temporary_evidence" ]]; then
    rm -f "$temporary_evidence"
  fi
}
trap cleanup EXIT

mkdir -p "$controlled_home" "$relay_data"
identity=$(hq_command identity init) || fail 'release candidate identity initialization failed'
installation_key=$(jq -er \
  'select(.schema == "hq-cli-output-v1" and .ok == true and .kind == "identity") | .data.signing_public_key' \
  <<<"$identity") || fail 'release candidate returned invalid identity metadata'
[[ "$installation_key" =~ ^[0-9a-f]{64}$ ]] || fail 'release candidate signing key was invalid'

sed \
  -e "s/REPLACE_WITH_FIRST_INSTALLATION_PUBLIC_KEY/$first_key/g" \
  -e "s/REPLACE_WITH_SECOND_INSTALLATION_PUBLIC_KEY/$second_key\",\n  \"$installation_key/g" \
  "$repository_root/deploy/rnostr/rnostr.toml.example" >"$relay_config"

docker run -d --name "$container" --user "$container_user" \
  -p "127.0.0.1:$relay_port:8080" \
  -v "$relay_config:/rnostr/config/rnostr.toml:ro" \
  -v "$relay_data:/rnostr/data" \
  "$relay_image" >/dev/null || fail 'controlled relay did not start'
if ! wait_for_relay "$relay_port"; then
  docker logs "$container" >&2 || true
  fail 'controlled relay did not become ready'
fi

ready=$(hq_command daemon readiness) || fail 'release candidate startup failed'
jq -e '.ok == true and .kind == "lifecycle" and .data.state == "ready"' \
  <<<"$ready" >/dev/null || fail 'release candidate did not become ready'
relay_add=$(hq_command relay add "$relay_url") || fail 'release candidate relay policy failed'
jq -e \
  '.ok == true and .kind == "relay_admin" and .data.operation == "relay_add" and
   (.data.policies | any(.endpoint == $relay and .enabled == true))' \
  --arg relay "$relay_url" <<<"$relay_add" >/dev/null || fail 'release relay policy was incomplete'

HQ_RUN_CONTROLLED_RELAY_SMOKE=1 HQ_CONTROLLED_RELAY_URL="$relay_url" \
  cargo test --locked --manifest-path "$repository_root/Cargo.toml" -p hq-relay \
    --test rnostr_interop controlled_rnostr_auth_publish_retained_and_reconnect -- --exact --ignored \
  || {
    docker logs "$container" >&2 || true
    fail 'controlled relay offline catch-up contract failed'
  }

docker stop --time 5 "$container" >/dev/null || fail 'controlled relay stop failed'
if curl -fsS "http://127.0.0.1:$relay_port/" >/dev/null 2>&1; then
  fail 'controlled relay remained reachable after stop'
fi
hq_command relay sync "$relay_url" >/dev/null || fail 'relay-loss synchronization wake failed'
loss_status=$(hq_command daemon status) || fail 'release candidate did not survive relay loss'
jq -e '.ok == true and .kind == "lifecycle" and .data.state == "ready"' \
  <<<"$loss_status" >/dev/null || fail 'release candidate was not ready during relay loss'

docker start "$container" >/dev/null || fail 'controlled relay restart failed'
if ! wait_for_relay "$relay_port"; then
  docker logs "$container" >&2 || true
  fail 'controlled relay did not recover'
fi
HQ_RUN_CONTROLLED_RELAY_SMOKE=1 HQ_CONTROLLED_RELAY_URL="$relay_url" \
  cargo test --locked --manifest-path "$repository_root/Cargo.toml" -p hq-relay \
    --test rnostr_interop controlled_rnostr_auth_publish_retained_and_reconnect -- --exact --ignored \
  || {
    docker logs "$container" >&2 || true
    fail 'controlled relay reconnect contract failed'
  }
hq_command relay sync "$relay_url" >/dev/null || fail 'post-recovery synchronization wake failed'
recovery_status=$(hq_command relay status) || fail 'post-recovery relay status failed'
jq -e \
  '.ok == true and .kind == "relay_admin" and
   (.data.policies | any(.endpoint == $relay and .enabled == true))' \
  --arg relay "$relay_url" <<<"$recovery_status" >/dev/null ||
  fail 'release relay policy was not retained after recovery'

cargo test --locked --manifest-path "$repository_root/Cargo.toml" -p hq-testkit \
  --test supervisor_recovery provider_poll_failure_is_redacted_and_releases_exact_worker_ownership \
  -- --exact || fail 'provider transport-crash containment contract failed'
cargo test --locked --manifest-path "$repository_root/Cargo.toml" -p hq-testkit \
  --test supervisor_recovery restart_reconciles_response_loss_and_partial_event_persistence_before_forced_teardown \
  -- --exact || fail 'provider forced-drain ownership contract failed'
cargo test --locked --manifest-path "$repository_root/Cargo.toml" -p hq-node \
  --test node_components shutdown_closes_admission_drains_in_order_escalates_and_releases_every_owner \
  -- --exact || fail 'ordered node drain contract failed'

hq_command daemon stop >/dev/null || fail 'release candidate clean shutdown failed'
wait_for_offline_ownership || fail 'release candidate did not release state ownership'

temporary_evidence=$(mktemp "$evidence_parent/.hq-rust-controlled-failure-evidence.XXXXXX")
jq -cn \
  --arg version "$version" --arg revision "$revision" --arg rust_host "$rust_host" \
  --arg relay_image "$relay_image" \
  '{schema:"hq-rust-controlled-failure-rehearsal-v1",version:$version,revision:$revision,rust_host:$rust_host,operating_system:"Linux",architecture:"x86_64",relay_image:$relay_image,release_candidate_startup:"passed",offline_catch_up:"passed",relay_loss_observed:"passed",relay_recovery:"passed",provider_transport_crash:"passed",provider_worker_ownership_released:true,ordered_drain:"passed",clean_shutdown:"passed",state_scope:"ephemeral",relay_scope:"controlled_pinned",production_identity_access:"prohibited"}' \
  >"$temporary_evidence"
chmod 600 "$temporary_evidence"
ln "$temporary_evidence" "$evidence_output" || fail 'could not publish controlled-failure evidence'
rm -f "$temporary_evidence"
temporary_evidence=

"$repository_root/scripts/verify-rust-controlled-failure.sh" "$evidence_output" "$revision"
