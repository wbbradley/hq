package domain

import (
	"context"
	"errors"
	"reflect"
	"testing"
)

func projectCommandSamples() []ProjectCommandData {
	return []ProjectCommandData{
		ProjectCreateCommand{Name: "create"},
		ProjectOpenCommand{},
		ProjectArchiveSetCommand{Archived: true},
		ProjectMetadataUpdateCommand{Name: "renamed", Brief: "brief"},
		ProjectCloseBeginCommand{},
		ProjectCloseFinalizeCommand{Forced: true, RuntimeObservation: "stopped"},
		ProjectResourceAddCommand{Path: ProjectPathInput{DisplayPath: "/repo"}, Primary: true},
		ProjectResourceRemoveCommand{ResourceID: "remove"},
		ProjectResourceReplaceCommand{ResourceID: "old", Path: ProjectPathInput{DisplayPath: "/new"}},
		ProjectResourcePrimaryCommand{ResourceID: "primary"},
		ProjectResourceCheckCommand{ResourceID: "check"},
		ProjectAssignmentAssignCommand{Agent: "alice"},
		ProjectAssignmentActivateCommand{Harness: "codex", ExternalThread: "thread", LaunchDirectory: "/repo"},
		ProjectAssignmentAbortCommand{Diagnostic: "abort"},
		ProjectAssignmentBlockCommand{Diagnostic: "block"},
		ProjectAssignmentUnassignCommand{Forced: true, RuntimeObservation: "stopped"},
		ProjectCodexActivateCommand{AgentName: "alice"},
		ProjectCodexCloseCommand{Force: true},
		ProjectCodexHandoffCommand{NewAgentName: "bob"},
		ProjectProvisionWorktreeCommand{Name: "worktree", Destination: "/worktree"},
	}
}

func TestProjectCommandRegistryHasTypedCodecForEveryOperation(t *testing.T) {
	samples := projectCommandSamples()
	definitions := ProjectCommandDefinitions()
	if len(samples) != len(definitions) {
		t.Fatalf("typed samples=%d registry definitions=%d", len(samples), len(definitions))
	}
	seen := make(map[ProjectCommandOperation]bool, len(samples))
	for _, sample := range samples {
		operation, body, err := EncodeProjectCommand(sample)
		if err != nil {
			t.Fatalf("encode %T: %v", sample, err)
		}
		decoded, err := DecodeProjectCommand(operation, body)
		if err != nil {
			t.Fatalf("decode %s: %v", operation, err)
		}
		if decoded.Operation() != operation || reflect.TypeOf(decoded).Elem() != reflect.TypeOf(sample) {
			t.Fatalf("round trip %s = %T from %T", operation, decoded, sample)
		}
		if seen[operation] {
			t.Fatalf("duplicate typed sample for %s", operation)
		}
		seen[operation] = true
	}
	for _, definition := range definitions {
		if !seen[definition.Operation] {
			t.Fatalf("registry operation %s has no typed round-trip sample", definition.Operation)
		}
		if definition.RequiresRuntime != (definition.Operation == ProjectCommandCodexActivate || definition.Operation == ProjectCommandCodexClose || definition.Operation == ProjectCommandCodexHandoff || definition.Operation == ProjectCommandProvisionWorktree) {
			t.Fatalf("runtime metadata for %s = %t", definition.Operation, definition.RequiresRuntime)
		}
	}
	if _, err := DecodeProjectCommand("project.future", []byte(`{}`)); err == nil {
		t.Fatal("unknown project command decoded")
	}
}

type recordingProjectCommandTarget struct{ calls []ProjectCommandOperation }

