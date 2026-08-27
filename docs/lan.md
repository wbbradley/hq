# Two-machine LAN setup

This setup runs one HQ installation and one local HQ node on each machine. Both nodes connect as clients to one retained Nostr relay. They do not listen for network peers and do not trust hostnames, IP addresses, or ports.

Each machine keeps a separate root key, installation ID, SQLite database, agent mailboxes, and local delivery leases. A signed human-account grant makes both installations devices of one human account. Each device then projects the same human inbox while agent mailboxes stay local to their harness sessions.

## Run the retained relay

HQ tests against [rnostr](https://github.com/rnostr/rnostr) v0.4.9. The checked-in Compose file pins image digest `sha256:c022e4384f8fe1eb6023d497fe0c5cf9cd13d239f62160713546de4522f69214`. rnostr stores events in LMDB and implements the NIP-01 publish and query frames, `EOSE`, `OK`, and NIP-42 that HQ needs.

Run `hq identity show` on both machines. Copy both 64-character public keys, not the private backups, to the relay host. Then prepare the relay:

```sh
cp deploy/rnostr/rnostr.toml.example deploy/rnostr/rnostr.toml
# Replace both REPLACE_WITH_* values with the two public keys.
docker compose -f deploy/rnostr/compose.yaml up -d
```

The sample binds TCP port 7447 on every relay-host interface and keeps data under `deploy/rnostr/data`. Restrict that port to the LAN with the host firewall. Use `wss://` and a TLS proxy if traffic crosses an untrusted network. A plain `ws://` link still encrypts HQ bodies with NIP-44, but it exposes traffic data and does not authenticate the relay host.

The `[auth.req]` and `[auth.event]` lists contain installation root public keys used for NIP-42 client auth. Do not use `event_pubkey_whitelist`: NIP-59 kind-1059 wrappers use a new author key for every event.

## Pair the machines

Add the same URL on both machines:

```sh
hq relay add ws://relay.lan:7447
```

On the second machine, run `hq identity show`. On the account creator, use that installation ID and `npub` to write a signed invitation:

```sh
hq human invite --name desktop --relay ws://relay.lan:7447 INSTALLATION_ID NPUB > desktop.hq-invite.json
```

Copy the invitation to the second machine and join:

```sh
hq human join desktop.hq-invite.json
hq sync
hq human devices
```

Run `hq sync` once on the creator, then once more on the added machine. Both `hq human devices` outputs should list both installations as active. Each machine can now use `hq` to view and answer the shared human inbox.

## Keep each node running

Every local command uses the node, and the client auto-starts one owner when necessary. A service manager is still recommended: it keeps retained-relay subscriptions warm, sends queued events promptly, and reconnects after network or relay loss without waiting for the next command.

For a systemd user service:

```sh
mkdir -p ~/.config/systemd/user
cp deploy/systemd/hq-daemon.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now hq-daemon.service
hq daemon status
```

The unit searches `~/go/bin`, `~/.local/bin`, `/usr/local/bin`, and the system path. Edit its `PATH` if `go env GOBIN` places HQ elsewhere. Enable user lingering if the daemon must run while the user has no login session.

For launchd, copy `deploy/launchd/com.wbbradley.hq.daemon.plist` to `~/Library/LaunchAgents`, replace the two absolute-path markers, and load it:

```sh
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.wbbradley.hq.daemon.plist
hq daemon status
```

Use one node per database. `hq daemon status` shows whether lifecycle RPC responds; `hq daemon restart` replaces the node instance and connected clients reconnect/resubscribe. `hq status` uses domain RPC to show account members, pending device fanout, relay-accepted sends, last receive time, invalid account traffic, and revoked-device traffic. Short database paths use a sibling mode-`0600` Unix socket; long paths use a stable hash under the user runtime directory. Windows named-pipe transport is not available yet.

## Rust transport interoperability smoke

The Rust transport has a separate ignored, explicitly opted-in smoke. Its wrapper starts the pinned
rnostr digest with two fixed test-only keys in temporary state, then proves bounded WS framing,
NIP-42 for both clients, exact kind-1059 publish acknowledgement, retained `REQ`/`EVENT`/`EOSE`,
exact canonical opening, reconnect, and repeated catch-up:

```sh
HQ_RUN_CONTROLLED_RELAY_SMOKE=1 ./scripts/rust-relay-smoke.sh
```

Docker and an available daemon are preflight requirements; absence exits with an actionable error
and is not a merge-gate failure. The default port is 17448. `HQ_CONTROLLED_RELAY_PORT` changes the
temporary container port, `HQ_CONTROLLED_RELAY_KEEP=1` retains its printed state directory, and a
pre-existing controlled `ws://` or `wss://` endpoint can be tested by setting
`HQ_CONTROLLED_RELAY_URL` and invoking the ignored Cargo test directly. Never put production keys in
this test.

## Legacy end-to-end smoke test

The older full-product smoke builds the Go client while the Rust CLI is incomplete. It creates three
temporary state directories, starts the pinned rnostr container with two allowed installations,
pairs two human devices, sends asynchronous transport probes and cross-machine answers, restarts the
relay, catches up an offline device, checks duplicate suppression and auth failure, and revokes the
second device.

```sh
HQ_RUN_REAL_RELAY_SMOKE=1 ./scripts/lan-smoke.sh
```

The test uses local port 17447 and removes its container and temporary files on exit. Set `HQ_SMOKE_PORT` to choose another port. Set `HQ_SMOKE_KEEP=1` to retain the printed temporary directory for inspection, then remove that directory when finished.

## Manual two-machine check

- Run `hq identity show` on both machines and confirm the installation IDs and public keys differ.
- Configure both public keys in one retained relay and add the same relay URL to both HQ databases.
- Create the invitation on the account creator, join on the other machine, sync both, and confirm both devices are active.
- Start several Codex, Claude Code, or Pi sessions on each host. Use `hq ask` when each session should block for its answer; use `hq send` when testing delivery asynchronously.
- Open both TUIs. Confirm both TUIs show every question with the source device, installation, repository, worktree, and branch.
- Answer one question from the source host and one from the other host. Confirm each waiting agent receives the right answer.
- Stop one HQ node, then run `hq list` and confirm the client auto-starts exactly one replacement owner.
- Keep a TUI open while running `hq daemon restart`; confirm it reports reconnect state briefly, reloads a full snapshot, and receives later messages.
- Stop the relay. Send messages asynchronously on both hosts, restart the relay, and confirm `hq status` moves each send from queued to relay accepted.
- Stop one machine long enough for several asynchronous messages to reach the relay. Restart the machine and confirm its TUI catches up without duplicate rows.
- Put a bad relay URL in one installation, confirm local use still works, restore the common URL, and confirm recovery.
- Restart both service-manager jobs and the relay. Confirm the shared inbox remains intact, retained catch-up has no duplicate rows, and new messages arrive without manual `hq sync` calls.
- Revoke the added device. Confirm the device records the revoke but receives no later account fanout.
- Run `hq identity export` on the creator and store the encrypted backup away from both machines. Do not run the exported identity on two active hosts.

## Security and data limits

HQ assumes agents and other same-user local processes cooperate. SQLite stores message bodies in plaintext. Mode `0600` does not hide the database or root key from a process running as the same user.

The relay sees recipient root public keys, one-use wrapper keys, event sizes, random times, client addresses, and NIP-42 installation keys. NIP-44 hides the message body and HQ context, but it does not provide forward secrecy or post-compromise security. A stolen installation root key can decrypt retained wrappers for that key.

Shared human accounts replicate the human inbox and signed account facts. Shared human accounts do not merge installation-private agent bindings, leases, relay state, or databases. Running one imported installation identity on two active hosts remains unsupported.
