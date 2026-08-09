# HQ sync semantics for agents

Run `hq agents` first for the normal workflow.

HQ commits each signed local event before relay sync. A message ID from `hq ask` proves that HQ saved the local message.

After the local save, HQ runs a short relay sync and wakes a running sync daemon. A relay-pending note on stderr does not undo the local save. No daemon is required.

`wait` and `poll` also run bounded relay sync. Agents do not need relay keys, relay credentials, daemon access, or manual sync commands.

Do not pass `--no-sync` unless the human asks for offline-only work. Do not run `hq sync`, `hq daemon`, `hq relay`, or `hq status`; those commands manage transport for the human-owned installation.
