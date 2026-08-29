#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
rust_host=x86_64-unknown-linux-gnu

fail() {
  printf 'Rust cutover rollback rehearsal failed: %s\n' "$*" >&2
  exit 1
}

if (($# != 3)); then
  fail 'usage: scripts/rehearse-rust-cutover-rollback.sh BINARY OUTPUT REVISION'
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
  fail 'cutover rollback rehearsal requires Linux x86_64'
[[ $(rustc -vV | sed -n 's/^host: //p') == "$rust_host" ]] ||
  fail 'native Rust host does not match cutover rollback target'

version_record=$($binary --output json version) || fail 'binary version command failed'
version=$(jq -er \
  'select(.schema == "hq-cli-output-v1" and .ok == true and .kind == "version" and .data.name == "hq") | .data.version' \
  <<<"$version_record") || fail 'binary returned invalid version metadata'
[[ $(jq -er '.data.commit' <<<"$version_record") == "$revision" ]] ||
  fail 'binary embedded revision does not match rehearsal revision'

rehearsal_root=$(mktemp -d /tmp/hq-rust-cutover-rollback.XXXXXX)
case "$rehearsal_root" in
  /tmp/hq-rust-cutover-rollback.*) ;;
  *) fail 'temporary rehearsal root escaped its fixed namespace' ;;
esac
rust_state="$rehearsal_root/rust-state"
controlled_home="$rehearsal_root/home"
go_archive="$rehearsal_root/archived-go"
go_binary="$go_archive/bin/hq"
go_state="$go_archive/state"
go_key="$go_state/hq.key"
go_database="$go_state/hq.db"
go_log="$go_archive/log/hq.log"
selector="$rehearsal_root/operator-selected-hq"
temporary_evidence=

hq_command() {
  HOME="$controlled_home" "$binary" --state-root "$rust_state" --output json "$@"
}

inventory_target_daemons() {
  # shellcheck disable=SC2009 # Preserve complete PID/PPID/state/argv evidence, not just matching PIDs.
  ps -axo pid=,ppid=,stat=,args= |
    grep -E '/target/(debug|release)/h[q]( |$)' |
    LC_ALL=C sort || true
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

archive_metadata() {
  stat -c '%n|%d|%i|%a|%s|%Y|%Z' \
    "$go_archive" "$go_archive/bin" "$go_binary" "$go_state" \
    "$go_key" "$go_database" "$go_archive/log" "$go_log"
}

cleanup() {
  if [[ -d "$rust_state" ]]; then
    hq_command daemon stop >/dev/null 2>&1 || true
  fi
  chmod 700 "$go_archive" "$go_archive/bin" "$go_state" "$go_archive/log" >/dev/null 2>&1 || true
  chmod 600 "$go_binary" "$go_key" "$go_database" "$go_log" >/dev/null 2>&1 || true
  rm -rf "$rehearsal_root"
  if [[ -n "$temporary_evidence" ]]; then
    rm -f "$temporary_evidence"
  fi
}
trap cleanup EXIT

mkdir -p "$controlled_home" "$go_archive/bin" "$go_state" "$go_archive/log"
printf '#!/usr/bin/env sh\nprintf "synthetic archived Go binary must never run\\n" >&2\nexit 99\n' \
  >"$go_binary"
printf 'synthetic archived Go private-key sentinel\n' >"$go_key"
printf 'synthetic archived Go database sentinel\n' >"$go_database"
printf 'synthetic archived Go log sentinel\n' >"$go_log"
chmod 500 "$go_archive" "$go_archive/bin" "$go_state" "$go_archive/log"
chmod 500 "$go_binary"
chmod 000 "$go_key" "$go_database" "$go_log"
go_metadata_before=$(archive_metadata)

ln -s "$binary" "$selector"
unrelated_before=$(inventory_target_daemons)

identity=$(hq_command identity init) || fail 'release candidate identity initialization failed'
jq -e '.ok == true and .kind == "identity"' <<<"$identity" >/dev/null ||
  fail 'release candidate identity result was invalid'
ready=$(hq_command daemon readiness) || fail 'release candidate startup failed'
jq -e '.ok == true and .kind == "lifecycle" and .data.state == "ready"' \
  <<<"$ready" >/dev/null || fail 'release candidate did not become ready'
hq_command daemon stop >/dev/null || fail 'release candidate clean shutdown failed'
wait_for_offline_ownership || fail 'release candidate did not release state ownership'

replacement_selector="$selector.next"
ln -s "$go_binary" "$replacement_selector"
mv "$replacement_selector" "$selector"
[[ -L "$selector" && $(readlink "$selector") == "$go_binary" ]] ||
  fail 'offline rollback selector did not point at the archived Go executable'

go_metadata_after=$(archive_metadata)
[[ "$go_metadata_before" == "$go_metadata_after" ]] ||
  fail 'archived synthetic Go installation metadata changed'
unrelated_after=$(inventory_target_daemons)
[[ "$unrelated_before" == "$unrelated_after" ]] ||
  fail 'unrelated target-directory HQ process inventory changed'

temporary_evidence=$(mktemp "$evidence_parent/.hq-rust-cutover-rollback-evidence.XXXXXX")
jq -cn \
  --arg version "$version" --arg revision "$revision" --arg rust_host "$rust_host" \
  '{schema:"hq-rust-cutover-rollback-rehearsal-v1",version:$version,revision:$revision,rust_host:$rust_host,operating_system:"Linux",architecture:"x86_64",release_candidate_startup:"passed",release_candidate_clean_shutdown:"passed",release_candidate_state_scope:"ephemeral",archived_go_installation:"synthetic_read_only",archived_go_installation_unchanged:true,archived_go_process_started:false,archived_go_state_opened:false,rollback_selector:"switched_while_offline",unrelated_hq_processes:"preserved",production_identity_access:"prohibited"}' \
  >"$temporary_evidence"
chmod 600 "$temporary_evidence"
ln "$temporary_evidence" "$evidence_output" || fail 'could not publish cutover rollback evidence'
rm -f "$temporary_evidence"
temporary_evidence=

"$repository_root/scripts/verify-rust-cutover-rollback.sh" "$evidence_output" "$revision"
