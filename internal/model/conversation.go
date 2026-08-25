package model

import "time"

// ConversationKey identifies one counterparty conversation. HarnessSessionID is
// preferred; ThreadID is the canonical HQ fallback when harness correlation is
// absent.
type ConversationKey struct {
	CounterpartyMailboxID string `json:"counterparty_mailbox_id"`
	HarnessProvider       string `json:"harness_provider,omitempty"`
	HarnessSessionID      string `json:"harness_session_id,omitempty"`
	ThreadID              string `json:"thread_id,omitempty"`
}

func (k ConversationKey) Valid() bool {
	harnessConversation := k.HarnessProvider != "" && k.HarnessSessionID != "" && k.ThreadID == ""
	hqConversation := k.HarnessProvider == "" && k.HarnessSessionID == "" && k.ThreadID != ""
	return k.CounterpartyMailboxID != "" && (harnessConversation || hqConversation)
}

type ConversationFilter struct {
	IncludeSent     bool   `json:"include_sent,omitempty"`
	IncludeArchived bool   `json:"include_archived,omitempty"`
	Cursor          string `json:"cursor,omitempty"`
	Limit           int    `json:"limit,omitempty"`
}

type ConversationSummary struct {
	Key          ConversationKey `json:"key"`
	Latest       Message         `json:"latest"`
	OldestOpen   *Message        `json:"oldest_open,omitempty"`
	OpenCount    int             `json:"open_count"`
	HasSent      bool            `json:"has_sent,omitempty"`
	HasArchived  bool            `json:"has_archived,omitempty"`
	LastActivity time.Time       `json:"last_activity"`
}

type ConversationPage struct {
	Conversations []ConversationSummary `json:"conversations"`
	NextCursor    string                `json:"next_cursor,omitempty"`
}

type ConversationHistoryFilter struct {
	Key    ConversationKey `json:"key"`
	Cursor string          `json:"cursor,omitempty"`
	Limit  int             `json:"limit,omitempty"`
}

type MessagePage struct {
	Messages   []Message `json:"messages"`
	NextCursor string    `json:"next_cursor,omitempty"`
}
