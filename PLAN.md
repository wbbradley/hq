# HQ

## Next Up

### Paint intersecting conversation entry slices

Replace `visible_conversation_entries`' whole-entry range with a pure viewport layout containing
each intersecting entry's identity/index, first visible visual row, and painted height.

- Make `ConversationEntryLayout` the single width-specific row model for measurement, slicing,
  continuation cues, selection painting, and output. Slice cached Markdown lines without reparsing
  or losing styles, grapheme boundaries, table clipping, or terminal safety.
- Paint every entry slice that intersects the viewport; never leave usable transcript rows blank
  merely because a complete entry does not fit, including one- and two-row message areas.
- Keep selection, reply/archive/restore, and technical-detail authority attached to the stable fact
  identity even when only part of an entry is visible. Show noncanonical continuation cues when its
  header or body continues outside the viewport.
- Opening at the tail shows the bottom slice of an oversized latest message; explicitly selecting
  an entry reveals its beginning. Append new live content follows the tail only when the reader was
  already at the bottom.
- Cover oversized entries above, at, and below the anchor; top/middle/bottom Markdown slices; narrow
  panes and wide Unicode; and drafts, inspectors, and interaction banners.

Done when every nonempty conversation viewport paints available transcript content and every visual
row of an oversized message is reachable.

### Qualify oversized conversation scrolling

- Add an installed PTY regression proving the complete oversized message and adjacent entries are
  reachable with row and entry navigation across redraws.
- Retain the Markdown redraw qualification budget and bounded artifact cache.
- Update the TUI, conversation-surface, acceptance-scenario, and behavior-ledger contracts for the
  final viewport, navigation, continuation-cue, selection, and follow-tail behavior.
- Run formatting, architecture/spec verifiers, strict locked workspace Clippy, the complete locked
  all-target/all-feature test suite, and installed conversation PTY coverage.

Done when installed behavior and qualification evidence prove the complete oversized-message task.
