package store

import (
	"context"
	"encoding/json"
	"path/filepath"
	"reflect"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/localwire"
	"github.com/wbbradley/hq/internal/model"
)

func TestPendingProjectCommandResumesOnceAfterReopen(t *testing.T) {
	path := filepath.Join(t.TempDir(), "hq.db")
	database := openStore(t, path)
	ctx := context.Background()
	project, err := database.CreateProject(ctx, domain.CreateProjectRequest{Name: "restart command", Open: true})
	if err != nil {
		t.Fatal(err)
	}
	account, membership, _, err := database.localAccountAction(ctx, "")
	if err != nil {
		t.Fatal(err)
	}
	commandID := "019d0000-0000-7000-8000-000000000811"
	payload, err := event.MarshalPayload(event.ProjectCommandPayload{
		CommandID: commandID, ProjectID: project.ID, ExpectedHead: project.HeadEventID,
		Operation: "project.future", Body: json.RawMessage(`{}`),
	})
	if err != nil {
		t.Fatal(err)
	}
	parents := uniqueSorted(append(membership, project.HeadEventID))
	signed := signConformanceEvent(t, database, event.Content{
		Type: event.TypeProjectCommand, Sender: database.localAddress(model.HumanMailboxID), Recipient: database.localAddress(model.HumanMailboxID),
		Audience: &event.Audience{HumanAccountID: account.ID}, Parents: parents, Authorities: uniqueSorted(membership), Scope: event.ScopeAccountAddressed, Payload: payload,
	}, time.Unix(1_900_000_200, 0).UTC())
	tx, err := database.db.BeginTx(ctx, nil)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := database.ingestCanonicalTx(ctx, tx, []event.SignedEvent{signed}, true); err != nil {
		tx.Rollback()
		t.Fatal(err)
	}
	if err := tx.Commit(); err != nil {
		t.Fatal(err)
	}
	if err := database.Close(); err != nil {
		t.Fatal(err)
	}
	database, err = Open(path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Close() })
	if err := database.ProcessProjectCommands(ctx); err != nil {
		t.Fatal(err)
	}
	if err := database.ProcessProjectCommands(ctx); err != nil {
		t.Fatal(err)
	}
	rows, err := database.db.Query(`SELECT raw FROM canonical_events WHERE event_type=?`, event.TypeProjectResult)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	received, rejected := 0, 0
	for rows.Next() {
		var raw []byte
		if err := rows.Scan(&raw); err != nil {
			t.Fatal(err)
		}
		var result event.ProjectCommandResultPayload
		if json.Unmarshal(event.Inspect(raw).Event.Content.Payload, &result) != nil || result.CommandID != commandID {
			continue
		}
		switch result.Stage {
		case string(domain.ProjectCommandReceived):
			received++
		case string(domain.ProjectCommandRejected):
			rejected++
		}
	}
	if received != 1 || rejected != 1 {
		t.Fatalf("resumed project command results: received=%d rejected=%d; want one of each", received, rejected)
	}
}

