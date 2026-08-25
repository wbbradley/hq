package domain

import (
	"context"
	"time"

	"github.com/wbbradley/hq/internal/model"
)

type HarnessActivityKind string

const (
	HarnessActivityOperation HarnessActivityKind = "operation-status"
	HarnessActivityPlan      HarnessActivityKind = "plan"
	HarnessActivityDiff      HarnessActivityKind = "diff"
	HarnessActivityCommand   HarnessActivityKind = "command"
	HarnessActivityFile      HarnessActivityKind = "file-change"
	HarnessActivityTool      HarnessActivityKind = "tool-call"
	HarnessActivityProgress  HarnessActivityKind = "progress"
)

type HarnessActivityStatus string

const (
	HarnessActivityRunning     HarnessActivityStatus = "running"
	HarnessActivityCompleted   HarnessActivityStatus = "completed"
	HarnessActivityFailed      HarnessActivityStatus = "failed"
	HarnessActivityInterrupted HarnessActivityStatus = "interrupted"
)

const (
	HarnessActivityTitleBytes       = 1 << 10
	HarnessActivityBodyBytes        = 12 << 10
	HarnessActivityCommandBodyBytes = 12 << 10
	HarnessActivityProgressBytes    = 4 << 10
	HarnessActivityProgressRetained = 200
)

// HarnessActivity is a non-actionable runtime projection. Canonical activities
// carry EventID and source identity; legacy local rows leave those fields empty.
// An activity is never a message and cannot be replied to or archived.
type HarnessActivity struct {
	EventID           string                   `json:"event_id,omitempty"`
	InstallationID    string                   `json:"installation_id,omitempty"`
	MailboxID         string                   `json:"mailbox_id"`
	AudienceAccountID string                   `json:"audience_account_id,omitempty"`
	Harness           string                   `json:"harness"`
	SessionID         string                   `json:"session_id"`
	OperationID       string                   `json:"operation_id"`
	Correlation       model.MessageCorrelation `json:"correlation,omitzero"`
	RuntimeID         string                   `json:"runtime_id,omitempty"`
	Sequence          uint64                   `json:"sequence,omitempty"`
	DisplayOrder      int                      `json:"display_order,omitempty"`
	Kind              HarnessActivityKind      `json:"kind"`
	ItemID            string                   `json:"item_id,omitempty"`
	Status            HarnessActivityStatus    `json:"status,omitempty"`
	Title             string                   `json:"title,omitempty"`
	Body              string                   `json:"body,omitempty"`
	Truncated         bool                     `json:"truncated,omitempty"`
	OccurredAt        time.Time                `json:"occurred_at"`
}

type HarnessActivityFilter struct {
	InstallationID string `json:"installation_id,omitempty"`
	MailboxID      string `json:"mailbox_id"`
	Harness        string `json:"harness,omitempty"`
	SessionID      string `json:"session_id,omitempty"`
	Limit          int    `json:"limit,omitempty"`
}

type HarnessActivityWriter interface {
	UpsertHarnessActivity(context.Context, HarnessActivity) error
}

type HarnessActivityReader interface {
	ListHarnessActivities(context.Context, HarnessActivityFilter) ([]HarnessActivity, error)
}

type HarnessActivityOperations interface {
	HarnessActivityWriter
	HarnessActivityReader
}
