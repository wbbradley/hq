#!/usr/bin/env bash
set -euo pipefail

if [[ "${HQ_RUN_REAL_RELAY_SMOKE:-}" != "1" ]]; then
  echo "Set HQ_RUN_REAL_RELAY_SMOKE=1 to run the Docker-backed LAN smoke test." >&2
  exit 2
fi

for command in docker go jq curl sed; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Missing required command: $command" >&2
    exit 2
  fi
done

repo_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
smoke_dir=$(mktemp -d "${TMPDIR:-/tmp}/hq-lan-smoke.XXXXXX")
container="hq-lan-smoke-$$"
port="${HQ_SMOKE_PORT:-17447}"
relay_url="ws://127.0.0.1:${port}"
hq_bin="$smoke_dir/hq"
laptop_db="$smoke_dir/laptop/hq.db"
desktop_db="$smoke_dir/desktop/hq.db"
stranger_db="$smoke_dir/stranger/hq.db"
laptop_daemon_pid=""
desktop_daemon_pid=""

cleanup() {
  if [[ -n "$laptop_daemon_pid" ]]; then
    kill "$laptop_daemon_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$desktop_daemon_pid" ]]; then
    kill "$desktop_daemon_pid" >/dev/null 2>&1 || true
  fi
  docker rm -f "$container" >/dev/null 2>&1 || true
  if [[ "${HQ_SMOKE_KEEP:-}" == "1" ]]; then
    echo "Kept smoke state at $smoke_dir"
  else
    rm -rf -- "$smoke_dir"
  fi
}
trap cleanup EXIT

mkdir -p "$smoke_dir/laptop/worktree" "$smoke_dir/desktop/worktree" "$smoke_dir/relay/data"
go -C "$repo_dir" build -o "$hq_bin" ./cmd/hq
"$hq_bin" --db "$laptop_db" identity init >/dev/null
"$hq_bin" --db "$desktop_db" identity init >/dev/null
"$hq_bin" --db "$stranger_db" identity init >/dev/null

laptop_identity=$("$hq_bin" --db "$laptop_db" identity show --json)
desktop_identity=$("$hq_bin" --db "$desktop_db" identity show --json)
desktop_id=$(jq -r .installation_id <<<"$desktop_identity")
laptop_key=$(jq -r .public_key <<<"$laptop_identity")
desktop_key=$(jq -r .public_key <<<"$desktop_identity")
desktop_npub=$(jq -r .npub <<<"$desktop_identity")

sed \
  -e "s/REPLACE_WITH_FIRST_INSTALLATION_PUBLIC_KEY/$laptop_key/g" \
  -e "s/REPLACE_WITH_SECOND_INSTALLATION_PUBLIC_KEY/$desktop_key/g" \
  "$repo_dir/deploy/rnostr/rnostr.toml.example" >"$smoke_dir/relay/rnostr.toml"

docker run -d --name "$container" \
  -p "127.0.0.1:${port}:8080" \
  -v "$smoke_dir/relay/rnostr.toml:/rnostr/config/rnostr.toml:ro" \
  -v "$smoke_dir/relay/data:/rnostr/data" \
  rnostr/rnostr:v0.4.9@sha256:c022e4384f8fe1eb6023d497fe0c5cf9cd13d239f62160713546de4522f69214 >/dev/null

for ((attempt = 0; attempt < 100; attempt++)); do
  if curl -fsS "http://127.0.0.1:${port}/" >/dev/null; then
    break
  fi
  sleep 0.1
done
curl -fsS "http://127.0.0.1:${port}/" >/dev/null

"$hq_bin" --db "$laptop_db" relay add "$relay_url"
"$hq_bin" --db "$desktop_db" relay add "$relay_url"
"$hq_bin" --db "$laptop_db" human invite --name desktop --relay "$relay_url" "$desktop_id" "$desktop_npub" >"$smoke_dir/desktop.invite.json"
"$hq_bin" --db "$desktop_db" human join "$smoke_dir/desktop.invite.json"
for _ in 1 2 3; do
  "$hq_bin" --db "$desktop_db" sync
  "$hq_bin" --db "$laptop_db" sync
done

# Prove that the two long-running clients deliver live traffic without a manual sync.
"$hq_bin" --db "$laptop_db" daemon run >"$smoke_dir/laptop-daemon.log" 2>&1 &
laptop_daemon_pid=$!
"$hq_bin" --db "$desktop_db" daemon run >"$smoke_dir/desktop-daemon.log" 2>&1 &
desktop_daemon_pid=$!
for ((attempt = 0; attempt < 100; attempt++)); do
  if "$hq_bin" --db "$laptop_db" daemon status >/dev/null 2>&1 && "$hq_bin" --db "$desktop_db" daemon status >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done
"$hq_bin" --db "$laptop_db" daemon status >/dev/null
"$hq_bin" --db "$desktop_db" daemon status >/dev/null

live_question=$(HQ_SESSION=live-agent "$hq_bin" --db "$laptop_db" ask --dir "$smoke_dir/laptop/worktree" "live daemon question")
for ((attempt = 0; attempt < 100; attempt++)); do
  if "$hq_bin" --db "$desktop_db" list --recipient human --json | jq -e '[(. // [])[] | select(.id == "'"$live_question"'")] | length == 1' >/dev/null; then
    break
  fi
  sleep 0.1
done
"$hq_bin" --db "$desktop_db" list --recipient human --json | jq -e '[(. // [])[] | select(.id == "'"$live_question"'")] | length == 1' >/dev/null
"$hq_bin" --db "$desktop_db" answer "$live_question" "live daemon reply"
HQ_SESSION=live-agent "$hq_bin" --db "$laptop_db" wait --timeout 10s "$live_question" | grep -Fx "live daemon reply" >/dev/null

