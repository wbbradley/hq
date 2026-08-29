#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
matrix_file="$repository_root/qualification/platform-matrix.tsv"

fail() {
  printf 'Rust recovery rehearsal failed: %s\n' "$*" >&2
  exit 1
}

if (($# != 4)); then
  fail 'usage: scripts/rehearse-rust-recovery.sh BINARY OUTPUT REVISION RUST_HOST'
fi

binary=$1
evidence_output=$2
revision=$3
rust_host=$4
[[ "$binary" == /* && -f "$binary" && -x "$binary" && ! -L "$binary" ]] ||
  fail 'binary must be an absolute executable regular file'
[[ "$evidence_output" == /* ]] || fail 'evidence output path must be absolute'
evidence_parent=$(dirname "$evidence_output")
[[ -d "$evidence_parent" && ! -L "$evidence_parent" ]] ||
  fail 'evidence output parent must be an existing regular directory'
[[ ! -e "$evidence_output" && ! -L "$evidence_output" ]] || fail 'evidence output already exists'
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || fail 'revision must be a full lowercase Git SHA'

matrix_row=$(awk -F '|' -v host="$rust_host" '$5 == host { print; found = 1 } END { if (!found) exit 1 }' \
  "$matrix_file") || fail 'Rust host is not in the recovery matrix'
IFS='|' read -r _evidence _runner expected_os expected_architecture expected_host <<<"$matrix_row"
[[ $(uname -s) == "$expected_os" ]] || fail 'operating system does not match recovery target'
[[ $(uname -m) == "$expected_architecture" ]] || fail 'architecture does not match recovery target'
[[ $(rustc -vV | sed -n 's/^host: //p') == "$expected_host" ]] ||
  fail 'native Rust host does not match recovery target'

version_record=$($binary --output json version) || fail 'binary version command failed'
version=$(jq -er \
  'select(.schema == "hq-cli-output-v1" and .ok == true and .kind == "version" and .data.name == "hq") | .data.version' \
  <<<"$version_record") || fail 'binary returned invalid version metadata'
[[ $(jq -er '.data.commit' <<<"$version_record") == "$revision" ]] ||
  fail 'binary embedded revision does not match rehearsal revision'

temporary_base=/tmp
rehearsal_root=$(mktemp -d "$temporary_base/hq-rust-recovery.XXXXXX")
case "$rehearsal_root" in
  "$temporary_base"/hq-rust-recovery.*) ;;
  *) fail 'temporary rehearsal root escaped its fixed namespace' ;;
esac
original_state="$rehearsal_root/original-state"
replacement_state="$rehearsal_root/replacement-state"
backup="$rehearsal_root/identity-backup.v1.json"
controlled_home="$rehearsal_root/home"
legacy_go_state="$controlled_home/.local/state/hq"
legacy_go_key="$legacy_go_state/hq.key"
legacy_go_database="$legacy_go_state/hq.db"
temporary_evidence=

hq_command() {
  HOME="$controlled_home" "$binary" "$@"
}

wait_for_offline_ownership() {
  local state_root=$1
  local attempt=0
  while ((attempt < 100)); do
    if hq_command --state-root "$state_root" --output json identity show >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.05
    attempt=$((attempt + 1))
  done
  return 1
}

cleanup() {
  if [[ -d "$original_state" ]]; then
    hq_command --state-root "$original_state" daemon stop >/dev/null 2>&1 || true
  fi
  if [[ -d "$replacement_state" ]]; then
    hq_command --state-root "$replacement_state" daemon stop >/dev/null 2>&1 || true
  fi
  chmod 700 "$legacy_go_state" >/dev/null 2>&1 || true
  chmod 600 "$legacy_go_key" "$legacy_go_database" >/dev/null 2>&1 || true
  rm -rf "$rehearsal_root"
  if [[ -n "$temporary_evidence" ]]; then
    rm -f "$temporary_evidence"
  fi
}
trap cleanup EXIT

mkdir -p "$legacy_go_state"
printf 'synthetic inaccessible Go key sentinel\n' >"$legacy_go_key"
printf 'synthetic inaccessible Go database sentinel\n' >"$legacy_go_database"
chmod 000 "$legacy_go_key" "$legacy_go_database"
chmod 500 "$legacy_go_state"
if [[ "$expected_os" == 'Darwin' ]]; then
  go_state_before=$(stat -f '%d:%i:%p:%z:%m:%c' "$legacy_go_state" "$legacy_go_key" "$legacy_go_database")
else
  go_state_before=$(stat -c '%d:%i:%a:%s:%Y:%Z' "$legacy_go_state" "$legacy_go_key" "$legacy_go_database")
fi

identity_original=$(hq_command --state-root "$original_state" --output json identity init) ||
  fail 'original identity initialization failed'
jq -e '.ok == true and .kind == "identity"' <<<"$identity_original" >/dev/null ||
  fail 'original identity result was invalid'
hq_command --state-root "$original_state" --output json \
  config set default-provider recovery-provider >/dev/null || fail 'local configuration failed'
original_ready=$(hq_command --state-root "$original_state" --output json daemon readiness) ||
  fail 'original node startup failed'
jq -e '.ok == true and .kind == "lifecycle" and .data.state == "ready"' \
  <<<"$original_ready" >/dev/null || fail 'original node did not become ready'
hq_command --state-root "$original_state" --output json human create recovery >/dev/null ||
  fail 'recovery account creation failed'
hq_command --state-root "$original_state" --output json agent create recovery-agent >/dev/null ||
  fail 'recovery agent creation failed'

human_before=$(hq_command --state-root "$original_state" --output json human show) ||
  fail 'pre-repair human snapshot failed'
agents_before=$(hq_command --state-root "$original_state" --output json agent list) ||
  fail 'pre-repair agent snapshot failed'
repair=$(hq_command --state-root "$original_state" --output json relay repair) ||
  fail 'explicit database repair failed'
jq -e \
  '.schema == "hq-cli-output-v1" and .ok == true and .kind == "relay_admin" and .data.operation == "relay_repair" and .data.outcome == "repaired" and (.data.revision | type == "number")' \
  <<<"$repair" >/dev/null || fail 'database repair evidence was incomplete'
human_after=$(hq_command --state-root "$original_state" --output json human show) ||
  fail 'post-repair human snapshot failed'
agents_after=$(hq_command --state-root "$original_state" --output json agent list) ||
  fail 'post-repair agent snapshot failed'
human_before_canonical=$(jq -Sc '.data' <<<"$human_before")
human_after_canonical=$(jq -Sc '.data' <<<"$human_after")
agents_before_canonical=$(jq -Sc '.data' <<<"$agents_before")
agents_after_canonical=$(jq -Sc '.data' <<<"$agents_after")
[[ "$human_before_canonical" == "$human_after_canonical" ]] ||
  fail 'database repair changed the human projection'
[[ "$agents_before_canonical" == "$agents_after_canonical" ]] ||
  fail 'database repair changed the agent projection'

restart=$(hq_command --state-root "$original_state" --output json daemon restart) ||
  fail 'original node restart failed'
jq -e '.ok == true and .kind == "lifecycle" and .data.state == "ready"' \
  <<<"$restart" >/dev/null || fail 'original node did not become ready after restart'
hq_command --state-root "$original_state" --output json daemon stop >/dev/null ||
  fail 'original node clean shutdown failed'
wait_for_offline_ownership "$original_state" || fail 'original node did not release state ownership'

password="hq-ephemeral-recovery-$revision-$RANDOM-$RANDOM"
printf '%s\n' "$password" | hq_command --state-root "$original_state" --output json \
  identity export "$backup" --password-stdin >/dev/null || fail 'identity export failed'
[[ -f "$backup" && ! -L "$backup" ]] || fail 'identity backup was not a regular file'
printf '%s\n' "$password" | hq_command --state-root "$replacement_state" --output json \
  identity import "$backup" --password-stdin >/dev/null || fail 'identity import failed'
password=

identity_replacement=$(hq_command --state-root "$replacement_state" --output json identity show) ||
  fail 'replacement identity inspection failed'
identity_original_canonical=$(jq -Sc '.data' <<<"$identity_original")
identity_replacement_canonical=$(jq -Sc '.data' <<<"$identity_replacement")
[[ "$identity_original_canonical" == "$identity_replacement_canonical" ]] ||
  fail 'replacement identity does not match original authority'
[[ ! -e "$replacement_state/hq.sqlite3" && ! -L "$replacement_state/hq.sqlite3" ]] ||
  fail 'identity import unexpectedly restored database history'
[[ ! -e "$replacement_state/local-config.v1.json" && ! -L "$replacement_state/local-config.v1.json" ]] ||
  fail 'identity import unexpectedly restored local configuration'
replacement_config=$(hq_command --state-root "$replacement_state" --output json config get) ||
  fail 'replacement configuration inspection failed'
jq -e '.ok == true and .kind == "configuration" and .data.default_provider == null and .data.relays == []' \
  <<<"$replacement_config" >/dev/null || fail 'replacement inherited excluded configuration'

replacement_ready=$(hq_command --state-root "$replacement_state" --output json daemon readiness) ||
  fail 'replacement node startup failed'
jq -e '.ok == true and .kind == "lifecycle" and .data.state == "ready"' \
  <<<"$replacement_ready" >/dev/null || fail 'replacement node did not become ready'
replacement_human=$(hq_command --state-root "$replacement_state" --output json human show) ||
  fail 'replacement empty-history inspection failed'
replacement_agents=$(hq_command --state-root "$replacement_state" --output json agent list) ||
  fail 'replacement empty agent-history inspection failed'
jq -e '.data.accounts == [] and .data.active_account == null' <<<"$replacement_human" >/dev/null ||
  fail 'identity-only replacement unexpectedly restored account history'
jq -e '.data.agents == []' <<<"$replacement_agents" >/dev/null ||
  fail 'identity-only replacement unexpectedly restored agent history'
hq_command --state-root "$replacement_state" --output json daemon stop >/dev/null ||
  fail 'replacement node clean shutdown failed'
wait_for_offline_ownership "$replacement_state" || fail 'replacement node did not release ownership'

if [[ "$expected_os" == 'Darwin' ]]; then
  go_state_after=$(stat -f '%d:%i:%p:%z:%m:%c' "$legacy_go_state" "$legacy_go_key" "$legacy_go_database")
else
  go_state_after=$(stat -c '%d:%i:%a:%s:%Y:%Z' "$legacy_go_state" "$legacy_go_key" "$legacy_go_database")
fi
[[ "$go_state_before" == "$go_state_after" ]] || fail 'synthetic inaccessible Go state changed'

temporary_evidence=$(mktemp "$evidence_parent/.hq-rust-recovery-evidence.XXXXXX")
jq -cn \
  --arg version "$version" --arg revision "$revision" --arg rust_host "$rust_host" \
  --arg operating_system "$expected_os" --arg architecture "$expected_architecture" \
  '{schema:"hq-rust-recovery-rehearsal-v1",version:$version,revision:$revision,rust_host:$rust_host,operating_system:$operating_system,architecture:$architecture,identity_round_trip:"passed",identity_backup_scope:"identity_only",database_history_restore:"unsupported",database_repair:"passed",original_restart:"passed",node_replacement:"passed",clean_shutdown:"passed",go_state_access:"prohibited",go_state_unchanged:true}' \
  >"$temporary_evidence"
chmod 600 "$temporary_evidence"
ln "$temporary_evidence" "$evidence_output" || fail 'could not publish recovery evidence'
rm -f "$temporary_evidence"
temporary_evidence=

printf '%s\n' "$evidence_output"
