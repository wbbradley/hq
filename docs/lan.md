# HQ node service management

HQ installs one executable. Client commands auto-start a node when needed, so a service manager is
optional. A foreground service is useful for continuous relay subscriptions and prompt remote
delivery.

Use the exact absolute path of the verified candidate. Do not use `/usr/bin/env hq`, a mutable
selection symlink, or a `PATH` search for the HQ executable in a service definition. The service
may retain a narrow `PATH` so HQ can launch the pinned provider binary.

Before installing either service, verify that the intended state root has no owner:

```sh
/absolute/path/to/hq daemon status
```

If more than one HQ daemon is visible, inspect full process arguments. Different explicit state
roots are independent installations. Stop only the intended installation with its exact binary and
`--state-root` option; never kill every process named `hq`.

## systemd user service

Copy the template and replace `REPLACE_WITH_ABSOLUTE_HQ_PATH` with the immutable installed path:

```sh
install -d "$HOME/.config/systemd/user"
cp deploy/systemd/hq-daemon.service "$HOME/.config/systemd/user/hq-daemon.service"
sed -i "s|REPLACE_WITH_ABSOLUTE_HQ_PATH|$HOME/.local/bin/hq|g" \
  "$HOME/.config/systemd/user/hq-daemon.service"
systemd-analyze --user verify "$HOME/.config/systemd/user/hq-daemon.service"
systemctl --user daemon-reload
```

The preceding commands only install and verify the definition. Starting or enabling it is a
separate operator action:

```sh
systemctl --user start hq-daemon.service
systemctl --user enable hq-daemon.service
systemctl --user status hq-daemon.service
```

For an explicit state root, add `--state-root /absolute/private/path` before `daemon run` in
`ExecStart`. Create that directory as the service user with mode `0700`. To replace an executable,
stop the unit, install and verify the new absolute path, update `ExecStart`, reload, and then start.

## launchd user agent

Copy the template, replace all three markers, and validate it:

```sh
install -d "$HOME/Library/LaunchAgents" "$HOME/Library/Logs/HQ"
chmod 700 "$HOME/Library/Logs/HQ"
cp deploy/launchd/com.wbbradley.hq.daemon.plist \
  "$HOME/Library/LaunchAgents/com.wbbradley.hq.daemon.plist"
sed -i '' "s|REPLACE_WITH_ABSOLUTE_HQ_PATH|$HOME/.local/bin/hq|g" \
  "$HOME/Library/LaunchAgents/com.wbbradley.hq.daemon.plist"
sed -i '' "s|REPLACE_WITH_ABSOLUTE_LOG_DIRECTORY|$HOME/Library/Logs/HQ|g" \
  "$HOME/Library/LaunchAgents/com.wbbradley.hq.daemon.plist"
sed -i '' "s|REPLACE_WITH_USER|$(id -un)|g" \
  "$HOME/Library/LaunchAgents/com.wbbradley.hq.daemon.plist"
plutil -lint "$HOME/Library/LaunchAgents/com.wbbradley.hq.daemon.plist"
```

Loading it is a separate operator action:

```sh
launchctl bootstrap "gui/$(id -u)" \
  "$HOME/Library/LaunchAgents/com.wbbradley.hq.daemon.plist"
launchctl print "gui/$(id -u)/com.wbbradley.hq.daemon"
```

For an explicit state root, insert `--state-root` and the absolute path in `ProgramArguments`
before `daemon`. To replace an executable, boot out the exact label, install and verify the new
path, update and validate the plist, and bootstrap it only after operator authorization.

## Logs and provider environment

systemd writes foreground output to the user journal. launchd writes both streams to the protected
file configured in the template. HQ and provider diagnostics are redacted, but logs remain private
operational data and should not be included in identity backups.

Both templates set `PATH` to `$HOME/.local/bin`, `/usr/local/bin`, `/usr/bin`, and `/bin`. Adjust it
to the verified location of Codex CLI `0.150.1` if necessary. Never add a Go binary directory or an
archived installation to the service environment.

Service installation alone does not authorize a production start. Follow
[rust/cutover.md](rust/cutover.md) for soak, cutover, and rollback approvals.
