#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
algebra="$repository_root/docs/rust/causal-algebra.md"
catalog="$repository_root/docs/rust/semantic-fact-catalog.md"
scenarios="$repository_root/docs/rust/acceptance-scenarios.md"

fail() {
  printf 'causal specification verification failed: %s\n' "$1" >&2
  exit 1
}

for required_file in "$algebra" "$catalog" "$scenarios"; do
  [[ -f "$required_file" ]] || fail "missing ${required_file#"$repository_root"/}"
done

for law in \
  LAW-MERGE-SET-UNION \
  LAW-INPUT-INVARIANCE \
  LAW-INCREMENTAL-BATCH-EQUALITY \
  LAW-CAUSAL-DOMINANCE \
  LAW-EXACT-MAXIMAL-FRONTIERS \
  LAW-DEFERRED-READINESS \
  LAW-HISTORICAL-AUTHORITY \
  LAW-PROJECTION-RETRACTION \
  LAW-DETERMINISTIC-CONFLICTS; do
  grep -Fq "$law" "$algebra" || fail "missing algebra law $law"
  grep -Fq "$law" "$scenarios" || fail "missing acceptance scenario for $law"
done

for concept in \
  'Structural reachability' \
  'Usable reachability' \
  'Required causal dependencies' \
  'Fact decisions' \
  'Complete-batch reduction' \
  'Projection support and retraction' \
  'Canonical presentation comparator' \
  'Normalized reduction report'; do
  grep -Fq "$concept" "$algebra" || fail "algebra does not define $concept"
done

awk -F '|' '
  function trim(value) {
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
    return value
  }
  /^\| FCT-[0-9][0-9][0-9] / {
    id = trim($2)
    name = trim($3)
    protocol = trim($4)
    scope = trim($5)
    parents = trim($6)
    authorities = trim($7)
    validation = trim($8)
    unresolved = trim($9)
    conflict = trim($10)
    projection = trim($11)
    retention = trim($12)
    observations = trim($13)

    if (seen[id]++) {
      printf "duplicate fact ID %s\n", id > "/dev/stderr"
      invalid = 1
    }
    if (protocol !~ /^(canonical|remote-control)$/) {
      printf "invalid protocol class for %s: %s\n", id, protocol > "/dev/stderr"
      invalid = 1
    }
    if (retention !~ /^(canonical-permanent|canonical-compacted-view|control-permanent)$/) {
      printf "invalid retention class for %s: %s\n", id, retention > "/dev/stderr"
      invalid = 1
    }
    if (name == "" || scope == "" || parents == "" || authorities == "" || validation == "" || unresolved == "" || conflict == "" || projection == "" || observations == "") {
      printf "fact %s has an empty catalog field\n", id > "/dev/stderr"
      invalid = 1
    }
    count++
  }
  END {
    if (count < 48) {
      printf "expected at least 48 semantic facts, found %d\n", count > "/dev/stderr"
      invalid = 1
    }
    exit invalid
  }
' "$catalog" || fail "semantic fact catalog is incomplete or invalid"

for fact_family in \
  InstallationDeclared \
  MailboxCreated \
  PeerRouteSet \
  MailboxAccessGranted \
  MailboxAccessRevoked \
  MailboxActionObserved \
  HumanAccountCreated \
  HumanDeviceGranted \
  HumanDeviceAccepted \
  HumanDeviceRevoked \
  QuestionAsked \
  AsynchronousMessageSent \
  AnswerGiven \
  ThreadCancelled \
  MessageArchived \
  MessageRestored \
  MessageRejected \
  HarnessActivityRecorded \
  AgentNameClaimed \
  AgentRetired \
  ProviderSessionSelected \
  ProjectCreated \
  ProjectAssignmentRunnable \
  ProjectInputAccepted \
  ProjectInputDispatched \
  ProjectOutputRecorded \
  RemoteProjectCommandRequested \
  RemoteProjectCommandReceipt \
  RemoteProjectCommandOutcome; do
  grep -Fq "$fact_family" "$catalog" || fail "missing semantic fact family $fact_family"
done

awk -F '|' '
  function trim(value) {
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
    return value
  }
  /^\| [A-Z]+-[0-9][0-9][0-9] / {
    id = trim($2)
    name = trim($3)
    given = trim($4)
    action = trim($5)
    expected = trim($6)
    evidence = trim($7)
    if (seen[id]++) {
      printf "duplicate scenario ID %s\n", id > "/dev/stderr"
      invalid = 1
    }
    if (name == "" || given == "" || action == "" || expected == "" || evidence == "") {
      printf "scenario %s has an empty field\n", id > "/dev/stderr"
      invalid = 1
    }
    count++
  }
  END {
    if (count < 50) {
      printf "expected at least 50 acceptance scenarios, found %d\n", count > "/dev/stderr"
      invalid = 1
    }
    exit invalid
  }
' "$scenarios" || fail "acceptance scenario catalog is incomplete or invalid"

for regression in \
  REG-AUTHORITY-MAXIMAL-REGRANT \
  REG-CONVERSATION-COMPARATOR \
  REG-INDEXED-PAGINATION \
  REG-NONDISRUPTIVE-RELAY-WAKE; do
  grep -Fq "$regression" "$scenarios" || fail "missing inherited regression scenario $regression"
done

for attack in \
  unrelated-parent-authority-attack \
  concurrent-revoke-action \
  conflicting-unique-root \
  project-linear-fork \
  cross-project-resource-conflict \
  same-id-different-input; do
  grep -Fq "$attack" "$scenarios" || fail "missing attack or safety scenario $attack"
done

for reference in \
  docs/rust/causal-algebra.md \
  docs/rust/semantic-fact-catalog.md \
  docs/rust/acceptance-scenarios.md \
  docs/rust/behavior-ledger.md; do
  grep -Fq "$reference" "$algebra" "$catalog" "$scenarios" || fail "missing cross-document reference $reference"
done

if grep -En '(TBD|TODO|unclassified|undecided|implementation-defined conflict)' "$algebra" "$catalog" "$scenarios"; then
  fail "causal specifications contain an unresolved marker"
fi

printf 'Rust causal algebra, fact catalog, and acceptance scenarios verified.\n'