# Stop one machine's client, retain a message, then catch up when that client returns.
"$hq_bin" --db "$desktop_db" daemon stop
wait "$desktop_daemon_pid"
desktop_daemon_pid=""
machine_outage_question=$(HQ_SESSION=machine-outage-agent "$hq_bin" --db "$laptop_db" ask --dir "$smoke_dir/laptop/worktree" "caught after machine outage")
"$hq_bin" --db "$desktop_db" list --recipient human --json | jq -e '[(. // [])[] | select(.id == "'"$machine_outage_question"'")] | length == 0' >/dev/null
"$hq_bin" --db "$desktop_db" daemon run >>"$smoke_dir/desktop-daemon.log" 2>&1 &
desktop_daemon_pid=$!
for ((attempt = 0; attempt < 100; attempt++)); do
  if "$hq_bin" --db "$desktop_db" list --recipient human --json | jq -e '[(. // [])[] | select(.id == "'"$machine_outage_question"'")] | length == 1' >/dev/null; then
    break
  fi
  sleep 0.1
done
"$hq_bin" --db "$desktop_db" list --recipient human --json | jq -e '[(. // [])[] | select(.id == "'"$machine_outage_question"'")] | length == 1' >/dev/null
"$hq_bin" --db "$laptop_db" daemon stop
"$hq_bin" --db "$desktop_db" daemon stop
wait "$laptop_daemon_pid"
wait "$desktop_daemon_pid"
laptop_daemon_pid=""
desktop_daemon_pid=""

laptop_question=$(HQ_SESSION=laptop-agent "$hq_bin" --db "$laptop_db" ask --dir "$smoke_dir/laptop/worktree" "question from laptop")
desktop_question=$(HQ_SESSION=desktop-agent "$hq_bin" --db "$desktop_db" ask --dir "$smoke_dir/desktop/worktree" "question from desktop")
for _ in 1 2; do
  "$hq_bin" --db "$laptop_db" sync
  "$hq_bin" --db "$desktop_db" sync
done

"$hq_bin" --db "$desktop_db" list --recipient human --json | jq -e '[(. // [])[] | select(.body == "question from laptop")] | length == 1' >/dev/null
"$hq_bin" --db "$laptop_db" list --recipient human --json | jq -e '[(. // [])[] | select(.body == "question from desktop")] | length == 1' >/dev/null

"$hq_bin" --db "$desktop_db" answer "$laptop_question" "reply from desktop"
"$hq_bin" --db "$laptop_db" answer "$desktop_question" "reply from laptop"
for _ in 1 2; do
  "$hq_bin" --db "$laptop_db" sync
  "$hq_bin" --db "$desktop_db" sync
done
HQ_SESSION=laptop-agent "$hq_bin" --db "$laptop_db" wait --timeout 10s "$laptop_question" | grep -Fx "reply from desktop" >/dev/null
HQ_SESSION=desktop-agent "$hq_bin" --db "$desktop_db" wait --timeout 10s "$desktop_question" | grep -Fx "reply from laptop" >/dev/null
"$hq_bin" --db "$laptop_db" status --json | jq -e '.relay_accepted > 0 and (.relays | any(.last_event != null))' >/dev/null
"$hq_bin" --db "$desktop_db" status --json | jq -e '.relay_accepted > 0 and (.relays | any(.last_event != null))' >/dev/null

# Keep the desktop offline, restart the retained relay, then prove catch-up.
docker stop "$container" >/dev/null
offline_question=$(HQ_SESSION=offline-agent "$hq_bin" --no-sync --db "$laptop_db" ask --dir "$smoke_dir/laptop/worktree" "retained through outage")
docker start "$container" >/dev/null
for ((attempt = 0; attempt < 100; attempt++)); do
  if curl -fsS "http://127.0.0.1:${port}/" >/dev/null; then
    break
  fi
  sleep 0.1
done
"$hq_bin" --db "$laptop_db" sync
"$hq_bin" --db "$desktop_db" sync
"$hq_bin" --db "$desktop_db" list --recipient human --json | jq -e '[(. // [])[] | select(.id == "'"$offline_question"'")] | length == 1' >/dev/null
"$hq_bin" --db "$desktop_db" sync
"$hq_bin" --db "$desktop_db" list --recipient human --json | jq -e '[(. // [])[] | select(.id == "'"$offline_question"'")] | length == 1' >/dev/null

# A key outside the NIP-42 allow-list cannot read this relay.
"$hq_bin" --no-sync --db "$stranger_db" relay add "$relay_url"
if "$hq_bin" --db "$stranger_db" sync >/dev/null 2>&1; then
  echo "unauthorized installation unexpectedly synced" >&2
  exit 1
fi

"$hq_bin" --db "$laptop_db" human revoke "$desktop_id"
"$hq_bin" --db "$desktop_db" sync || true
if "$hq_bin" --db "$desktop_db" human show >/dev/null 2>&1; then
  echo "revoked installation retained an active human account" >&2
  exit 1
fi
post_revoke_question=$(HQ_SESSION=post-revoke-agent "$hq_bin" --db "$laptop_db" ask --dir "$smoke_dir/laptop/worktree" "not sent after revoke")
"$hq_bin" --db "$desktop_db" sync || true
"$hq_bin" --db "$desktop_db" list --recipient human --all --json | jq -e '[(. // [])[] | select(.id == "'"$post_revoke_question"'")] | length == 0' >/dev/null

echo "LAN relay smoke test passed."
