package session

import (
	"errors"
	"fmt"
	"strings"

	"github.com/wbbradley/hq/internal/model"
)

var ErrNotFound = errors.New("no agent session found")

type IdentityResolver interface {
	Resolve(string) (model.SessionIdentity, error)
}

type Resolver struct {
	Getenv func(string) string
}

func (r Resolver) Resolve(explicit string) (model.SessionIdentity, error) {
	if value := strings.TrimSpace(explicit); value != "" {
		return custom(value)
	}
	if value := strings.TrimSpace(r.getenv("HQ_SESSION")); value != "" {
		return custom(value)
	}
	type candidate struct{ env, harness string }
	var found []model.SessionIdentity
	for _, item := range []candidate{
		{"CODEX_THREAD_ID", "codex"},
		{"CLAUDE_CODE_SESSION_ID", "claude-code"},
		{"PI_SESSION_ID", "pi"},
	} {
		if value := strings.TrimSpace(r.getenv(item.env)); value != "" {
			found = append(found, model.SessionIdentity{Harness: item.harness, ExternalSessionID: value})
		}
	}
	if len(found) == 0 {
		return model.SessionIdentity{}, ErrNotFound
	}
	if len(found) > 1 {
		var names []string
		for _, identity := range found {
			names = append(names, identity.Harness)
		}
		return model.SessionIdentity{}, fmt.Errorf("agent session is ambiguous: found %s", strings.Join(names, ", "))
	}
	return found[0], nil
}

func custom(value string) (model.SessionIdentity, error) {
	if value == "human" {
		return model.SessionIdentity{}, errors.New("\"human\" is reserved for the human mailbox")
	}
	return model.SessionIdentity{Harness: "custom", ExternalSessionID: value}, nil
}

func (r Resolver) getenv(name string) string {
	if r.Getenv == nil {
		return ""
	}
	return r.Getenv(name)
}
