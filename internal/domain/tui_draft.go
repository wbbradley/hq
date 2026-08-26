package domain

import (
	"context"
	"errors"
	"time"

	"github.com/wbbradley/hq/internal/model"
)

var (
	ErrTUIDraftNotFound = errors.New("TUI draft not found")
	ErrTUIDraftConflict = errors.New("TUI draft version conflict")
	ErrTUIDraftTarget   = errors.New("TUI draft target is no longer available")
)

// ProjectActivationIntent is local workflow intent. It is deliberately not a
// canonical project fact and is only acted on after the draft message commits.
type ProjectActivationIntent struct {
	ProjectID          string               `json:"project_id"`
	AgentName          string               `json:"agent_name"`
	Harness            string               `json:"harness"`
	Action             HarnessSessionAction `json:"action"`
	SessionID          string               `json:"session_id,omitempty"`
	Directory          string               `json:"directory"`
	Force              bool                 `json:"force,omitempty"`
	SourceProjectID    string               `json:"source_project_id,omitempty"`
	SourceExpectedHead string               `json:"source_expected_head,omitempty"`
}

// TUIDraft is unsigned installation-local editor state. ID is also the stable
// message/idempotency identity used by SubmitTUIDraft.
type TUIDraft struct {
	ID                 string                   `json:"id"`
	Version            uint64                   `json:"version"`
	Body               string                   `json:"body"`
	ReplyToMessageID   string                   `json:"reply_to_message_id,omitempty"`
	Conversation       model.ConversationKey    `json:"conversation,omitzero"`
	RecipientMailboxID string                   `json:"recipient_mailbox_id,omitempty"`
	RecipientLabel     string                   `json:"recipient_label,omitempty"`
	RecipientAddress   model.MessageAddress     `json:"recipient_address,omitzero"`
	RecipientNamed     bool                     `json:"recipient_named,omitempty"`
	Repository         model.RepositoryContext  `json:"repository,omitzero"`
	Activation         *ProjectActivationIntent `json:"activation,omitempty"`
	CreatedAt          time.Time                `json:"created_at"`
	UpdatedAt          time.Time                `json:"updated_at"`
}

type TUIDraftSubmission struct {
	MessageID  string                   `json:"message_id"`
	Activation *ProjectActivationIntent `json:"activation,omitempty"`
}

type TUIDraftOperations interface {
	ListTUIDrafts(context.Context) ([]TUIDraft, error)
	PutTUIDraft(context.Context, TUIDraft) (TUIDraft, error)
	DeleteTUIDraft(context.Context, string, uint64) error
	SubmitTUIDraft(context.Context, string, uint64) (TUIDraftSubmission, error)
}
