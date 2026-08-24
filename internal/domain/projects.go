package domain

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/wbbradley/hq/internal/model"
)

var (
	ErrProjectNotFound       = errors.New("project not found")
	ErrProjectStale          = errors.New("project head is stale")
	ErrProjectState          = errors.New("project lifecycle does not allow this operation")
	ErrResourceConflict      = errors.New("project resource conflicts with an active claim")
	ErrResourceNotFound      = errors.New("project resource not found")
	ErrAgentAssigned         = errors.New("agent is already assigned to a project")
	ErrProjectAssigned       = errors.New("project is already assigned to an agent")
	ErrProjectThreadMismatch = errors.New("execution thread scope does not match the assignment")
	ErrProjectCommandPending = errors.New("project already has an unresolved remote command")
	ErrProjectRuntimeUnknown = errors.New("project runtime quiescence is unknown")
)

type ProjectLifecycle string

const (
	ProjectPreparing ProjectLifecycle = "preparing"
	ProjectOpen      ProjectLifecycle = "open"
	ProjectClosing   ProjectLifecycle = "closing"
	ProjectClosed    ProjectLifecycle = "closed"
)

type AssignmentState string

const (
	AssignmentConfiguring AssignmentState = "configuring"
	AssignmentRunnable    AssignmentState = "runnable"
	AssignmentBlocked     AssignmentState = "blocked"
	AssignmentEnded       AssignmentState = "ended"
)

type ResourceHealthState string

const (
	ResourceHealthy      ResourceHealthState = "healthy"
	ResourceMissing      ResourceHealthState = "missing"
	ResourceInaccessible ResourceHealthState = "inaccessible"
	ResourceMalformed    ResourceHealthState = "malformed"
	ResourceUnknown      ResourceHealthState = "unknown"
)

type ProjectResource struct {
	ID               string              `json:"id"`
	Kind             string              `json:"kind"`
	HomeInstallation string              `json:"home_installation"`
	DisplayLocator   string              `json:"display_locator"`
	CanonicalLocator string              `json:"canonical_locator"`
	Health           ResourceHealthState `json:"health"`
	LastCheckedAt    *time.Time          `json:"last_checked_at,omitempty"`
	HealthDetails    map[string]string   `json:"health_details,omitempty"`
	PendingCommand   *ProjectCommand     `json:"pending_command,omitempty"`
}

type ProjectAssignment struct {
	ID               string          `json:"id"`
	AgentName        string          `json:"agent_name"`
	State            AssignmentState `json:"state"`
	SelectedThreadID string          `json:"selected_thread_id,omitempty"`
	StartedAt        time.Time       `json:"started_at"`
	EndedAt          *time.Time      `json:"ended_at,omitempty"`
}

type ProjectThread struct {
	ID           string    `json:"id"`
	ProjectID    string    `json:"project_id"`
	AgentName    string    `json:"agent_name"`
	Harness      string    `json:"harness"`
	ExternalID   string    `json:"external_id"`
	LaunchDir    string    `json:"launch_dir"`
	CreatedAt    time.Time `json:"created_at"`
	RetiredAgent bool      `json:"retired_agent"`
}

type ProjectDelivery struct {
	Message          model.Message `json:"message"`
	ProjectID        string        `json:"project_id"`
	Sequence         int64         `json:"sequence"`
	AssignmentID     string        `json:"assignment_id"`
	AgentName        string        `json:"agent_name"`
	ProjectThreadID  string        `json:"project_thread_id"`
	ExternalThreadID string        `json:"external_thread_id"`
	Dispatched       bool          `json:"dispatched"`
}

// ProjectOutputBinding is immutable provenance captured when a project runtime
// is connected. The store compares it with current authority when each output
// is persisted; an old runtime is retained but marked late.
type ProjectOutputBinding struct {
	ProjectID        string `json:"project_id"`
	AssignmentID     string `json:"assignment_id"`
	AgentName        string `json:"agent_name"`
	ProjectThreadID  string `json:"project_thread_id"`
	ExternalThreadID string `json:"external_thread_id"`
	RuntimeOwner     string `json:"runtime_owner_token,omitempty"`
	RuntimeState     string `json:"runtime_state,omitempty"`
}

