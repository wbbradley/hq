// Package harnessbridge connects HQ's durable messaging model to a harness-neutral runtime.
package harnessbridge

import (
	"context"
	"io"
	"log/slog"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/harness"
	"github.com/wbbradley/hq/internal/model"
)

const (
	defaultRepairInterval = 5 * time.Minute
	defaultLeaseDuration  = 30 * time.Second
	defaultRenewInterval  = 10 * time.Second
	shutdownTimeout       = 3 * time.Second
)

type ClaimStore interface {
	Claim(context.Context, domain.Claim, string) (model.Message, error)
	Complete(context.Context, string, string) error
	Release(context.Context, string, string) error
}

type QuestionStore interface {
	HumanMailbox(context.Context) (model.Mailbox, error)
	Create(context.Context, model.Message) error
	Get(context.Context, string) (model.Message, error)
	List(context.Context, model.Filter) ([]model.Message, error)
	Archive(context.Context, string) error
}

type Store interface {
	ClaimStore
	QuestionStore
	CreateNamedAgent(context.Context, string, string) (domain.NamedAgent, error)
	SelectNamedAgentSession(context.Context, string, model.SessionIdentity, model.RepositoryContext) (domain.NamedAgent, error)
	AcquireNamedAgent(context.Context, string, string, time.Duration) (domain.NamedAgent, error)
	RenewNamedAgent(context.Context, string, string, time.Duration) (domain.NamedAgent, error)
	ReleaseNamedAgent(context.Context, string, string) error
}

type ProjectStore interface {
	domain.ProjectDeliveryOperations
	domain.ProjectOutputOperations
}

type DeliveryState string

const (
	DeliveryPending   DeliveryState = "pending"
	DeliveryUncertain DeliveryState = "uncertain"
	DeliveryAccepted  DeliveryState = "accepted"
)

type DeliveryLedger interface {
	Delivery(sessionID, messageID string) (DeliveryState, bool, error)
	SetDelivery(sessionID, messageID string, state DeliveryState) error
	OutputSent(sessionID, itemID string) (bool, error)
	MarkOutputSent(sessionID, itemID string) error
}

type ProjectBinding struct {
	ProjectID       string
	AssignmentID    string
	ProjectThreadID string
	MailboxID       string
	ProjectName     string
}

type Ready struct {
	AgentName string
	Session   harness.SessionIdentity
	Directory string
}

type Terminology struct {
	ProviderName    string
	SessionName     string
	OperationName   string
	ItemName        string
	ReadyBody       string
	ReadyStatus     string
	StoppedBody     string
	CancelledStatus string
	OutputNamespace string
	NewSessionHint  string
}

type Options struct {
	Directory          string
	Environment        []string
	AgentName          string
	ProjectID          string
	NewSession         bool
	RequestedSession   harness.SessionID
	InitialPrompt      string
	Repository         model.RepositoryContext
	Factory            harness.Factory
	ProviderOptions    harness.ProviderOptions
	Store              Store
	ProjectStore       ProjectStore
	Ledger             DeliveryLedger
	Stderr             io.Writer
	Sync               func(context.Context) error
	RepairInterval     time.Duration
	Updates            domain.ClientUpdates
	AgentLeaseDuration time.Duration
	AgentRenewInterval time.Duration
	ProjectReady       func(Ready) (ProjectBinding, error)
	OnReady            func(Ready)
	PublishStatus      func(context.Context, model.Mailbox, harness.SessionIdentity, string, string, time.Time) error
	SuppressStatus     bool
	Terminology        Terminology
	Logger             *slog.Logger
}
