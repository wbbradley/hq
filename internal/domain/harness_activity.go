package domain

import (
	"context"
	"time"
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
	HarnessActivityBodyBytes        = 64 << 10
	HarnessActivityCommandBodyBytes = 16 << 10
	HarnessActivityProgressBytes    = 4 << 10
	HarnessActivityProgressRetained = 200
)

// HarnessActivity is an installation-local, non-actionable runtime projection.
// It is never a signed message and cannot be replied to or archived.
type HarnessActivity struct {
	MailboxID   string                `json:"mailbox_id"`
	Harness     string                `json:"harness"`
	SessionID   string                `json:"session_id"`
	OperationID string                `json:"operation_id"`
	Kind        HarnessActivityKind   `json:"kind"`
	ItemID      string                `json:"item_id,omitempty"`
	Status      HarnessActivityStatus `json:"status,omitempty"`
	Title       string                `json:"title,omitempty"`
	Body        string                `json:"body,omitempty"`
	Truncated   bool                  `json:"truncated,omitempty"`
	OccurredAt  time.Time             `json:"occurred_at"`
}

type HarnessActivityFilter struct {
	MailboxID string `json:"mailbox_id"`
	Harness   string `json:"harness,omitempty"`
	SessionID string `json:"session_id,omitempty"`
	Limit     int    `json:"limit,omitempty"`
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
