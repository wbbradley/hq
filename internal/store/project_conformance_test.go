package store

import (
	"context"
	"encoding/json"
	"errors"
	"path/filepath"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/model"
)

type projectConformanceFixture struct {
	store   *SQLite
	project domain.Project
	ctx     context.Context
}

func newProjectConformanceFixture(t *testing.T, open bool) projectConformanceFixture {
	t.Helper()
	database := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	project, err := database.CreateProject(context.Background(), domain.CreateProjectRequest{Name: "conformance", Open: open})
	if err != nil {
		t.Fatal(err)
	}
	return projectConformanceFixture{store: database, project: project, ctx: context.Background()}
}

func (f projectConformanceFixture) signedInput(t *testing.T, messageID string, purpose model.MessagePurpose) event.SignedEvent {
	t.Helper()
	account, parents, deviceLabel, err := f.store.localAccountAction(f.ctx, "")
	if err != nil {
		t.Fatal(err)
	}
	payload, err := event.MarshalPayload(event.TextPayload{MessageID: messageID, Body: "conformance input", Purpose: purpose, ActorLabel: deviceLabel})
	if err != nil {
		t.Fatal(err)
	}
	contents := []event.Content{{
		Type: event.TypeMessage, Sender: f.store.localAddress(model.HumanMailboxID), Recipient: f.store.localAddress(f.project.MailboxID),
		Audience: &event.Audience{HumanAccountID: account.ID}, Parents: parents, Scope: event.ScopeAccountAddressed, Payload: payload,
	}}
	signed, err := f.store.signContents(f.ctx, contents, []time.Time{time.Now().UTC()})
	if err != nil {
		t.Fatal(err)
	}
	return signed[0]
}

func (f projectConformanceFixture) assertAcceptedOnce(t *testing.T, messageID string, wantSequence int64) {
	t.Helper()
	var count int
	var sequence int64
	var messageEventID, acceptanceEventID string
	err := f.store.db.QueryRow(`SELECT count(*),COALESCE(MIN(sequence),0),COALESCE(MIN(message_event_id),''),COALESCE(MIN(acceptance_event_id),'') FROM project_message_acceptances WHERE project_id=? AND message_id=?`, f.project.ID, messageID).Scan(&count, &sequence, &messageEventID, &acceptanceEventID)
	if err != nil || count != 1 || sequence != wantSequence {
		t.Fatalf("acceptance count=%d sequence=%d: %v", count, sequence, err)
	}
	message, err := f.store.Get(f.ctx, messageID)
	if err != nil || message.EventID != messageEventID || message.RecipientAddress.Kind != model.MailboxProject || message.RecipientAddress.MailboxID != f.project.MailboxID {
		t.Fatalf("accepted message = %#v, event=%q: %v", message, messageEventID, err)
	}
	var eventType string
	if err := f.store.db.QueryRow(`SELECT event_type FROM canonical_events WHERE event_id=?`, acceptanceEventID).Scan(&eventType); err != nil || eventType != string(event.TypeProjectEvent) {
		t.Fatalf("acceptance event %q type=%q: %v", acceptanceEventID, eventType, err)
	}
	project, err := f.store.GetProject(f.ctx, f.project.ID)
	if err != nil || project.HeadEventID != acceptanceEventID {
		t.Fatalf("project head=%q acceptance=%q: %v", project.HeadEventID, acceptanceEventID, err)
	}
}

