#!/usr/bin/env bash
set -Eeuo pipefail

repository_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
fixture="$(mktemp -d)"
trap 'rm -rf -- "$fixture"' EXIT

home="$fixture/home"
source_root="$fixture/source"
fake_bin="$fixture/fake-bin"
log="$fixture/commands.log"
mkdir -p "$home" "$source_root/crates/hq-node" "$fake_bin"
: >"$source_root/crates/hq-node/Cargo.toml"
: >"$log"

make_old_hq() {
  local path=$1
  mkdir -p "$(dirname -- "$path")"
  cat >"$path" <<'EOF'
#!/usr/bin/env bash
printf 'hq %s\n' "$*" >>"$HQ_BOOTSTRAP_TEST_LOG"
EOF
  chmod +x "$path"
}

cat >"$fake_bin/git" <<'EOF'
#!/usr/bin/env bash
case " $* " in
  *" rev-parse --show-toplevel "*)
    printf '%s\n' "$HQ_SOURCE_DIR"
    ;;
  *)
    printf '%040d\n' 0
    ;;
esac
EOF
chmod +x "$fake_bin/git"

cat >"$fake_bin/cargo" <<'EOF'
#!/usr/bin/env bash
printf 'cargo %s\n' "$*" >>"$HQ_BOOTSTRAP_TEST_LOG"
case " $* " in
  *" install "*)
    [[ ! -e "$HQ_INSTALL_ROOT/bin/hq" ]] || exit 90
    mkdir -p "$HQ_INSTALL_ROOT/bin"
    cat >"$HQ_INSTALL_ROOT/bin/hq" <<'INNER'
#!/usr/bin/env bash
printf 'new-hq %s\n' "$*" >>"$HQ_BOOTSTRAP_TEST_LOG"
while (($#)); do
  if [[ "$1" == --state-root ]]; then
    state_root=$2
    shift 2
    continue
  fi
  if [[ "$1" == identity && "${2:-}" == init ]]; then
    mkdir -p "$state_root"
    : >"$state_root/new-installation"
  fi
  shift
done
INNER
    chmod +x "$HQ_INSTALL_ROOT/bin/hq"
    ;;
esac
EOF
chmod +x "$fake_bin/cargo"

cat >"$fake_bin/systemctl" <<'EOF'
#!/usr/bin/env bash
printf 'systemctl %s\n' "$*" >>"$HQ_BOOTSTRAP_TEST_LOG"
EOF
chmod +x "$fake_bin/systemctl"

cat >"$fake_bin/uname" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "${HQ_BOOTSTRAP_TEST_UNAME:-Linux}"
EOF
chmod +x "$fake_bin/uname"

cat >"$fake_bin/launchctl" <<'EOF'
#!/usr/bin/env bash
printf 'launchctl %s\n' "$*" >>"$HQ_BOOTSTRAP_TEST_LOG"
EOF
chmod +x "$fake_bin/launchctl"

make_old_hq "$fake_bin/hq"
make_old_hq "$home/.local/bin/hq"
make_old_hq "$home/.cargo/bin/hq"
make_old_hq "$fixture/install/bin/hq"

for state_root in \
  "$fixture/xdg-state/hq" \
  "$home/.local/state/hq" \
  "$fixture/selected-state"; do
  mkdir -p "$state_root"
  : >"$state_root/old-installation"
done
mkdir -p "$home/.config/systemd/user"
: >"$home/.config/systemd/user/hq-daemon.service"

HOME="$home" \
PATH="$fake_bin:/usr/bin:/bin" \
XDG_STATE_HOME="$fixture/xdg-state" \
CARGO_HOME="$home/.cargo" \
HQ_SOURCE_DIR="$source_root" \
HQ_INSTALL_ROOT="$fixture/install" \
HQ_STATE_ROOT="$fixture/selected-state" \
HQ_BOOTSTRAP_TEST_LOG="$log" \
  "$repository_root/scripts/hq-bootstrap" tester >/dev/null

grep -Fq 'systemctl --user disable --now hq-daemon.service' "$log"
for state_root in \
  "$fixture/xdg-state/hq" \
  "$home/.local/state/hq" \
  "$fixture/selected-state"; do
  grep -Fq -- "--state-root $state_root daemon stop" "$log"
  [[ ! -e "$state_root/old-installation" ]]
done

[[ ! -e "$home/.config/systemd/user/hq-daemon.service" ]]
[[ ! -e "$home/.local/bin/hq" ]]
[[ ! -e "$home/.cargo/bin/hq" ]]
[[ -x "$fixture/install/bin/hq" ]]
[[ -e "$fixture/selected-state/new-installation" ]]
if grep -Fq 'agent create alice' "$log" || grep -Fq 'project create source' "$log"; then
  printf 'default bootstrap unexpectedly created fixtures\n' >&2
  exit 1
fi

mkdir -p "$home/Library/LaunchAgents"
: >"$home/Library/LaunchAgents/com.wbbradley.hq.daemon.plist"
HOME="$home" \
PATH="$fake_bin:/usr/bin:/bin" \
XDG_STATE_HOME="$fixture/xdg-state" \
CARGO_HOME="$home/.cargo" \
HQ_SOURCE_DIR="$source_root" \
HQ_INSTALL_ROOT="$fixture/install" \
HQ_STATE_ROOT="$fixture/selected-state" \
HQ_BOOTSTRAP_TEST_LOG="$log" \
HQ_BOOTSTRAP_TEST_UNAME=Darwin \
  "$repository_root/scripts/hq-bootstrap" --fixtures tester >/dev/null

grep -Fq 'launchctl bootout gui/' "$log"
grep -Fq '/com.wbbradley.hq.daemon' "$log"
grep -Fq 'new-hq --state-root '"$fixture/selected-state"' agent create alice' "$log"
grep -Fq 'new-hq --state-root '"$fixture/selected-state"' project create source --path '"$source_root" "$log"
[[ ! -e "$home/Library/LaunchAgents/com.wbbradley.hq.daemon.plist" ]]

printf 'hq-bootstrap teardown test passed\n'
