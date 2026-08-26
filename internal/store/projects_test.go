package store

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/model"
)

func TestCanonicalizeProjectPathResolvesExistingAncestor(t *testing.T) {
	root := t.TempDir()
	real := filepath.Join(root, "real")
	if err := os.Mkdir(real, 0o700); err != nil {
		t.Fatal(err)
	}
	link := filepath.Join(root, "link")
	if err := os.Symlink(real, link); err != nil {
		t.Fatal(err)
	}
	got, err := canonicalizeProjectPath(filepath.Join(link, "future", "child"))
	if err != nil {
		t.Fatal(err)
	}
	if got.display != filepath.Join(link, "future", "child") {
		t.Fatalf("display = %q", got.display)
	}
	canonicalReal, err := filepath.EvalSymlinks(real)
	if err != nil {
		t.Fatal(err)
	}
	if got.canonical != filepath.Join(canonicalReal, "future", "child") {
		t.Fatalf("canonical = %q", got.canonical)
	}
	if got.health != domain.ResourceMissing {
		t.Fatalf("health = %q", got.health)
	}
}

func TestOpenObservesChangedSymlinkIdentityWithoutRewritingResource(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	root := t.TempDir()
	display := filepath.Join(root, "link", "future")
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "identity check", Paths: []domain.ProjectPathInput{{DisplayPath: display}}})
	if err != nil {
		t.Fatal(err)
	}
	originalCanonical := project.Resources[0].CanonicalLocator
	target := t.TempDir()
	if err := os.Symlink(target, filepath.Join(root, "link")); err != nil {
		t.Fatal(err)
	}
	opened, err := s.OpenProject(ctx, project.ID, project.HeadEventID)
	if err != nil {
		t.Fatal(err)
	}
	if opened.Lifecycle != domain.ProjectOpen || opened.Resources[0].CanonicalLocator != originalCanonical || opened.Resources[0].Health != domain.ResourceMalformed {
		t.Fatalf("opened resource identity = %#v", opened.Resources[0])
	}
	var healthEvents, notices int
	if err := s.db.QueryRow(`SELECT count(*) FROM project_events WHERE project_id=? AND event_type='project.resource.health'`, project.ID).Scan(&healthEvents); err != nil || healthEvents != 1 {
		t.Fatalf("health events = %d, %v", healthEvents, err)
	}
	if err := s.db.QueryRow(`SELECT count(*) FROM messages WHERE actor_label=? AND presentation='notice' AND details='' AND technical_sections_json LIKE '%"namespace":"hq.project.resource_health"%' AND technical_sections_json LIKE '%"current_health"%' AND technical_sections_json LIKE '%malformed%'`, "HQ · "+project.Name).Scan(&notices); err != nil || notices != 1 {
		t.Fatalf("health notices = %d, %v", notices, err)
	}
	var noticeID string
	if err := s.db.QueryRow(`SELECT id FROM messages WHERE actor_label=? AND body LIKE 'Project resource condition changed%'`, "HQ · "+project.Name).Scan(&noticeID); err != nil {
		t.Fatal(err)
	}
	notice, err := s.Get(ctx, noticeID)
	if err != nil || strings.Join(technicalFieldKeys(notice.TechnicalSections, "hq.project.resource_health"), ",") != "project_id,resource_id,previous_health,current_health,health_details" {
		t.Fatalf("resource notice = %#v, %v", notice, err)
	}
	assertCanonicalMessageSchema(t, s, noticeID, event.Schema3)
}

func TestOpenChecksExpectedHeadBeforeLifecycle(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "stale open"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.SetProjectArchived(ctx, project.ID, project.HeadEventID, true); err != nil {
		t.Fatal(err)
	}
	_, err = s.OpenProject(ctx, project.ID, project.HeadEventID)
	var stale *domain.StaleProjectHead
	if !errors.As(err, &stale) {
		t.Fatalf("open error = %v, want stale project head", err)
	}
}

func TestProjectInputRoutingPreservesTypedMessageSemantics(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "typed input routing"})
	if err != nil {
		t.Fatal(err)
	}
	input := model.Message{
		ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140f21", SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: project.MailboxID,
		Body: "typed project input", Details: "human project instructions", Presentation: model.PresentationNotice,
		Correlation:       model.MessageCorrelation{Provider: "home-built", SessionID: "session", OperationID: "operation", ItemID: "item", RequestID: "request"},
		TechnicalSections: []model.TechnicalSection{{Namespace: "vendor.project_input", Fields: []model.TechnicalField{{Key: "second", Value: "2"}, {Key: "first", Label: "First", Value: "1"}}}},
		Context:           model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC(),
	}
	if err := s.Create(ctx, input); err != nil {
		t.Fatal(err)
	}
	stored, err := s.Get(ctx, input.ID)
	if err != nil {
		t.Fatal(err)
	}
	input.Purpose = model.MessagePurposeProjectInput
	assertMessageSemantics(t, stored, input)
}

func TestQueuedProjectCommandEventuallyReportsUnreachableHome(t *testing.T) {
	command := &domain.ProjectCommand{
		Stage:     domain.ProjectCommandQueued,
		CreatedAt: time.Now().UTC().Add(-projectHomeUnreachableAfter),
	}
	annotateProjectCommandReachability(command, time.Now().UTC())
	if !strings.Contains(command.Diagnostic, "offline or unreachable") {
		t.Fatalf("diagnostic = %q", command.Diagnostic)
	}
	command.Stage = domain.ProjectCommandReceived
	command.Diagnostic = ""
	annotateProjectCommandReachability(command, time.Now().UTC())
	if command.Diagnostic != "" {
		t.Fatalf("acknowledged command diagnostic = %q", command.Diagnostic)
	}
}

func TestUnknownCanonicalProjectCommandIsRejectedWithoutMutation(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "unknown command", Open: true})
	if err != nil {
		t.Fatal(err)
	}
	account, parents, _, err := s.localAccountAction(ctx, "")
	if err != nil {
		t.Fatal(err)
	}
	authorities := uniqueSorted(parents)
	parents = uniqueSorted(append(parents, project.HeadEventID))
	commandID := "019c0000-0000-7000-8000-000000000091"
	payload, _ := event.MarshalPayload(event.ProjectCommandPayload{CommandID: commandID, ProjectID: project.ID, ExpectedHead: project.HeadEventID, Operation: "project.future", Body: json.RawMessage(`{}`)})
	content := event.Content{Type: event.TypeProjectCommand, Sender: s.localAddress(model.HumanMailboxID), Recipient: s.localAddress(model.HumanMailboxID), Audience: &event.Audience{HumanAccountID: account.ID}, Parents: parents, Authorities: authorities, Scope: event.ScopeAccountAddressed, Payload: payload}
	signed, err := s.signContents(ctx, []event.Content{content}, []time.Time{time.Now().UTC()})
	if err != nil {
		t.Fatal(err)
	}
	if err := s.AppendCanonical(ctx, signed); err != nil {
		t.Fatal(err)
	}
	after, err := s.GetProject(ctx, project.ID)
	if err != nil || after.HeadEventID != project.HeadEventID {
		t.Fatalf("unknown command mutated project: %#v, %v", after, err)
	}
	rows, err := s.db.Query(`SELECT raw FROM canonical_events WHERE event_type=?`, event.TypeProjectResult)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	matched := false
	for rows.Next() {
		var raw []byte
		if err := rows.Scan(&raw); err != nil {
			t.Fatal(err)
		}
		var result event.ProjectCommandResultPayload
		if json.Unmarshal(event.Inspect(raw).Event.Content.Payload, &result) == nil && result.CommandID == commandID {
			matched = matched || result.Stage == string(domain.ProjectCommandRejected) && strings.Contains(result.Diagnostic, "unsupported project command operation")
		}
	}
	if !matched {
		t.Fatal("unknown command did not publish a deterministic rejection")
	}
}

