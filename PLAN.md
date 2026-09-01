# HQ

## Next Up

### Scroll oversized conversation messages through the viewport

The transcript currently budgets whole entry heights. An oversized selected entry is painted only
from its first row, content below that clipped slice is unreachable, and oversized neighboring
entries can be omitted wholesale. Treat the transcript as a continuous sequence of measured visual
rows while retaining entry identity for selection and actions.

#### Behavior

- Paint every entry slice that intersects the viewport; never leave usable transcript rows blank
  merely because a complete entry does not fit, including one- and two-row message areas.
- Up/Down scroll by visual row. Keep `j`/`k` as previous/next-entry navigation so a long message can
  be skipped, and use Home/End for the start/end of the selected entry. PageDown remains the request
  for older history.
- Keep selection, reply/archive/restore, and technical-detail authority attached to the stable fact
  identity even when only part of an entry is visible. Show noncanonical continuation cues when its
  header or body continues outside the viewport.
- Opening at the tail shows the bottom slice of an oversized latest message; explicitly selecting
  an entry reveals its beginning. Append new live content only follows the tail when the reader was
  already at the bottom.
- Resize, refresh, paging, draft/inspector/banner layout changes, and pending-to-canonical message
  reconciliation preserve or deterministically clamp the same entry and nearby visual row.

#### Implementation and verification

- Replace `visible_conversation_entries`' whole-entry range with a pure viewport layout containing
  each intersecting entry's identity/index, first visible visual row, and painted height.
- Make `ConversationEntryLayout` the single width-specific row model for measurement, slicing,
  continuation cues, selection painting, and output. Slice cached Markdown lines without reparsing
  or losing styles, grapheme boundaries, table clipping, or terminal safety.
- Store viewport position and explicit follow-tail intent separately from `conversation_anchor` and
  `technical_scroll`. If the model needs renderer measurements, pass a typed passive observation;
  render-cache state must not become selection authority.
- Cover oversized entries above, at, and below the anchor; top/middle/bottom Markdown slices; narrow
  panes and wide Unicode; row and entry navigation; tail-follow; resize; refresh/reorder; paging;
  pending identity replacement; drafts, inspectors, and interaction banners. Add an installed PTY
  regression proving the whole long message and adjacent entries are reachable.
- Retain the Markdown redraw qualification budget and bounded artifact cache. Update the TUI,
  conversation-surface, acceptance-scenario, and behavior-ledger contracts.

Done when every nonempty conversation viewport paints available transcript content, every visual
row of an oversized message is reachable, and entry selection/action identity remains stable across
layout and authoritative-state changes.
