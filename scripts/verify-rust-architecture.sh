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
  hq-projects
  hq-protocol
  hq-reducer
  hq-relay
  hq-resources
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

grep -Eq '^tungstenite(\.workspace)?[[:space:]]*=' \
  "$repository_root/crates/hq-relay/Cargo.toml" ||
  fail "hq-relay must own the bounded WebSocket adapter dependency"
for crate in "${expected_crates[@]}"; do
  if [[ "$crate" != hq-relay ]] && grep -Eq '^tungstenite(\.workspace)?[[:space:]]*=' \
    "$repository_root/crates/$crate/Cargo.toml"; then
    fail "$crate may not depend directly on tungstenite; WebSocket transport belongs to hq-relay"
  fi
done

allowed_internal_dependency() {
  case "$1:$2" in
    hq-reducer:hq-domain | \
      hq-protocol:hq-domain | \
      hq-application:hq-domain | hq-application:hq-reducer | \
      hq-store:hq-domain | hq-store:hq-reducer | hq-store:hq-protocol | hq-store:hq-harness | \
      hq-store:hq-application | \
      hq-local-api:hq-domain | hq-local-api:hq-protocol | hq-local-api:hq-application | \
      hq-relay:hq-domain | hq-relay:hq-protocol | hq-relay:hq-application | \
      hq-resources:hq-domain | \
      hq-harness:hq-domain | \
      hq-projects:hq-domain | hq-projects:hq-reducer | \
      hq-projects:hq-application | hq-projects:hq-harness | hq-projects:hq-resources | \
      hq-codex:hq-domain | hq-codex:hq-harness | hq-codex:hq-testkit | \
      hq-tui:hq-domain | hq-tui:hq-application | \
      hq-node:hq-domain | hq-node:hq-reducer | hq-node:hq-protocol | \
      hq-node:hq-application | hq-node:hq-store | hq-node:hq-local-api | \
      hq-node:hq-relay | hq-node:hq-harness | hq-node:hq-codex | \
      hq-node:hq-resources | hq-node:hq-projects | \
      hq-node:hq-tui | hq-node:hq-testkit | \
      hq-testkit:hq-domain | hq-testkit:hq-reducer | hq-testkit:hq-protocol | \
      hq-testkit:hq-application | hq-testkit:hq-harness)
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
grep -Eq '^hq-codex(\.workspace)?[[:space:]]*=' "$repository_root/crates/hq-node/Cargo.toml" ||
  fail "hq-node must own the concrete Codex adapter dependency"
grep -Fq 'compose_codex_registry(' "$repository_root/crates/hq-node/src/foreground.rs" ||
  fail "foreground composition must register the concrete Codex adapter"

if grep -ERq --include='*.rs' 'hq_testkit' \
  "$repository_root/crates/hq-codex/src/adapter.rs" \
  "$repository_root/crates/hq-codex/src/process.rs" \
  "$repository_root/crates/hq-codex/src/protocol.rs" \
  "$repository_root/crates/hq-codex/src/transport.rs"; then
  fail "hq-codex production sources may not depend on hq-testkit"
fi

if grep -ERiq --include='Cargo.toml' --include='*.rs' \
  '(^|[^a-z])(codex|claude|anthropic|openai)([^a-z]|$)' \
  "$repository_root/crates/hq-harness"; then
  fail "hq-harness contains provider-specific vocabulary"
fi

if grep -Eiq '^[[:space:]]*(serde|serde_json|tokio)(\.workspace)?[[:space:]]*=' \
  "$repository_root/crates/hq-harness/Cargo.toml"; then
  fail "hq-harness may not depend on serialization or asynchronous runtime crates"
fi

if grep -ERq --include='*.rs' \
  '(serde(::|_)|tokio::|std::fs|std::process|derive\([^)]*(Serialize|Deserialize))' \
  "$repository_root/crates/hq-harness/src"; then
  fail "hq-harness contains serialization, runtime, filesystem, or process API use"
fi

[[ -f "$repository_root/docs/harness-contract-v1.md" ]] ||
  fail "missing provider-neutral harness contract"
