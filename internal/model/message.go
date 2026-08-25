package model

import "time"

const HumanMailboxID = "00000000-0000-7000-8000-000000000000"

type MailboxKind string

const (
	MailboxHuman   MailboxKind = "human"
	MailboxAgent   MailboxKind = "agent"
	MailboxProject MailboxKind = "project"
	MailboxRemote  MailboxKind = "remote"
)

type MessagePurpose string

const (
	MessagePurposeConversation     MessagePurpose = "conversation"
	MessagePurposeProjectInput     MessagePurpose = "project-input"
	MessagePurposeProtocolQuestion MessagePurpose = "protocol-question"
	MessagePurposeProtocolAnswer   MessagePurpose = "protocol-answer"
	MessagePurposeProjectOutput    MessagePurpose = "project-output"
	MessagePurposeSystemNotice     MessagePurpose = "system-notice"
)

func NormalizeMessagePurpose(purpose MessagePurpose) MessagePurpose {
	if purpose == "" {
		return MessagePurposeConversation
	}
	return purpose
}

func (purpose MessagePurpose) Valid() bool {
	switch purpose {
	case "", MessagePurposeConversation, MessagePurposeProjectInput, MessagePurposeProtocolQuestion, MessagePurposeProtocolAnswer, MessagePurposeProjectOutput, MessagePurposeSystemNotice:
		return true
	default:
		return false
	}
}

type MessageAddress struct {
	InstallationID string      `json:"installation_id"`
	MailboxID      string      `json:"mailbox_id"`
	Kind           MailboxKind `json:"kind"`
	Label          string      `json:"label"`
	Harness        string      `json:"harness,omitempty"`
	Name           string      `json:"name,omitempty"`
}

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
	ID                      string             `json:"id"`
	EventID                 string             `json:"event_id"`
	ThreadID                string             `json:"thread_id"`
	Purpose                 MessagePurpose     `json:"purpose"`
	Presentation            PresentationKind   `json:"presentation,omitempty"`
	Correlation             MessageCorrelation `json:"correlation,omitzero"`
	TechnicalSections       []TechnicalSection `json:"technical_sections,omitempty"`
	HarnessProvider         string             `json:"harness_provider,omitempty"`
	HarnessSessionID        string             `json:"harness_session_id,omitempty"`
	HarnessOperationID      string             `json:"harness_operation_id,omitempty"`
	Incomplete              bool               `json:"incomplete_causal_history,omitempty"`
	PeerReceived            bool               `json:"peer_received,omitempty"`
	Rejected                bool               `json:"rejected,omitempty"`
	DeliveryState           string             `json:"delivery_state,omitempty"`
	AudienceAccountID       string             `json:"audience_account_id,omitempty"`
	Context                 RepositoryContext  `json:"context"`
	SenderMailboxID         string             `json:"sender_mailbox_id"`
	RecipientMailboxID      string             `json:"recipient_mailbox_id"`
	SenderInstallationID    string             `json:"sender_installation_id"`
	RecipientInstallationID string             `json:"recipient_installation_id"`
	SenderLabel             string             `json:"sender"`
	SourceDeviceLabel       string             `json:"source_device,omitempty"`
	RecipientLabel          string             `json:"recipient"`
	SenderAddress           MessageAddress     `json:"sender_address"`
	RecipientAddress        MessageAddress     `json:"recipient_address"`
	Body                    string             `json:"body"`
	Details                 string             `json:"details,omitempty"`
	ReplyTo                 *string            `json:"reply_to,omitempty"`
	CreatedAt               time.Time          `json:"created_at"`
	ArchivedAt              *time.Time         `json:"archived_at,omitempty"`
	CompletedAt             *time.Time         `json:"completed_at,omitempty"`
}

type Filter struct {
	Directory             string         `json:"directory,omitempty"`
	SenderMailboxID       string         `json:"sender_mailbox_id,omitempty"`
	RecipientMailboxID    string         `json:"recipient_mailbox_id,omitempty"`
	CounterpartyMailboxID string         `json:"counterparty_mailbox_id,omitempty"`
	ThreadID              string         `json:"thread_id,omitempty"`
	HarnessProvider       string         `json:"harness_provider,omitempty"`
	HarnessSessionID      string         `json:"harness_session_id,omitempty"`
	HarnessOperationID    string         `json:"harness_operation_id,omitempty"`
	ReplyTo               string         `json:"reply_to,omitempty"`
	Purpose               MessagePurpose `json:"purpose,omitempty"`
	Archived              *bool          `json:"archived,omitempty"`
	Completed             *bool          `json:"completed,omitempty"`
	Limit                 int            `json:"limit,omitempty"`
	NewestFirst           bool           `json:"newest_first,omitempty"`
}
