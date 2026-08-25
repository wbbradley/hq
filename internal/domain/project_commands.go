package domain

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
)

type ProjectCommandOperation string

const (
	ProjectCommandCreate             ProjectCommandOperation = "project.create"
	ProjectCommandOpen               ProjectCommandOperation = "project.open"
	ProjectCommandArchiveSet         ProjectCommandOperation = "project.archive.set"
	ProjectCommandMetadataUpdate     ProjectCommandOperation = "project.metadata.update"
	ProjectCommandCloseBegin         ProjectCommandOperation = "project.close.begin"
	ProjectCommandCloseFinalize      ProjectCommandOperation = "project.close.finalize"
	ProjectCommandResourceAdd        ProjectCommandOperation = "project.resource.add"
	ProjectCommandResourceRemove     ProjectCommandOperation = "project.resource.remove"
	ProjectCommandResourceReplace    ProjectCommandOperation = "project.resource.replace"
	ProjectCommandResourcePrimary    ProjectCommandOperation = "project.resource.primary"
	ProjectCommandResourceCheck      ProjectCommandOperation = "project.resource.check"
	ProjectCommandAssignmentAssign   ProjectCommandOperation = "project.assignment.assign"
	ProjectCommandAssignmentActivate ProjectCommandOperation = "project.assignment.activate"
	ProjectCommandAssignmentAbort    ProjectCommandOperation = "project.assignment.abort"
	ProjectCommandAssignmentBlock    ProjectCommandOperation = "project.assignment.block"
	ProjectCommandAssignmentUnassign ProjectCommandOperation = "project.assignment.unassign"
	ProjectCommandHarnessActivate    ProjectCommandOperation = "harness.project.activate"
	ProjectCommandHarnessClose       ProjectCommandOperation = "harness.project.close"
	ProjectCommandHarnessHandoff     ProjectCommandOperation = "harness.project.handoff"
	ProjectCommandProvisionWorktree  ProjectCommandOperation = "project.provision-worktree"
)

var ErrProjectCommandRequiresRuntime = errors.New("project command requires the daemon runtime")

type ProjectCommandData interface {
	Operation() ProjectCommandOperation
	projectCommandData()
}

type ProjectCreateCommand CreateProjectRequest
type ProjectOpenCommand struct{}
type ProjectArchiveSetCommand struct {
	Archived bool `json:"archived"`
}
type ProjectMetadataUpdateCommand struct {
	Name  string `json:"name"`
	Brief string `json:"brief"`
}
type ProjectCloseBeginCommand struct{}
type ProjectCloseFinalizeCommand struct {
	Forced             bool   `json:"forced"`
	RuntimeObservation string `json:"runtime_observation"`
}
type ProjectResourceAddCommand struct {
	Path    ProjectPathInput `json:"path"`
	Primary bool             `json:"primary"`
}
type ProjectResourceRemoveCommand struct {
	ResourceID string `json:"resource_id"`
}
type ProjectResourcePrimaryCommand struct {
	ResourceID string `json:"resource_id"`
}
type ProjectResourceCheckCommand struct {
	ResourceID string `json:"resource_id"`
}
type ProjectResourceReplaceCommand struct {
	ResourceID string           `json:"resource_id"`
	Path       ProjectPathInput `json:"path"`
}
type ProjectAssignmentAssignCommand struct {
	Agent string `json:"agent"`
}
type ProjectAssignmentActivateCommand ActivateProjectAssignmentRequest
type ProjectAssignmentAbortCommand struct {
	Diagnostic string `json:"diagnostic"`
}
type ProjectAssignmentBlockCommand struct {
	Diagnostic string `json:"diagnostic"`
}
type ProjectAssignmentUnassignCommand struct {
	Forced             bool   `json:"forced"`
	RuntimeObservation string `json:"runtime_observation"`
}
type ProjectHarnessActivateCommand ProjectHarnessActivationRequest
type ProjectHarnessCloseCommand ProjectHarnessCloseRequest
type ProjectHarnessHandoffCommand ProjectHarnessHandoffRequest
type ProjectProvisionWorktreeCommand ProjectWorktreeRequest

