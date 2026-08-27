#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ledger="$repository_root/docs/rust/behavior-ledger.md"
platform_adr="$repository_root/docs/adr/0001-rust-platform-and-packaging.md"
identity_adr="$repository_root/docs/adr/0002-rust-identity-backup-boundary.md"
workflow_adr="$repository_root/docs/adr/0003-rust-client-and-provider-workflows.md"

fail() {
  printf 'behavior ledger verification failed: %s\n' "$1" >&2
  exit 1
}

for required_file in "$ledger" "$platform_adr" "$identity_adr" "$workflow_adr"; do
  [[ -f "$required_file" ]] || fail "missing ${required_file#"$repository_root"/}"
done

expected_commit=a2684b21de1d11c2fa0aad2ea3fd83b6c836fe82
expected_tree=4f18888fc6dc2f82cc315b0b0986a153850a3d01
actual_tree=$(git -C "$repository_root" show -s --format=%T "$expected_commit")
[[ "$actual_tree" == "$expected_tree" ]] || fail "frozen Go tree does not match the recorded commit"
grep -Fq "$expected_commit" "$ledger" || fail "frozen Go commit is not recorded"
grep -Fq "$expected_tree" "$ledger" || fail "frozen Go tree is not recorded"

for source in \
  rust-rewrite-design.md \
  crdt-algebra-laws.html \
  rust-port.md \
  rust-port-transcript.md \
  README.md \
  docs/design.md \
  docs/events.md \
  docs/nostr.md \
  docs/lan.md \
  docs/harnesses.md \
  docs/projects.md \
  internal/cli/app.go \
  internal/agenthelp; do
  grep -Fq "$source" "$ledger" || fail "source coverage marker is absent for $source"
done

for regression in \
  REG-AUTHORITY-MAXIMAL-REGRANT \
  REG-CONVERSATION-COMPARATOR \
  REG-INDEXED-PAGINATION \
  REG-NONDISRUPTIVE-RELAY-WAKE; do
  grep -Fq "$regression" "$ledger" || fail "missing inherited regression $regression"
done

awk -F '|' '
  function trim(value) {
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
    return value
  }
  /^\| [A-Z]+-[0-9][0-9][0-9] / {
    id = trim($2)
    classification = trim($3)
    release = trim($4)
    capability = trim($5)
    contract = trim($6)
    owner = trim($7)

    if (seen[id]++) {
      printf "duplicate behavior ID %s\n", id > "/dev/stderr"
      invalid = 1
    }
    if (classification !~ /^(retain|redesign|drop)$/) {
      printf "invalid classification for %s: %s\n", id, classification > "/dev/stderr"
      invalid = 1
    }
    if (release !~ /^(required|deferred|excluded)$/) {
      printf "invalid release disposition for %s: %s\n", id, release > "/dev/stderr"
      invalid = 1
    }
    if (classification == "drop" && release != "excluded") {
      printf "dropped behavior %s must be excluded\n", id > "/dev/stderr"
      invalid = 1
    }
    if (classification != "drop" && release == "excluded") {
      printf "non-dropped behavior %s cannot be excluded\n", id > "/dev/stderr"
      invalid = 1
    }
    if (capability == "" || contract == "" || owner == "") {
      printf "behavior %s has an empty required field\n", id > "/dev/stderr"
      invalid = 1
    }
    count++
  }
  END {
    if (count < 70) {
      printf "expected at least 70 classified behaviors, found %d\n", count > "/dev/stderr"
      invalid = 1
    }
    exit invalid
  }
' "$ledger" || fail "behavior table is incomplete or invalid"

for adr in "$platform_adr" "$identity_adr" "$workflow_adr"; do
  grep -Eq '^Status: Accepted$' "$adr" || fail "ADR is not accepted: ${adr#"$repository_root"/}"
done

for reference in \
  docs/adr/0001-rust-platform-and-packaging.md \
  docs/adr/0002-rust-identity-backup-boundary.md \
  docs/adr/0003-rust-client-and-provider-workflows.md; do
  grep -Fq "$reference" "$ledger" || fail "ledger does not reference $reference"
done

if grep -En '(TBD|TODO|unclassified|undecided)' "$ledger" "$platform_adr" "$identity_adr" "$workflow_adr"; then
  fail "product-boundary documents contain an unresolved marker"
fi

printf 'Rust behavior ledger verified.\n'
