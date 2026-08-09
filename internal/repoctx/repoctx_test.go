package repoctx

import (
	"context"
	"os/exec"
	"path/filepath"
	"testing"
)

func TestParseRemote(t *testing.T) {
	tests := []struct {
		remote string
		host   string
		owner  string
		repo   string
	}{
		{"git@github.com:wbbradley/hq.git", "github.com", "wbbradley", "hq"},
		{"https://github.com/wbbradley/hq.git", "github.com", "wbbradley", "hq"},
		{"ssh://git@github.example.com/team/tool.git", "github.example.com", "team", "tool"},
	}
	for _, tt := range tests {
		host, owner, repo, err := parseRemote(tt.remote)
		if err != nil {
			t.Errorf("parseRemote(%q): %v", tt.remote, err)
			continue
		}
		if host != tt.host || owner != tt.owner || repo != tt.repo {
			t.Errorf("parseRemote(%q) = %q, %q, %q", tt.remote, host, owner, repo)
		}
	}
}

func TestCompactRemote(t *testing.T) {
	tests := map[string]string{
		"git@github.com:wbbradley/hq.git":                 "wbbradley/hq",
		"https://github.com/wbbradley/hq":                 "wbbradley/hq",
		"ssh://git@gitlab.com/group/subgroup/project.git": "group/subgroup/project",
		"https://example.com/team/tool.git":               "https://example.com/team/tool.git",
	}
	for remote, want := range tests {
		if got := compactRemote(remote); got != want {
			t.Errorf("compactRemote(%q) = %q, want %q", remote, got, want)
		}
	}
}

func TestRemotesUseMainRepositoryConfigFromWorktree(t *testing.T) {
	ctx := context.Background()
	root := filepath.Join(t.TempDir(), "main")
	worktree := filepath.Join(t.TempDir(), "linked")
	runGit(t, "init", root)
	runGit(t, "-C", root, "config", "user.email", "test@example.com")
	runGit(t, "-C", root, "config", "user.name", "Test")
	runGit(t, "-C", root, "commit", "--allow-empty", "-m", "test")
	runGit(t, "-C", root, "remote", "add", "origin", "git@github.com:wbbradley/hq.git")
	runGit(t, "-C", root, "worktree", "add", "-b", "linked-test", worktree)

	remotes, err := (GitHub{}).Remotes(ctx, worktree)
	if err != nil {
		t.Fatal(err)
	}
	if len(remotes) != 1 || remotes[0].Name != "origin" || remotes[0].Display != "wbbradley/hq" {
		t.Fatalf("remotes = %#v", remotes)
	}
	snapshot := (GitHub{}).Snapshot(ctx, worktree)
	common, err := filepath.EvalSymlinks(filepath.Join(root, ".git"))
	if err != nil {
		t.Fatal(err)
	}
	gotCommon, err := filepath.EvalSymlinks(snapshot.GitCommonDir)
	if err != nil {
		t.Fatal(err)
	}
	if snapshot.Directory != worktree || snapshot.Worktree != worktree || gotCommon != common || snapshot.Branch != "linked-test" || snapshot.RemoteIdentity != "origin: wbbradley/hq" {
		t.Fatalf("snapshot = %#v", snapshot)
	}
}

func runGit(t *testing.T, args ...string) {
	t.Helper()
	if output, err := exec.Command("git", args...).CombinedOutput(); err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, output)
	}
}
