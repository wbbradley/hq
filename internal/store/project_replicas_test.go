package store

import (
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/projectstate"
)

func TestReplicaReducerRetainsLastValidHeadOnUnknownEvent(t *testing.T) {
	createdBody, _ := json.Marshal(map[string]any{"data": projectstate.Created{Request: domain.CreateProjectRequest{Name: "replica", Open: true}, MailboxID: "mailbox"}})
	unknownBody, _ := json.Marshal(map[string]any{"data": map[string]any{"future": true}})
	events := []replicaProjectEvent{
		{id: "created", home: "remote", created: time.Unix(100, 0).UTC(), payload: event.ProjectEventPayload{ProjectID: "project", Operation: string(projectstate.OperationCreated), Body: createdBody}},
		{id: "unknown", home: "remote", created: time.Unix(101, 0).UTC(), payload: event.ProjectEventPayload{ProjectID: "project", PreviousEventID: "created", Operation: "project.future", Body: unknownBody}},
	}
	project, ok, diagnostic := reduceReplicaProject("project", events)
	if !ok || project.HeadEventID != "created" || project.Name != "replica" || !strings.Contains(diagnostic, "unsupported project event operation") {
		t.Fatalf("replica = %#v ok=%t diagnostic=%q", project, ok, diagnostic)
	}
}

func TestReplicaReducerRetainsLastValidHeadOnMalformedTransition(t *testing.T) {
	createdBody, _ := json.Marshal(map[string]any{"data": projectstate.Created{Request: domain.CreateProjectRequest{Name: "replica"}, MailboxID: "mailbox"}})
	closingBody, _ := json.Marshal(map[string]any{"data": projectstate.Closing{}})
	events := []replicaProjectEvent{
		{id: "created", home: "remote", created: time.Unix(100, 0).UTC(), payload: event.ProjectEventPayload{ProjectID: "project", Operation: string(projectstate.OperationCreated), Body: createdBody}},
		{id: "closing", home: "remote", created: time.Unix(101, 0).UTC(), payload: event.ProjectEventPayload{ProjectID: "project", PreviousEventID: "created", Operation: string(projectstate.OperationClosing), Body: closingBody}},
	}
	project, ok, diagnostic := reduceReplicaProject("project", events)
	if !ok || project.HeadEventID != "created" || !strings.Contains(diagnostic, "closing requires an open project") {
		t.Fatalf("replica = %#v ok=%t diagnostic=%q", project, ok, diagnostic)
	}
}
