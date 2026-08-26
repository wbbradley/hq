package store

import (
	"context"
	"errors"
	"fmt"
	"math/rand"
	"path/filepath"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/projectstate"
)

type canonicalArrivalSchedule struct {
	name    string
	batches [][]int
}

func runSignedDAGConformance(t *testing.T, build func(*testing.T, *SQLite) []event.SignedEvent) {
	t.Helper()
	probe := openStore(t, filepath.Join(t.TempDir(), "probe.db"))
	eventCount := len(build(t, probe))
	if eventCount == 0 {
		t.Fatal("signed DAG fixture produced no events")
	}
	runSignedDAGConformanceSchedules(t, build, defaultCanonicalArrivalSchedules(eventCount))
}

func runSignedDAGConformanceSchedules(t *testing.T, build func(*testing.T, *SQLite) []event.SignedEvent, schedules []canonicalArrivalSchedule) {
	t.Helper()
	probe := openStore(t, filepath.Join(t.TempDir(), "schedule-probe.db"))
	eventCount := len(build(t, probe))
	for _, schedule := range schedules {
		t.Run(schedule.name, func(t *testing.T) {
			database := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
			events := build(t, database)
			if len(events) != eventCount {
				t.Fatalf("fixture event count changed from %d to %d", eventCount, len(events))
			}
			for batchIndex, indexes := range schedule.batches {
				batch := make([]event.SignedEvent, 0, len(indexes))
				for _, index := range indexes {
					batch = append(batch, events[index])
				}
				if err := database.AppendCanonical(context.Background(), batch); err != nil {
					t.Fatalf("arrival batch %d indexes %v: %v", batchIndex, indexes, err)
				}
			}
			assertIncrementalMatchesBatchRebuild(t, database)
		})
	}
}

func defaultCanonicalArrivalSchedules(eventCount int) []canonicalArrivalSchedule {
	var schedules []canonicalArrivalSchedule
	for prefix := 1; prefix <= eventCount; prefix++ {
		batch := make([]int, prefix)
		for index := range prefix {
			batch[index] = index
		}
		schedules = append(schedules, canonicalArrivalSchedule{name: fmt.Sprintf("prefix-%02d", prefix), batches: [][]int{batch}})
	}
	forward, reverse, shuffled := make([][]int, eventCount), make([][]int, eventCount), make([]int, eventCount)
	for index := range eventCount {
		forward[index] = []int{index}
		reverse[index] = []int{eventCount - index - 1}
		shuffled[index] = index
	}
	rand.New(rand.NewSource(0x4851)).Shuffle(eventCount, func(left, right int) {
		shuffled[left], shuffled[right] = shuffled[right], shuffled[left]
	})
	shuffledBatches := make([][]int, eventCount)
	for index, eventIndex := range shuffled {
		shuffledBatches[index] = []int{eventIndex}
	}
	duplicate := append(copySchedule(forward), []int{0}, []int{eventCount - 1}, []int{0, eventCount - 1})
	schedules = append(schedules,
		canonicalArrivalSchedule{name: "forward-one-at-a-time", batches: forward},
		canonicalArrivalSchedule{name: "reverse-late-dependencies", batches: reverse},
		canonicalArrivalSchedule{name: "seeded-shuffle", batches: shuffledBatches},
		canonicalArrivalSchedule{name: "duplicates", batches: duplicate},
	)
	return schedules
}

func copySchedule(schedule [][]int) [][]int {
	result := make([][]int, len(schedule))
	for index := range schedule {
		result[index] = append([]int(nil), schedule[index]...)
	}
	return result
}

func TestIncrementalReductionConformsForLateParentsAndDuplicates(t *testing.T) {
	runSignedDAGConformance(t, func(t *testing.T, database *SQLite) []event.SignedEvent {
		agent := resolveAgent(t, database, "codex", "conformance-late-parent", "/repo")
		started := time.Unix(1_800_000_000, 0).UTC()
		questionPayload := conformancePayload(t, event.TextPayload{MessageID: "019d0000-0000-7000-8000-000000000101", Body: "question"})
		question := signConformanceEvent(t, database, event.Content{
			Type: event.TypeQuestion, Sender: database.localAddress(agent.ID), Recipient: database.localAddress(model.HumanMailboxID),
			Scope: event.ScopeInstallationPrivate, Payload: questionPayload,
		}, started)
		answerPayload := conformancePayload(t, event.TextPayload{MessageID: "019d0000-0000-7000-8000-000000000102", Body: "answer"})
		answer := signConformanceEvent(t, database, event.Content{
			Type: event.TypeAnswer, Sender: database.localAddress(model.HumanMailboxID), Recipient: database.localAddress(agent.ID),
			ThreadID: question.ID(), Parents: []string{question.ID()}, Scope: event.ScopeInstallationPrivate, Payload: answerPayload,
		}, started.Add(time.Second))
		return []event.SignedEvent{question, answer}
	})
}