func (ProjectCreateCommand) Operation() ProjectCommandOperation     { return ProjectCommandCreate }
func (ProjectOpenCommand) Operation() ProjectCommandOperation       { return ProjectCommandOpen }
func (ProjectArchiveSetCommand) Operation() ProjectCommandOperation { return ProjectCommandArchiveSet }
func (ProjectMetadataUpdateCommand) Operation() ProjectCommandOperation {
	return ProjectCommandMetadataUpdate
}
func (ProjectCloseBeginCommand) Operation() ProjectCommandOperation { return ProjectCommandCloseBegin }
func (ProjectCloseFinalizeCommand) Operation() ProjectCommandOperation {
	return ProjectCommandCloseFinalize
}
func (ProjectResourceAddCommand) Operation() ProjectCommandOperation {
	return ProjectCommandResourceAdd
}
func (ProjectResourceRemoveCommand) Operation() ProjectCommandOperation {
	return ProjectCommandResourceRemove
}
func (ProjectResourcePrimaryCommand) Operation() ProjectCommandOperation {
	return ProjectCommandResourcePrimary
}
func (ProjectResourceCheckCommand) Operation() ProjectCommandOperation {
	return ProjectCommandResourceCheck
}
func (ProjectResourceReplaceCommand) Operation() ProjectCommandOperation {
	return ProjectCommandResourceReplace
}
func (ProjectAssignmentAssignCommand) Operation() ProjectCommandOperation {
	return ProjectCommandAssignmentAssign
}
func (ProjectAssignmentActivateCommand) Operation() ProjectCommandOperation {
	return ProjectCommandAssignmentActivate
}
func (ProjectAssignmentAbortCommand) Operation() ProjectCommandOperation {
	return ProjectCommandAssignmentAbort
}
func (ProjectAssignmentBlockCommand) Operation() ProjectCommandOperation {
	return ProjectCommandAssignmentBlock
}
func (ProjectAssignmentUnassignCommand) Operation() ProjectCommandOperation {
	return ProjectCommandAssignmentUnassign
}
func (ProjectHarnessActivateCommand) Operation() ProjectCommandOperation {
	return ProjectCommandHarnessActivate
}
func (ProjectHarnessCloseCommand) Operation() ProjectCommandOperation {
	return ProjectCommandHarnessClose
}
func (ProjectHarnessHandoffCommand) Operation() ProjectCommandOperation {
	return ProjectCommandHarnessHandoff
}
func (ProjectProvisionWorktreeCommand) Operation() ProjectCommandOperation {
	return ProjectCommandProvisionWorktree
}

func (ProjectCreateCommand) projectCommandData()             {}
func (ProjectOpenCommand) projectCommandData()               {}
func (ProjectArchiveSetCommand) projectCommandData()         {}
func (ProjectMetadataUpdateCommand) projectCommandData()     {}
func (ProjectCloseBeginCommand) projectCommandData()         {}
func (ProjectCloseFinalizeCommand) projectCommandData()      {}
func (ProjectResourceAddCommand) projectCommandData()        {}
func (ProjectResourceRemoveCommand) projectCommandData()     {}
func (ProjectResourcePrimaryCommand) projectCommandData()    {}
func (ProjectResourceCheckCommand) projectCommandData()      {}
func (ProjectResourceReplaceCommand) projectCommandData()    {}
func (ProjectAssignmentAssignCommand) projectCommandData()   {}
func (ProjectAssignmentActivateCommand) projectCommandData() {}
func (ProjectAssignmentAbortCommand) projectCommandData()    {}
func (ProjectAssignmentBlockCommand) projectCommandData()    {}
func (ProjectAssignmentUnassignCommand) projectCommandData() {}
func (ProjectHarnessActivateCommand) projectCommandData()    {}
func (ProjectHarnessCloseCommand) projectCommandData()       {}
func (ProjectHarnessHandoffCommand) projectCommandData()     {}
func (ProjectProvisionWorktreeCommand) projectCommandData()  {}

type ProjectCommandDefinition struct {
	Operation       ProjectCommandOperation
	CreatesProject  bool
	RequiresRuntime bool
}

type registeredProjectCommand struct {
	definition ProjectCommandDefinition
	newData    func() ProjectCommandData
	execute    func(context.Context, ProjectOperations, ProjectCommand, ProjectCommandData) (Project, error)
}

var projectCommandRegistry = buildProjectCommandRegistry()

