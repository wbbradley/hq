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
)

// CodexLaunchRequest is local control-plane input. Environment is sensitive
// and transient: implementations must never persist, log, or return it.
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
