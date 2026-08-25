package domain

import (
	"context"
	"encoding/json"
	"time"

	"github.com/wbbradley/hq/internal/model"
)

type HarnessSessionAction string

const (
	HarnessSessionCurrent HarnessSessionAction = "current"
	HarnessSessionNew     HarnessSessionAction = "new"
	HarnessSessionResume  HarnessSessionAction = "resume"
)

type HarnessRuntimePhase string

const (
	HarnessRuntimeOffline  HarnessRuntimePhase = "offline"
	HarnessRuntimeStarting HarnessRuntimePhase = "starting"
	HarnessRuntimeRunning  HarnessRuntimePhase = "running"
	HarnessRuntimeStopping HarnessRuntimePhase = "stopping"
	HarnessRuntimeFailed   HarnessRuntimePhase = "failed"
	HarnessRuntimeConflict HarnessRuntimePhase = "ownership-conflict"
	HarnessRuntimePending  HarnessRuntimePhase = "pending-home-command"
)

type HarnessWorkState string

const (
	HarnessWorkWaiting HarnessWorkState = "waiting"
	HarnessWorkWorking HarnessWorkState = "working"
	HarnessWorkUnknown HarnessWorkState = "unknown"
)

// HarnessLaunchDefaults contains local preferences used when the daemon must
// construct a launch request itself. Explicit client launch requests already
// contain their resolved values.
type HarnessLaunchDefaults struct {
	Harness         string
	ProviderOptions json.RawMessage
}

type HarnessPendingWorkKind string

const (
	HarnessPendingDirect  HarnessPendingWorkKind = "direct-agent"
	HarnessPendingProject HarnessPendingWorkKind = "project-assignment"
)

// HarnessPendingWork is the durable launch identity for an inbox that currently
// has incomplete delivery. Environment and runtime presence are deliberately
// absent: they are transient daemon concerns.
type HarnessPendingWork struct {
	Kind            HarnessPendingWorkKind  `json:"kind"`
	AgentName       string                  `json:"agent_name"`
	MailboxID       string                  `json:"mailbox_id"`
	Harness         string                  `json:"harness"`
	SessionID       string                  `json:"session_id"`
	Repository      model.RepositoryContext `json:"repository"`
	ProjectID       string                  `json:"project_id,omitempty"`
	AssignmentID    string                  `json:"assignment_id,omitempty"`
	ProjectThreadID string                  `json:"project_thread_id,omitempty"`
}

type HarnessPendingWorkOperations interface {
	ListHarnessPendingWork(context.Context) ([]HarnessPendingWork, error)
}

// HarnessLaunchRequest is local control-plane input. Environment is sensitive:
// implementations may retain it only in daemon memory for automatic relaunch,
// and must never write it to durable storage, log it, or return it.
type HarnessLaunchRequest struct {
	RequestID       string                  `json:"request_id"`
	AgentName       string                  `json:"agent_name"`
	Harness         string                  `json:"harness"`
	Action          HarnessSessionAction    `json:"action"`
	SessionID       string                  `json:"session_id,omitempty"`
	Directory       string                  `json:"directory"`
	Repository      model.RepositoryContext `json:"repository"`
	Environment     []string                `json:"environment"`
	InitialPrompt   string                  `json:"initial_prompt,omitempty"`
	ProviderOptions json.RawMessage         `json:"provider_options,omitempty"`
	ConfirmSwitch   bool                    `json:"confirm_switch,omitempty"`
}

type HarnessRuntime struct {
	AgentName         string              `json:"agent_name"`
	Harness           string              `json:"harness"`
	SessionID         string              `json:"session_id,omitempty"`
	Directory         string              `json:"directory,omitempty"`
	Phase             HarnessRuntimePhase `json:"phase"`
	StartedAt         *time.Time          `json:"started_at,omitempty"`
	Error             string              `json:"error,omitempty"`
	WorkState         HarnessWorkState    `json:"work_state,omitempty"`
	ActiveOperationID string              `json:"active_operation_id,omitempty"`
}

type ProjectHarnessClosePreview struct {
	Project       Project                     `json:"project"`
	Runtime       HarnessRuntime              `json:"runtime"`
	Resources     []ResourceReleaseAssessment `json:"resources,omitempty"`
	RequiresForce bool                        `json:"requires_force"`
}

type HarnessRuntimeController interface {
	LaunchHarnessAgent(context.Context, HarnessLaunchRequest) (HarnessRuntime, error)
	StopHarnessAgent(context.Context, string) (HarnessRuntime, error)
	HarnessAgentRuntime(context.Context, string) (HarnessRuntime, error)
}

type ProjectHarnessActivationRequest struct {
	ProjectID    string               `json:"project_id"`
	ExpectedHead string               `json:"expected_head_event_id"`
	AgentName    string               `json:"agent_name"`
	Launch       HarnessLaunchRequest `json:"launch"`
}

type ProjectHarnessActivation struct {
	Project Project        `json:"project"`
	Runtime HarnessRuntime `json:"runtime"`
}

type ProjectHarnessCloseRequest struct {
	RequestID    string `json:"request_id"`
	ProjectID    string `json:"project_id"`
	ExpectedHead string `json:"expected_head_event_id"`
	Force        bool   `json:"force,omitempty"`
	Archive      bool   `json:"archive,omitempty"`
}

type ProjectHarnessReplaceRequest struct {
	RequestID          string               `json:"request_id"`
	SourceProjectID    string               `json:"source_project_id"`
	SourceExpectedHead string               `json:"source_expected_head_event_id"`
	TargetProjectID    string               `json:"target_project_id"`
	TargetExpectedHead string               `json:"target_expected_head_event_id"`
	AgentName          string               `json:"agent_name"`
	Force              bool                 `json:"force,omitempty"`
	Launch             HarnessLaunchRequest `json:"launch"`
}

type ProjectHarnessHandoffRequest struct {
	RequestID    string               `json:"request_id"`
	ProjectID    string               `json:"project_id"`
	ExpectedHead string               `json:"expected_head_event_id"`
	NewAgentName string               `json:"new_agent_name"`
	Force        bool                 `json:"force,omitempty"`
	Launch       HarnessLaunchRequest `json:"launch"`
}

type HarnessRetireAgentRequest struct {
	RequestID string `json:"request_id"`
	AgentName string `json:"agent_name"`
	Force     bool   `json:"force,omitempty"`
}

type ProjectHarnessRuntimeController interface {
	ActivateHarnessProject(context.Context, ProjectHarnessActivationRequest) (ProjectHarnessActivation, error)
	CloseHarnessProject(context.Context, ProjectHarnessCloseRequest) (Project, error)
	PreviewHarnessProjectClose(context.Context, string) (ProjectHarnessClosePreview, error)
	ReplaceHarnessProject(context.Context, ProjectHarnessReplaceRequest) (ProjectHarnessActivation, error)
	HandoffHarnessProject(context.Context, ProjectHarnessHandoffRequest) (ProjectHarnessActivation, error)
	RetireHarnessAgent(context.Context, HarnessRetireAgentRequest) error
}

// HarnessRuntimeAutoStarter is implemented by a daemon-owned runtime that can
// wake the named agent addressed by a newly committed local human message.
// Implementations must copy Environment before returning if they retain it.
type HarnessRuntimeAutoStarter interface {
	WakeHarnessAgent(model.Message, []string)
}
