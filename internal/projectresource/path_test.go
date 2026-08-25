package projectresource

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"github.com/wbbradley/hq/internal/domain"
)

func TestPathReleaseInspectorClassifiesGitState(t *testing.T) {
	repository := t.TempDir()
	if output, err := exec.Command("git", "-C", repository, "init", "-q").CombinedOutput(); err != nil {
		t.Fatalf("git init: %v: %s", err, output)
	}
	inspector := PathReleaseInspector{}
	resource := domain.ProjectResource{ID: "path", Kind: "path", DisplayLocator: repository}
	if got := inspector.AssessRelease(context.Background(), resource); got.State != domain.ResourceReleaseClean || got.Identity == "" {
		t.Fatalf("clean assessment = %#v", got)
	}
	if err := os.WriteFile(filepath.Join(repository, "untracked.txt"), []byte("work"), 0o600); err != nil {
		t.Fatal(err)
	}
	if got := inspector.AssessRelease(context.Background(), resource); got.State != domain.ResourceReleaseDirty || len(got.Details) == 0 {
		t.Fatalf("dirty assessment = %#v", got)
	}
}

func TestPathReleaseInspectorTreatsNonGitPathAsNotApplicable(t *testing.T) {
	resource := domain.ProjectResource{ID: "path", Kind: "path", DisplayLocator: t.TempDir()}
	if got := (PathReleaseInspector{}).AssessRelease(context.Background(), resource); got.State != domain.ResourceReleaseNotApplicable {
		t.Fatalf("assessment = %#v", got)
	}
}

func TestPathReleaseInspectorTreatsMissingPathAsUnknown(t *testing.T) {
	resource := domain.ProjectResource{ID: "path", Kind: "path", DisplayLocator: filepath.Join(t.TempDir(), "missing")}
	if got := (PathReleaseInspector{}).AssessRelease(context.Background(), resource); got.State != domain.ResourceReleaseUnknown {
		t.Fatalf("assessment = %#v", got)
	}
}
