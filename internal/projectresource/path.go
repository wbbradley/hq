package projectresource

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/wbbradley/hq/internal/domain"
)

type CommandRunner func(context.Context, string, ...string) ([]byte, error)

type PathReleaseInspector struct{ RunGit CommandRunner }

func (PathReleaseInspector) Kind() string { return "path" }

func (p PathReleaseInspector) AssessRelease(ctx context.Context, resource domain.ProjectResource) domain.ResourceReleaseAssessment {
	result := domain.ResourceReleaseAssessment{ResourceID: resource.ID, Kind: resource.Kind, Locator: resource.DisplayLocator}
	info, err := os.Stat(resource.DisplayLocator)
	if err != nil {
		result.State, result.Summary = domain.ResourceReleaseUnknown, err.Error()
		return result
	}
	directory := resource.DisplayLocator
	if !info.IsDir() {
		directory = filepath.Dir(directory)
	}
	run := p.RunGit
	if run == nil {
		run = runGit
	}
	top, err := run(ctx, directory, "rev-parse", "--show-toplevel")
	if err != nil {
		if strings.Contains(strings.ToLower(string(top)), "not a git repository") {
			result.State, result.Summary = domain.ResourceReleaseNotApplicable, "path is not in a Git worktree"
		} else {
			result.State, result.Summary = domain.ResourceReleaseUnknown, strings.TrimSpace(string(top))
			if result.Summary == "" {
				result.Summary = err.Error()
			}
		}
		return result
	}
	result.Identity = filepath.Clean(strings.TrimSpace(string(top)))
	status, err := run(ctx, result.Identity, "status", "--porcelain=v1", "--untracked-files=all")
	if err != nil {
		result.State, result.Summary = domain.ResourceReleaseUnknown, err.Error()
		return result
	}
	lines := strings.Split(strings.TrimSpace(string(status)), "\n")
	if len(lines) == 1 && lines[0] == "" {
		result.State, result.Summary = domain.ResourceReleaseClean, "Git worktree is clean"
		return result
	}
	result.State, result.Summary = domain.ResourceReleaseDirty, "Git worktree has staged, modified, deleted, or untracked files"
	result.Details = lines
	return result
}

func runGit(ctx context.Context, directory string, args ...string) ([]byte, error) {
	command := exec.CommandContext(ctx, "git", append([]string{"-C", directory}, args...)...)
	output, err := command.CombinedOutput()
	if errors.Is(ctx.Err(), context.Canceled) {
		return output, ctx.Err()
	}
	return output, err
}