func buildProjectCommandRegistry() map[ProjectCommandOperation]registeredProjectCommand {
	registry := make(map[ProjectCommandOperation]registeredProjectCommand)
	local := func(operation ProjectCommandOperation, newData func() ProjectCommandData, execute func(context.Context, ProjectOperations, ProjectCommand, ProjectCommandData) (Project, error)) {
		if _, exists := registry[operation]; exists {
			panic("duplicate project command registration: " + operation)
		}
		registry[operation] = registeredProjectCommand{definition: ProjectCommandDefinition{Operation: operation}, newData: newData, execute: execute}
	}
	runtime := func(operation ProjectCommandOperation, creates bool, newData func() ProjectCommandData) {
		if _, exists := registry[operation]; exists {
			panic("duplicate project command registration: " + operation)
		}
		registry[operation] = registeredProjectCommand{definition: ProjectCommandDefinition{Operation: operation, CreatesProject: creates, RequiresRuntime: true}, newData: newData}
	}
	local(ProjectCommandCreate, func() ProjectCommandData { return &ProjectCreateCommand{} }, func(ctx context.Context, target ProjectOperations, command ProjectCommand, data ProjectCommandData) (Project, error) {
		request := CreateProjectRequest(*data.(*ProjectCreateCommand))
		request.ID, request.HomeInstallation = command.ProjectID, command.HomeInstallation
		return target.CreateProject(ctx, request)
	})
	local(ProjectCommandOpen, func() ProjectCommandData { return &ProjectOpenCommand{} }, func(ctx context.Context, target ProjectOperations, command ProjectCommand, _ ProjectCommandData) (Project, error) {
		return target.OpenProject(ctx, command.ProjectID, command.ExpectedHead)
	})
	local(ProjectCommandArchiveSet, func() ProjectCommandData { return &ProjectArchiveSetCommand{} }, func(ctx context.Context, target ProjectOperations, command ProjectCommand, data ProjectCommandData) (Project, error) {
		return target.SetProjectArchived(ctx, command.ProjectID, command.ExpectedHead, data.(*ProjectArchiveSetCommand).Archived)
	})
	local(ProjectCommandMetadataUpdate, func() ProjectCommandData { return &ProjectMetadataUpdateCommand{} }, func(ctx context.Context, target ProjectOperations, command ProjectCommand, data ProjectCommandData) (Project, error) {
		value := data.(*ProjectMetadataUpdateCommand)
		return target.UpdateProjectMetadata(ctx, command.ProjectID, command.ExpectedHead, value.Name, value.Brief)
	})
	local(ProjectCommandCloseBegin, func() ProjectCommandData { return &ProjectCloseBeginCommand{} }, func(ctx context.Context, target ProjectOperations, command ProjectCommand, _ ProjectCommandData) (Project, error) {
		return target.BeginCloseProject(ctx, command.ProjectID, command.ExpectedHead)
	})
	local(ProjectCommandCloseFinalize, func() ProjectCommandData { return &ProjectCloseFinalizeCommand{} }, func(ctx context.Context, target ProjectOperations, command ProjectCommand, data ProjectCommandData) (Project, error) {
		value := data.(*ProjectCloseFinalizeCommand)
		return target.FinalizeCloseProject(ctx, command.ProjectID, command.ExpectedHead, value.Forced, value.RuntimeObservation)
	})
	local(ProjectCommandResourceAdd, func() ProjectCommandData { return &ProjectResourceAddCommand{} }, func(ctx context.Context, target ProjectOperations, command ProjectCommand, data ProjectCommandData) (Project, error) {
		value := data.(*ProjectResourceAddCommand)
		return target.AddProjectPath(ctx, command.ProjectID, command.ExpectedHead, value.Path, value.Primary)
	})
	local(ProjectCommandResourceRemove, func() ProjectCommandData { return &ProjectResourceRemoveCommand{} }, func(ctx context.Context, target ProjectOperations, command ProjectCommand, data ProjectCommandData) (Project, error) {
		return target.RemoveProjectResource(ctx, command.ProjectID, command.ExpectedHead, data.(*ProjectResourceRemoveCommand).ResourceID)
	})
	local(ProjectCommandResourcePrimary, func() ProjectCommandData { return &ProjectResourcePrimaryCommand{} }, func(ctx context.Context, target ProjectOperations, command ProjectCommand, data ProjectCommandData) (Project, error) {
		return target.SetProjectPrimaryResource(ctx, command.ProjectID, command.ExpectedHead, data.(*ProjectResourcePrimaryCommand).ResourceID)
	})
	local(ProjectCommandResourceCheck, func() ProjectCommandData { return &ProjectResourceCheckCommand{} }, func(ctx context.Context, target ProjectOperations, command ProjectCommand, data ProjectCommandData) (Project, error) {
		if _, err := target.CheckProjectResource(ctx, command.ProjectID, data.(*ProjectResourceCheckCommand).ResourceID); err != nil {
			return Project{}, err
		}
		return target.GetProject(ctx, command.ProjectID)
	})
	local(ProjectCommandResourceReplace, func() ProjectCommandData { return &ProjectResourceReplaceCommand{} }, func(ctx context.Context, target ProjectOperations, command ProjectCommand, data ProjectCommandData) (Project, error) {
		value := data.(*ProjectResourceReplaceCommand)
		return target.ReplaceProjectPath(ctx, command.ProjectID, command.ExpectedHead, value.ResourceID, value.Path)
	})
	local(ProjectCommandAssignmentAssign, func() ProjectCommandData { return &ProjectAssignmentAssignCommand{} }, func(ctx context.Context, target ProjectOperations, command ProjectCommand, data ProjectCommandData) (Project, error) {
		return target.AssignProject(ctx, command.ProjectID, command.ExpectedHead, data.(*ProjectAssignmentAssignCommand).Agent)
	})
	local(ProjectCommandAssignmentActivate, func() ProjectCommandData { return &ProjectAssignmentActivateCommand{} }, func(ctx context.Context, target ProjectOperations, command ProjectCommand, data ProjectCommandData) (Project, error) {
		return target.ActivateProjectAssignment(ctx, command.ProjectID, command.ExpectedHead, ActivateProjectAssignmentRequest(*data.(*ProjectAssignmentActivateCommand)))
	})
	local(ProjectCommandAssignmentAbort, func() ProjectCommandData { return &ProjectAssignmentAbortCommand{} }, func(ctx context.Context, target ProjectOperations, command ProjectCommand, data ProjectCommandData) (Project, error) {
		return target.AbortProjectAssignment(ctx, command.ProjectID, command.ExpectedHead, data.(*ProjectAssignmentAbortCommand).Diagnostic)
	})
	local(ProjectCommandAssignmentBlock, func() ProjectCommandData { return &ProjectAssignmentBlockCommand{} }, func(ctx context.Context, target ProjectOperations, command ProjectCommand, data ProjectCommandData) (Project, error) {
		return target.BlockProjectAssignment(ctx, command.ProjectID, command.ExpectedHead, data.(*ProjectAssignmentBlockCommand).Diagnostic)
	})
	local(ProjectCommandAssignmentUnassign, func() ProjectCommandData { return &ProjectAssignmentUnassignCommand{} }, func(ctx context.Context, target ProjectOperations, command ProjectCommand, data ProjectCommandData) (Project, error) {
		value := data.(*ProjectAssignmentUnassignCommand)
		return target.UnassignProject(ctx, command.ProjectID, command.ExpectedHead, value.Forced, value.RuntimeObservation)
	})
	runtime(ProjectCommandHarnessActivate, false, func() ProjectCommandData { return &ProjectHarnessActivateCommand{} })
	runtime(ProjectCommandHarnessClose, false, func() ProjectCommandData { return &ProjectHarnessCloseCommand{} })
	runtime(ProjectCommandHarnessHandoff, false, func() ProjectCommandData { return &ProjectHarnessHandoffCommand{} })
	runtime(ProjectCommandProvisionWorktree, true, func() ProjectCommandData { return &ProjectProvisionWorktreeCommand{} })
	registry[ProjectCommandCreate] = withCreation(registry[ProjectCommandCreate])
	return registry
}

