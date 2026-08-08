package model

import "time"

type Status string

const (
	StatusPending   Status = "pending"
	StatusAnswered  Status = "answered"
	StatusCancelled Status = "cancelled"
)

type Question struct {
	ID          string     `json:"id"`
	Directory   string     `json:"directory"`
	SessionID   string     `json:"session_id"`
	Prompt      string     `json:"prompt"`
	Details     string     `json:"details,omitempty"`
	Status      Status     `json:"status"`
	Response    *string    `json:"response,omitempty"`
	CreatedAt   time.Time  `json:"created_at"`
	AnsweredAt  *time.Time `json:"answered_at,omitempty"`
	CompletedAt *time.Time `json:"completed_at,omitempty"`
}

type Filter struct {
	Directory string
	SessionID string
	Status    Status
	Limit     int
}