[[ -f "$repository_root/docs/harness-supervisor-v1.md" ]] ||
  fail "missing provider-neutral harness supervisor contract"
[[ -f "$repository_root/docs/testing/conformance-v1.md" ]] ||
  fail "missing reusable harness conformance contract"
grep -Fq 'pub const ALL: [Self; 14]' "$repository_root/crates/hq-testkit/src/harness.rs" ||
  fail "harness conformance suite must expose its complete deterministic scenario inventory"

for crate in "${expected_crates[@]}"; do
  if [[ "$crate" != hq-store ]] && grep -Eq '^rusqlite(\.workspace)?[[:space:]]*=' \
    "$repository_root/crates/$crate/Cargo.toml"; then
    fail "$crate may not depend directly on rusqlite; SQLite belongs to hq-store"
  fi
done

while IFS= read -r rust_source; do
  if grep -Eq '(^|[^a-z])rusqlite::' "$rust_source" &&
    [[ "$rust_source" != "$repository_root/crates/hq-store/src/database.rs" ]] &&
    [[ "$rust_source" != "$repository_root/crates/hq-store/src/database/"*.rs ]]; then
    fail "rusqlite production API use escaped hq-store's private database module: $rust_source"
  fi
done < <(find "$repository_root"/crates/hq-*/src -type f -name '*.rs' | sort)

for core_crate in hq-domain hq-reducer hq-application; do
  if grep -ERiq --include='Cargo.toml' --include='*.rs' \
    '(tokio|rusqlite|sqlite|nostr|ratatui|std::fs|std::process|codex|claude|anthropic|openai)' \
    "$repository_root/crates/$core_crate"; then
    fail "$core_crate contains a forbidden runtime, adapter, filesystem, process, or provider-specific reference"
  fi
done

if grep -ERq --include='*.rs' '(hq_store|StoredRelay|rusqlite)' \
  "$repository_root/crates/hq-relay/src"; then
  fail "hq-relay contains storage-adapter vocabulary"
fi

if grep -ERq --include='*.rs' '(hq_relay|RelayPortError|DurableEnvelope)' \
  "$repository_root/crates/hq-store/src"; then
  fail "hq-store contains relay-consumer vocabulary"
fi

grep -Fq 'impl RelayStatePort for RelayStoreAdapter' \
  "$repository_root/crates/hq-node/src/relay_store.rs" ||
  fail "hq-node must own the relay/store record mapping"

grep -Fq 'impl HarnessStatePort for HarnessStoreAdapter' \
  "$repository_root/crates/hq-node/src/harness_store.rs" ||
  fail "hq-node must own the harness/store record mapping"
grep -Fq 'impl<P: CommitFacts + Send + Sync> HarnessPersistencePort' \
  "$repository_root/crates/hq-node/src/harness_persistence.rs" ||
  fail "hq-node must own canonical normalized harness persistence"
grep -Fq 'plan_harness_output(' "$repository_root/crates/hq-application/src/harness.rs" ||
  fail "hq-application must own pure normalized output fact planning"
grep -Fq 'plan_harness_activity(' "$repository_root/crates/hq-application/src/harness.rs" ||
  fail "hq-application must own pure normalized activity fact planning"

grep -Fq 'impl ProjectSagaStore for ProjectSagaStoreAdapter' \
  "$repository_root/crates/hq-node/src/project_store.rs" ||
  fail "hq-node must own the project-workflow/store record mapping"

if grep -ERq --include='*.rs' '(hq_store|StoredProjectSaga|rusqlite)' \
  "$repository_root/crates/hq-projects/src"; then
  fail "hq-projects contains storage-adapter vocabulary"
fi

grep -Eq '^hq-domain(\.workspace)?[[:space:]]*=' \
  "$repository_root/crates/hq-resources/Cargo.toml" ||
  fail "hq-resources must depend inward on hq-domain"
[[ -f "$repository_root/docs/path-resources-v1.md" ]] ||
  fail "missing path-resource identity and observation contract"

grep -Fq 'impl NodeComponent for RelayNodeComponent' \
  "$repository_root/crates/hq-node/src/relay_component.rs" ||
  fail "hq-node must own the concrete relay component lifecycle"