func TestWorktreeDestinationReservationBlocksOverlappingClaims(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	repository := t.TempDir()
	destination := filepath.Join(t.TempDir(), "reserved", "worktree")
	request := domain.ProjectWorktreeRequest{RequestID: "019c0000-0000-7000-8000-000000000011", ProjectID: "019c0000-0000-7000-8000-000000000012", Name: "reserved", Repository: repository, Destination: destination, Branch: "feature"}
	operation, err := s.BeginProjectWorktreeProvision(ctx, request)
	if err != nil {
		t.Fatal(err)
	}
	_, err = s.CreateProject(ctx, domain.CreateProjectRequest{Name: "intruder", Paths: []domain.ProjectPathInput{{DisplayPath: filepath.Join(destination, "child")}}, Open: true})
	var conflict *domain.ProjectConflict
	if !errors.As(err, &conflict) || conflict.ConflictingProject != request.ProjectID {
		t.Fatalf("reservation conflict = %#v, %v", conflict, err)
	}
	created, err := s.CreateProject(domain.WithProjectProvisioning(ctx, operation.ID), domain.CreateProjectRequest{ID: request.ProjectID, Name: request.Name, Paths: []domain.ProjectPathInput{{DisplayPath: destination}}, Open: true})
	if err != nil || created.ID != request.ProjectID {
		t.Fatalf("reservation owner create = %#v, %v", created, err)
	}
}

func TestProjectClaimsRejectEqualAncestorAndDescendant(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	root := t.TempDir()
	first, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "first", Paths: []domain.ProjectPathInput{{DisplayPath: filepath.Join(root, "work")}}, Open: true})
	if err != nil {
		t.Fatal(err)
	}
	for _, path := range []string{filepath.Join(root, "work"), root, filepath.Join(root, "work", "child")} {
		_, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "conflict", Paths: []domain.ProjectPathInput{{DisplayPath: path}}, Open: true})
		var conflict *domain.ProjectConflict
		if !errors.As(err, &conflict) {
			t.Fatalf("create with %q = %v, want project conflict", path, err)
		}
		if conflict.ConflictingProject != first.ID {
			t.Fatalf("conflicting project = %q, want %q", conflict.ConflictingProject, first.ID)
		}
	}
	var audits int
	var details string
	if err := s.db.QueryRow(`SELECT count(*),MAX(details_json) FROM project_audit_log WHERE outcome='rejected'`).Scan(&audits, &details); err != nil || audits != 3 || !strings.Contains(details, `"overlap"`) || !strings.Contains(details, `"conflicting_path"`) || !strings.Contains(details, `"proposed_agent"`) {
		t.Fatalf("conflict audit count=%d details=%q err=%v", audits, details, err)
	}
	closed, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "closed overlap", Paths: []domain.ProjectPathInput{{DisplayPath: root}}})
	if err != nil {
		t.Fatalf("closed project should retain desired resource without a claim: %v", err)
	}
	if closed.Lifecycle != domain.ProjectClosed {
		t.Fatalf("lifecycle = %q", closed.Lifecycle)
	}
}

func TestProjectPredecessorMayBeUnreachable(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	predecessor := "019c0000-0000-7000-8000-000000000155"
	project, err := s.CreateProject(context.Background(), domain.CreateProjectRequest{Name: "successor", PredecessorProjectID: predecessor})
	if err != nil {
		t.Fatal(err)
	}
	if project.PredecessorProjectID != predecessor {
		t.Fatalf("predecessor = %q", project.PredecessorProjectID)
	}
}