func TestProjectIngressConformanceMatrix(t *testing.T) {
	tests := []struct {
		name string
		run  func(*testing.T, projectConformanceFixture, string) projectConformanceFixture
	}{
		{name: "local create", run: func(t *testing.T, f projectConformanceFixture, id string) projectConformanceFixture {
			if err := f.store.Create(f.ctx, model.Message{ID: id, SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: f.project.MailboxID, Body: "created", CreatedAt: time.Now().UTC()}); err != nil {
				t.Fatal(err)
			}
			return f
		}},
		{name: "local reply", run: func(t *testing.T, f projectConformanceFixture, id string) projectConformanceFixture {
			outputID := "019d0000-0000-7000-8000-000000000011"
			if err := f.store.Create(f.ctx, model.Message{ID: outputID, SenderMailboxID: f.project.MailboxID, RecipientMailboxID: model.HumanMailboxID, Body: "output", CreatedAt: time.Now().UTC()}); err != nil {
				t.Fatal(err)
			}
			if err := f.store.Reply(f.ctx, outputID, model.Message{ID: id, SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: f.project.MailboxID, Body: "reply", CreatedAt: time.Now().UTC()}); err != nil {
				t.Fatal(err)
			}
			original, err := f.store.Get(f.ctx, outputID)
			if err != nil || original.ArchivedAt == nil {
				t.Fatalf("replied-to output = %#v, %v", original, err)
			}
			return f
		}},
		{name: "canonical append", run: func(t *testing.T, f projectConformanceFixture, id string) projectConformanceFixture {
			if err := f.store.AppendCanonical(f.ctx, []event.SignedEvent{f.signedInput(t, id, model.MessagePurposeProjectInput)}); err != nil {
				t.Fatal(err)
			}
			return f
		}},
		{name: "duplicate canonical replay", run: func(t *testing.T, f projectConformanceFixture, id string) projectConformanceFixture {
			signed := f.signedInput(t, id, model.MessagePurposeProjectInput)
			if err := f.store.AppendCanonical(f.ctx, []event.SignedEvent{signed}); err != nil {
				t.Fatal(err)
			}
			if err := f.store.AppendCanonical(f.ctx, []event.SignedEvent{signed}); err != nil {
				t.Fatal(err)
			}
			return f
		}},
		{name: "canonical rebuild", run: func(t *testing.T, f projectConformanceFixture, id string) projectConformanceFixture {
			if err := f.store.AppendCanonical(f.ctx, []event.SignedEvent{f.signedInput(t, id, model.MessagePurposeProjectInput)}); err != nil {
				t.Fatal(err)
			}
			if err := f.store.Rebuild(f.ctx); err != nil {
				t.Fatal(err)
			}
			return f
		}},
		{name: "startup recovery", run: func(t *testing.T, f projectConformanceFixture, id string) projectConformanceFixture {
			signed := f.signedInput(t, id, model.MessagePurposeProjectInput)
			tx, err := f.store.db.BeginTx(f.ctx, nil)
			if err != nil {
				t.Fatal(err)
			}
			if _, err := f.store.ingestCanonicalProjectionTx(f.ctx, tx, []event.SignedEvent{signed}, true); err != nil {
				t.Fatal(err)
			}
			if err := tx.Commit(); err != nil {
				t.Fatal(err)
			}
			var before int
			if err := f.store.db.QueryRow(`SELECT count(*) FROM project_message_acceptances WHERE message_id=?`, id).Scan(&before); err != nil || before != 0 {
				t.Fatalf("acceptances before startup repair=%d: %v", before, err)
			}
			path := f.store.database
			if err := f.store.Close(); err != nil {
				t.Fatal(err)
			}
			reopened, err := Open(path)
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(func() { _ = reopened.Close() })
			f.store = reopened
			return f
		}},
	}
	for index, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			fixture := newProjectConformanceFixture(t, true)
			messageID := []string{
				"019d0000-0000-7000-8000-000000000001", "019d0000-0000-7000-8000-000000000002", "019d0000-0000-7000-8000-000000000003",
				"019d0000-0000-7000-8000-000000000004", "019d0000-0000-7000-8000-000000000005", "019d0000-0000-7000-8000-000000000006",
			}[index]
			fixture = test.run(t, fixture, messageID)
			fixture.assertAcceptedOnce(t, messageID, 1)
		})
	}
}

