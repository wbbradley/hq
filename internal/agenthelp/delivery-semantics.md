# HQ delivery semantics for agents

Run `hq agents` first for the normal workflow.

`wait` and `poll` lease each message, write the full output once, and then mark the message complete and archived. HQ keeps the message. A crash after stdout but before the database update can cause one later read, so use the message ID as an idempotency key when a duplicate could trigger a side effect.

An HQ thread can contain more than one answer. `wait` returns the first answer ready for the current mailbox. Use `poll` for later answers and async messages.

Network events can arrive before their causal parents. Plain `poll` output marks such a message with `[incomplete causal history]`; JSON output and `get` expose `incomplete_causal_history`. Treat two copies with the same canonical `event_id` as one event.

Cancellation does not erase an answer. A thread can show both facts when an answer and cancellation cross in transit. Use an answer when the answer still helps; do not assume that the human saw the cancellation.
