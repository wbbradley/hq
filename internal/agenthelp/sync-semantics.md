# HQ sync semantics for agents

Run `hq agents` first for the normal workflow.

HQ commits each signed local event before relay sync. A message ID from `hq send`, or the ID included in an interrupted `hq ask` error, proves that HQ saved the local message.

After the local save, the HQ client asks the owning local node to synchronize promptly. A relay-pending note on stderr does not undo the local save. The client auto-starts the node when needed; agents do not open the database or run a relay worker.

`wait` and `poll` also run bounded relay sync. Agents do not need relay keys, relay credentials, daemon access, or manual sync commands.

The human may see an account question and reply from another paired machine. HQ keeps the original question ID and routes that signed answer back to this agent mailbox. Agents do not need to know which human device sent the answer.

Do not pass `--no-sync` unless the human asks to skip the immediate synchronization request. It is not an offline guarantee because a network-enabled node may still publish durable work. Do not run `hq sync`, `hq daemon`, `hq relay`, or `hq status`; those commands manage transport for the human-owned installation.
