#!/usr/bin/env bash
set -Eeuo pipefail

repository_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
fixture="$(mktemp -d)"
trap 'rm -rf -- "$fixture"' EXIT

export HQ_SOURCE_DIR="$fixture/source checkout"
export HQ_INSTALL_ROOT="$fixture/install root"
export HQ_RUST_TOOLCHAIN=custom-toolchain
export INSTALL_TEST_LOG="$fixture/commands.log"
mkdir -p "$fixture/bin" "$HQ_SOURCE_DIR/crates/hq-node"
: >"$HQ_SOURCE_DIR/crates/hq-node/Cargo.toml"
cat >"$fixture/bin/git" <<'EOF'
#!/usr/bin/env bash
printf 'test-commit\n'
EOF
cat >"$fixture/bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -Eeuo pipefail
printf '%s\n' "$HQ_BUILD_COMMIT" "$@" >"$INSTALL_TEST_LOG"
[[ "${INSTALL_TEST_FAIL:-false}" == false ]] || exit 42
mkdir -p "$HQ_INSTALL_ROOT/bin"
touch "$HQ_INSTALL_ROOT/bin/hq"
chmod +x "$HQ_INSTALL_ROOT/bin/hq"
EOF
chmod +x "$fixture/bin/git" "$fixture/bin/cargo"
export PATH="$fixture/bin:$PATH"

"$repository_root/scripts/cargo-install" >/dev/null
cat >"$fixture/expected" <<EOF
test-commit
+custom-toolchain
install
--force
--locked
--root
$HQ_INSTALL_ROOT
--path
$HQ_SOURCE_DIR/crates/hq-node
--bin
hq
EOF
diff -u "$fixture/expected" "$INSTALL_TEST_LOG"
[[ -x "$HQ_INSTALL_ROOT/bin/hq" ]]

if INSTALL_TEST_FAIL=true "$repository_root/scripts/cargo-install" >/dev/null 2>&1; then
  printf 'cargo failure unexpectedly succeeded\n' >&2
  exit 1
fi

rm "$INSTALL_TEST_LOG"
"$repository_root/scripts/cargo-install" --help >/dev/null
[[ ! -e "$INSTALL_TEST_LOG" ]]
if HQ_INSTALL_ROOT=relative "$repository_root/scripts/cargo-install" >/dev/null 2>&1; then
  printf 'relative installation root unexpectedly accepted\n' >&2
  exit 1
fi
[[ ! -e "$INSTALL_TEST_LOG" ]]

printf 'cargo-install test passed\n'
