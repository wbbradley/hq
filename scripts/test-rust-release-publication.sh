#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
matrix_file="$repository_root/qualification/platform-matrix.tsv"
fixture_root=$(mktemp -d "${TMPDIR:-/tmp}/hq-rust-release-publication-test.XXXXXX")
candidate_directory="$fixture_root/candidate"
revision=0123456789abcdef0123456789abcdef01234567
version=0.1.0

cleanup() {
  rm -rf "$fixture_root"
}
trap cleanup EXIT

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

mkdir "$candidate_directory"
while IFS='|' read -r _evidence _runner operating_system architecture rust_host remainder; do
  [[ "$operating_system" == 'operating_system' ]] && continue
  [[ -z "${remainder:-}" ]]
  archive="hq-v${version}-${rust_host}.tar.gz"
  printf 'release fixture for %s\n' "$rust_host" >"$candidate_directory/$archive"
  digest=$(sha256_file "$candidate_directory/$archive")
  printf '%s  %s\n' "$digest" "$archive" >"$candidate_directory/$archive.sha256"
  jq -cn \
    --arg version "$version" --arg revision "$revision" --arg rust_host "$rust_host" \
    --arg operating_system "$operating_system" --arg architecture "$architecture" \
    --arg archive "$archive" --arg archive_sha256 "$digest" \
    '{schema:"hq-rust-release-artifact-v1",version:$version,revision:$revision,rust_host:$rust_host,operating_system:$operating_system,architecture:$architecture,archive:$archive,archive_sha256:$archive_sha256,single_executable:true,installed_lifecycle:"passed"}' \
    >"$candidate_directory/hq-v${version}-${rust_host}.json"
  jq -cn \
    --arg version "$version" --arg revision "$revision" --arg rust_host "$rust_host" \
    --arg operating_system "$operating_system" --arg architecture "$architecture" \
    '{schema:"hq-rust-recovery-rehearsal-v1",version:$version,revision:$revision,rust_host:$rust_host,operating_system:$operating_system,architecture:$architecture,identity_round_trip:"passed",identity_backup_scope:"identity_only",database_history_restore:"unsupported",database_repair:"passed",original_restart:"passed",node_replacement:"passed",clean_shutdown:"passed",go_state_access:"prohibited",go_state_unchanged:true}' \
    >"$candidate_directory/hq-recovery-$rust_host.json"
done <"$matrix_file"

"$repository_root/scripts/verify-rust-release-matrix.sh" \
  "$candidate_directory" "$revision" "$candidate_directory/release-manifest.json" >/dev/null
"$repository_root/scripts/verify-rust-recovery-matrix.sh" \
  "$candidate_directory" "$revision" "$candidate_directory/recovery-manifest.json" >/dev/null
jq -cn --arg version "$version" --arg revision "$revision" \
  '{schema:"hq-rust-controlled-failure-rehearsal-v1",version:$version,revision:$revision,rust_host:"x86_64-unknown-linux-gnu",operating_system:"Linux",architecture:"x86_64",relay_image:"rnostr/rnostr:v0.4.9@sha256:c022e4384f8fe1eb6023d497fe0c5cf9cd13d239f62160713546de4522f69214",release_candidate_startup:"passed",offline_catch_up:"passed",relay_loss_observed:"passed",relay_recovery:"passed",provider_transport_crash:"passed",provider_worker_ownership_released:true,ordered_drain:"passed",clean_shutdown:"passed",state_scope:"ephemeral",relay_scope:"controlled_pinned",production_identity_access:"prohibited"}' \
  >"$candidate_directory/controlled-failure.json"
jq -cn --arg version "$version" --arg revision "$revision" \
  '{schema:"hq-rust-cutover-rollback-rehearsal-v1",version:$version,revision:$revision,rust_host:"x86_64-unknown-linux-gnu",operating_system:"Linux",architecture:"x86_64",release_candidate_startup:"passed",release_candidate_clean_shutdown:"passed",release_candidate_state_scope:"ephemeral",archived_go_installation:"synthetic_read_only",archived_go_installation_unchanged:true,archived_go_process_started:false,archived_go_state_opened:false,rollback_selector:"switched_while_offline",unrelated_hq_processes:"preserved",production_identity_access:"prohibited"}' \
  >"$candidate_directory/cutover-rollback.json"
"$repository_root/scripts/generate-rust-cutover-evidence.sh" \
  "$candidate_directory" "$revision" "$candidate_directory/cutover-evidence.json" >/dev/null

published_directory="$fixture_root/published"
"$repository_root/scripts/prepare-rust-release-publication.sh" \
  "$candidate_directory" "$revision" "v$version" "$published_directory"

[[ $(find "$published_directory" -type f | wc -l | tr -d ' ') == 13 ]]
for rust_host in $(tail -n +2 "$matrix_file" | cut -d '|' -f 5); do
  [[ -f "$published_directory/hq-v${version}-${rust_host}.tar.gz" ]]
  [[ -f "$published_directory/hq-v${version}-${rust_host}.tar.gz.sha256" ]]
done
for evidence in release-manifest.json recovery-manifest.json controlled-failure.json \
  cutover-rollback.json cutover-evidence.json; do
  [[ -f "$published_directory/$evidence" ]]
done

if "$repository_root/scripts/prepare-rust-release-publication.sh" \
  "$candidate_directory" "$revision" 'v0.1.1' "$fixture_root/wrong-tag" >/dev/null 2>&1; then
  printf 'release publication accepted a tag that disagrees with the candidate version\n' >&2
  exit 1
fi

cp -R "$candidate_directory" "$fixture_root/corrupt-candidate"
printf 'corrupt\n' >>"$fixture_root/corrupt-candidate/hq-v${version}-x86_64-unknown-linux-gnu.tar.gz"
if "$repository_root/scripts/prepare-rust-release-publication.sh" \
  "$fixture_root/corrupt-candidate" "$revision" "v$version" \
  "$fixture_root/corrupt-output" >/dev/null 2>&1; then
  printf 'release publication accepted a corrupt archive\n' >&2
  exit 1
fi

cp -R "$candidate_directory" "$fixture_root/extra-candidate"
touch "$fixture_root/extra-candidate/unexpected"
if "$repository_root/scripts/prepare-rust-release-publication.sh" \
  "$fixture_root/extra-candidate" "$revision" "v$version" \
  "$fixture_root/extra-output" >/dev/null 2>&1; then
  printf 'release publication accepted an unexpected candidate file\n' >&2
  exit 1
fi

printf 'Rust release publication tests passed.\n'
