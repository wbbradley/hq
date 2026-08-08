package repoctx

import "testing"

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
