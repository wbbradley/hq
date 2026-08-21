package node_test

import (
	"context"
	"errors"
	"path/filepath"
	"sync/atomic"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/hqclient"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/node"
	"github.com/wbbradley/hq/internal/store"
	"github.com/wbbradley/hq/internal/syncer"
)

func TestLiveNodeDomainRoundTripAndRuntimeOwnership(t *testing.T) {
	root := t.TempDir()
	t.Setenv("XDG_STATE_HOME", filepath.Join(root, "state"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(root, "config"))
	t.Setenv("XDG_RUNTIME_DIR", filepath.Join(root, "runtime"))
	databasePath := filepath.Join(root, "installation", "hq.db")
	keyPath, err := identity.KeyPath(databasePath)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := identity.Initialize(keyPath, nil); err != nil {
		t.Fatal(err)
	}
	var opens atomic.Int32
	ownershipChecked := make(chan error, 1)
	runner := node.Runner{Open: func(path string) (*store.SQLite, error) {
		opens.Add(1)
		lock, lockErr := (syncer.FileCoordinator{DatabasePath: path}).TryAcquire()
		if lock != nil {
			_ = lock.Release()
		}
		if !errors.Is(lockErr, syncer.ErrNodeOwned) {
			ownershipChecked <- errors.New("runtime factory opened before node ownership")
		} else {
			ownershipChecked <- nil
		}
		return store.Open(path)
	}}
	done := make(chan error, 1)
	go func() { done <- runner.Run(context.Background(), databasePath) }()
	waitForNode(t, databasePath)
	t.Cleanup(func() { _ = syncer.StopDaemon(databasePath) })
	if err := <-ownershipChecked; err != nil {
		t.Fatal(err)
	}
	client, err := hqclient.Open(context.Background(), databasePath)
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	ctx := context.Background()
	human, err := client.HumanMailbox(ctx)
	if err != nil || human.ID != model.HumanMailboxID {
		t.Fatalf("human mailbox = %#v, %v", human, err)
	}
	repository := model.RepositoryContext{Directory: "/repo", Branch: "main"}
	agent, err := client.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "rpc-thread"}, repository)
	if err != nil || agent.ID == "" {
		t.Fatalf("agent mailbox = %#v, %v", agent, err)
	}
	mailboxes, err := client.FindMailboxes(ctx, repository)
	if err != nil || len(mailboxes) != 1 || mailboxes[0].ID != agent.ID {
		t.Fatalf("mailboxes = %#v, %v", mailboxes, err)
	}

	questionID := uuid.Must(uuid.NewV7()).String()
	question := model.Message{ID: questionID, SenderMailboxID: agent.ID, RecipientMailboxID: human.ID, Body: "question", Context: repository, CreatedAt: time.Now().UTC()}
	if err := client.Create(ctx, question); err != nil {
		t.Fatal(err)
	}
	if got, err := client.Get(ctx, questionID); err != nil || got.Body != question.Body {
		t.Fatalf("question = %#v, %v", got, err)
	}
	if listed, err := client.List(ctx, model.Filter{RecipientMailboxID: human.ID}); err != nil || len(listed) != 1 {
		t.Fatalf("inbox = %#v, %v", listed, err)
	}
	replyID := uuid.Must(uuid.NewV7()).String()
	reply := model.Message{ID: replyID, SenderMailboxID: human.ID, RecipientMailboxID: agent.ID, Body: "answer", Context: repository, CreatedAt: time.Now().UTC()}
	if err := client.Reply(ctx, questionID, reply); err != nil {
		t.Fatal(err)
	}
	if archived, err := client.Get(ctx, questionID); err != nil || archived.ArchivedAt == nil {
		t.Fatalf("atomic reply/archive = %#v, %v", archived, err)
	}
	claimed, err := client.Claim(ctx, domain.Claim{MessageID: replyID}, "reply-token")
	if err != nil || claimed.ID != replyID {
		t.Fatalf("claimed reply = %#v, %v", claimed, err)
	}
	if err := client.Complete(ctx, replyID, "reply-token"); err != nil {
		t.Fatal(err)
	}
	if _, err := client.Claim(ctx, domain.Claim{MessageID: replyID}, "second-token"); !errors.Is(err, domain.ErrNotReady) {
		t.Fatalf("completed claim error = %v", err)
	}

	deliveryID := uuid.Must(uuid.NewV7()).String()
	if err := client.Create(ctx, model.Message{ID: deliveryID, SenderMailboxID: human.ID, RecipientMailboxID: agent.ID, Body: "delivery", Context: repository, CreatedAt: time.Now().UTC()}); err != nil {
		t.Fatal(err)
	}
	if _, err := client.Claim(ctx, domain.Claim{MessageID: deliveryID}, "release-token"); err != nil {
		t.Fatal(err)
	}
	if err := client.Release(ctx, deliveryID, "release-token"); err != nil {
		t.Fatal(err)
	}
	if _, err := client.Claim(ctx, domain.Claim{MessageID: deliveryID}, "complete-token"); err != nil {
		t.Fatal(err)
	}
	if err := client.Complete(ctx, deliveryID, "complete-token"); err != nil {
		t.Fatal(err)
	}

	archiveID := uuid.Must(uuid.NewV7()).String()
	if err := client.Create(ctx, model.Message{ID: archiveID, SenderMailboxID: agent.ID, RecipientMailboxID: human.ID, Body: "archive", Context: repository, CreatedAt: time.Now().UTC()}); err != nil {
		t.Fatal(err)
	}
	if err := client.Archive(ctx, archiveID); err != nil {
		t.Fatal(err)
	}
	if _, err := client.Get(ctx, uuid.Must(uuid.NewV7()).String()); !errors.Is(err, domain.ErrNotFound) {
		t.Fatalf("not-found mapping = %v", err)
	}

	peerID := uuid.Must(uuid.NewV7()).String()
	peer := domain.Peer{InstallationID: peerID, SignerKeyID: event.MustSecretKeyFromHex("42").PublicKeyHex(), Name: "peer", Trusted: true}
	if err := client.TrustPeer(ctx, peer); err != nil {
		t.Fatal(err)
	}
	if peers, err := client.ListPeers(ctx); err != nil || len(peers) != 1 || peers[0].InstallationID != peerID {
		t.Fatalf("peers = %#v, %v", peers, err)
	}
	if err := client.SetMailboxShare(ctx, agent.ID, peerID, true); err != nil {
		t.Fatal(err)
	}
	if err := client.DistrustPeer(ctx, peerID); err != nil {
		t.Fatal(err)
	}
	if account, err := client.HumanAccount(ctx); err != nil || account.ID == "" {
		t.Fatalf("human account = %#v, %v", account, err)
	}
	if devices, err := client.HumanDevices(ctx); err != nil || len(devices) != 1 {
		t.Fatalf("human devices = %#v, %v", devices, err)
	}
	targetID := uuid.Must(uuid.NewV7()).String()
	invite, err := client.CreateHumanInvite(ctx, domain.HumanInviteRequest{InstallationID: targetID, SignerKeyID: event.MustSecretKeyFromHex("43").PublicKeyHex(), Name: "desktop"})
	if err != nil || invite.TargetInstallationID != targetID {
		t.Fatalf("invite = %#v, %v", invite, err)
	}
	if err := client.RevokeHumanDevice(ctx, targetID); err != nil {
		t.Fatal(err)
	}
	if err := client.JoinHumanInvite(ctx, []byte("invalid")); err == nil {
		t.Fatal("invalid invite unexpectedly joined")
	}

	relay := domain.RelayConfig{URL: "wss://relay.example", Write: true}
	if err := client.AddRelay(ctx, relay); err != nil {
		t.Fatal(err)
	}
	if relays, err := client.ListRelays(ctx); err != nil || len(relays) != 1 || relays[0].URL != relay.URL {
		t.Fatalf("relays = %#v, %v", relays, err)
	}
	if status, err := client.NetworkStatus(ctx); err != nil || status.AccountMembers != 1 {
		t.Fatalf("network status = %#v, %v", status, err)
	}
	if err := client.RemoveRelay(ctx, relay.URL); err != nil {
		t.Fatal(err)
	}
	if err := client.Synchronize(ctx); err != nil {
		t.Fatal(err)
	}
	if opens.Load() != 1 {
		t.Fatalf("store opens = %d", opens.Load())
	}
	if err := client.Close(); err != nil {
		t.Fatal(err)
	}
	if err := syncer.StopDaemon(databasePath); err != nil {
		t.Fatal(err)
	}
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("node did not stop")
	}
}

