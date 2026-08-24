package store

import (
	"context"
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/projectstate"
)

func authoritativeTestEvent(t *testing.T, id, previous string, created time.Time, data projectstate.Data) replicaProjectEvent {
	t.Helper()
	body, err := marshalProjectAuditPayload(context.Background(), data)
	if err != nil {
		t.Fatal(err)
	}
	return replicaProjectEvent{id: id, home: "home", created: created, payload: event.ProjectEventPayload{ProjectID: "project", PreviousEventID: previous, Operation: string(data.Operation()), Body: body}}
}

func TestAuthoritativeReducerUsesLegacyThreadOnlyWhenAvailable(t *testing.T) {
	created := time.Unix(100, 0).UTC()
	events := []replicaProjectEvent{
		authoritativeTestEvent(t, "created", "", created, projectstate.Created{Request: domain.CreateProjectRequest{Name: "legacy", Open: true}, MailboxID: "mailbox"}),
		authoritativeTestEvent(t, "assign", "created", created.Add(time.Second), projectstate.AssignmentConfiguring{AssignmentID: "assignment", Agent: "alice"}),
		authoritativeTestEvent(t, "runnable", "assign", created.Add(2*time.Second), projectstate.AssignmentRunnable{AssignmentID: "assignment", Agent: "alice", ThreadID: "thread"}),
	}
	without := reduceAuthoritativeProject("project", events, nil, nil)
	if without.snapshot.Project.HeadEventID != "assign" || !strings.Contains(without.diagnostic, "lacks canonical thread details") {
		t.Fatalf("projection without legacy thread = %#v, %q", without.snapshot.Project, without.diagnostic)
	}
	legacy := map[string]projectstate.ThreadProjection{"thread": {ID: "thread", ProjectID: "project", Agent: "alice", Harness: "codex", ExternalThreadID: "external", LaunchDirectory: "/tmp/project", CreatedAt: created}}
	with := reduceAuthoritativeProject("project", events, legacy, nil)
	if with.diagnostic != "" || with.snapshot.Project.HeadEventID != "runnable" || len(with.snapshot.Threads) != 1 {
		t.Fatalf("projection with legacy thread = %#v, %q", with.snapshot, with.diagnostic)
	}
}

func TestAuthoritativeReducerStopsAtFork(t *testing.T) {
	created := time.Unix(100, 0).UTC()
	root := authoritativeTestEvent(t, "created", "", created, projectstate.Created{Request: domain.CreateProjectRequest{Name: "fork", Open: true}, MailboxID: "mailbox"})
	one := authoritativeTestEvent(t, "one", "created", created.Add(time.Second), projectstate.MetadataUpdated{Name: "one"})
	two := authoritativeTestEvent(t, "two", "created", created.Add(2*time.Second), projectstate.MetadataUpdated{Name: "two"})
	projection := reduceAuthoritativeProject("project", []replicaProjectEvent{root, one, two}, nil, nil)
	if projection.snapshot.Project.HeadEventID != "created" || !strings.Contains(projection.diagnostic, "forks at created") {
		raw, _ := json.Marshal(projection.snapshot)
		t.Fatalf("fork projection = %s, %q", raw, projection.diagnostic)
	}
}

func TestAuthoritativeReducerHydratesLegacyCreatedResources(t *testing.T) {
	created := time.Unix(100, 0).UTC()
	root := authoritativeTestEvent(t, "created", "", created, projectstate.Created{Request: domain.CreateProjectRequest{Name: "legacy", Open: true, PrimaryPath: 0}, MailboxID: "mailbox", ResourceIDs: []string{"resource"}})
	without := reduceAuthoritativeProject("project", []replicaProjectEvent{root}, nil, nil)
	if len(without.snapshot.Project.Resources) != 0 || !strings.Contains(without.diagnostic, "lacks canonical resource details") {
		t.Fatalf("legacy projection without resource = %#v, %q", without.snapshot, without.diagnostic)
	}
	legacy := map[string]projectstate.CreatedResource{"resource": {ID: "resource", Kind: "path", DisplayLocator: "/repo", CanonicalLocator: "/repo", Health: domain.ResourceHealthy, LastCheckedAt: created}}
	with := reduceAuthoritativeProject("project", []replicaProjectEvent{root}, nil, legacy)
	if with.diagnostic != "" || len(with.snapshot.Project.Resources) != 1 || len(with.snapshot.Claims) != 1 {
		t.Fatalf("hydrated legacy projection = %#v, %q", with.snapshot, with.diagnostic)
	}
}