grep -Fq 'impl NodeComponent for HarnessNodeComponent' \
  "$repository_root/crates/hq-node/src/harness_component.rs" ||
  fail "hq-node must own the concrete harness component lifecycle"
grep -Fq '.name("hq-harness-events".to_owned())' \
  "$repository_root/crates/hq-node/src/harness_component.rs" ||
  fail "the concrete harness component must own its joined event polling task"
grep -Fq '.poll_events()' "$repository_root/crates/hq-node/src/harness_component.rs" ||
  fail "the concrete harness event task must drive the neutral supervisor poll boundary"
grep -Fq 'foundation.compose_relay(' "$repository_root/crates/hq-node/src/foreground.rs" ||
  fail "foreground composition must construct the concrete relay through foundation ownership"

binary_manifests=()
while IFS= read -r manifest; do
  binary_manifests+=("$manifest")
done < <(grep -El '^name[[:space:]]*=[[:space:]]*"hq"[[:space:]]*$' \
  "$repository_root"/crates/hq-*/Cargo.toml || true)

[[ "${#binary_manifests[@]}" -eq 1 ]] || fail "expected exactly one hq binary declaration"
[[ "${binary_manifests[0]}" == "$repository_root/crates/hq-node/Cargo.toml" ]] ||
  fail "the hq binary must be owned by hq-node"

if grep -Eq '(hq_store|hq_relay|hq_codex|hq_resources|rusqlite|std::fs)' \
  "$repository_root/crates/hq-node/src/cli.rs" \
  "$repository_root/crates/hq-node/src/local_client.rs"; then
  fail "the CLI/local client may cross only node coordination and hq-local-api boundaries"
fi
if grep -Eq '(StoreGateway|Bip340Signer|FactMutation|SemanticPayload::(AgentNameClaimed|ProviderSessionSelected|ProviderSessionRenamed))' \
  "$repository_root/crates/hq-node/src/cli.rs" \
  "$repository_root/crates/hq-node/src/local_client.rs"; then
  fail "named-agent clients must use pure application planners and local API mutation DTOs"
fi
grep -Fq 'fn run_named_agent(' "$repository_root/crates/hq-node/src/cli.rs" ||
  fail "the installed CLI must expose named-agent catalog workflows"
grep -Fq 'client.agent_retirement(' "$repository_root/crates/hq-node/src/cli.rs" ||
  fail "named-agent retirement must cross the typed local API client"
grep -Fq 'fn run_harness(' "$repository_root/crates/hq-node/src/cli.rs" ||
  fail "the installed CLI must expose managed harness workflows"
grep -Fq 'client.agent_session(' "$repository_root/crates/hq-node/src/cli.rs" ||
  fail "managed harness clients must cross the typed local API client"
grep -Fq 'fn run_project_catalog(' "$repository_root/crates/hq-node/src/cli.rs" ||
  fail "the installed CLI must expose the authoritative project catalog"
grep -Fq 'project_catalog_view(&snapshot, action)' "$repository_root/crates/hq-node/src/cli.rs" ||
  fail "project catalog clients must derive only from a fresh local API snapshot"
if grep -Eq '(CodexHarness|CodexSession|CodexProcess|CodexProtocol)' \
  "$repository_root/crates/hq-node/src/cli.rs" \
  "$repository_root/crates/hq-node/src/local_client.rs"; then
  fail "managed harness clients must remain provider neutral"
fi
if grep -Eq '(ProjectWorkflowManager|CanonicalProjectPort|retire_idle_agent)' \
  "$repository_root/crates/hq-node/src/cli.rs" \
  "$repository_root/crates/hq-node/src/local_client.rs"; then
  fail "named-agent clients must not invoke project or canonical retirement adapters directly"
fi
grep -Fq 'MutationRequest::from_plan' "$repository_root/crates/hq-node/src/cli.rs" ||
  fail "canonical CLI administration must cross the local mutation boundary"
grep -Fq 'hq_node::execute_cli' "$repository_root/crates/hq-node/src/bin/hq.rs" ||
  fail "the installed binary must delegate to the typed CLI composition root"

printf 'Rust workspace architecture verified.\n'
