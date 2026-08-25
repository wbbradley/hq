package domain

import "github.com/wbbradley/hq/internal/model"

type ConversationEntryKind string

const (
	ConversationEntryMessage  ConversationEntryKind = "message"
	ConversationEntryActivity ConversationEntryKind = "activity"
)

// ConversationEntry is a read-side union. Activity is non-actionable; callers
// must use Message.ID only when Kind is ConversationEntryMessage.
type ConversationEntry struct {
	Kind         ConversationEntryKind `json:"kind"`
	EventID      string                `json:"event_id"`
	DisplayOrder int                   `json:"display_order"`
	Message      *model.Message        `json:"message,omitempty"`
	Activity     *HarnessActivity      `json:"activity,omitempty"`
}

func (e ConversationEntry) Valid() bool {
	if len(e.EventID) != 64 || e.DisplayOrder < 0 {
		return false
	}
	switch e.Kind {
	case ConversationEntryMessage:
		return e.Message != nil && e.Message.EventID == e.EventID && e.Activity == nil
	case ConversationEntryActivity:
		return e.Message == nil && e.Activity != nil && e.Activity.EventID == e.EventID && e.Activity.DisplayOrder == e.DisplayOrder
	default:
		return false
	}
}

type ConversationEntryPage struct {
	Entries    []ConversationEntry `json:"entries"`
	NextCursor string              `json:"next_cursor,omitempty"`
}