type ProjectActivationOperation struct {
	ID             string           `json:"id"`
	ProjectID      string           `json:"project_id"`
	AgentName      string           `json:"agent_name"`
	PriorLifecycle ProjectLifecycle `json:"prior_lifecycle"`
	AssignmentID   string           `json:"assignment_id,omitempty"`
	State          string           `json:"state"`
	LastError      string           `json:"last_error,omitempty"`
	CreatedAt      time.Time        `json:"created_at"`
	UpdatedAt      time.Time        `json:"updated_at"`
}

type Project struct {
	ID                   string             `json:"id"`
	HomeInstallation     string             `json:"home_installation"`
	MailboxID            string             `json:"mailbox_id"`
	PredecessorProjectID string             `json:"predecessor_project_id,omitempty"`
	Name                 string             `json:"name"`
	Brief                string             `json:"brief,omitempty"`
	Lifecycle            ProjectLifecycle   `json:"lifecycle"`
	Archived             bool               `json:"archived"`
	PrimaryResourceID    string             `json:"primary_resource_id,omitempty"`
	HeadEventID          string             `json:"head_event_id"`
	Resources            []ProjectResource  `json:"resources"`
	Assignment           *ProjectAssignment `json:"assignment,omitempty"`
	CreatedAt            time.Time          `json:"created_at"`
	UpdatedAt            time.Time          `json:"updated_at"`
	ReadOnlyReplica      bool               `json:"read_only_replica,omitempty"`
	PendingCommand       *ProjectCommand    `json:"pending_command,omitempty"`
	LatestCommand        *ProjectCommand    `json:"latest_command,omitempty"`
	SuggestedAgentName   string             `json:"suggested_agent_name,omitempty"`
}

type ProjectCommandStage string

const (
	ProjectCommandAccepted  ProjectCommandStage = "accepted"
	ProjectCommandQueued    ProjectCommandStage = "queued"
	ProjectCommandReceived  ProjectCommandStage = "received"
	ProjectCommandCommitted ProjectCommandStage = "committed"
	ProjectCommandRejected  ProjectCommandStage = "rejected"
)

type ProjectCommand struct {
	ID               string                  `json:"id"`
	ProjectID        string                  `json:"project_id"`
	HomeInstallation string                  `json:"home_installation"`
	ExpectedHead     string                  `json:"expected_head_event_id"`
	Operation        ProjectCommandOperation `json:"operation"`
	Body             []byte                  `json:"body,omitempty"`
	Stage            ProjectCommandStage     `json:"stage"`
	CurrentHead      string                  `json:"current_head_event_id,omitempty"`
	Diagnostic       string                  `json:"diagnostic,omitempty"`
	CreatedAt        time.Time               `json:"created_at"`
	UpdatedAt        time.Time               `json:"updated_at"`
}

type ProjectPathInput struct {
	DisplayPath string `json:"display_path"`
}

type CreateProjectRequest struct {
	ID                   string             `json:"id,omitempty"`
	HomeInstallation     string             `json:"home_installation,omitempty"`
	Name                 string             `json:"name"`
	Brief                string             `json:"brief,omitempty"`
	PredecessorProjectID string             `json:"predecessor_project_id,omitempty"`
	Paths                []ProjectPathInput `json:"paths,omitempty"`
	PrimaryPath          int                `json:"primary_path,omitempty"`
	Open                 bool               `json:"open"`
}