func TestProjectMessagePurposeConformanceMatrix(t *testing.T) {
	tests := []struct {
		purpose model.MessagePurpose
		accept  bool
	}{
		{model.MessagePurposeConversation, true},
		{model.MessagePurposeProjectInput, true},
		{model.MessagePurposeProtocolQuestion, false},
		{model.MessagePurposeProtocolAnswer, false},
		{model.MessagePurposeProjectOutput, false},
		{model.MessagePurposeSystemNotice, false},
	}
	for index, test := range tests {
		t.Run(string(test.purpose), func(t *testing.T) {
			fixture := newProjectConformanceFixture(t, true)
			messageID := []string{
				"019d0000-0000-7000-8000-000000000021", "019d0000-0000-7000-8000-000000000022", "019d0000-0000-7000-8000-000000000023",
				"019d0000-0000-7000-8000-000000000024", "019d0000-0000-7000-8000-000000000025", "019d0000-0000-7000-8000-000000000026",
			}[index]
			head := fixture.project.HeadEventID
			if err := fixture.store.AppendCanonical(fixture.ctx, []event.SignedEvent{fixture.signedInput(t, messageID, test.purpose)}); err != nil {
				t.Fatal(err)
			}
			var count int
			if err := fixture.store.db.QueryRow(`SELECT count(*) FROM project_message_acceptances WHERE message_id=?`, messageID).Scan(&count); err != nil || count != map[bool]int{true: 1, false: 0}[test.accept] {
				t.Fatalf("purpose %s acceptances=%d: %v", test.purpose, count, err)
			}
			if test.accept {
				fixture.assertAcceptedOnce(t, messageID, 1)
			} else if project, err := fixture.store.GetProject(fixture.ctx, fixture.project.ID); err != nil || project.HeadEventID != head {
				t.Fatalf("ineligible purpose advanced project: %#v, %v", project, err)
			}
		})
	}
}

func TestProjectDestinationConformanceMatrix(t *testing.T) {
	fixture := newProjectConformanceFixture(t, true)
	agent, err := fixture.store.CreateNamedAgent(fixture.ctx, "destination-agent", "")
	if err != nil {
		t.Fatal(err)
	}
	tests := []struct {
		name      string
		messageID string
		mailboxID string
		kind      model.MailboxKind
		accept    bool
	}{
		{name: "human mailbox", messageID: "019d0000-0000-7000-8000-000000000071", mailboxID: model.HumanMailboxID, kind: model.MailboxHuman},
		{name: "direct named agent", messageID: "019d0000-0000-7000-8000-000000000072", mailboxID: agent.MailboxID, kind: model.MailboxAgent},
		{name: "home project", messageID: "019d0000-0000-7000-8000-000000000073", mailboxID: fixture.project.MailboxID, kind: model.MailboxProject, accept: true},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if err := fixture.store.Create(fixture.ctx, model.Message{ID: test.messageID, SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: test.mailboxID, Body: test.name, CreatedAt: time.Now().UTC()}); err != nil {
				t.Fatal(err)
			}
			stored, err := fixture.store.Get(fixture.ctx, test.messageID)
			if err != nil || stored.RecipientAddress.Kind != test.kind || stored.RecipientAddress.MailboxID != test.mailboxID {
				t.Fatalf("destination message = %#v, %v", stored, err)
			}
			var count int
			if err := fixture.store.db.QueryRow(`SELECT count(*) FROM project_message_acceptances WHERE message_id=?`, test.messageID).Scan(&count); err != nil || count != map[bool]int{true: 1, false: 0}[test.accept] {
				t.Fatalf("destination acceptances=%d: %v", count, err)
			}
		})
	}
}