func TestCLIAndTUIAndCodexClientsShareOneNodeStore(t *testing.T) {
	root := t.TempDir()
	t.Setenv("XDG_STATE_HOME", filepath.Join(root, "state"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(root, "config"))
	t.Setenv("XDG_RUNTIME_DIR", filepath.Join(root, "runtime"))
	databasePath := filepath.Join(root, "installation", "hq.db")
	keyPath, err := identity.KeyPath(databasePath)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := identity.Initialize(keyPath, nil); err != nil {
		t.Fatal(err)
	}

	var opens atomic.Int32
	runner := node.Runner{Open: func(path string) (*store.SQLite, error) {
		opens.Add(1)
		return store.Open(path)
	}}
	done := make(chan error, 1)
	go func() { done <- runner.Run(context.Background(), databasePath) }()
	waitForNode(t, databasePath)
	t.Cleanup(func() { _ = syncer.StopDaemon(databasePath) })

	clients := make([]*hqclient.Client, 3)
	clientRoles := []string{"CLI", "TUI", "Codex"}
	for index := range clients {
		clients[index], err = hqclient.Open(context.Background(), databasePath)
		if err != nil {
			t.Fatal(err)
		}
		defer clients[index].Close()
	}
	ctx := context.Background()
	repository := model.RepositoryContext{Directory: "/shared/repository"}
	agent, err := clients[0].ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "shared-node"}, repository)
	if err != nil {
		t.Fatal(err)
	}
	message := model.Message{
		ID: uuid.Must(uuid.NewV7()).String(), SenderMailboxID: model.HumanMailboxID,
		RecipientMailboxID: agent.ID, Body: "one node, three clients", Context: repository,
		CreatedAt: time.Now().UTC(),
	}
	if err := clients[0].Create(ctx, message); err != nil {
		t.Fatal(err)
	}
	for index, client := range clients {
		got, err := client.Get(ctx, message.ID)
		if err != nil || got.Body != message.Body {
			t.Fatalf("%s client read = %#v, %v", clientRoles[index], got, err)
		}
		listed, err := client.List(ctx, model.Filter{RecipientMailboxID: agent.ID})
		if err != nil || len(listed) != 1 || listed[0].ID != message.ID {
			t.Fatalf("%s client list = %#v, %v", clientRoles[index], listed, err)
		}
	}
	if _, err := clients[1].Claim(ctx, domain.Claim{MessageID: message.ID}, "tui-lease"); err != nil {
		t.Fatal(err)
	}
	if _, err := clients[2].Claim(ctx, domain.Claim{MessageID: message.ID}, "codex-lease"); !errors.Is(err, domain.ErrNotReady) {
		t.Fatalf("competing client claim = %v", err)
	}
	if err := clients[1].Release(ctx, message.ID, "tui-lease"); err != nil {
		t.Fatal(err)
	}
	if _, err := clients[2].Claim(ctx, domain.Claim{MessageID: message.ID}, "codex-lease"); err != nil {
		t.Fatal(err)
	}
	if opens.Load() != 1 {
		t.Fatalf("concrete store opens = %d", opens.Load())
	}

	for _, client := range clients {
		if err := client.Close(); err != nil {
			t.Fatal(err)
		}
	}
	if err := syncer.StopDaemon(databasePath); err != nil {
		t.Fatal(err)
	}
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("node did not stop")
	}
}

func waitForNode(t *testing.T, databasePath string) {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		if _, err := syncer.DaemonStatus(databasePath); err == nil {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("node did not become ready")
}