type ProjectWorktreeRequest struct {
	RequestID            string             `json:"request_id"`
	ProjectID            string             `json:"project_id,omitempty"`
	HomeInstallation     string             `json:"home_installation,omitempty"`
	Name                 string             `json:"name"`
	Brief                string             `json:"brief,omitempty"`
	PredecessorProjectID string             `json:"predecessor_project_id,omitempty"`
	Repository           string             `json:"repository"`
	MergeBase            string             `json:"merge_base,omitempty"`
	Destination          string             `json:"destination"`
	Branch               string             `json:"branch"`
	AdditionalPaths      []ProjectPathInput `json:"additional_paths,omitempty"`
	PrimaryPath          int                `json:"primary_path,omitempty"`
	Open                 bool               `json:"open"`
}

type ProjectWorktreeOperation struct {
	ID                   string                 `json:"id"`
	ProjectID            string                 `json:"project_id"`
	Request              ProjectWorktreeRequest `json:"request"`
	CanonicalRepository  string                 `json:"canonical_repository"`
	CanonicalDestination string                 `json:"canonical_destination"`
	State                string                 `json:"state"`
	LastError            string                 `json:"last_error,omitempty"`
	CreatedAt            time.Time              `json:"created_at"`
	UpdatedAt            time.Time              `json:"updated_at"`
}

type ProjectWorktreeProvisioner interface {
	ProvisionProjectWorktree(context.Context, ProjectWorktreeRequest) (Project, error)
}

type ActivateProjectAssignmentRequest struct {
	ThreadID        string `json:"thread_id,omitempty"`
	Harness         string `json:"harness,omitempty"`
	ExternalThread  string `json:"external_thread,omitempty"`
	LaunchDirectory string `json:"launch_directory,omitempty"`
}

type ProjectConflict struct {
	RequestedProjectID string `json:"requested_project_id"`
	RequestedDisplay   string `json:"requested_display_path,omitempty"`
	RequestedPath      string `json:"requested_path"`
	ConflictingProject string `json:"conflicting_project_id"`
	ConflictingDisplay string `json:"conflicting_display_path,omitempty"`
	ConflictingPath    string `json:"conflicting_path"`
	Overlap            string `json:"overlap"`
}

func (e *ProjectConflict) Error() string {
	return fmt.Sprintf("%s: %s is %s of %s claimed by project %s", ErrResourceConflict, e.RequestedPath, e.Overlap, e.ConflictingPath, e.ConflictingProject)
}
func (e *ProjectConflict) Unwrap() error { return ErrResourceConflict }

type StaleProjectHead struct {
	ProjectID string
	Expected  string
	Current   string
}

func (e *StaleProjectHead) Error() string {
	return fmt.Sprintf("%s: project %s expected %s, current %s", ErrProjectStale, e.ProjectID, e.Expected, e.Current)
}
func (e *StaleProjectHead) Unwrap() error { return ErrProjectStale }

// ProjectOperations is the daemon-owned project mutation boundary. Every
// mutation except creation compares the caller's observed head event.
type ProjectOperations interface {
	CreateProject(context.Context, CreateProjectRequest) (Project, error)
	GetProject(context.Context, string) (Project, error)
	ListProjects(context.Context, bool) ([]Project, error)
	ListProjectThreads(context.Context, string) ([]ProjectThread, error)
	OpenProject(context.Context, string, string) (Project, error)
	BeginCloseProject(context.Context, string, string) (Project, error)
	FinalizeCloseProject(context.Context, string, string, bool, string) (Project, error)
	SetProjectArchived(context.Context, string, string, bool) (Project, error)
	UpdateProjectMetadata(context.Context, string, string, string, string) (Project, error)
	AddProjectPath(context.Context, string, string, ProjectPathInput, bool) (Project, error)
	RemoveProjectResource(context.Context, string, string, string) (Project, error)
	ReplaceProjectPath(context.Context, string, string, string, ProjectPathInput) (Project, error)
	SetProjectPrimaryResource(context.Context, string, string, string) (Project, error)
	CheckProjectResource(context.Context, string, string) (ProjectResource, error)
	AssignProject(context.Context, string, string, string) (Project, error)
	ActivateProjectAssignment(context.Context, string, string, ActivateProjectAssignmentRequest) (Project, error)
	AbortProjectAssignment(context.Context, string, string, string) (Project, error)
	BlockProjectAssignment(context.Context, string, string, string) (Project, error)
	UnassignProject(context.Context, string, string, bool, string) (Project, error)
}