func TestSchema3ProtocolStateConformsAcrossReopenAndRepair(t *testing.T) {
	path := filepath.Join(t.TempDir(), "hq.db")
	database := openStore(t, path)
	ctx := context.Background()
	if localwire.DomainVersions.Min != 7 || localwire.DomainVersions.Max != 7 {
		t.Fatalf("domain wire versions = %#v; want exactly 7", localwire.DomainVersions)
	}
	var databaseVersion int
	if err := database.db.QueryRow(`PRAGMA user_version`).Scan(&databaseVersion); err != nil || databaseVersion != 33 {
		t.Fatalf("database version = %d, %v; want 33", databaseVersion, err)
	}
	agent := resolveAgent(t, database, "codex", "protocol-restart", "/repo")
	messageID := "019d0000-0000-7000-8000-000000000801"
	if err := database.Create(ctx, model.Message{
		ID: messageID, SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: agent.ID,
		Body: "survives reopen and repair", CreatedAt: time.Unix(1_900_000_100, 0).UTC(),
	}); err != nil {
		t.Fatal(err)
	}
	draft, err := database.PutTUIDraft(ctx, domain.TUIDraft{
		ID: "019d0000-0000-7000-8000-000000000802", Body: "unsigned restart state",
		RecipientMailboxID: agent.ID, RecipientLabel: agent.Label,
	})
	if err != nil {
		t.Fatal(err)
	}
	mutation := domain.Mutation{ID: "019d0000-0000-7000-8000-000000000803", Method: "relay/add", RequestDigest: "protocol-restart-relay"}
	if err := database.AddRelay(domain.WithMutation(ctx, mutation), RelayConfig{URL: "wss://relay.example", Read: true, Write: true, RequireAuth: true}); err != nil {
		t.Fatal(err)
	}
	peerKey := event.MustSecretKeyFromHex("801")
	if err := database.TrustPeer(ctx, Peer{
		InstallationID: "019d0000-0000-7000-8000-000000000804", SignerKeyID: peerKey.PublicKeyHex(),
		Name: "restart peer", Relays: []string{"wss://relay.example"},
	}); err != nil {
		t.Fatal(err)
	}
	wantCanonical := stableTableRows(t, database.db, "canonical_events", nil)
	wantOutbox := stableTableRows(t, database.db, "outbox", nil)
	if len(wantOutbox) == 0 {
		t.Fatal("peer capability produced no durable outbox state")
	}
	assertCanonicalSchema3(t, database)
	if err := database.Close(); err != nil {
		t.Fatal(err)
	}

	database, err = Open(path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Close() })
	if got, err := database.Get(ctx, messageID); err != nil || got.Body != "survives reopen and repair" {
		t.Fatalf("reopened message = %#v, %v", got, err)
	}
	if drafts, err := database.ListTUIDrafts(ctx); err != nil || len(drafts) != 1 || drafts[0].ID != draft.ID || drafts[0].Version != draft.Version {
		t.Fatalf("reopened drafts = %#v, %v", drafts, err)
	}
	if _, found, err := database.MutationResult(ctx, mutation); err != nil || !found {
		t.Fatalf("reopened mutation receipt found=%t err=%v", found, err)
	}
	if err := database.Rebuild(ctx); err != nil {
		t.Fatal(err)
	}
	assertCanonicalSchema3(t, database)
	if got := stableTableRows(t, database.db, "canonical_events", nil); !reflect.DeepEqual(got, wantCanonical) {
		t.Fatalf("repair changed exact canonical log\nwant=%#v\ngot=%#v", wantCanonical, got)
	}
	if got := stableTableRows(t, database.db, "outbox", nil); !reflect.DeepEqual(got, wantOutbox) {
		t.Fatalf("repair changed durable outbox state\nwant=%#v\ngot=%#v", wantOutbox, got)
	}
	if drafts, err := database.ListTUIDrafts(ctx); err != nil || len(drafts) != 1 || drafts[0].ID != draft.ID {
		t.Fatalf("drafts after repair = %#v, %v", drafts, err)
	}
	if _, found, err := database.MutationResult(ctx, mutation); err != nil || !found {
		t.Fatalf("mutation receipt after repair found=%t err=%v", found, err)
	}
	if relays, err := database.ListRelays(ctx); err != nil || len(relays) != 1 || relays[0].URL != "wss://relay.example" {
		t.Fatalf("relays after repair = %#v, %v", relays, err)
	}
}

func assertCanonicalSchema3(t *testing.T, database *SQLite) {
	t.Helper()
	rows, err := database.db.Query(`SELECT raw FROM canonical_events`)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	for rows.Next() {
		var raw []byte
		if err := rows.Scan(&raw); err != nil {
			t.Fatal(err)
		}
		inspection := event.Inspect(raw)
		if inspection.Status == event.StatusInvalid || inspection.Event.Content.Schema != event.Schema3 {
			t.Fatalf("canonical event is not valid schema 3: %#v", inspection)
		}
	}
	if err := rows.Err(); err != nil {
		t.Fatal(err)
	}
}
