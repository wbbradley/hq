package store

import (
	"bytes"
	"context"
	"errors"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/model"
)

func TestTwoInstallationsExchangeWrappedMessageAndDeduplicate(t *testing.T) {
	ctx := context.Background()
	senderPath := filepath.Join(t.TempDir(), "sender", "hq.db")
	receiverPath := filepath.Join(t.TempDir(), "receiver", "hq.db")
	sender := openStore(t, senderPath)
	receiver := openStore(t, receiverPath)
	senderID, senderKey := sender.InstallationIdentity()
	receiverID, receiverKey := receiver.InstallationIdentity()
	const relayOne = "wss://one.relay.test"
	const relayTwo = "wss://two.relay.test"
	if err := sender.TrustPeer(ctx, Peer{InstallationID: receiverID, SignerKeyID: receiverKey, Name: "receiver", Relays: []string{relayOne, relayTwo}}); err != nil {
		t.Fatal(err)
	}
	if err := receiver.TrustPeer(ctx, Peer{InstallationID: senderID, SignerKeyID: senderKey, Name: "sender", Relays: []string{relayOne}}); err != nil {
		t.Fatal(err)
	}
	var senderCommits, receiverCommits []domain.Invalidation
	sender.SetChangeObserver(func(commit domain.Invalidation) { senderCommits = append(senderCommits, commit) })
	receiver.SetChangeObserver(func(commit domain.Invalidation) { receiverCommits = append(receiverCommits, commit) })
	message := model.Message{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d90", SenderMailboxID: model.HumanMailboxID, Body: "wrapped hello", Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}
	if err := sender.CreatePeerMessage(ctx, message, receiverID, model.HumanMailboxID); err != nil {
		t.Fatal(err)
	}
	if prepared, err := sender.PrepareOutbound(ctx, 10); err != nil || prepared != 1 {
		t.Fatalf("prepared = %d, %v", prepared, err)
	}
	jobs, err := sender.RelayJobs(ctx, relayOne, 10, time.Now())
	if err != nil || len(jobs) != 1 {
		t.Fatalf("jobs = %#v, %v", jobs, err)
	}
	job := jobs[0]
	if len(senderCommits) != 1 || senderCommits[0].Revision == 0 {
		t.Fatalf("local commit notifications = %#v", senderCommits)
	}
	result, err := receiver.ReceiveGiftWrap(ctx, job.ExactGiftWrapBytes, relayOne, time.Now().UTC())
	if err != nil || result.Status != "projected" {
		t.Fatalf("receive = %#v, %v", result, err)
	}
	if len(receiverCommits) != 1 || receiverCommits[0].Revision == 0 {
		t.Fatalf("remote commit notifications = %#v", receiverCommits)
	}
	got, err := receiver.Get(ctx, message.ID)
	if err != nil || got.Body != message.Body || got.SenderInstallationID != senderID || got.RecipientInstallationID != receiverID {
		t.Fatalf("received message = %#v, %v", got, err)
	}
	duplicate, err := receiver.ReceiveGiftWrap(ctx, job.ExactGiftWrapBytes, relayOne, time.Now().UTC())
	if err != nil || duplicate.Status != "duplicate-wrapper" {
		t.Fatalf("duplicate = %#v, %v", duplicate, err)
	}
	if len(receiverCommits) != 1 {
		t.Fatalf("duplicate wrapper emitted a commit notification: %#v", receiverCommits)
	}
	if err := sender.RecordPublish(ctx, job.CanonicalEventID, job.RecipientInstallation, relayOne, true, false, "duplicate: already saved", time.Now(), time.Now()); err != nil {
		t.Fatal(err)
	}
	if jobs, err := sender.RelayJobs(ctx, relayOne, 10, time.Now()); err != nil || len(jobs) != 0 {
		t.Fatalf("accepted relay jobs = %#v, %v", jobs, err)
	}
	if jobs, err := sender.RelayJobs(ctx, relayTwo, 10, time.Now()); err != nil || len(jobs) != 1 {
		t.Fatalf("redundant relay jobs = %#v, %v", jobs, err)
	}
	attempts, err := sender.RelayAttempts(ctx, job.CanonicalEventID)
	if err != nil || len(attempts) != 1 || attempts[0].AcceptedAt == nil {
		t.Fatalf("attempts = %#v, %v", attempts, err)
	}
	var count int
	if err := receiver.db.QueryRow(`SELECT count(*) FROM messages WHERE id=?`, message.ID).Scan(&count); err != nil || count != 1 {
		t.Fatalf("message copies = %d, %v", count, err)
	}
}

func TestWrappedOutboxSurvivesRestartWithExactBytes(t *testing.T) {
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "sender", "hq.db")
	sender := openStore(t, path)
	receiver := openStore(t, filepath.Join(t.TempDir(), "receiver", "hq.db"))
	receiverID, receiverKey := receiver.InstallationIdentity()
	const relay = "wss://relay.test"
	if err := sender.TrustPeer(ctx, Peer{InstallationID: receiverID, SignerKeyID: receiverKey, Relays: []string{relay}}); err != nil {
		t.Fatal(err)
	}
	message := model.Message{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d91", SenderMailboxID: model.HumanMailboxID, Body: "persist me", Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}
	if err := sender.CreatePeerMessage(ctx, message, receiverID, model.HumanMailboxID); err != nil {
		t.Fatal(err)
	}
	if _, err := sender.PrepareOutbound(ctx, 10); err != nil {
		t.Fatal(err)
	}
	before, err := sender.RelayJobs(ctx, relay, 10, time.Now())
	if err != nil || len(before) != 1 {
		t.Fatalf("before restart = %#v, %v", before, err)
	}
	if err := sender.Close(); err != nil {
		t.Fatal(err)
	}
	reopened, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer reopened.Close()
	after, err := reopened.RelayJobs(ctx, relay, 10, time.Now())
	if err != nil || len(after) != 1 || string(after[0].ExactGiftWrapBytes) != string(before[0].ExactGiftWrapBytes) || after[0].GiftWrapEventID != before[0].GiftWrapEventID {
		t.Fatalf("after restart = %#v, %v", after, err)
	}
}

func TestRelayConfigRequiresAuthUnlessUnsafe(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	if err := s.AddRelay(ctx, RelayConfig{URL: "wss://RELAY.EXAMPLE/", Read: true}); err == nil || !strings.Contains(err.Error(), "require auth") {
		t.Fatalf("unauthenticated relay add = %v", err)
	}
	if err := s.AddRelay(ctx, RelayConfig{URL: "wss://RELAY.EXAMPLE/", Read: true, UnsafeNoAuth: true}); err != nil {
		t.Fatal(err)
	}
	if err := s.AddRelay(ctx, RelayConfig{URL: "wss://relay.example", Read: true, Write: true, RequireAuth: true}); err != nil {
		t.Fatal(err)
	}
	relays, err := s.ListRelays(ctx)
	if err != nil || len(relays) != 1 || relays[0].URL != "wss://relay.example" || !relays[0].RequireAuth {
		t.Fatalf("relays = %#v, %v", relays, err)
	}
	if err := s.RemoveRelay(ctx, "wss://relay.example/"); err != nil {
		t.Fatal(err)
	}
	relays, _ = s.ListRelays(ctx)
	if len(relays) != 0 {
		t.Fatalf("removed relays = %#v", relays)
	}
}

func TestConfiguredWriteRelayReceivesJobsWithoutPeerHint(t *testing.T) {
	ctx := context.Background()
	sender := openStore(t, filepath.Join(t.TempDir(), "sender", "hq.db"))
	receiver := openStore(t, filepath.Join(t.TempDir(), "receiver", "hq.db"))
	receiverID, receiverKey := receiver.InstallationIdentity()
	const relay = "wss://relay.test"
	if err := sender.TrustPeer(ctx, Peer{InstallationID: receiverID, SignerKeyID: receiverKey}); err != nil {
		t.Fatal(err)
	}
	if err := sender.AddRelay(ctx, RelayConfig{URL: relay, Write: true}); err != nil {
		t.Fatal(err)
	}
	message := model.Message{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d96", SenderMailboxID: model.HumanMailboxID, Body: "configured route", Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}
	if err := sender.CreatePeerMessage(ctx, message, receiverID, model.HumanMailboxID); err != nil {
		t.Fatal(err)
	}
	if _, err := sender.PrepareOutbound(ctx, 10); err != nil {
		t.Fatal(err)
	}
	jobs, err := sender.RelayJobs(ctx, relay, 10, time.Now())
	if err != nil || len(jobs) != 1 {
		t.Fatalf("configured relay jobs = %#v, %v", jobs, err)
	}
}

func TestNetworkStatusReportsQueueRejectAndInboundFailures(t *testing.T) {
	ctx := context.Background()
	sender := openStore(t, filepath.Join(t.TempDir(), "sender", "hq.db"))
	receiver := openStore(t, filepath.Join(t.TempDir(), "receiver", "hq.db"))
	receiverID, receiverKey := receiver.InstallationIdentity()
	const relay = "wss://relay.test"
	if err := sender.TrustPeer(ctx, Peer{InstallationID: receiverID, SignerKeyID: receiverKey, Relays: []string{relay}}); err != nil {
		t.Fatal(err)
	}
	message := model.Message{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d97", SenderMailboxID: model.HumanMailboxID, Body: "status", Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}
	if err := sender.CreatePeerMessage(ctx, message, receiverID, model.HumanMailboxID); err != nil {
		t.Fatal(err)
	}
	if _, err := sender.PrepareOutbound(ctx, 10); err != nil {
		t.Fatal(err)
	}
	jobs, err := sender.RelayJobs(ctx, relay, 10, time.Now())
	if err != nil || len(jobs) != 1 {
		t.Fatalf("jobs = %#v, %v", jobs, err)
	}
	now := time.Now().UTC()
	if err := sender.RecordPublish(ctx, jobs[0].CanonicalEventID, jobs[0].RecipientInstallation, relay, false, true, "rate-limited", now, now.Add(time.Minute)); err != nil {
		t.Fatal(err)
	}
	if err := sender.Stage(ctx, []byte("temporary"), relay, "", "busy", now, now.Add(time.Minute)); err != nil {
		t.Fatal(err)
	}
	if err := sender.Quarantine(ctx, []byte("bad"), relay, "", "invalid", now); err != nil {
		t.Fatal(err)
	}
	if err := sender.SetRelaySyncState(ctx, relay, true, true, "rate-limited", &now, &now); err != nil {
		t.Fatal(err)
	}
	status, err := sender.NetworkStatus(ctx)
	if err != nil || status.Queued != 1 || status.Rejected != 1 || status.Staged != 1 || status.Quarantined != 1 || len(status.Relays) != 1 || !status.Relays[0].Authenticated {
		t.Fatalf("network status = %#v, %v", status, err)
	}
	stored, err := sender.Get(ctx, message.ID)
	if err != nil || stored.DeliveryState != "rejected" {
		t.Fatalf("rejected message = %#v, %v", stored, err)
	}
}

func TestNetworkStatusReportsRelayAcceptanceAndLastReceive(t *testing.T) {
	ctx := context.Background()
	sender := openStore(t, filepath.Join(t.TempDir(), "sender", "hq.db"))
	receiver := openStore(t, filepath.Join(t.TempDir(), "receiver", "hq.db"))
	receiverID, receiverKey := receiver.InstallationIdentity()
	const relay = "wss://relay.test"
	if err := sender.TrustPeer(ctx, Peer{InstallationID: receiverID, SignerKeyID: receiverKey, Relays: []string{relay}}); err != nil {
		t.Fatal(err)
	}
	message := model.Message{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140e01", SenderMailboxID: model.HumanMailboxID, Body: "accepted", Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}
	if err := sender.CreatePeerMessage(ctx, message, receiverID, model.HumanMailboxID); err != nil {
		t.Fatal(err)
	}
	if _, err := sender.PrepareOutbound(ctx, 10); err != nil {
		t.Fatal(err)
	}
	jobs, err := sender.RelayJobs(ctx, relay, 10, time.Now())
	if err != nil || len(jobs) != 1 {
		t.Fatalf("jobs = %#v, %v", jobs, err)
	}
	now := time.Now().UTC().Truncate(time.Millisecond)
	if err := sender.RecordPublish(ctx, jobs[0].CanonicalEventID, jobs[0].RecipientInstallation, relay, true, false, "saved", now, now); err != nil {
		t.Fatal(err)
	}
	if err := sender.SetRelaySyncState(ctx, relay, true, true, "", nil, &now); err != nil {
		t.Fatal(err)
	}
	status, err := sender.NetworkStatus(ctx)
	if err != nil || status.RelayAccepted != 1 || len(status.Relays) != 1 || status.Relays[0].LastEvent == nil || !status.Relays[0].LastEvent.Equal(now) {
		t.Fatalf("network status = %#v, %v", status, err)
	}
}

func TestUnknownSenderGiftWrapIsQuarantined(t *testing.T) {
	ctx := context.Background()
	sender := openStore(t, filepath.Join(t.TempDir(), "sender", "hq.db"))
	receiver := openStore(t, filepath.Join(t.TempDir(), "receiver", "hq.db"))
	receiverID, receiverKey := receiver.InstallationIdentity()
	const relay = "wss://relay.test"
	if err := sender.TrustPeer(ctx, Peer{InstallationID: receiverID, SignerKeyID: receiverKey, Relays: []string{relay}}); err != nil {
		t.Fatal(err)
	}
	message := model.Message{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d92", SenderMailboxID: model.HumanMailboxID, Body: "unknown sender", Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}
	if err := sender.CreatePeerMessage(ctx, message, receiverID, model.HumanMailboxID); err != nil {
		t.Fatal(err)
	}
	_, _ = sender.PrepareOutbound(ctx, 10)
	jobs, _ := sender.RelayJobs(ctx, relay, 10, time.Now())
	if len(jobs) != 1 {
		t.Fatal("missing sender job")
	}
	if _, err := receiver.ReceiveGiftWrap(ctx, jobs[0].ExactGiftWrapBytes, relay, time.Now()); err == nil {
		t.Fatal("unknown sender projected")
	}
	if _, err := receiver.Get(ctx, message.ID); !errors.Is(err, ErrNotFound) {
		t.Fatalf("unknown sender message = %v", err)
	}
	var quarantined int
	if err := receiver.db.QueryRow(`SELECT count(*) FROM quarantine`).Scan(&quarantined); err != nil || quarantined != 1 {
		t.Fatalf("quarantine = %d, %v", quarantined, err)
	}
}

func TestDistinctWrappersDeduplicateLogicalEventAndRejectReusedKey(t *testing.T) {
	ctx := context.Background()
	sender := openStore(t, filepath.Join(t.TempDir(), "sender", "hq.db"))
	receiver := openStore(t, filepath.Join(t.TempDir(), "receiver", "hq.db"))
	senderID, senderKey := sender.InstallationIdentity()
	receiverID, receiverKey := receiver.InstallationIdentity()
	const relay = "wss://relay.test"
	if err := sender.TrustPeer(ctx, Peer{InstallationID: receiverID, SignerKeyID: receiverKey, Relays: []string{relay}}); err != nil {
		t.Fatal(err)
	}
	if err := receiver.TrustPeer(ctx, Peer{InstallationID: senderID, SignerKeyID: senderKey, Relays: []string{relay}}); err != nil {
		t.Fatal(err)
	}
	first := model.Message{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d93", SenderMailboxID: model.HumanMailboxID, Body: "logical duplicate", Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}
	if err := sender.CreatePeerMessage(ctx, first, receiverID, model.HumanMailboxID); err != nil {
		t.Fatal(err)
	}
	jobs, err := sender.PendingOutbox(ctx, 10)
	if err != nil || len(jobs) != 1 {
		t.Fatalf("pending jobs = %#v, %v", jobs, err)
	}
	canonical := event.Inspect(jobs[0].ExactCanonicalBytes)
	if canonical.Status == event.StatusInvalid {
		t.Fatal(canonical.Err)
	}
	fixed := func() time.Time { return time.Unix(1_900_000_000, 0) }
	firstWrap, err := sender.WireCodec(bytes.NewReader(bytes.Repeat([]byte{8}, 1024)), fixed).Wrap(canonical.Event, receiverKey)
	if err != nil {
		t.Fatal(err)
	}
	secondWrap, err := sender.WireCodec(bytes.NewReader(bytes.Repeat([]byte{9}, 1024)), fixed).Wrap(canonical.Event, receiverKey)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := receiver.ReceiveGiftWrap(ctx, firstWrap.ExactWire, relay, time.Now()); err != nil {
		t.Fatal(err)
	}
	duplicate, err := receiver.ReceiveGiftWrap(ctx, secondWrap.ExactWire, relay, time.Now())
	if err != nil || duplicate.Status != "duplicate-logical" {
		t.Fatalf("logical duplicate = %#v, %v", duplicate, err)
	}

	second := model.Message{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d94", SenderMailboxID: model.HumanMailboxID, Body: "reused key", Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}
	if err := sender.CreatePeerMessage(ctx, second, receiverID, model.HumanMailboxID); err != nil {
		t.Fatal(err)
	}
	jobs, err = sender.PendingOutbox(ctx, 10)
	if err != nil || len(jobs) != 2 {
		t.Fatalf("pending jobs after second message = %#v, %v", jobs, err)
	}
	var secondCanonical event.Inspection
	for _, job := range jobs {
		candidate := event.Inspect(job.ExactCanonicalBytes)
		if candidate.Status != event.StatusInvalid && candidate.Event.ID() != canonical.Event.ID() {
			secondCanonical = candidate
		}
	}
	if secondCanonical.Status == event.StatusInvalid || len(secondCanonical.Event.Wire) == 0 {
		t.Fatalf("second canonical event = %#v", secondCanonical)
	}
	reused, err := sender.WireCodec(bytes.NewReader(bytes.Repeat([]byte{8}, 1024)), fixed).Wrap(secondCanonical.Event, receiverKey)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := receiver.ReceiveGiftWrap(ctx, reused.ExactWire, relay, time.Now()); err == nil || !strings.Contains(err.Error(), "reused an ephemeral") {
		t.Fatalf("reused wrapper key error = %v", err)
	}
	var count int
	if err := receiver.db.QueryRow(`SELECT count(*) FROM messages WHERE id=?`, first.ID).Scan(&count); err != nil || count != 1 {
		t.Fatalf("logical message copies = %d, %v", count, err)
	}
}

func TestRevokedAgentMailboxShareRejectsInboundMessage(t *testing.T) {
	ctx := context.Background()
	sender := openStore(t, filepath.Join(t.TempDir(), "sender", "hq.db"))
	receiver := openStore(t, filepath.Join(t.TempDir(), "receiver", "hq.db"))
	senderID, senderKey := sender.InstallationIdentity()
	receiverID, receiverKey := receiver.InstallationIdentity()
	const relay = "wss://relay.test"
	if err := sender.TrustPeer(ctx, Peer{InstallationID: receiverID, SignerKeyID: receiverKey, Relays: []string{relay}}); err != nil {
		t.Fatal(err)
	}
	if err := receiver.TrustPeer(ctx, Peer{InstallationID: senderID, SignerKeyID: senderKey, Relays: []string{relay}}); err != nil {
		t.Fatal(err)
	}
	agent, err := receiver.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "private-agent"}, model.RepositoryContext{Directory: "/repo"})
	if err != nil {
		t.Fatal(err)
	}
	if err := receiver.SetMailboxShare(ctx, agent.ID, senderID, true); err != nil {
		t.Fatal(err)
	}
	if err := receiver.SetMailboxShare(ctx, agent.ID, senderID, false); err != nil {
		t.Fatal(err)
	}
	message := model.Message{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d95", SenderMailboxID: model.HumanMailboxID, Body: "private agent", Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}
	if err := sender.CreatePeerMessage(ctx, message, receiverID, agent.ID); err != nil {
		t.Fatal(err)
	}
	if _, err := sender.PrepareOutbound(ctx, 10); err != nil {
		t.Fatal(err)
	}
	jobs, err := sender.RelayJobs(ctx, relay, 10, time.Now())
	if err != nil || len(jobs) != 1 {
		t.Fatalf("relay jobs = %#v, %v", jobs, err)
	}
	if _, err := receiver.ReceiveGiftWrap(ctx, jobs[0].ExactGiftWrapBytes, relay, time.Now()); err == nil {
		t.Fatal("message reached an agent after its mailbox share was revoked")
	}
	if _, err := receiver.Get(ctx, message.ID); !errors.Is(err, ErrNotFound) {
		t.Fatalf("revoked-share message = %v", err)
	}
}