func TestProjectHistoryFansOutAndAuthenticatesOnAnotherHumanDevice(t *testing.T) {
	ctx := context.Background()
	creator := openStore(t, filepath.Join(t.TempDir(), "creator", "hq.db"))
	invited := openStore(t, filepath.Join(t.TempDir(), "invited", "hq.db"))
	invitedID, invitedKey := invited.InstallationIdentity()
	bundle, err := creator.CreateHumanInvite(ctx, HumanInviteRequest{InstallationID: invitedID, SignerKeyID: invitedKey, Name: "desktop"})
	if err != nil {
		t.Fatal(err)
	}
	rawBundle, _ := json.Marshal(bundle)
	if err := invited.JoinHumanInvite(ctx, rawBundle); err != nil {
		t.Fatal(err)
	}
	if err := creator.AppendCanonical(ctx, []event.SignedEvent{canonicalEventByType(t, invited, event.TypeHumanDeviceAccept)}); err != nil {
		t.Fatal(err)
	}
	project, err := creator.CreateProject(ctx, domain.CreateProjectRequest{Name: "replicated", Open: true})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := creator.UpdateProjectMetadata(ctx, project.ID, project.HeadEventID, "replicated renamed", "shared brief"); err != nil {
		t.Fatal(err)
	}
	rows, err := creator.db.Query(`SELECT raw FROM canonical_events WHERE event_type=? ORDER BY created_at,event_id`, event.TypeProjectEvent)
	if err != nil {
		t.Fatal(err)
	}
	var history []event.SignedEvent
	for rows.Next() {
		var raw []byte
		if err := rows.Scan(&raw); err != nil {
			t.Fatal(err)
		}
		inspection := event.Inspect(raw)
		if inspection.Status != event.StatusProjected || inspection.Event.Content.Scope != event.ScopeAccountAddressed || inspection.Event.Content.Audience == nil {
			t.Fatalf("project event inspection = %#v", inspection)
		}
		history = append(history, inspection.Event)
	}
	rows.Close()
	if len(history) != 2 {
		t.Fatalf("project history events = %d", len(history))
	}
	for _, item := range history {
		var fanout int
		if err := creator.db.QueryRow(`SELECT count(*) FROM outbox WHERE event_id=? AND recipient_installation_id=?`, item.ID(), invitedID).Scan(&fanout); err != nil || fanout != 1 {
			t.Fatalf("project event %s fanout=%d err=%v", item.ID(), fanout, err)
		}
	}
	if err := invited.AppendCanonical(ctx, history); err != nil {
		t.Fatal(err)
	}
	var projected int
	if err := invited.db.QueryRow(`SELECT count(*) FROM canonical_events WHERE event_type=? AND installation_id=? AND reduction_status=?`, event.TypeProjectEvent, project.HomeInstallation, event.StatusProjected).Scan(&projected); err != nil || projected != 2 {
		t.Fatalf("remote projected project events=%d err=%v", projected, err)
	}
	replica, err := invited.GetProject(ctx, project.ID)
	if err != nil || !replica.ReadOnlyReplica || replica.HomeInstallation != project.HomeInstallation || replica.Name != "replicated renamed" || replica.Brief != "shared brief" || replica.Lifecycle != domain.ProjectOpen {
		t.Fatalf("remote project replica = %#v, %v", replica, err)
	}
	messageID := "019c0000-0000-7000-8000-000000000361"
	if err := invited.Create(ctx, model.Message{ID: messageID, SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: replica.MailboxID, Body: "remote project work", CreatedAt: time.Now().UTC()}); err != nil {
		t.Fatal(err)
	}
	remoteMessage, err := invited.Get(ctx, messageID)
	if err != nil || remoteMessage.RecipientInstallationID != project.HomeInstallation || remoteMessage.AudienceAccountID == "" {
		t.Fatalf("remote project message = %#v, %v", remoteMessage, err)
	}
	var remoteRaw []byte
	if err := invited.db.QueryRow(`SELECT raw FROM canonical_events WHERE event_id=?`, remoteMessage.EventID).Scan(&remoteRaw); err != nil {
		t.Fatal(err)
	}
	if err := creator.AppendCanonical(ctx, []event.SignedEvent{event.Inspect(remoteRaw).Event}); err != nil {
		t.Fatal(err)
	}
	var sequence int64
	var acceptanceID string
	if err := creator.db.QueryRow(`SELECT sequence,acceptance_event_id FROM project_message_acceptances WHERE message_id=?`, messageID).Scan(&sequence, &acceptanceID); err != nil || sequence != 1 {
		t.Fatalf("home acceptance sequence=%d event=%q err=%v", sequence, acceptanceID, err)
	}
	var acceptanceRaw []byte
	if err := creator.db.QueryRow(`SELECT raw FROM canonical_events WHERE event_id=?`, acceptanceID).Scan(&acceptanceRaw); err != nil {
		t.Fatal(err)
	}
	if err := invited.AppendCanonical(ctx, []event.SignedEvent{event.Inspect(acceptanceRaw).Event}); err != nil {
		t.Fatal(err)
	}
	replica, err = invited.GetProject(ctx, project.ID)
	if err != nil || replica.HeadEventID != acceptanceID {
		t.Fatalf("replica after home acceptance = %#v, %v", replica, err)
	}
	queued, err := invited.UpdateProjectMetadata(ctx, replica.ID, replica.HeadEventID, "remote rename", "remote brief")
	if err != nil || queued.PendingCommand == nil || queued.PendingCommand.Stage != domain.ProjectCommandQueued {
		t.Fatalf("queued remote command = %#v, %v", queued, err)
	}
	commandID := queued.PendingCommand.ID
	if _, err := invited.SetProjectArchived(ctx, replica.ID, replica.HeadEventID, true); !errors.Is(err, domain.ErrProjectCommandPending) {
		t.Fatalf("second unresolved command = %v", err)
	}
	var commandRaw []byte
	if err := invited.db.QueryRow(`SELECT raw FROM canonical_events WHERE event_type=? ORDER BY created_at DESC,event_id DESC LIMIT 1`, event.TypeProjectCommand).Scan(&commandRaw); err != nil {
		t.Fatal(err)
	}
	if err := creator.AppendCanonical(ctx, []event.SignedEvent{event.Inspect(commandRaw).Event}); err != nil {
		t.Fatal(err)
	}
	homeProject, err := creator.GetProject(ctx, project.ID)
	if err != nil || homeProject.Name != "remote rename" || homeProject.Brief != "remote brief" {
		t.Fatalf("home after remote command = %#v, %v", homeProject, err)
	}
	rows, err = creator.db.Query(`SELECT raw FROM canonical_events WHERE event_type IN (?,?) AND event_id NOT IN (SELECT event_id FROM canonical_events WHERE 0) ORDER BY created_at,event_id`, event.TypeProjectEvent, event.TypeProjectResult)
	if err != nil {
		t.Fatal(err)
	}
	var convergence []event.SignedEvent
	for rows.Next() {
		var raw []byte
		_ = rows.Scan(&raw)
		item := event.Inspect(raw).Event
		if item.ID() == homeProject.HeadEventID || item.Content.Type == event.TypeProjectResult {
			var result event.ProjectCommandResultPayload
			if item.Content.Type != event.TypeProjectResult || json.Unmarshal(item.Content.Payload, &result) == nil && result.CommandID == commandID {
				convergence = append(convergence, item)
			}
		}
	}
	rows.Close()
	if err := invited.AppendCanonical(ctx, convergence); err != nil {
		t.Fatal(err)
	}
	replica, err = invited.GetProject(ctx, project.ID)
	if err != nil || replica.Name != "remote rename" || replica.PendingCommand != nil || replica.LatestCommand == nil || replica.LatestCommand.Stage != domain.ProjectCommandCommitted {
		t.Fatalf("replica after command result = %#v, %v", replica, err)
	}
	remoteCreated, err := invited.CreateProject(ctx, domain.CreateProjectRequest{Name: "created elsewhere", HomeInstallation: project.HomeInstallation, Brief: "remote creation"})
	if err != nil || remoteCreated.Lifecycle != domain.ProjectPreparing || remoteCreated.PendingCommand == nil {
		t.Fatalf("remote creation queued = %#v, %v", remoteCreated, err)
	}
	createCommandID := remoteCreated.PendingCommand.ID
	if err := invited.db.QueryRow(`SELECT raw FROM canonical_events WHERE event_type=? ORDER BY rowid DESC LIMIT 1`, event.TypeProjectCommand).Scan(&commandRaw); err != nil {
		t.Fatal(err)
	}
	if err := creator.AppendCanonical(ctx, []event.SignedEvent{event.Inspect(commandRaw).Event}); err != nil {
		t.Fatal(err)
	}
	createdAtHome, err := creator.GetProject(ctx, remoteCreated.ID)
	if err != nil || createdAtHome.Name != "created elsewhere" || createdAtHome.HomeInstallation != project.HomeInstallation {
		t.Fatalf("home-created remote project = %#v, %v", createdAtHome, err)
	}
	rows, err = creator.db.Query(`SELECT raw FROM canonical_events WHERE event_type IN (?,?)`, event.TypeProjectEvent, event.TypeProjectResult)
	if err != nil {
		t.Fatal(err)
	}
	convergence = nil
	for rows.Next() {
		var raw []byte
		_ = rows.Scan(&raw)
		item := event.Inspect(raw).Event
		if item.Content.Type == event.TypeProjectEvent {
			var payload event.ProjectEventPayload
			_ = json.Unmarshal(item.Content.Payload, &payload)
			if payload.ProjectID == remoteCreated.ID {
				convergence = append(convergence, item)
			}
		} else {
			var payload event.ProjectCommandResultPayload
			_ = json.Unmarshal(item.Content.Payload, &payload)
			if payload.CommandID == createCommandID {
				convergence = append(convergence, item)
			}
		}
	}
	rows.Close()
	if err := invited.AppendCanonical(ctx, convergence); err != nil {
		t.Fatal(err)
	}
	createdReplica, err := invited.GetProject(ctx, remoteCreated.ID)
	if err != nil || createdReplica.Lifecycle != domain.ProjectClosed || createdReplica.Name != "created elsewhere" || createdReplica.PendingCommand != nil || createdReplica.LatestCommand == nil || createdReplica.LatestCommand.Stage != domain.ProjectCommandCommitted {
		t.Fatalf("converged remote creation = %#v, %v", createdReplica, err)
	}
	creator.SetProjectCommandHandler(func(commandCtx context.Context, command domain.ProjectCommand, data domain.ProjectCommandData) (domain.Project, error) {
		if command.Operation != domain.ProjectCommandProvisionWorktree {
			return domain.Project{}, fmt.Errorf("unexpected runtime operation %s", command.Operation)
		}
		value, ok := data.(*domain.ProjectProvisionWorktreeCommand)
		if !ok {
			return domain.Project{}, fmt.Errorf("unexpected runtime data %T", data)
		}
		request := domain.ProjectWorktreeRequest(*value)
		return creator.CreateProject(domain.WithProjectProvisioning(commandCtx, command.ID), domain.CreateProjectRequest{ID: command.ProjectID, Name: request.Name, Brief: request.Brief, Paths: []domain.ProjectPathInput{{DisplayPath: request.Destination}}})
	})
	worktreeRequest := domain.ProjectWorktreeRequest{RequestID: "019c0000-0000-7000-8000-000000000371", ProjectID: "019c0000-0000-7000-8000-000000000372", HomeInstallation: project.HomeInstallation, Name: "remote worktree", Repository: t.TempDir(), Destination: filepath.Join(t.TempDir(), "remote-worktree"), Branch: "remote-feature"}
	queuedWorktree, remote, err := invited.QueueProjectWorktreeProvision(ctx, worktreeRequest)
	if err != nil || !remote || queuedWorktree.PendingCommand == nil {
		t.Fatalf("queued remote worktree = %#v remote=%t err=%v", queuedWorktree, remote, err)
	}
	if err := invited.db.QueryRow(`SELECT raw FROM canonical_events WHERE event_type=? ORDER BY rowid DESC LIMIT 1`, event.TypeProjectCommand).Scan(&commandRaw); err != nil {
		t.Fatal(err)
	}
	if err := creator.AppendCanonical(ctx, []event.SignedEvent{event.Inspect(commandRaw).Event}); err != nil {
		t.Fatal(err)
	}
	provisionedAtHome, err := creator.GetProject(ctx, worktreeRequest.ProjectID)
	if err != nil || provisionedAtHome.Name != worktreeRequest.Name || len(provisionedAtHome.Resources) != 1 {
		t.Fatalf("remote worktree at home = %#v, %v", provisionedAtHome, err)
	}
	var committed int
	rows, err = creator.db.Query(`SELECT raw FROM canonical_events WHERE event_type=?`, event.TypeProjectResult)
	if err != nil {
		t.Fatal(err)
	}
	for rows.Next() {
		var raw []byte
		_ = rows.Scan(&raw)
		var payload event.ProjectCommandResultPayload
		_ = json.Unmarshal(event.Inspect(raw).Event.Content.Payload, &payload)
		if payload.CommandID == worktreeRequest.RequestID && payload.Stage == "committed" {
			committed++
		}
	}
	rows.Close()
	if committed != 1 {
		t.Fatalf("remote worktree committed results = %d", committed)
	}
}

