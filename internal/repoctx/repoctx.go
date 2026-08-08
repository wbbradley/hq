package repoctx

import (
	"context"
	"errors"
	"fmt"
	"net/url"
	"os/exec"
	"path"
	"strings"
	"time"

	"github.com/cli/go-gh/v2/pkg/api"
	"github.com/cli/go-gh/v2/pkg/auth"
)

var ErrUnavailable = errors.New("github unavailable")

type PullRequest struct {
	Number int
	Title  string
	URL    string
}

type Provider interface {
	Branch(context.Context, string) (string, error)
	PullRequest(context.Context, string, string) (*PullRequest, error)
}

type GitHub struct{}

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

func (GitHub) PullRequest(ctx context.Context, directory, branch string) (*PullRequest, error) {
	remote, err := exec.CommandContext(ctx, "git", "-C", directory, "remote", "get-url", "origin").Output()
	if err != nil {
		return nil, fmt.Errorf("read git remote: %w", err)
	}
	host, owner, repo, err := parseRemote(strings.TrimSpace(string(remote)))
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
	if strings.HasPrefix(remote, "git@") && strings.Contains(remote, ":") {
		parts := strings.SplitN(strings.TrimPrefix(remote, "git@"), ":", 2)
		host = parts[0]
		remote = parts[1]
	} else {
		u, parseErr := url.Parse(remote)
		if parseErr != nil || u.Hostname() == "" {
			return "", "", "", fmt.Errorf("parse git remote %q", remote)
		}
		host = u.Hostname()
		remote = strings.TrimPrefix(u.Path, "/")
	}
	remote = strings.TrimSuffix(remote, ".git")
	owner, repo = path.Split(remote)
	owner = strings.TrimSuffix(owner, "/")
	if host == "" || owner == "" || repo == "" || strings.Contains(owner, "/") {
		return "", "", "", fmt.Errorf("parse git remote %q", remote)
	}
	return host, owner, repo, nil
}
