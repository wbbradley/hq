package projectstate

import (
	"encoding/json"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
)

func TestSupportedOperationsHaveTypedRoundTrips(t *testing.T) {
	samples := []Data{
		Created{}, Opened{}, Closing{}, Closed{}, Archived{}, Unarchived{}, MetadataUpdated{},
		ResourceAdded{}, ResourceRemoved{}, ResourceReplaced{}, PrimaryResourceChanged{}, ResourceHealth{},
		AssignmentConfiguring{}, AssignmentRunnable{}, AssignmentBlocked{}, AssignmentEnded{},
		MessageAccepted{}, MessageDispatched{},
	}
	supported := SupportedOperations()
	if len(samples) != len(supported) {
		t.Fatalf("typed samples=%d supported operations=%d", len(samples), len(supported))
	}
	seen := make(map[Operation]bool, len(samples))
	for _, sample := range samples {
		if seen[sample.Operation()] {
			t.Fatalf("duplicate operation %q", sample.Operation())
		}
		seen[sample.Operation()] = true
		raw, err := MarshalData(sample)
		if err != nil {
			t.Fatal(err)
		}
		envelope, _ := json.Marshal(map[string]json.RawMessage{"data": raw})
		decoded, err := DecodeAudit(string(sample.Operation()), envelope)
		if err != nil || decoded.Operation() != sample.Operation() {
			t.Fatalf("round trip %q = %T, %v", sample.Operation(), decoded, err)
		}
	}
	for _, operation := range supported {
		if !seen[operation] {
			t.Fatalf("operation %q has no typed data", operation)
		}
	}
}

func TestDecodeAuditRejectsUnknownAndMalformedEvents(t *testing.T) {
	if _, err := DecodeAudit("project.future", json.RawMessage(`{"data":{}}`)); err == nil {
		t.Fatal("unknown operation decoded")
	}
	if _, err := DecodeAudit(string(OperationOpened), json.RawMessage(`{"data":`)); err == nil {
		t.Fatal("malformed envelope decoded")
	}
}

func TestReducerAppliesTypedProjectHistory(t *testing.T) {
	createdAt := time.Unix(100, 0).UTC()
	created := Created{Request: domain.CreateProjectRequest{Name: "typed", Open: true, PrimaryPath: 0}, MailboxID: "mailbox", Resources: []CreatedResource{{ID: "resource", Kind: "path", CanonicalLocator: "/repo", DisplayLocator: "/repo", Health: domain.ResourceHealthy, LastCheckedAt: createdAt}}}
	snapshot, err := Apply(Snapshot{}, Event{ID: "created", ProjectID: "project", HomeInstallation: "home", CreatedAt: createdAt, Data: created})
	if err != nil {
		t.Fatal(err)
	}
	steps := []struct {
		id   string
		data Data
	}{
		{id: "metadata", data: &MetadataUpdated{Name: "renamed", Brief: "brief"}},
		{id: "assign", data: &AssignmentConfiguring{AssignmentID: "assignment", Agent: "alice"}},
		{id: "run", data: &AssignmentRunnable{AssignmentID: "assignment", Agent: "alice", ThreadID: "thread"}},
		{id: "accept", data: &MessageAccepted{MessageID: "message", MessageEventID: "message-event", Sequence: 1}},
		{id: "dispatch", data: &MessageDispatched{MessageID: "message", Sequence: 1, AssignmentID: "assignment", Agent: "alice", ProjectThreadID: "thread", ExternalThreadID: "external"}},
	}
	for index, step := range steps {
		next, applyErr := Apply(snapshot, Event{ID: step.id, ProjectID: "project", HomeInstallation: "home", PreviousEventID: snapshot.Project.HeadEventID, CreatedAt: createdAt.Add(time.Duration(index+1) * time.Second), Data: step.data})
		if applyErr != nil {
			t.Fatalf("apply %s: %v", step.id, applyErr)
		}
		snapshot = next
	}
	if snapshot.Project.Name != "renamed" || snapshot.Project.Assignment == nil || snapshot.Project.Assignment.State != domain.AssignmentRunnable || len(snapshot.Acceptances) != 1 || len(snapshot.Dispatches) != 1 || snapshot.Project.HeadEventID != "dispatch" {
		t.Fatalf("snapshot = %#v", snapshot)
	}
}