func TestProjectCloseReopenAndArchiveLifecycle(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "lifecycle", Paths: []domain.ProjectPathInput{{DisplayPath: t.TempDir()}}, Open: true})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.BeginCloseProject(ctx, project.ID, "wrong"); !errors.Is(err, domain.ErrProjectStale) {
		t.Fatalf("stale close = %v", err)
	}
	closing, err := s.BeginCloseProject(ctx, project.ID, project.HeadEventID)
	if err != nil {
		t.Fatal(err)
	}
	if closing.Lifecycle != domain.ProjectClosing {
		t.Fatalf("lifecycle = %q", closing.Lifecycle)
	}
	var claims int
	if err := s.db.QueryRow(`SELECT count(*) FROM resource_claim_epochs WHERE project_id=? AND released_event_id IS NULL`, project.ID).Scan(&claims); err != nil || claims != 1 {
		t.Fatalf("closing claims = %d, %v", claims, err)
	}
	closed, err := s.FinalizeCloseProject(ctx, project.ID, closing.HeadEventID, true, "unknown")
	if err != nil {
		t.Fatal(err)
	}
	if closed.Lifecycle != domain.ProjectClosed {
		t.Fatalf("lifecycle = %q", closed.Lifecycle)
	}
	if err := s.db.QueryRow(`SELECT count(*) FROM resource_claim_epochs WHERE project_id=? AND released_event_id IS NULL`, project.ID).Scan(&claims); err != nil || claims != 0 {
		t.Fatalf("closed claims = %d, %v", claims, err)
	}
	archived, err := s.SetProjectArchived(ctx, project.ID, closed.HeadEventID, true)
	if err != nil {
		t.Fatal(err)
	}
	if !archived.Archived {
		t.Fatal("project was not archived")
	}
	if _, err := s.OpenProject(ctx, project.ID, archived.HeadEventID); !errors.Is(err, domain.ErrProjectState) {
		t.Fatalf("open archived = %v", err)
	}
	unarchived, err := s.SetProjectArchived(ctx, project.ID, archived.HeadEventID, false)
	if err != nil {
		t.Fatal(err)
	}
	reopened, err := s.OpenProject(ctx, project.ID, unarchived.HeadEventID)
	if err != nil {
		t.Fatal(err)
	}
	if reopened.Lifecycle != domain.ProjectOpen || reopened.Archived {
		t.Fatalf("reopened = %#v", reopened)
	}
	var events int
	if err := s.db.QueryRow(`SELECT count(*) FROM project_events WHERE project_id=?`, project.ID).Scan(&events); err != nil || events != 6 {
		t.Fatalf("history events = %d, %v", events, err)
	}
	var signedEvents int
	if err := s.db.QueryRow(`SELECT count(*) FROM project_events p JOIN canonical_events c ON c.event_id=p.event_id WHERE p.project_id=? AND c.event_type='project.event' AND c.reduction_status='projected'`, project.ID).Scan(&signedEvents); err != nil || signedEvents != events {
		t.Fatalf("signed history events = %d of %d, %v", signedEvents, events, err)
	}
}

func TestReopenConflictIsAtomic(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	root := t.TempDir()
	first, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "first", Paths: []domain.ProjectPathInput{{DisplayPath: filepath.Join(root, "one")}, {DisplayPath: filepath.Join(root, "two")}}})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "blocker", Paths: []domain.ProjectPathInput{{DisplayPath: filepath.Join(root, "two", "child")}}, Open: true}); err != nil {
		t.Fatal(err)
	}
	if _, err := s.OpenProject(ctx, first.ID, first.HeadEventID); !errors.Is(err, domain.ErrResourceConflict) {
		t.Fatalf("reopen = %v", err)
	}
	got, err := s.GetProject(ctx, first.ID)
	if err != nil {
		t.Fatal(err)
	}
	if got.Lifecycle != domain.ProjectClosed || got.HeadEventID != first.HeadEventID {
		t.Fatalf("failed reopen changed project: %#v", got)
	}
	var claims int
	if err := s.db.QueryRow(`SELECT count(*) FROM resource_claim_epochs WHERE project_id=? AND released_event_id IS NULL`, first.ID).Scan(&claims); err != nil || claims != 0 {
		t.Fatalf("partial claims = %d, %v", claims, err)
	}
}

func TestProjectMailboxSurvivesCanonicalProjectionRebuild(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	project, err := s.CreateProject(context.Background(), domain.CreateProjectRequest{Name: "durable mailbox"})
	if err != nil {
		t.Fatal(err)
	}
	if err := s.Rebuild(context.Background()); err != nil {
		t.Fatal(err)
	}
	got, err := s.GetProject(context.Background(), project.ID)
	if err != nil {
		t.Fatal(err)
	}
	var kind string
	if err := s.db.QueryRow(`SELECT kind FROM mailboxes WHERE id=?`, got.MailboxID).Scan(&kind); err != nil || kind != "project" {
		t.Fatalf("project mailbox kind = %q, %v", kind, err)
	}
}

func TestProjectAssignmentAndImmutableThreadScope(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	if _, err := s.CreateNamedAgent(ctx, "alice", ""); err != nil {
		t.Fatal(err)
	}
	if _, err := s.CreateNamedAgent(ctx, "bob", ""); err != nil {
		t.Fatal(err)
	}
	first, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "first", Open: true})
	if err != nil {
		t.Fatal(err)
	}
	second, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "second", Open: true})
	if err != nil {
		t.Fatal(err)
	}
	configuring, err := s.AssignProject(ctx, first.ID, first.HeadEventID, "alice")
	if err != nil {
		t.Fatal(err)
	}
	if configuring.Assignment == nil || configuring.Assignment.State != domain.AssignmentConfiguring {
		t.Fatalf("assignment = %#v", configuring.Assignment)
	}
	assignedAgent, err := s.GetNamedAgent(ctx, "alice")
	if err != nil {
		t.Fatal(err)
	}
	if assignedAgent.Idle || assignedAgent.AssignedProjectID != first.ID {
		t.Fatalf("assigned agent idleness = %#v", assignedAgent)
	}
	if err := s.RetireNamedAgent(ctx, "alice"); !errors.Is(err, domain.ErrAgentAssigned) {
		t.Fatalf("retire assigned agent = %v", err)
	}
	if _, err := s.AssignProject(ctx, second.ID, second.HeadEventID, "alice"); !errors.Is(err, domain.ErrAgentAssigned) {
		t.Fatalf("double agent assignment = %v", err)
	}
	runnable, err := s.ActivateProjectAssignment(ctx, first.ID, configuring.HeadEventID, domain.ActivateProjectAssignmentRequest{Harness: "codex", ExternalThread: "thread-one", LaunchDirectory: t.TempDir()})
	if err != nil {
		t.Fatal(err)
	}
	threadID := runnable.Assignment.SelectedThreadID
	if threadID == "" || runnable.Assignment.State != domain.AssignmentRunnable {
		t.Fatalf("runnable assignment = %#v", runnable.Assignment)
	}
	unassigned, err := s.UnassignProject(ctx, first.ID, runnable.HeadEventID, false, "stopped")
	if err != nil {
		t.Fatal(err)
	}
	idleAgent, err := s.GetNamedAgent(ctx, "alice")
	if err != nil || !idleAgent.Idle || idleAgent.AssignedProjectID != "" {
		t.Fatalf("unassigned agent idleness = %#v, %v", idleAgent, err)
	}
	bobConfiguring, err := s.AssignProject(ctx, first.ID, unassigned.HeadEventID, "bob")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.ActivateProjectAssignment(ctx, first.ID, bobConfiguring.HeadEventID, domain.ActivateProjectAssignmentRequest{ThreadID: threadID}); !errors.Is(err, domain.ErrProjectThreadMismatch) {
		t.Fatalf("cross-agent thread resume = %v", err)
	}
	bobEnded, err := s.AbortProjectAssignment(ctx, first.ID, bobConfiguring.HeadEventID, "bring-up failed")
	if err != nil {
		t.Fatal(err)
	}
	aliceAgain, err := s.AssignProject(ctx, first.ID, bobEnded.HeadEventID, "alice")
	if err != nil {
		t.Fatal(err)
	}
	resumed, err := s.ActivateProjectAssignment(ctx, first.ID, aliceAgain.HeadEventID, domain.ActivateProjectAssignmentRequest{ThreadID: threadID})
	if err != nil {
		t.Fatal(err)
	}
	if resumed.Assignment.SelectedThreadID != threadID {
		t.Fatalf("resumed thread = %q, want %q", resumed.Assignment.SelectedThreadID, threadID)
	}
}

