package domain

import (
	"strings"
	"testing"

	"github.com/wbbradley/hq/internal/model"
)

func TestConversationEntryValidatesDiscriminatedShapeAndCanonicalIdentity(t *testing.T) {
	eventID := strings.Repeat("a", 64)
	message := model.Message{EventID: eventID}
	activity := HarnessActivity{EventID: eventID}
	valid := []ConversationEntry{
		{Kind: ConversationEntryMessage, EventID: eventID, Message: &message},
		{Kind: ConversationEntryActivity, EventID: eventID, Activity: &activity},
	}
	for _, entry := range valid {
		if !entry.Valid() {
			t.Fatalf("valid entry rejected: %#v", entry)
		}
	}
	invalid := []ConversationEntry{
		{},
		{Kind: ConversationEntryMessage, EventID: "short", Message: &message},
		{Kind: ConversationEntryMessage, EventID: eventID, DisplayOrder: -1, Message: &message},
		{Kind: ConversationEntryMessage, EventID: eventID, Message: &message, Activity: &activity},
		{Kind: ConversationEntryMessage, EventID: strings.Repeat("b", 64), Message: &message},
		{Kind: ConversationEntryActivity, EventID: strings.Repeat("b", 64), Activity: &activity},
		{Kind: ConversationEntryActivity, EventID: eventID, DisplayOrder: 1, Activity: &activity},
	}
	for _, entry := range invalid {
		if entry.Valid() {
			t.Fatalf("invalid entry accepted: %#v", entry)
		}
	}
}
