package model

import "time"

const HumanSession = "human"

type MailboxKind string

const (
	MailboxHuman MailboxKind = "human"
	MailboxAgent MailboxKind = "agent"
)

type Message struct {
	ID               string     `json:"id"`
	Directory        string     `json:"directory"`
	SenderSession    string     `json:"sender_session"`
	RecipientSession string     `json:"recipient_session"`
	Body             string     `json:"body"`
	Details          string     `json:"details,omitempty"`
	ReplyTo          *string    `json:"reply_to,omitempty"`
	CreatedAt        time.Time  `json:"created_at"`
	ArchivedAt       *time.Time `json:"archived_at,omitempty"`
	CompletedAt      *time.Time `json:"completed_at,omitempty"`
}

type Filter struct {
	Directory        string
	SenderSession    string
	RecipientSession string
	ReplyTo          string
	Archived         *bool
	Completed        *bool
	Limit            int
	NewestFirst      bool
}