func TestResourceReplaceIsAtomicAndPreservesPrimary(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	root := t.TempDir()
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "resources", Paths: []domain.ProjectPathInput{{DisplayPath: filepath.Join(root, "old")}}, Open: true})
	if err != nil {
		t.Fatal(err)
	}
	oldID := project.PrimaryResourceID
	replaced, err := s.ReplaceProjectPath(ctx, project.ID, project.HeadEventID, oldID, domain.ProjectPathInput{DisplayPath: filepath.Join(root, "new")})
	if err != nil {
		t.Fatal(err)
	}
	if len(replaced.Resources) != 1 || replaced.Resources[0].ID == oldID || replaced.PrimaryResourceID != replaced.Resources[0].ID {
		t.Fatalf("replaced resources = %#v, primary = %q", replaced.Resources, replaced.PrimaryResourceID)
	}
	var oldClaims, newClaims int
	if err := s.db.QueryRow(`SELECT count(*) FROM resource_claim_epochs WHERE resource_id=? AND released_event_id IS NULL`, oldID).Scan(&oldClaims); err != nil {
		t.Fatal(err)
	}
	if err := s.db.QueryRow(`SELECT count(*) FROM resource_claim_epochs WHERE resource_id=? AND released_event_id IS NULL`, replaced.Resources[0].ID).Scan(&newClaims); err != nil {
		t.Fatal(err)
	}
	if oldClaims != 0 || newClaims != 1 {
		t.Fatalf("active claims old=%d new=%d", oldClaims, newClaims)
	}
	removed, err := s.RemoveProjectResource(ctx, project.ID, replaced.HeadEventID, replaced.Resources[0].ID)
	if err != nil {
		t.Fatal(err)
	}
	if len(removed.Resources) != 0 || removed.PrimaryResourceID != "" || removed.Lifecycle != domain.ProjectOpen {
		t.Fatalf("removed project = %#v", removed)
	}
}

func TestResourceHealthDetectsReservedPathIdentityChange(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	root := t.TempDir()
	firstTarget, secondTarget := filepath.Join(root, "first"), filepath.Join(root, "second")
	if err := os.Mkdir(firstTarget, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(secondTarget, 0o700); err != nil {
		t.Fatal(err)
	}
	link := filepath.Join(root, "selected")
	if err := os.Symlink(firstTarget, link); err != nil {
		t.Fatal(err)
	}
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "health", Paths: []domain.ProjectPathInput{{DisplayPath: filepath.Join(link, "future")}}})
	if err != nil {
		t.Fatal(err)
	}
	if project.Resources[0].Health != domain.ResourceMissing {
		t.Fatalf("initial health = %q", project.Resources[0].Health)
	}
	if err := os.Remove(link); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(secondTarget, link); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(filepath.Join(secondTarget, "future"), 0o700); err != nil {
		t.Fatal(err)
	}
	checked, err := s.CheckProjectResource(ctx, project.ID, project.Resources[0].ID)
	if err != nil {
		t.Fatal(err)
	}
	if checked.Health != domain.ResourceMalformed || checked.HealthDetails["expected_canonical"] == "" || checked.HealthDetails["observed_canonical"] == "" {
		t.Fatalf("checked health = %#v", checked)
	}
	notices, err := s.List(ctx, model.Filter{SenderMailboxID: project.MailboxID, RecipientMailboxID: model.HumanMailboxID, Limit: 10})
	if err != nil || len(notices) != 1 || notices[0].SenderLabel != "HQ · health" || !strings.Contains(notices[0].Body, "condition changed") {
		t.Fatalf("degradation notices = %#v, %v", notices, err)
	}
	if _, err := s.CheckProjectResource(ctx, project.ID, project.Resources[0].ID); err != nil {
		t.Fatal(err)
	}
	notices, _ = s.List(ctx, model.Filter{SenderMailboxID: project.MailboxID, RecipientMailboxID: model.HumanMailboxID, Limit: 10})
	if len(notices) != 1 {
		t.Fatalf("identical health check emitted another notice: %#v", notices)
	}
	if err := os.Remove(link); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(firstTarget, link); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(filepath.Join(firstTarget, "future"), 0o700); err != nil {
		t.Fatal(err)
	}
	if _, err := s.CheckProjectResource(ctx, project.ID, project.Resources[0].ID); err != nil {
		t.Fatal(err)
	}
	notices, _ = s.List(ctx, model.Filter{SenderMailboxID: project.MailboxID, RecipientMailboxID: model.HumanMailboxID, Limit: 10})
	if len(notices) != 2 || !strings.Contains(notices[1].Body, "recovered") {
		t.Fatalf("recovery notices = %#v", notices)
	}
}

func TestProjectSchemaRejectsBrokenCoreInvariants(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "constraints", Paths: []domain.ProjectPathInput{{DisplayPath: t.TempDir()}}, Open: true})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.db.Exec(`UPDATE projects SET home_installation_id='elsewhere' WHERE id=?`, project.ID); err == nil {
		t.Fatal("mutable project home was accepted")
	}
	if _, err := s.db.Exec(`UPDATE resources SET canonical_locator='/different' WHERE id=?`, project.Resources[0].ID); err == nil {
		t.Fatal("mutable resource identity was accepted")
	}
	if _, err := s.db.Exec(`UPDATE projects SET lifecycle='closed' WHERE id=?`, project.ID); err == nil {
		t.Fatal("closed project retained active claim")
	}
	closed, err := s.BeginCloseProject(ctx, project.ID, project.HeadEventID)
	if err != nil {
		t.Fatal(err)
	}
	closed, err = s.FinalizeCloseProject(ctx, project.ID, closed.HeadEventID, false, "stopped")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.db.Exec(`INSERT INTO resource_claim_epochs(id,project_id,resource_id,acquired_event_id,acquired_at) VALUES (?,?,?,?,0)`, "019c0000-0000-7000-8000-000000000199", project.ID, project.Resources[0].ID, closed.HeadEventID); err == nil {
		t.Fatal("active claim on closed project was accepted")
	}
}

func TestProjectMessagesReceiveHomeSequenceWhileClosedAndArchived(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "mailbox"})
	if err != nil {
		t.Fatal(err)
	}
	first := model.Message{ID: "019c0000-0000-7000-8000-000000000301", SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: project.MailboxID, Body: "first accepted", CreatedAt: time.Now().UTC().Add(time.Hour)}
	second := model.Message{ID: "019c0000-0000-7000-8000-000000000302", SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: project.MailboxID, Body: "second accepted", CreatedAt: time.Now().UTC().Add(-time.Hour)}
	if err := s.Create(ctx, first); err != nil {
		t.Fatal(err)
	}
	afterFirst, err := s.GetProject(ctx, project.ID)
	if err != nil {
		t.Fatal(err)
	}
	if afterFirst.HeadEventID == project.HeadEventID {
		t.Fatal("message acceptance did not advance project history")
	}
	_, err = s.SetProjectArchived(ctx, project.ID, afterFirst.HeadEventID, true)
	if err != nil {
		t.Fatal(err)
	}
	if err := s.Create(ctx, second); err != nil {
		t.Fatal(err)
	}
	got, err := s.GetProject(ctx, project.ID)
	if err != nil {
		t.Fatal(err)
	}
	if !got.Archived || got.Lifecycle != domain.ProjectClosed {
		t.Fatalf("message changed archived lifecycle: %#v", got)
	}
	var notices int
	if err := s.db.QueryRow(`SELECT count(*) FROM messages WHERE sender_mailbox_id=? AND recipient_mailbox_id=? AND actor_label=? AND body LIKE 'New activity is waiting%'`, project.MailboxID, model.HumanMailboxID, "HQ · "+project.Name).Scan(&notices); err != nil || notices != 2 {
		t.Fatalf("pending project notices = %d, %v", notices, err)
	}
	if err := s.db.QueryRow(`SELECT count(*) FROM messages WHERE sender_mailbox_id=? AND presentation='notice' AND details='' AND technical_sections_json LIKE '%"namespace":"hq.project.pending_message"%'`, project.MailboxID).Scan(&notices); err != nil || notices != 2 {
		t.Fatalf("typed pending project notices = %d, %v", notices, err)
	}
	rows, err := s.db.Query(`SELECT sequence,message_id FROM project_message_acceptances WHERE project_id=? ORDER BY sequence`, project.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	want := []string{first.ID, second.ID}
	for index := 0; rows.Next(); index++ {
		var sequence int
		var id string
		if err := rows.Scan(&sequence, &id); err != nil {
			t.Fatal(err)
		}
		if index >= len(want) || sequence != index+1 || id != want[index] {
			t.Fatalf("acceptance %d = sequence %d message %s", index, sequence, id)
		}
		want[index] = ""
	}
	for _, missing := range want {
		if missing != "" {
			t.Fatalf("missing acceptance for %s", missing)
		}
	}
	if err := s.Rebuild(ctx); err != nil {
		t.Fatal(err)
	}
	var count int
	if err := s.db.QueryRow(`SELECT count(*) FROM project_message_acceptances WHERE project_id=?`, project.ID).Scan(&count); err != nil || count != 2 {
		t.Fatalf("acceptances after rebuild = %d, %v", count, err)
	}
}

