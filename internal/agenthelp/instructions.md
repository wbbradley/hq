# HQ instructions for agents

HQ lets an agent queue a question for a person and read the answer later.

## Set the session scope

Set `HQ_SESSION` to one stable ID for the whole agent run. Keep the same work dir and session ID when asking and polling. HQ scopes each question by the absolute work dir, session ID, and a UUIDv7 question ID.

```sh
export HQ_SESSION="stable-agent-run-id"
```

When `HQ_SESSION` is empty, HQ uses `CODEX_THREAD_ID` when Codex provides that variable. An explicit `--session` flag wins over both variables.

## Ask a question

Write a clear prompt that states the choice the person must make. Put useful context, options, and tradeoffs in `--details`. Save the question ID from stdout.

```sh
question_id=$(hq ask \
  --details "Option A keeps the old API. Option B removes it." \
  "Should I choose option A or option B?")
```

`hq ask` reads the prompt from stdin when no prompt argument is present. Add `--json` when structured output is easier to use.

## Read answers

Use `wait` only when the answer blocks all useful work:

```sh
answer=$(hq wait --timeout 30m "$question_id")
```

Use `poll` when the agent can keep working. `poll` reads all ready answers in the current work dir and session:

```sh
if answers=$(hq poll); then
  printf '%s\n' "$answers"
else
  status=$?
  if [ "$status" -ne 3 ]; then
    exit "$status"
  fi
fi
```

`poll` exits with code 3 and writes nothing when no answer is ready. Other errors go to stderr. Plain `wait` output contains only the response. Plain `poll` output contains one tab-separated question ID and response per line. Pass `--json` for structured output.

## Delivery rules

`wait` and `poll` lease each answer, write the full output once, and then set `completed_at`. HQ keeps every question in the database. A process crash after the stdout write but before the database update can cause one later retry, so use the question ID as an idempotency key when a duplicate response matters.

Do not use the human `tui`, `list`, `answer`, or `cancel` commands to read an agent response. Use `wait`, `poll`, or `get`.

Bare `hq` does not open the TUI when stdin or stdout is redirected. Bare non-interactive mode lists pending questions in the current work dir and inferred session. When HQ cannot infer a session, bare mode lists pending questions from all sessions in the current work dir.