func TestProjectStateAndDispatchConformanceMatrix(t *testing.T) {
	tests := []struct {
		name         string
		configure    func(*testing.T, projectConformanceFixture) projectConformanceFixture
		dispatchable bool
	}{
		{name: "open runnable", dispatchable: true, configure: func(t *testing.T, f projectConformanceFixture) projectConformanceFixture {
			if _, err := f.store.CreateNamedAgent(f.ctx, "runner", ""); err != nil {
				t.Fatal(err)
			}
			project, err := f.store.AssignProject(f.ctx, f.project.ID, f.project.HeadEventID, "runner")
			if err == nil {
				project, err = f.store.ActivateProjectAssignment(f.ctx, project.ID, project.HeadEventID, domain.ActivateProjectAssignmentRequest{Harness: "codex", ExternalThread: "state-thread", LaunchDirectory: t.TempDir()})
			}
			if err != nil {
				t.Fatal(err)
			}
			f.project = project
			return f
		}},
		{name: "open unassigned", configure: func(_ *testing.T, f projectConformanceFixture) projectConformanceFixture { return f }},
		{name: "closing", configure: func(t *testing.T, f projectConformanceFixture) projectConformanceFixture {
			project, err := f.store.BeginCloseProject(f.ctx, f.project.ID, f.project.HeadEventID)
			if err != nil {
				t.Fatal(err)
			}
			f.project = project
			return f
		}},
		{name: "closed", configure: func(t *testing.T, f projectConformanceFixture) projectConformanceFixture {
			project, err := f.store.BeginCloseProject(f.ctx, f.project.ID, f.project.HeadEventID)
			if err == nil {
				project, err = f.store.FinalizeCloseProject(f.ctx, project.ID, project.HeadEventID, false, "stopped")
			}
			if err != nil {
				t.Fatal(err)
			}
			f.project = project
			return f
		}},
		{name: "archived", configure: func(t *testing.T, f projectConformanceFixture) projectConformanceFixture {
			project, err := f.store.BeginCloseProject(f.ctx, f.project.ID, f.project.HeadEventID)
			if err == nil {
				project, err = f.store.FinalizeCloseProject(f.ctx, project.ID, project.HeadEventID, false, "stopped")
			}
			if err == nil {
				project, err = f.store.SetProjectArchived(f.ctx, project.ID, project.HeadEventID, true)
			}
			if err != nil {
				t.Fatal(err)
			}
			f.project = project
			return f
		}},
	}
	for index, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			fixture := test.configure(t, newProjectConformanceFixture(t, true))
			messageID := []string{
				"019d0000-0000-7000-8000-000000000031", "019d0000-0000-7000-8000-000000000032", "019d0000-0000-7000-8000-000000000033",
				"019d0000-0000-7000-8000-000000000034", "019d0000-0000-7000-8000-000000000035",
			}[index]
			if err := fixture.store.Create(fixture.ctx, model.Message{ID: messageID, SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: fixture.project.MailboxID, Body: test.name, CreatedAt: time.Now().UTC()}); err != nil {
				t.Fatal(err)
			}
			fixture.assertAcceptedOnce(t, messageID, 1)
			project, err := fixture.store.GetProject(fixture.ctx, fixture.project.ID)
			if err != nil {
				t.Fatal(err)
			}
			assignmentID, threadID := "missing", "missing"
			if project.Assignment != nil {
				assignmentID, threadID = project.Assignment.ID, project.Assignment.SelectedThreadID
			}
			delivery, claimErr := fixture.store.ClaimProjectMessage(fixture.ctx, project.ID, assignmentID, threadID, "state-owner")
			if test.dispatchable {
				if claimErr != nil || delivery.Message.ID != messageID || delivery.Sequence != 1 {
					t.Fatalf("runnable delivery = %#v, %v", delivery, claimErr)
				}
			} else if !errors.Is(claimErr, domain.ErrNotReady) {
				t.Fatalf("ineligible state claim = %#v, %v", delivery, claimErr)
			}
		})
	}
}

