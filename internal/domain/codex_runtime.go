package domain

import (
	"context"
	"time"

	"github.com/wbbradley/hq/internal/model"
)

type CodexSessionAction string

const (
	CodexSessionCurrent CodexSessionAction = "current"
	CodexSessionNew     CodexSessionAction = "new"
	CodexSessionResume  CodexSessionAction = "resume"
)

type CodexRuntimePhase string

const (
	CodexRuntimeOffline  CodexRuntimePhase = "offline"
	CodexRuntimeStarting CodexRuntimePhase = "starting"
	CodexRuntimeRunning  CodexRuntimePhase = "running"
	CodexRuntimeStopping CodexRuntimePhase = "stopping"
	CodexRuntimeFailed   CodexRuntimePhase = "failed"
	CodexRuntimeConflict CodexRuntimePhase = "ownership-conflict"
	CodexRuntimePending  CodexRuntimePhase = "pending-home-command"
)

// CodexLaunchDefaults contains local preferences used when the daemon must
// construct a launch request itself. Explicit client launch requests already
// contain their resolved values.
type CodexLaunchDefaults struct {
	Yolo bool
}

type CodexPendingWorkKind string

const (
	CodexPendingDirect  CodexPendingWorkKind = "direct-agent"
	CodexPendingProject CodexPendingWorkKind = "project-assignment"
)

// CodexPendingWork is the durable launch identity for an inbox that currently
// has incomplete delivery. Environment and runtime presence are deliberately
// absent: they are transient daemon concerns.
type CodexPendingWork struct {
	Kind            CodexPendingWorkKind    `json:"kind"`
	AgentName       string                  `json:"agent_name"`
	MailboxID       string                  `json:"mailbox_id"`
	SessionID       string                  `json:"session_id"`
	Repository      model.RepositoryContext `json:"repository"`
	ProjectID       string                  `json:"project_id,omitempty"`
	AssignmentID    string                  `json:"assignment_id,omitempty"`
	ProjectThreadID string                  `json:"project_thread_id,omitempty"`
}

type CodexPendingWorkOperations interface {
	ListCodexPendingWork(context.Context) ([]CodexPendingWork, error)
}

// CodexLaunchRequest is local control-plane input. Environment is sensitive:
// implementations may retain it only in daemon memory for automatic relaunch,
// and must never write it to durable storage, log it, or return it.
type CodexLaunchRequest struct {
	RequestID     string                  `json:"request_id"`
	AgentName     string                  `json:"agent_name"`
	Action        CodexSessionAction      `json:"action"`
	SessionID     string                  `json:"session_id,omitempty"`
	Directory     string                  `json:"directory"`
	Repository    model.RepositoryContext `json:"repository"`
	Environment   []string                `json:"environment"`
	InitialPrompt string                  `json:"initial_prompt,omitempty"`
	Yolo          bool                    `json:"yolo,omitempty"`
	ConfirmSwitch bool                    `json:"confirm_switch,omitempty"`
}

type CodexRuntime struct {
	AgentName string            `json:"agent_name"`
	ThreadID  string            `json:"thread_id,omitempty"`
	Directory string            `json:"directory,omitempty"`
	Phase     CodexRuntimePhase `json:"phase"`
	StartedAt *time.Time        `json:"started_at,omitempty"`
	Error     string            `json:"error,omitempty"`
}

type CodexRuntimeController interface {
	LaunchCodexAgent(context.Context, CodexLaunchRequest) (CodexRuntime, error)
	StopCodexAgent(context.Context, string) (CodexRuntime, error)
	CodexAgentRuntime(context.Context, string) (CodexRuntime, error)
}

type ProjectCodexActivationRequest struct {
	ProjectID    string             `json:"project_id"`
	ExpectedHead string             `json:"expected_head_event_id"`
	AgentName    string             `json:"agent_name"`
	Launch       CodexLaunchRequest `json:"launch"`
}

type ProjectCodexActivation struct {
	Project Project      `json:"project"`
	Runtime CodexRuntime `json:"runtime"`
}

type ProjectCodexCloseRequest struct {
	RequestID    string `json:"request_id"`
	ProjectID    string `json:"project_id"`
	ExpectedHead string `json:"expected_head_event_id"`
	Force        bool   `json:"force,omitempty"`
	Archive      bool   `json:"archive,omitempty"`
}

type ProjectCodexHandoffRequest struct {
	RequestID    string             `json:"request_id"`
	ProjectID    string             `json:"project_id"`
	ExpectedHead string             `json:"expected_head_event_id"`
	NewAgentName string             `json:"new_agent_name"`
	Force        bool               `json:"force,omitempty"`
	Launch       CodexLaunchRequest `json:"launch"`
}

type CodexRetireAgentRequest struct {
	RequestID string `json:"request_id"`
	AgentName string `json:"agent_name"`
	Force     bool   `json:"force,omitempty"`
}

type ProjectCodexRuntimeController interface {
	ActivateCodexProject(context.Context, ProjectCodexActivationRequest) (ProjectCodexActivation, error)
	CloseCodexProject(context.Context, ProjectCodexCloseRequest) (Project, error)
	HandoffCodexProject(context.Context, ProjectCodexHandoffRequest) (ProjectCodexActivation, error)
	RetireCodexAgent(context.Context, CodexRetireAgentRequest) error
}

// CodexRuntimeAutoStarter is implemented by a daemon-owned runtime that can
// wake the named agent addressed by a newly committed local human message.
// Implementations must copy Environment before returning if they retain it.
type CodexRuntimeAutoStarter interface {
	WakeCodexAgent(model.Message, []string)
}