func TestProjectDispatchIsOrderedAndBoundToRunnableAssignment(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	clock := time.Now().UTC()
	s.now = func() time.Time { return clock }
	ctx := context.Background()
	if _, err := s.CreateNamedAgent(ctx, "alice", ""); err != nil {
		t.Fatal(err)
	}
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "dispatch", Open: true, Paths: []domain.ProjectPathInput{{DisplayPath: t.TempDir()}}})
	if err != nil {
		t.Fatal(err)
	}
	project, err = s.AssignProject(ctx, project.ID, project.HeadEventID, "alice")
	if err != nil {
		t.Fatal(err)
	}
	project, err = s.ActivateProjectAssignment(ctx, project.ID, project.HeadEventID, domain.ActivateProjectAssignmentRequest{Harness: "codex", ExternalThread: "codex-thread", LaunchDirectory: t.TempDir()})
	if err != nil {
		t.Fatal(err)
	}
	for index, id := range []string{"019c0000-0000-7000-8000-000000000311", "019c0000-0000-7000-8000-000000000312"} {
		message := model.Message{ID: id, SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: project.MailboxID, Body: fmt.Sprintf("message %d", index+1), CreatedAt: time.Now().UTC()}
		if err := s.Create(ctx, message); err != nil {
			t.Fatal(err)
		}
	}
	project, err = s.GetProject(ctx, project.ID)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.Claim(ctx, domain.Claim{RecipientMailboxID: project.MailboxID}, "ordinary"); !errors.Is(err, domain.ErrNotReady) {
		t.Fatalf("ordinary claim consumed project work: %v", err)
	}
	first, err := s.ClaimProjectMessage(ctx, project.ID, project.Assignment.ID, project.Assignment.SelectedThreadID, "owner-one")
	if err != nil {
		t.Fatal(err)
	}
	if first.Sequence != 1 || first.Message.Body != "message 1" || first.AgentName != "alice" || first.ExternalThreadID != "codex-thread" {
		t.Fatalf("first delivery = %#v", first)
	}
	if _, err := s.ClaimProjectMessage(ctx, project.ID, project.Assignment.ID, project.Assignment.SelectedThreadID, "owner-two"); !errors.Is(err, domain.ErrClaimed) {
		t.Fatalf("concurrent project claim = %v", err)
	}
	if err := s.MarkProjectDispatchUncertain(ctx, first.Message.ID, "owner-one"); err != nil {
		t.Fatal(err)
	}
	if err := s.RecordProjectDispatch(ctx, first.Message.ID, "owner-one"); err != nil {
		t.Fatal(err)
	}
	clock = clock.Add(projectDispatchLease + time.Second)
	recovered, err := s.ClaimProjectMessage(ctx, project.ID, project.Assignment.ID, project.Assignment.SelectedThreadID, "recovery-owner")
	if err != nil || !recovered.Dispatched || recovered.Message.ID != first.Message.ID {
		t.Fatalf("recover recorded dispatch = %#v, %v", recovered, err)
	}
	if err := s.Complete(ctx, first.Message.ID, "recovery-owner"); err != nil {
		t.Fatal(err)
	}
	second, err := s.ClaimProjectMessage(ctx, project.ID, project.Assignment.ID, project.Assignment.SelectedThreadID, "owner-two")
	if err != nil {
		t.Fatal(err)
	}
	if second.Sequence != 2 || second.Message.Body != "message 2" {
		t.Fatalf("second delivery = %#v", second)
	}
	var assignmentID, agent, external string
	if err := s.db.QueryRow(`SELECT assignment_id,agent_name,external_thread_id FROM project_dispatch_records WHERE message_id=?`, first.Message.ID).Scan(&assignmentID, &agent, &external); err != nil {
		t.Fatal(err)
	}
	if assignmentID != project.Assignment.ID || agent != "alice" || external != "codex-thread" {
		t.Fatalf("dispatch provenance = %s %s %s", assignmentID, agent, external)
	}
	head := project.HeadEventID
	if err := s.db.QueryRow(`SELECT head_event_id FROM projects WHERE id=?`, project.ID).Scan(&head); err != nil {
		t.Fatal(err)
	}
	if err := s.Rebuild(ctx); err != nil {
		t.Fatal(err)
	}
	rebuilt, err := s.GetProject(ctx, project.ID)
	if err != nil {
		t.Fatal(err)
	}
	if rebuilt.HeadEventID != head || rebuilt.Assignment == nil || rebuilt.Assignment.ID != project.Assignment.ID || len(rebuilt.Resources) != 1 {
		t.Fatalf("rebuilt project = %#v", rebuilt)
	}
	for table, want := range map[string]int{"project_threads": 1, "project_assignment_epochs": 1, "project_message_acceptances": 2, "project_dispatch_records": 1, "resource_claim_epochs": 1} {
		var got int
		if err := s.db.QueryRow(`SELECT count(*) FROM `+table+` WHERE project_id=?`, project.ID).Scan(&got); err != nil || got != want {
			t.Fatalf("%s after clean rebuild = %d, want %d: %v", table, got, want, err)
		}
	}
}

func TestUncertainProjectDispatchBlocksTakeoverRedispatch(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	for _, name := range []string{"alice", "bob"} {
		if _, err := s.CreateNamedAgent(ctx, name, ""); err != nil {
			t.Fatal(err)
		}
	}
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "uncertain", Open: true})
	if err != nil {
		t.Fatal(err)
	}
	project, err = s.AssignProject(ctx, project.ID, project.HeadEventID, "alice")
	if err != nil {
		t.Fatal(err)
	}
	project, err = s.ActivateProjectAssignment(ctx, project.ID, project.HeadEventID, domain.ActivateProjectAssignmentRequest{Harness: "codex", ExternalThread: "old-thread", LaunchDirectory: t.TempDir()})
	if err != nil {
		t.Fatal(err)
	}
	message := model.Message{ID: "019c0000-0000-7000-8000-000000000321", SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: project.MailboxID, Body: "once", CreatedAt: time.Now().UTC()}
	if err := s.Create(ctx, message); err != nil {
		t.Fatal(err)
	}
	project, _ = s.GetProject(ctx, project.ID)
	delivery, err := s.ClaimProjectMessage(ctx, project.ID, project.Assignment.ID, project.Assignment.SelectedThreadID, "old-owner")
	if err != nil {
		t.Fatal(err)
	}
	if err := s.MarkProjectDispatchUncertain(ctx, delivery.Message.ID, "old-owner"); err != nil {
		t.Fatal(err)
	}
	if err := s.ReleaseProjectMessage(ctx, delivery.Message.ID, "old-owner"); err != nil {
		t.Fatal(err)
	}
	project, err = s.UnassignProject(ctx, project.ID, project.HeadEventID, true, "unknown")
	if err != nil {
		t.Fatal(err)
	}
	project, err = s.AssignProject(ctx, project.ID, project.HeadEventID, "bob")
	if err != nil {
		t.Fatal(err)
	}
	project, err = s.ActivateProjectAssignment(ctx, project.ID, project.HeadEventID, domain.ActivateProjectAssignmentRequest{Harness: "codex", ExternalThread: "new-thread", LaunchDirectory: t.TempDir()})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.ClaimProjectMessage(ctx, project.ID, project.Assignment.ID, project.Assignment.SelectedThreadID, "new-owner"); !errors.Is(err, domain.ErrClaimed) {
		t.Fatalf("takeover redispatch = %v", err)
	}
}