func withCreation(item registeredProjectCommand) registeredProjectCommand {
	item.definition.CreatesProject = true
	return item
}

func ProjectCommandDefinitions() []ProjectCommandDefinition {
	result := make([]ProjectCommandDefinition, 0, len(projectCommandRegistry))
	for _, item := range projectCommandRegistry {
		result = append(result, item.definition)
	}
	sort.Slice(result, func(i, j int) bool { return result[i].Operation < result[j].Operation })
	return result
}

func DecodeProjectCommand(operation ProjectCommandOperation, body []byte) (ProjectCommandData, error) {
	registered, ok := projectCommandRegistry[operation]
	if !ok {
		return nil, fmt.Errorf("unsupported project command operation %q", operation)
	}
	data := registered.newData()
	if len(body) == 0 || !json.Valid(body) {
		return nil, fmt.Errorf("project command %q body must be valid JSON", operation)
	}
	if err := json.Unmarshal(body, data); err != nil {
		return nil, fmt.Errorf("decode project command %q: %w", operation, err)
	}
	return data, nil
}

func EncodeProjectCommand(data ProjectCommandData) (ProjectCommandOperation, []byte, error) {
	if data == nil {
		return "", nil, errors.New("project command data is required")
	}
	operation := data.Operation()
	if _, ok := projectCommandRegistry[operation]; !ok {
		return "", nil, fmt.Errorf("unsupported project command operation %q", operation)
	}
	body, err := json.Marshal(data)
	return operation, body, err
}

func ExecuteProjectCommand(ctx context.Context, target ProjectOperations, command ProjectCommand, data ProjectCommandData) (Project, error) {
	registered, ok := projectCommandRegistry[command.Operation]
	if !ok {
		return Project{}, fmt.Errorf("unsupported project command operation %q", command.Operation)
	}
	if data == nil || data.Operation() != command.Operation {
		return Project{}, errors.New("project command data does not match its operation")
	}
	if registered.definition.RequiresRuntime {
		return Project{}, ErrProjectCommandRequiresRuntime
	}
	return registered.execute(ctx, target, command, data)
}

func ProjectCommandRequiresRuntime(operation ProjectCommandOperation) bool {
	item, ok := projectCommandRegistry[operation]
	return ok && item.definition.RequiresRuntime
}

func ProjectCommandCreatesProject(operation ProjectCommandOperation) bool {
	item, ok := projectCommandRegistry[operation]
	return ok && item.definition.CreatesProject
}