func TestReducerDoesNotAdvanceOnInvalidTransition(t *testing.T) {
	created := Created{Request: domain.CreateProjectRequest{Name: "closed"}, MailboxID: "mailbox"}
	snapshot, err := Apply(Snapshot{}, Event{ID: "created", ProjectID: "project", HomeInstallation: "home", CreatedAt: time.Unix(100, 0).UTC(), Data: created})
	if err != nil {
		t.Fatal(err)
	}
	if next, err := Apply(snapshot, Event{ID: "closing", ProjectID: "project", HomeInstallation: "home", PreviousEventID: "created", Data: &Closing{}}); err == nil || next.Project.HeadEventID != "created" || next.Project.Lifecycle != domain.ProjectClosed {
		t.Fatalf("invalid transition = %#v, %v", next, err)
	}
}

func TestReducerCoversEverySupportedOperation(t *testing.T) {
	createdAt := time.Unix(200, 0).UTC()
	created := Created{Request: domain.CreateProjectRequest{Name: "complete", PrimaryPath: 0}, MailboxID: "mailbox", Resources: []CreatedResource{{ID: "first", Kind: "path", CanonicalLocator: "/first", Health: domain.ResourceHealthy, LastCheckedAt: createdAt}}}
	snapshot, err := Apply(Snapshot{}, Event{ID: "created", ProjectID: "project", HomeInstallation: "home", CreatedAt: createdAt, Data: created})
	if err != nil {
		t.Fatal(err)
	}
	seen := map[Operation]bool{OperationCreated: true}
	steps := []Data{
		&Opened{},
		&ResourceHealth{ResourceID: "first", Health: domain.ResourceUnknown, LastCheckedAt: createdAt.Add(time.Second)},
		&ResourceAdded{ResourceID: "second", Kind: "path", CanonicalLocator: "/second", Primary: true, Health: domain.ResourceHealthy, LastCheckedAt: createdAt.Add(2 * time.Second)},
		&PrimaryResourceChanged{ResourceID: "first"},
		&ResourceReplaced{OldResourceID: "first", NewResourceID: "replacement", CanonicalLocator: "/replacement", Health: domain.ResourceHealthy, LastCheckedAt: createdAt.Add(3 * time.Second)},
		&ResourceRemoved{ResourceID: "second"},
		&MetadataUpdated{Name: "renamed", Brief: "brief"},
		&AssignmentConfiguring{AssignmentID: "assignment", Agent: "alice"},
		&AssignmentRunnable{AssignmentID: "assignment", Agent: "alice", ThreadID: "thread"},
		&AssignmentBlocked{AssignmentID: "assignment", Agent: "alice"},
		&AssignmentEnded{AssignmentID: "assignment", Agent: "alice"},
		&MessageAccepted{MessageID: "message", MessageEventID: "message-event", Sequence: 1},
		&MessageDispatched{MessageID: "message", Sequence: 1, AssignmentID: "assignment", Agent: "alice", ProjectThreadID: "thread", ExternalThreadID: "external"},
		&Closing{},
		&Closed{},
		&Archived{},
		&Unarchived{},
	}
	for index, data := range steps {
		id := string(data.Operation())
		next, applyErr := Apply(snapshot, Event{ID: id, ProjectID: "project", HomeInstallation: "home", PreviousEventID: snapshot.Project.HeadEventID, CreatedAt: createdAt.Add(time.Duration(index+1) * time.Second), Data: data})
		if applyErr != nil {
			t.Fatalf("apply %s: %v", data.Operation(), applyErr)
		}
		seen[data.Operation()] = true
		snapshot = next
	}
	for _, operation := range SupportedOperations() {
		if !seen[operation] {
			t.Fatalf("reducer did not cover %q", operation)
		}
	}
	if len(snapshot.Resources) != 3 || len(snapshot.Claims) != 3 {
		t.Fatalf("historical resources=%d claims=%d", len(snapshot.Resources), len(snapshot.Claims))
	}
	for _, claim := range snapshot.Claims {
		if claim.ReleasedEventID == "" {
			t.Fatalf("claim was not released: %#v", claim)
		}
	}
}
