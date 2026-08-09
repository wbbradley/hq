package repoctx

import (
	"context"
	"errors"
	"fmt"
	"net/url"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/cli/go-gh/v2/pkg/api"
	"github.com/cli/go-gh/v2/pkg/auth"
	"github.com/wbbradley/hq/internal/model"
)

var ErrUnavailable = errors.New("github unavailable")

type PullRequest struct {
	Number int
	Title  string
	URL    string
}

type Remote struct {
	Name    string
	URL     string
	Display string
}

type Provider interface {
	Branch(context.Context, string) (string, error)
	Remotes(context.Context, string) ([]Remote, error)
	PullRequest(context.Context, string, string) (*PullRequest, error)
}

type GitHub struct{}

func (GitHub) Snapshot(ctx context.Context, directory string) model.RepositoryContext {
	result := model.RepositoryContext{Directory: filepath.Clean(directory)}
	if value, err := gitOutput(ctx, directory, "rev-parse", "--path-format=absolute", "--git-common-dir"); err == nil {
		result.GitCommonDir = filepath.Clean(value)
	}
	if value, err := gitOutput(ctx, directory, "rev-parse", "--show-toplevel"); err == nil {
		result.Worktree = filepath.Clean(value)
	}
	if value, err := (GitHub{}).Branch(ctx, directory); err == nil {
		result.Branch = value
	}
	if remotes, err := (GitHub{}).Remotes(ctx, directory); err == nil {
		values := make([]string, 0, len(remotes))
		for _, remote := range remotes {
			values = append(values, remote.Name+": "+remote.Display)
		}
		sort.Strings(values)
		result.RemoteIdentity = strings.Join(values, " · ")
	}
	return result
}

func gitOutput(ctx context.Context, directory string, args ...string) (string, error) {
	commandArgs := append([]string{"-C", directory}, args...)
	output, err := exec.CommandContext(ctx, "git", commandArgs...).Output()
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(string(output)), nil
}

func (GitHub) Branch(ctx context.Context, directory string) (string, error) {
	output, err := exec.CommandContext(ctx, "git", "-C", directory, "branch", "--show-current").Output()
	if err != nil {
		return "", fmt.Errorf("read git branch: %w", err)
	}
	branch := strings.TrimSpace(string(output))
	if branch != "" {
		return branch, nil
	}
	output, err = exec.CommandContext(ctx, "git", "-C", directory, "rev-parse", "--short", "HEAD").Output()
	if err != nil {
		return "", fmt.Errorf("read git commit: %w", err)
	}
	return "detached@" + strings.TrimSpace(string(output)), nil
}

func (GitHub) Remotes(ctx context.Context, directory string) ([]Remote, error) {
	output, err := exec.CommandContext(ctx, "git", "-C", directory, "remote").Output()
	if err != nil {
		return nil, fmt.Errorf("list git remotes: %w", err)
	}
	names := strings.Fields(string(output))
	remotes := make([]Remote, 0, len(names))
	for _, name := range names {
		output, err := exec.CommandContext(ctx, "git", "-C", directory, "remote", "get-url", name).Output()
		if err != nil {
			return nil, fmt.Errorf("read git remote %s: %w", name, err)
		}
		raw := strings.TrimSpace(string(output))
		remotes = append(remotes, Remote{Name: name, URL: raw, Display: compactRemote(raw)})
	}
	return remotes, nil
}

func (GitHub) PullRequest(ctx context.Context, directory, branch string) (*PullRequest, error) {
	remotes, err := (GitHub{}).Remotes(ctx, directory)
	if err != nil {
		return nil, err
	}
	var remote string
	for _, candidate := range remotes {
		host, _, _, parseErr := parseRemote(candidate.URL)
		if parseErr == nil && host != "gitlab.com" && (remote == "" || candidate.Name == "origin") {
			remote = candidate.URL
			if candidate.Name == "origin" {
				break
			}
		}
	}
	if remote == "" {
		return nil, ErrUnavailable
	}
	host, owner, repo, err := parseRemote(remote)
	if err != nil {
		return nil, err
	}
	token, _ := auth.TokenForHost(host)
	if token == "" {
		return nil, ErrUnavailable
	}
	client, err := api.NewRESTClient(api.ClientOptions{
		Host: host, AuthToken: token, Timeout: 5 * time.Second, LogIgnoreEnv: true,
	})
	if err != nil {
		return nil, fmt.Errorf("create github client: %w", err)
	}
	var response []struct {
		Number  int    `json:"number"`
		Title   string `json:"title"`
		HTMLURL string `json:"html_url"`
	}
	endpoint := fmt.Sprintf("repos/%s/%s/pulls?state=open&head=%s&per_page=1",
		url.PathEscape(owner), url.PathEscape(repo), url.QueryEscape(owner+":"+branch))
	if err := client.Get(endpoint, &response); err != nil {
		return nil, fmt.Errorf("find pull request: %w", err)
	}
	if len(response) == 0 {
		return nil, nil
	}
	return &PullRequest{Number: response[0].Number, Title: response[0].Title, URL: response[0].HTMLURL}, nil
}

func parseRemote(remote string) (host, owner, repo string, err error) {
	host, repoPath, parseErr := remoteParts(remote)
	if parseErr != nil {
		return "", "", "", parseErr
	}
	parts := strings.Split(repoPath, "/")
	if len(parts) != 2 {
		return "", "", "", fmt.Errorf("parse git remote %q", remote)
	}
	return host, parts[0], parts[1], nil
}

func compactRemote(remote string) string {
	host, repoPath, err := remoteParts(remote)
	if err == nil && (host == "github.com" || host == "gitlab.com") {
		return repoPath
	}
	return remote
}

func remoteParts(remote string) (host, repoPath string, err error) {
	if !strings.Contains(remote, "://") && strings.Contains(remote, ":") {
		parts := strings.SplitN(remote, ":", 2)
		host = parts[0]
		if at := strings.LastIndex(host, "@"); at >= 0 {
			host = host[at+1:]
		}
		repoPath = parts[1]
	} else {
		u, parseErr := url.Parse(remote)
		if parseErr != nil || u.Hostname() == "" {
			return "", "", fmt.Errorf("parse git remote %q", remote)
		}
		host = u.Hostname()
		repoPath = strings.TrimPrefix(u.Path, "/")
	}
	repoPath = strings.TrimSuffix(repoPath, ".git")
	if host == "" || repoPath == "" {
		return "", "", fmt.Errorf("parse git remote %q", remote)
	}
	return host, repoPath, nil
}