func TestIncrementalReductionConformsForMessageLifecycle(t *testing.T) {
	runSignedDAGConformance(t, func(t *testing.T, database *SQLite) []event.SignedEvent {
		agent := resolveAgent(t, database, "codex", "conformance-message-state", "/repo")
		started := time.Unix(1_800_000_100, 0).UTC()
		message := signConformanceEvent(t, database, event.Content{
			Type: event.TypeMessage, Sender: database.localAddress(model.HumanMailboxID), Recipient: database.localAddress(agent.ID), Scope: event.ScopeInstallationPrivate,
			Payload: conformancePayload(t, event.TextPayload{MessageID: "019d0000-0000-7000-8000-000000000111", Body: "stateful"}),
		}, started)
		archive := signMessageState(t, database, event.TypeMessageArchive, message.ID(), []string{message.ID()}, started.Add(time.Second))
		restore := signMessageState(t, database, event.TypeMessageRestore, message.ID(), []string{message.ID(), archive.ID()}, started.Add(2*time.Second))
		reject := signMessageState(t, database, event.TypeMessageReject, message.ID(), []string{message.ID(), restore.ID()}, started.Add(3*time.Second))
		return []event.SignedEvent{message, archive, restore, reject}
	})
}

func TestIncrementalReductionConformsForCapabilityRevokeAndRegrant(t *testing.T) {
	build := func(t *testing.T, database *SQLite) []event.SignedEvent {
		agent := resolveAgent(t, database, "codex", "conformance-capability", "/repo")
		remoteID := "019d0000-0000-7000-8000-000000000121"
		remoteKey := event.MustSecretKeyFromHex("121")
		started := time.Unix(1_800_000_200, 0).UTC()
		binding := signConformanceEvent(t, database, event.Content{
			Type: event.TypePeerBindingSet, Scope: event.ScopeInstallationPrivate,
			Payload: conformancePayload(t, event.PeerPayload{InstallationID: remoteID, SignerKeyID: remoteKey.PublicKeyHex(), Name: "remote"}),
		}, started)
		access := event.MailboxAccessPayload{MailboxID: agent.ID, GranteeInstallationID: remoteID, GranteeSignerKeyID: remoteKey.PublicKeyHex()}
		grant := signConformanceEvent(t, database, event.Content{
			Type: event.TypeMailboxAccessGrant, Sender: database.localAddress(model.HumanMailboxID), Recipient: &event.MailboxAddress{InstallationID: remoteID, MailboxID: model.HumanMailboxID},
			Scope: event.ScopePeerAddressed, Payload: conformancePayload(t, access),
		}, started.Add(time.Second))
		first := signRemoteConformanceMessage(t, database, remoteID, remoteKey, agent.ID, grant.ID(), "019d0000-0000-7000-8000-000000000122", started.Add(2*time.Second))
		revoke := signConformanceEvent(t, database, event.Content{
			Type: event.TypeMailboxAccessRevoke, Sender: database.localAddress(model.HumanMailboxID), Recipient: &event.MailboxAddress{InstallationID: remoteID, MailboxID: model.HumanMailboxID},
			Parents: []string{grant.ID()}, Authorities: []string{grant.ID()}, Scope: event.ScopePeerAddressed, Payload: conformancePayload(t, access),
		}, started.Add(3*time.Second))
		regrant := signConformanceEvent(t, database, event.Content{
			Type: event.TypeMailboxAccessGrant, Sender: database.localAddress(model.HumanMailboxID), Recipient: &event.MailboxAddress{InstallationID: remoteID, MailboxID: model.HumanMailboxID},
			Parents: []string{revoke.ID()}, Scope: event.ScopePeerAddressed, Payload: conformancePayload(t, access),
		}, started.Add(4*time.Second))
		second := signRemoteConformanceMessage(t, database, remoteID, remoteKey, agent.ID, regrant.ID(), "019d0000-0000-7000-8000-000000000123", started.Add(5*time.Second))
		return []event.SignedEvent{binding, grant, first, revoke, regrant, second}
	}
	forward := [][]int{{0}, {1}, {2}, {3}, {4}, {5}}
	runSignedDAGConformanceSchedules(t, build, []canonicalArrivalSchedule{
		{name: "prefix-binding", batches: [][]int{{0}}},
		{name: "prefix-grant", batches: [][]int{{0, 1}}},
		{name: "prefix-authorized-action", batches: [][]int{{0, 1, 2}}},
		{name: "revoke-action-regrant", batches: forward},
		{name: "late-peer-binding", batches: [][]int{{1}, {0}, {2}, {3}, {4}, {5}}},
		{name: "duplicates", batches: append(copySchedule(forward), []int{0}, []int{2}, []int{5})},
	})
}