func TestProjectOutputRetainsAssignmentProvenanceAndMarksLateRuntime(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	for _, name := range []string{"alice", "bob"} {
		if _, err := s.CreateNamedAgent(ctx, name, ""); err != nil {
			t.Fatal(err)
		}
	}
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "output project", Open: true})
	if err != nil {
		t.Fatal(err)
	}
	project, err = s.AssignProject(ctx, project.ID, project.HeadEventID, "alice")
	if err != nil {
		t.Fatal(err)
	}
	project, err = s.ActivateProjectAssignment(ctx, project.ID, project.HeadEventID, domain.ActivateProjectAssignmentRequest{Harness: "codex", ExternalThread: "old-output-thread", LaunchDirectory: t.TempDir()})
	if err != nil {
		t.Fatal(err)
	}
	old := domain.ProjectOutputBinding{ProjectID: project.ID, AssignmentID: project.Assignment.ID, AgentName: "alice", ProjectThreadID: project.Assignment.SelectedThreadID, ExternalThreadID: "old-output-thread", RuntimeOwner: "old-owner", RuntimeState: "connected"}
	currentID := "019c0000-0000-7000-8000-000000000351"
	currentInput := model.Message{
		ID: currentID, SenderMailboxID: project.MailboxID, RecipientMailboxID: model.HumanMailboxID, Body: "current output", Details: "Visible explanation.",
		Presentation:      model.PresentationFinalAnswer,
		Correlation:       model.MessageCorrelation{Provider: "home-built", SessionID: "output-session", OperationID: "output-operation", ItemID: "output-item"},
		TechnicalSections: []model.TechnicalSection{{Namespace: "vendor.output", Fields: []model.TechnicalField{{Key: "diagnostic", Value: "kept"}}}},
		CreatedAt:         time.Now().UTC(),
	}
	if err := s.CreateProjectOutput(ctx, old, currentInput); err != nil {
		t.Fatal(err)
	}
	if err := s.CreateProjectOutput(ctx, old, currentInput); err != nil {
		t.Fatalf("idempotent project output retry: %v", err)
	}
	collision := currentInput
	collision.Presentation = model.PresentationStatus
	if err := s.CreateProjectOutput(ctx, old, collision); err == nil || !strings.Contains(err.Error(), "collides") {
		t.Fatalf("project output collision = %v", err)
	}
	current, err := s.Get(ctx, currentID)
	if err != nil {
		t.Fatal(err)
	}
	if current.SenderLabel != "alice · output project" || current.Details != currentInput.Details || current.Presentation != currentInput.Presentation || current.Correlation != currentInput.Correlation || !technicalFieldEquals(current.TechnicalSections, "vendor.output", "diagnostic", "kept") || !technicalFieldEquals(current.TechnicalSections, "hq.project.output_provenance", "assignment_id", old.AssignmentID) {
		t.Fatalf("current output = %#v", current)
	}
	if len(current.TechnicalSections) != 2 || current.TechnicalSections[0].Namespace != "vendor.output" || current.TechnicalSections[1].Namespace != "hq.project.output_provenance" || strings.Join(technicalFieldKeys(current.TechnicalSections, "hq.project.output_provenance"), ",") != "project_id,assignment_id,project_thread_id" {
		t.Fatalf("current technical order = %#v", current.TechnicalSections)
	}
	assertCanonicalMessageSchema(t, s, currentID, event.Schema3)
	project, err = s.UnassignProject(ctx, project.ID, project.HeadEventID, true, "forced takeover")
	if err != nil {
		t.Fatal(err)
	}
	project, err = s.AssignProject(ctx, project.ID, project.HeadEventID, "bob")
	if err != nil {
		t.Fatal(err)
	}
	project, err = s.ActivateProjectAssignment(ctx, project.ID, project.HeadEventID, domain.ActivateProjectAssignmentRequest{Harness: "codex", ExternalThread: "new-output-thread", LaunchDirectory: t.TempDir()})
	if err != nil {
		t.Fatal(err)
	}
	lateID := "019c0000-0000-7000-8000-000000000352"
	if err := s.CreateProjectOutput(ctx, old, model.Message{ID: lateID, SenderMailboxID: project.MailboxID, RecipientMailboxID: model.HumanMailboxID, Body: "old runtime spoke", CreatedAt: time.Now().UTC()}); err != nil {
		t.Fatal(err)
	}
	late, err := s.Get(ctx, lateID)
	if err != nil {
		t.Fatal(err)
	}
	if late.SenderLabel != "alice · output project (late from inactive assignment)" || !technicalFieldEquals(late.TechnicalSections, "hq.project.output_provenance", "late", "yes") || !technicalFieldEquals(late.TechnicalSections, "hq.project.output_provenance", "current_assignment_id", project.Assignment.ID) {
		t.Fatalf("late output = %#v", late)
	}
	if strings.Join(technicalFieldKeys(late.TechnicalSections, "hq.project.output_provenance"), ",") != "project_id,assignment_id,project_thread_id,late,current_assignment_id,current_agent,current_project_thread_id" {
		t.Fatalf("late technical order = %#v", late.TechnicalSections)
	}
	var markedLate, forced int
	var currentAssignment, owner string
	if err := s.db.QueryRow(`SELECT late,current_assignment_id,runtime_owner_token,forced_transition FROM project_output_provenance WHERE message_id=?`, lateID).Scan(&markedLate, &currentAssignment, &owner, &forced); err != nil {
		t.Fatal(err)
	}
	if markedLate != 1 || forced != 1 || currentAssignment != project.Assignment.ID || owner != "old-owner" {
		t.Fatalf("provenance late=%d forced=%d current=%q owner=%q", markedLate, forced, currentAssignment, owner)
	}
}

func technicalFieldEquals(sections []model.TechnicalSection, namespace, key, value string) bool {
	for _, section := range sections {
		if section.Namespace != namespace {
			continue
		}
		for _, field := range section.Fields {
			if field.Key == key && field.Value == value {
				return true
			}
		}
	}
	return false
}

func technicalFieldKeys(sections []model.TechnicalSection, namespace string) []string {
	for _, section := range sections {
		if section.Namespace != namespace {
			continue
		}
		keys := make([]string, len(section.Fields))
		for index, field := range section.Fields {
			keys[index] = field.Key
		}
		return keys
	}
	return nil
}

func assertCanonicalMessageSchema(t *testing.T, s *SQLite, messageID string, want int) {
	t.Helper()
	var raw []byte
	if err := s.db.QueryRow(`SELECT c.raw FROM canonical_events c JOIN messages m ON m.event_id=c.event_id WHERE m.id=?`, messageID).Scan(&raw); err != nil {
		t.Fatal(err)
	}
	inspection := event.Inspect(raw)
	if inspection.Status != event.StatusProjected || inspection.Event.Content.Schema != want {
		t.Fatalf("canonical message schema = %d, status %s, error %v", inspection.Event.Content.Schema, inspection.Status, inspection.Err)
	}
}