func TestReplicaProjectInputConvergesAcrossReorderReplayAndHomeRestart(t *testing.T) {
	ctx := context.Background()
	home := openStore(t, filepath.Join(t.TempDir(), "home", "hq.db"))
	replica := openStore(t, filepath.Join(t.TempDir(), "replica", "hq.db"))
	replicaID, replicaKey := replica.InstallationIdentity()
	bundle, err := home.CreateHumanInvite(ctx, HumanInviteRequest{InstallationID: replicaID, SignerKeyID: replicaKey, Name: "replica"})
	if err != nil {
		t.Fatal(err)
	}
	rawBundle, _ := json.Marshal(bundle)
	if err := replica.JoinHumanInvite(ctx, rawBundle); err != nil {
		t.Fatal(err)
	}
	if err := home.AppendCanonical(ctx, []event.SignedEvent{canonicalEventByType(t, replica, event.TypeHumanDeviceAccept)}); err != nil {
		t.Fatal(err)
	}
	project, err := home.CreateProject(ctx, domain.CreateProjectRequest{Name: "remote matrix", Open: true})
	if err != nil {
		t.Fatal(err)
	}
	project, err = home.UpdateProjectMetadata(ctx, project.ID, project.HeadEventID, "remote matrix renamed", "")
	if err != nil {
		t.Fatal(err)
	}
	rows, err := home.db.Query(`SELECT raw FROM canonical_events WHERE event_type=? ORDER BY created_at,event_id`, event.TypeProjectEvent)
	if err != nil {
		t.Fatal(err)
	}
	var history []event.SignedEvent
	for rows.Next() {
		var raw []byte
		if err := rows.Scan(&raw); err != nil {
			t.Fatal(err)
		}
		item := event.Inspect(raw).Event
		var payload event.ProjectEventPayload
		if json.Unmarshal(item.Content.Payload, &payload) == nil && payload.ProjectID == project.ID {
			history = append(history, item)
		}
	}
	rows.Close()
	if len(history) != 2 {
		t.Fatalf("project history=%d", len(history))
	}
	if err := replica.AppendCanonical(ctx, []event.SignedEvent{history[1], history[0]}); err != nil {
		t.Fatal(err)
	}
	if err := replica.AppendCanonical(ctx, []event.SignedEvent{history[1]}); err != nil {
		t.Fatal(err)
	}
	projectReplica, err := replica.GetProject(ctx, project.ID)
	if err != nil || projectReplica.Name != project.Name || projectReplica.HeadEventID != project.HeadEventID {
		t.Fatalf("reordered replica = %#v, %v", projectReplica, err)
	}
	messageID := "019d0000-0000-7000-8000-000000000041"
	if err := replica.Create(ctx, model.Message{ID: messageID, SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: projectReplica.MailboxID, Body: "remote input", CreatedAt: time.Now().UTC()}); err != nil {
		t.Fatal(err)
	}
	var localAcceptances int
	if err := replica.db.QueryRow(`SELECT count(*) FROM project_message_acceptances WHERE message_id=?`, messageID).Scan(&localAcceptances); err != nil || localAcceptances != 0 {
		t.Fatalf("replica accepted authoritative input=%d: %v", localAcceptances, err)
	}
	rows, err = replica.db.Query(`SELECT raw FROM canonical_events WHERE event_type IN (?,?)`, event.TypeMessage, event.TypeAnswer)
	if err != nil {
		t.Fatal(err)
	}
	var remoteInput event.SignedEvent
	for rows.Next() {
		var raw []byte
		if err := rows.Scan(&raw); err != nil {
			t.Fatal(err)
		}
		item := event.Inspect(raw).Event
		var payload event.TextPayload
		if json.Unmarshal(item.Content.Payload, &payload) == nil && payload.MessageID == messageID {
			remoteInput = item
		}
	}
	rows.Close()
	if remoteInput.ID() == "" {
		t.Fatal("remote project input canonical event not found")
	}
	if err := home.AppendCanonical(ctx, []event.SignedEvent{remoteInput}); err != nil {
		t.Fatal(err)
	}
	if err := home.AppendCanonical(ctx, []event.SignedEvent{remoteInput}); err != nil {
		t.Fatal(err)
	}
	fixture := projectConformanceFixture{store: home, project: project, ctx: ctx}
	fixture.assertAcceptedOnce(t, messageID, 1)
	databasePath := home.database
	if err := home.Close(); err != nil {
		t.Fatal(err)
	}
	reopened, err := Open(databasePath)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = reopened.Close() })
	fixture.store = reopened
	fixture.assertAcceptedOnce(t, messageID, 1)
}