func TestIncrementalReductionConformsForHumanMembership(t *testing.T) {
	build := func(t *testing.T, database *SQLite) []event.SignedEvent {
		ctx := context.Background()
		account, accountParents, _, err := database.localAccountAction(ctx, "")
		if err != nil {
			t.Fatal(err)
		}
		remoteID := "019d0000-0000-7000-8000-000000000131"
		remoteKey := event.MustSecretKeyFromHex("131")
		device := event.HumanDevicePayload{
			AccountID: account.ID, CreatorInstallationID: account.CreatorInstallationID, CreatorSignerKeyID: account.CreatorSignerKeyID,
			InstallationID: remoteID, SignerKeyID: remoteKey.PublicKeyHex(), Label: "tablet", Relays: []string{"wss://relay.example"},
		}
		started := time.Unix(1_800_000_300, 0).UTC()
		grant := signConformanceEvent(t, database, event.Content{
			Type: event.TypeHumanDeviceGrant, Sender: database.localAddress(model.HumanMailboxID), Recipient: &event.MailboxAddress{InstallationID: remoteID, MailboxID: model.HumanMailboxID},
			Audience: &event.Audience{HumanAccountID: account.ID}, Parents: accountParents, Authorities: accountParents, Scope: event.ScopeAccountAddressed, Payload: conformancePayload(t, device),
		}, started)
		accept, err := event.Sign(event.Content{
			Type: event.TypeHumanDeviceAccept, InstallationID: remoteID,
			Sender: &event.MailboxAddress{InstallationID: remoteID, MailboxID: model.HumanMailboxID}, Recipient: database.localAddress(model.HumanMailboxID),
			Audience: &event.Audience{HumanAccountID: account.ID}, Parents: []string{grant.ID()}, Authorities: []string{grant.ID()}, Scope: event.ScopeAccountAddressed, Payload: conformancePayload(t, device),
		}, started.Add(time.Second), remoteKey)
		if err != nil {
			t.Fatal(err)
		}
		message, err := event.Sign(event.Content{
			Type: event.TypeQuestion, InstallationID: remoteID,
			Sender:   &event.MailboxAddress{InstallationID: remoteID, MailboxID: "019d0000-0000-7000-8000-000000000133"},
			Audience: &event.Audience{HumanAccountID: account.ID}, Parents: []string{accept.ID()}, Authorities: []string{accept.ID()}, Scope: event.ScopeAccountAddressed,
			Payload: conformancePayload(t, event.TextPayload{MessageID: "019d0000-0000-7000-8000-000000000132", Body: "account traffic"}),
		}, started.Add(2*time.Second), remoteKey)
		if err != nil {
			t.Fatal(err)
		}
		revoke := signConformanceEvent(t, database, event.Content{
			Type: event.TypeHumanDeviceRevoke, Sender: database.localAddress(model.HumanMailboxID), Recipient: &event.MailboxAddress{InstallationID: remoteID, MailboxID: model.HumanMailboxID},
			Audience: &event.Audience{HumanAccountID: account.ID}, Parents: uniqueSorted([]string{grant.ID(), accept.ID()}), Authorities: []string{grant.ID()}, Scope: event.ScopeAccountAddressed, Payload: conformancePayload(t, device),
		}, started.Add(3*time.Second))
		return []event.SignedEvent{grant, accept, message, revoke}
	}
	forward := [][]int{{0}, {1}, {2}, {3}}
	runSignedDAGConformanceSchedules(t, build, []canonicalArrivalSchedule{
		{name: "prefix-grant", batches: [][]int{{0}}},
		{name: "prefix-accept", batches: [][]int{{0, 1}}},
		{name: "membership-revoke", batches: forward},
		{name: "late-membership", batches: [][]int{{3}, {2}, {1}, {0}}},
		{name: "duplicates", batches: append(copySchedule(forward), []int{0}, []int{1}, []int{2})},
	})
}

func TestIncrementalReductionConformsForActivityCoalescing(t *testing.T) {
	runSignedDAGConformance(t, func(t *testing.T, database *SQLite) []event.SignedEvent {
		ctx := context.Background()
		mailbox := harnessActivityMailbox(t, database, "conformance-activity")
		account, accountParents, _, err := database.localAccountAction(ctx, "")
		if err != nil {
			t.Fatal(err)
		}
		started := time.Unix(1_800_000_400, 0).UTC()
		var events []event.SignedEvent
		for index, body := range []string{"starting", "halfway", "done"} {
			parents := append([]string(nil), accountParents...)
			if len(events) != 0 {
				parents = append(parents, events[len(events)-1].ID())
			}
			payload := event.HarnessActivityPayload{
				Correlation: model.MessageCorrelation{Provider: "codex", SessionID: "conformance-activity", OperationID: "operation", ItemID: "progress"},
				Kind:        domain.HarnessActivityProgress, Status: domain.HarnessActivityRunning, Body: body,
				OccurredAt: started.Add(time.Duration(index) * time.Second).UnixMilli(), RuntimeID: "runtime", Sequence: uint64(index + 1),
			}
			events = append(events, signConformanceEvent(t, database, event.Content{
				Schema: event.Schema3, Type: event.TypeHarnessActivity, Sender: database.localAddress(mailbox.ID),
				Audience: &event.Audience{HumanAccountID: account.ID}, Parents: uniqueSorted(parents), Authorities: accountParents,
				Scope: event.ScopeAccountAddressed, Payload: conformancePayload(t, payload),
			}, started.Add(time.Duration(index)*time.Second)))
		}
		return events
	})
}

