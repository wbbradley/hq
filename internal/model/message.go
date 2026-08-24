package model

import "time"

const HumanMailboxID = "00000000-0000-7000-8000-000000000000"

type MailboxKind string

const (
	MailboxHuman   MailboxKind = "human"
	MailboxAgent   MailboxKind = "agent"
	MailboxProject MailboxKind = "project"
)

type SessionIdentity struct {
	Harness           string `json:"harness"`
	ExternalSessionID string `json:"-"`
}

type RepositoryContext struct {
	Directory      string `json:"directory"`
	GitCommonDir   string `json:"git_common_dir,omitempty"`
	RemoteIdentity string `json:"remote_identity,omitempty"`
	Worktree       string `json:"worktree,omitempty"`
	Branch         string `json:"branch,omitempty"`
}

type Mailbox struct {
	ID        string            `json:"id"`
	Kind      MailboxKind       `json:"kind"`
	Harness   string            `json:"harness,omitempty"`
	Label     string            `json:"label"`
	CreatedAt time.Time         `json:"created_at"`
	LastSeen  time.Time         `json:"last_seen_at"`
	Context   RepositoryContext `json:"context,omitempty"`
}

type Message struct {
	ID                      string            `json:"id"`
	EventID                 string            `json:"event_id"`
	ThreadID                string            `json:"thread_id"`
	CodexThreadID           string            `json:"codex_thread_id,omitempty"`
	CodexTurnID             string            `json:"codex_turn_id,omitempty"`
	Incomplete              bool              `json:"incomplete_causal_history,omitempty"`
	PeerReceived            bool              `json:"peer_received,omitempty"`
	Rejected                bool              `json:"rejected,omitempty"`
	DeliveryState           string            `json:"delivery_state,omitempty"`
	AudienceAccountID       string            `json:"audience_account_id,omitempty"`
	Context                 RepositoryContext `json:"context"`
	SenderMailboxID         string            `json:"sender_mailbox_id"`
	RecipientMailboxID      string            `json:"recipient_mailbox_id"`
	SenderInstallationID    string            `json:"sender_installation_id"`
	RecipientInstallationID string            `json:"recipient_installation_id"`
	SenderLabel             string            `json:"sender"`
	SourceDeviceLabel       string            `json:"source_device,omitempty"`
	RecipientLabel          string            `json:"recipient"`
	Body                    string            `json:"body"`
	Details                 string            `json:"details,omitempty"`
	ReplyTo                 *string           `json:"reply_to,omitempty"`
	CreatedAt               time.Time         `json:"created_at"`
	ArchivedAt              *time.Time        `json:"archived_at,omitempty"`
	CompletedAt             *time.Time        `json:"completed_at,omitempty"`
}

type Filter struct {
	Directory             string `json:"directory,omitempty"`
	SenderMailboxID       string `json:"sender_mailbox_id,omitempty"`
	RecipientMailboxID    string `json:"recipient_mailbox_id,omitempty"`
	CounterpartyMailboxID string `json:"counterparty_mailbox_id,omitempty"`
	ThreadID              string `json:"thread_id,omitempty"`
	CodexThreadID         string `json:"codex_thread_id,omitempty"`
	CodexTurnID           string `json:"codex_turn_id,omitempty"`
	ReplyTo               string `json:"reply_to,omitempty"`
	Archived              *bool  `json:"archived,omitempty"`
	Completed             *bool  `json:"completed,omitempty"`
	Limit                 int    `json:"limit,omitempty"`
	NewestFirst           bool   `json:"newest_first,omitempty"`
}