func (target *recordingProjectCommandTarget) record(operation ProjectCommandOperation) (Project, error) {
	target.calls = append(target.calls, operation)
	return Project{}, nil
}
func (target *recordingProjectCommandTarget) CreateProject(context.Context, CreateProjectRequest) (Project, error) {
	return target.record(ProjectCommandCreate)
}
func (target *recordingProjectCommandTarget) GetProject(context.Context, string) (Project, error) {
	return Project{}, nil
}
func (target *recordingProjectCommandTarget) ListProjects(context.Context, bool) ([]Project, error) {
	return nil, nil
}
func (target *recordingProjectCommandTarget) ListProjectThreads(context.Context, string) ([]ProjectThread, error) {
	return nil, nil
}
func (target *recordingProjectCommandTarget) OpenProject(context.Context, string, string) (Project, error) {
	return target.record(ProjectCommandOpen)
}
func (target *recordingProjectCommandTarget) BeginCloseProject(context.Context, string, string) (Project, error) {
	return target.record(ProjectCommandCloseBegin)
}
func (target *recordingProjectCommandTarget) FinalizeCloseProject(context.Context, string, string, bool, string) (Project, error) {
	return target.record(ProjectCommandCloseFinalize)
}
func (target *recordingProjectCommandTarget) SetProjectArchived(context.Context, string, string, bool) (Project, error) {
	return target.record(ProjectCommandArchiveSet)
}
func (target *recordingProjectCommandTarget) UpdateProjectMetadata(context.Context, string, string, string, string) (Project, error) {
	return target.record(ProjectCommandMetadataUpdate)
}
func (target *recordingProjectCommandTarget) AddProjectPath(context.Context, string, string, ProjectPathInput, bool) (Project, error) {
	return target.record(ProjectCommandResourceAdd)
}
func (target *recordingProjectCommandTarget) RemoveProjectResource(context.Context, string, string, string) (Project, error) {
	return target.record(ProjectCommandResourceRemove)
}
func (target *recordingProjectCommandTarget) ReplaceProjectPath(context.Context, string, string, string, ProjectPathInput) (Project, error) {
	return target.record(ProjectCommandResourceReplace)
}
func (target *recordingProjectCommandTarget) SetProjectPrimaryResource(context.Context, string, string, string) (Project, error) {
	return target.record(ProjectCommandResourcePrimary)
}
func (target *recordingProjectCommandTarget) CheckProjectResource(context.Context, string, string) (ProjectResource, error) {
	target.calls = append(target.calls, ProjectCommandResourceCheck)
	return ProjectResource{}, nil
}
func (target *recordingProjectCommandTarget) AssignProject(context.Context, string, string, string) (Project, error) {
	return target.record(ProjectCommandAssignmentAssign)
}
func (target *recordingProjectCommandTarget) ActivateProjectAssignment(context.Context, string, string, ActivateProjectAssignmentRequest) (Project, error) {
	return target.record(ProjectCommandAssignmentActivate)
}
func (target *recordingProjectCommandTarget) AbortProjectAssignment(context.Context, string, string, string) (Project, error) {
	return target.record(ProjectCommandAssignmentAbort)
}
func (target *recordingProjectCommandTarget) BlockProjectAssignment(context.Context, string, string, string) (Project, error) {
	return target.record(ProjectCommandAssignmentBlock)
}
func (target *recordingProjectCommandTarget) UnassignProject(context.Context, string, string, bool, string) (Project, error) {
	return target.record(ProjectCommandAssignmentUnassign)
}

func TestProjectCommandRegistryExecutesEveryLocalOperation(t *testing.T) {
	target := &recordingProjectCommandTarget{}
	for _, sample := range projectCommandSamples() {
		operation, body, err := EncodeProjectCommand(sample)
		if err != nil {
			t.Fatal(err)
		}
		decoded, err := DecodeProjectCommand(operation, body)
		if err != nil {
			t.Fatal(err)
		}
		_, err = ExecuteProjectCommand(context.Background(), target, ProjectCommand{Operation: operation}, decoded)
		if ProjectCommandRequiresRuntime(operation) {
			if !errors.Is(err, ErrProjectCommandRequiresRuntime) {
				t.Fatalf("runtime operation %s execution = %v", operation, err)
			}
			continue
		}
		if err != nil || len(target.calls) == 0 || target.calls[len(target.calls)-1] != operation {
			t.Fatalf("local operation %s calls=%v err=%v", operation, target.calls, err)
		}
	}
}