func TestIncrementalReductionConformsForProjectForkAndConflicts(t *testing.T) {
	database := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	resourceRoot := t.TempDir()
	first, err := database.CreateProject(ctx, domain.CreateProjectRequest{Name: "first", Open: true, Paths: []domain.ProjectPathInput{{DisplayPath: resourceRoot}}})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := database.CreateProject(ctx, domain.CreateProjectRequest{Name: "resource conflict", Open: true, Paths: []domain.ProjectPathInput{{DisplayPath: filepath.Join(resourceRoot, "child")}}}); !errors.Is(err, domain.ErrResourceConflict) {
		t.Fatalf("overlapping resource error = %v", err)
	}
	agent, err := database.CreateNamedAgent(ctx, "conflicted-agent", "")
	if err != nil {
		t.Fatal(err)
	}
	first, err = database.AssignProject(ctx, first.ID, first.HeadEventID, agent.Name)
	if err != nil {
		t.Fatal(err)
	}
	second, err := database.CreateProject(ctx, domain.CreateProjectRequest{Name: "second", Open: true})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := database.AssignProject(ctx, second.ID, second.HeadEventID, agent.Name); !errors.Is(err, domain.ErrAgentAssigned) {
		t.Fatalf("cross-project agent error = %v", err)
	}
	account, parents, _, err := database.localAccountAction(ctx, "")
	if err != nil {
		t.Fatal(err)
	}
	started := time.Unix(1_800_000_500, 0).UTC()
	one, _, err := database.signProjectEvent(ctx, first.ID, first.HeadEventID, projectstate.MetadataUpdated{Name: "fork-one"}, started, account.ID, parents)
	if err != nil {
		t.Fatal(err)
	}
	two, _, err := database.signProjectEvent(ctx, first.ID, first.HeadEventID, projectstate.MetadataUpdated{Name: "fork-two"}, started.Add(time.Second), account.ID, parents)
	if err != nil {
		t.Fatal(err)
	}
	if err := database.AppendCanonical(ctx, []event.SignedEvent{two, one, two}); err != nil {
		t.Fatal(err)
	}
	assertIncrementalMatchesBatchRebuild(t, database)
}

func conformancePayload(t *testing.T, value any) []byte {
	t.Helper()
	payload, err := event.MarshalPayload(value)
	if err != nil {
		t.Fatal(err)
	}
	return payload
}

func signConformanceEvent(t *testing.T, database *SQLite, content event.Content, createdAt time.Time) event.SignedEvent {
	t.Helper()
	signed, err := database.signer.Sign(context.Background(), content, createdAt)
	if err != nil {
		t.Fatal(err)
	}
	return signed
}

func signMessageState(t *testing.T, database *SQLite, kind event.Type, target string, parents []string, createdAt time.Time) event.SignedEvent {
	t.Helper()
	return signConformanceEvent(t, database, event.Content{
		Type: kind, Sender: database.localAddress(model.HumanMailboxID), Parents: uniqueSorted(parents), Scope: event.ScopeInstallationPrivate,
		Payload: conformancePayload(t, event.TargetPayload{TargetEventID: target, Reason: string(kind)}),
	}, createdAt)
}

func signRemoteConformanceMessage(t *testing.T, database *SQLite, remoteID string, remoteKey event.SecretKey, recipient, authority, messageID string, createdAt time.Time) event.SignedEvent {
	t.Helper()
	signed, err := event.Sign(event.Content{
		Type: event.TypeMessage, InstallationID: remoteID,
		Sender: &event.MailboxAddress{InstallationID: remoteID, MailboxID: model.HumanMailboxID}, Recipient: database.localAddress(recipient),
		Parents: []string{authority}, Authorities: []string{authority}, Scope: event.ScopePeerAddressed,
		Payload: conformancePayload(t, event.TextPayload{MessageID: messageID, Body: messageID}),
	}, createdAt, remoteKey)
	if err != nil {
		t.Fatal(err)
	}
	return signed
}