func TestReplyToProjectOutputIsAcceptedAndDispatchable(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	if _, err := s.CreateNamedAgent(ctx, "alice", ""); err != nil {
		t.Fatal(err)
	}
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "reply project", Open: true})
	if err != nil {
		t.Fatal(err)
	}
	project, err = s.AssignProject(ctx, project.ID, project.HeadEventID, "alice")
	if err != nil {
		t.Fatal(err)
	}
	project, err = s.ActivateProjectAssignment(ctx, project.ID, project.HeadEventID, domain.ActivateProjectAssignmentRequest{Harness: "codex", ExternalThread: "reply-thread", LaunchDirectory: t.TempDir()})
	if err != nil {
		t.Fatal(err)
	}
	binding := domain.ProjectOutputBinding{ProjectID: project.ID, AssignmentID: project.Assignment.ID, AgentName: "alice", ProjectThreadID: project.Assignment.SelectedThreadID, ExternalThreadID: "reply-thread", RuntimeState: "connected"}
	outputID := "019c0000-0000-7000-8000-000000000361"
	if err := s.CreateProjectOutput(ctx, binding, model.Message{ID: outputID, SenderMailboxID: project.MailboxID, RecipientMailboxID: model.HumanMailboxID, Body: "project answer", Details: "Kind: final-answer", CreatedAt: time.Now().UTC()}); err != nil {
		t.Fatal(err)
	}
	replyID := "019c0000-0000-7000-8000-000000000362"
	reply := model.Message{ID: replyID, SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: project.MailboxID, Body: "follow-up", CreatedAt: time.Now().UTC()}
	if err := s.Reply(ctx, outputID, reply); err != nil {
		t.Fatal(err)
	}
	var sequence int64
	if err := s.db.QueryRow(`SELECT sequence FROM project_message_acceptances WHERE project_id=? AND message_id=?`, project.ID, replyID).Scan(&sequence); err != nil || sequence != 1 {
		t.Fatalf("reply acceptance sequence = %d, %v", sequence, err)
	}
	original, err := s.Get(ctx, outputID)
	if err != nil || original.ArchivedAt == nil {
		t.Fatalf("original output = %#v, %v", original, err)
	}
	storedReply, err := s.Get(ctx, replyID)
	if err != nil || storedReply.Purpose != model.MessagePurposeProjectInput || storedReply.RecipientAddress.Kind != model.MailboxProject || storedReply.RecipientLabel != project.Name {
		t.Fatalf("stored reply = %#v, %v", storedReply, err)
	}
	delivery, err := s.ClaimProjectMessage(ctx, project.ID, project.Assignment.ID, project.Assignment.SelectedThreadID, "reply-owner")
	if err != nil || delivery.Message.ID != replyID || delivery.Sequence != 1 || delivery.AgentName != "alice" {
		t.Fatalf("reply delivery = %#v, %v", delivery, err)
	}
}

func TestStructuredReplyToProjectQuestionIsNotAcceptedAsConversation(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "approval project", Open: true})
	if err != nil {
		t.Fatal(err)
	}
	questionID := "019c0000-0000-7000-8000-000000000365"
	question := model.Message{ID: questionID, SenderMailboxID: project.MailboxID, RecipientMailboxID: model.HumanMailboxID, Purpose: model.MessagePurposeProtocolQuestion, Body: "Approve?", CreatedAt: time.Now().UTC()}
	if err := s.Create(ctx, question); err != nil {
		t.Fatal(err)
	}
	replyID := "019c0000-0000-7000-8000-000000000366"
	if err := s.Reply(ctx, questionID, model.Message{ID: replyID, SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: project.MailboxID, Body: "yes", CreatedAt: time.Now().UTC()}); err != nil {
		t.Fatal(err)
	}
	stored, err := s.Get(ctx, replyID)
	if err != nil || stored.Purpose != model.MessagePurposeProtocolAnswer {
		t.Fatalf("protocol reply = %#v, %v", stored, err)
	}
	var acceptances int
	if err := s.db.QueryRow(`SELECT count(*) FROM project_message_acceptances WHERE message_id=?`, replyID).Scan(&acceptances); err != nil || acceptances != 0 {
		t.Fatalf("protocol reply acceptances = %d, %v", acceptances, err)
	}
}

func TestGenericCanonicalAppendAcceptsLocalProjectReplyAndRestartIsIdempotent(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	s := openStore(t, database)
	ctx := context.Background()
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "repair replies"})
	if err != nil {
		t.Fatal(err)
	}
	outputID := "019c0000-0000-7000-8000-000000000371"
	if err := s.Create(ctx, model.Message{ID: outputID, SenderMailboxID: project.MailboxID, RecipientMailboxID: model.HumanMailboxID, Body: "old project output", CreatedAt: time.Now().UTC()}); err != nil {
		t.Fatal(err)
	}
	original, err := s.messageRecord(ctx, outputID)
	if err != nil {
		t.Fatal(err)
	}
	account, parents, deviceLabel, err := s.localAccountAction(ctx, "")
	if err != nil {
		t.Fatal(err)
	}
	replyID := "019c0000-0000-7000-8000-000000000372"
	payload, _ := event.MarshalPayload(event.TextPayload{MessageID: replyID, Body: "stranded follow-up", ActorLabel: deviceLabel})
	content := event.Content{Type: event.TypeAnswer, Sender: s.localAddress(model.HumanMailboxID), Recipient: s.localAddress(project.MailboxID), Audience: &event.Audience{HumanAccountID: account.ID}, ThreadID: original.eventID, Parents: uniqueSorted(append(parents, original.eventID)), Authorities: uniqueSorted(parents), Scope: event.ScopeAccountAddressed, Payload: payload}
	if err := s.appendContents(ctx, []event.Content{content}, []time.Time{time.Now().UTC()}, nil); err != nil {
		t.Fatal(err)
	}
	var before int
	if err := s.db.QueryRow(`SELECT count(*) FROM project_message_acceptances WHERE message_id=?`, replyID).Scan(&before); err != nil || before != 1 {
		t.Fatalf("generic append acceptance count=%d err=%v", before, err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	reopened, err := Open(database)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { reopened.Close() })
	var sequence int64
	if err := reopened.db.QueryRow(`SELECT sequence FROM project_message_acceptances WHERE project_id=? AND message_id=?`, project.ID, replyID).Scan(&sequence); err != nil || sequence != 1 {
		t.Fatalf("reply sequence after restart = %d, %v", sequence, err)
	}
}

func TestRebuildReconcilesProjectInputMissingFromLegacyHistory(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "legacy input"})
	if err != nil {
		t.Fatal(err)
	}
	account, parents, deviceLabel, err := s.localAccountAction(ctx, "")
	if err != nil {
		t.Fatal(err)
	}
	messageID := "019c0000-0000-7000-8000-000000000373"
	payload, _ := event.MarshalPayload(event.TextPayload{MessageID: messageID, Body: "legacy unaccepted input", Purpose: model.MessagePurposeProjectInput, ActorLabel: deviceLabel})
	content := event.Content{Type: event.TypeMessage, Sender: s.localAddress(model.HumanMailboxID), Recipient: s.localAddress(project.MailboxID), Audience: &event.Audience{HumanAccountID: account.ID}, Parents: parents, Authorities: uniqueSorted(parents), Scope: event.ScopeAccountAddressed, Payload: payload}
	signed, err := s.signContents(ctx, []event.Content{content}, []time.Time{time.Now().UTC()})
	if err != nil {
		t.Fatal(err)
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.ingestCanonicalProjectionTx(ctx, tx, signed, true); err != nil {
		t.Fatal(err)
	}
	if err := tx.Commit(); err != nil {
		t.Fatal(err)
	}
	var count int
	if err := s.db.QueryRow(`SELECT count(*) FROM project_message_acceptances WHERE message_id=?`, messageID).Scan(&count); err != nil || count != 0 {
		t.Fatalf("legacy acceptance before rebuild=%d err=%v", count, err)
	}
	if err := s.Rebuild(ctx); err != nil {
		t.Fatal(err)
	}
	if err := s.db.QueryRow(`SELECT count(*) FROM project_message_acceptances WHERE message_id=?`, messageID).Scan(&count); err != nil || count != 1 {
		t.Fatalf("legacy acceptance after rebuild=%d err=%v", count, err)
	}
	if err := s.Rebuild(ctx); err != nil {
		t.Fatal(err)
	}
	if err := s.db.QueryRow(`SELECT count(*) FROM project_message_acceptances WHERE message_id=?`, messageID).Scan(&count); err != nil || count != 1 {
		t.Fatalf("legacy acceptance after repeated rebuild=%d err=%v", count, err)
	}
}
