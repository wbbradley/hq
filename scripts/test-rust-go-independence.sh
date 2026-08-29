#!/usr/bin/env bash

set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
verifier="$repository_root/scripts/verify-rust-go-independence.sh"
fixture=$(mktemp -d "${TMPDIR:-/tmp}/hq-rust-go-independence.XXXXXX")

cleanup() {
  rm -rf "$fixture"
}
trap cleanup EXIT

mkdir -p \
  "$fixture/crates/hq-node/src/identity" \
  "$fixture/.github/workflows" \
  "$fixture/deploy" \
  "$fixture/scripts"
touch "$fixture/Cargo.toml" "$fixture/Cargo.lock" "$fixture/crates/hq-node/Cargo.toml"
cat >"$fixture/crates/hq-node/src/identity/paths.rs" <<'EOF'
root.join("identity.v1");
root.join("local-config.v1.json");
root.join("hq.sqlite3");
root.join("node.lock");
EOF
cat >"$fixture/crates/hq-node/src/main.rs" <<'EOF'
fn main() {}
EOF
cat >"$fixture/.github/workflows/release.yml" <<'EOF'
- run: cargo build --locked --release -p hq-node --bin hq
EOF
cat >"$fixture/deploy/hq.service" <<'EOF'
ExecStart=/usr/local/bin/hq daemon run
EOF
cat >"$fixture/scripts/package-rust-release.sh" <<'EOF'
tar -czf "$archive" -C "$stage_directory" hq
EOF

"$verifier" --scan-fixture "$fixture" >/dev/null

cat >"$fixture/crates/hq-node/src/main.rs" <<'EOF'
fn main() {
    std::process::Command::new("go");
}
EOF
if "$verifier" --scan-fixture "$fixture" >/dev/null 2>&1; then
  printf 'Go-independence verifier accepted a production Go command\n' >&2
  exit 1
fi

cat >"$fixture/crates/hq-node/src/main.rs" <<'EOF'
fn main() {}
EOF
cat >"$fixture/scripts/package-rust-release.sh" <<'EOF'
tar -czf "$archive" -C "$stage_directory" hq go/bin/hq
EOF
if "$verifier" --scan-fixture "$fixture" >/dev/null 2>&1; then
  printf 'Go-independence verifier accepted Go in the release package\n' >&2
  exit 1
fi

printf 'Rust Go-independence verifier tests passed.\n'