// ProjectDeliveryOperations is used only by a daemon-owned runtime bridge.
// It binds claims and immutable dispatch facts to the runnable assignment.
type ProjectDeliveryOperations interface {
	ClaimProjectMessage(context.Context, string, string, string, string) (ProjectDelivery, error)
	MarkProjectDispatchUncertain(context.Context, string, string) error
	RecordProjectDispatch(context.Context, string, string) error
	ReleaseProjectMessage(context.Context, string, string) error
}

type ProjectOutputOperations interface {
	CreateProjectOutput(context.Context, ProjectOutputBinding, model.Message) error
}

type ProjectCommandOperations interface {
	QueueProjectCommand(context.Context, ProjectCommand) (Project, error)
	QueueProjectWorktreeProvision(context.Context, ProjectWorktreeRequest) (Project, bool, error)
}

// ProjectWorkflowOperations persists runtime-crossing saga state so daemon
// restart can compensate an unfinished activation deterministically.
type ProjectWorkflowOperations interface {
	ObserveProjectResources(context.Context, string, string) (Project, error)
	BeginProjectActivation(context.Context, string, string, string, string) (ProjectActivationOperation, error)
	SetProjectActivationAssignment(context.Context, string, string) error
	CompleteProjectActivation(context.Context, string) error
	FailProjectActivation(context.Context, string, string) error
	ListIncompleteProjectActivations(context.Context) ([]ProjectActivationOperation, error)
	BeginProjectRuntimeOperation(context.Context, ProjectRuntimeOperation) (ProjectRuntimeOperation, error)
	AdvanceProjectRuntimeOperation(context.Context, string, string, string, string) error
	BeginProjectWorktreeProvision(context.Context, ProjectWorktreeRequest) (ProjectWorktreeOperation, error)
	AdvanceProjectWorktreeProvision(context.Context, string, string, string) error
	BeginAgentRetirement(context.Context, AgentRetirementOperation) (AgentRetirementOperation, error)
	AdvanceAgentRetirement(context.Context, string, string, string) error
}

type AgentRetirementOperation struct {
	ID        string    `json:"id"`
	AgentName string    `json:"agent_name"`
	ProjectID string    `json:"project_id,omitempty"`
	Force     bool      `json:"force,omitempty"`
	State     string    `json:"state"`
	LastError string    `json:"last_error,omitempty"`
	CreatedAt time.Time `json:"created_at"`
	UpdatedAt time.Time `json:"updated_at"`
}

type projectProvisioningContextKey struct{}

func WithProjectProvisioning(ctx context.Context, operationID string) context.Context {
	return context.WithValue(ctx, projectProvisioningContextKey{}, operationID)
}

func ProjectProvisioningFromContext(ctx context.Context) string {
	value, _ := ctx.Value(projectProvisioningContextKey{}).(string)
	return value
}

// ProjectRuntimeOperation is the durable receipt for a close or handoff saga.
// Request fields are immutable; State and CurrentHead advance after each
// authoritative boundary so a retried command can safely continue.
type ProjectRuntimeOperation struct {
	ID           string    `json:"id"`
	Kind         string    `json:"kind"`
	ProjectID    string    `json:"project_id"`
	ExpectedHead string    `json:"expected_head_event_id"`
	CurrentHead  string    `json:"current_head_event_id"`
	TargetAgent  string    `json:"target_agent,omitempty"`
	Force        bool      `json:"force,omitempty"`
	Archive      bool      `json:"archive,omitempty"`
	State        string    `json:"state"`
	LastError    string    `json:"last_error,omitempty"`
	CreatedAt    time.Time `json:"created_at"`
	UpdatedAt    time.Time `json:"updated_at"`
}
