package node_test

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"net"
	"path/filepath"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/domainrpc"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/hqclient"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/localwire"
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
	subscription, err := clients[1].Subscribe(ctx, domain.TopicMessages)
	if err != nil {
		t.Fatal(err)
	}
	defer subscription.Close()
	if snapshot, err := clients[1].List(ctx, model.Filter{RecipientMailboxID: agent.ID}); err != nil || len(snapshot) != 0 {
		t.Fatalf("initial subscribed snapshot = %#v, %v", snapshot, err)
	}
	if err := clients[0].Create(ctx, message); err != nil {
		t.Fatal(err)
	}
	select {
	case change := <-subscription.Changes():
		if change.Revision == 0 || len(change.Topics) == 0 || change.Topics[0] != domain.TopicMessages {
			t.Fatalf("message invalidation = %#v", change)
		}
	case <-time.After(time.Second):
		t.Fatal("subscribed client did not receive the committed message invalidation")
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

func TestMutationRetryReplaysReceiptAcrossNodeRestart(t *testing.T) {
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
	runner := node.Runner{}
	start := func() chan error {
		done := make(chan error, 1)
		go func() { done <- runner.Run(context.Background(), databasePath) }()
		waitForNode(t, databasePath)
		return done
	}
	stop := func(done chan error) {
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

	done := start()
	client, err := hqclient.Open(context.Background(), databasePath)
	if err != nil {
		t.Fatal(err)
	}
	repository := model.RepositoryContext{Directory: "/mutation-retry"}
	agent, err := client.ResolveMailbox(context.Background(), model.SessionIdentity{Harness: "codex", ExternalSessionID: "retry"}, repository)
	if err != nil {
		t.Fatal(err)
	}
	client.Close()
	message := model.Message{
		ID: uuid.Must(uuid.NewV7()).String(), SenderMailboxID: model.HumanMailboxID,
		RecipientMailboxID: agent.ID, Body: "commit before response loss", Context: repository,
		CreatedAt: time.Now().UTC(),
	}
	mutationID := uuid.Must(uuid.NewV7()).String()
	request := domainrpc.MessageRequest{MutationID: mutationID, Message: message}
	abandonedConnection := sendMutationWithoutReadingResponse(t, databasePath, request)
	observer, err := hqclient.Open(context.Background(), databasePath)
	if err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(2 * time.Second)
	for {
		if got, getErr := observer.Get(context.Background(), message.ID); getErr == nil && got.Body == message.Body {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("mutation did not commit before the simulated response loss")
		}
		time.Sleep(time.Millisecond)
	}
	observer.Close()
	abandonedConnection.Close()
	stop(done)

	done = start()
	rawClients := make([]*localwire.Client, 4)
	for index := range rawClients {
		rawClients[index] = openRawDomainClient(t, databasePath)
	}
	startCalls := make(chan struct{})
	errorsByClient := make(chan error, len(rawClients))
	var calls sync.WaitGroup
	for _, concurrentClient := range rawClients {
		calls.Add(1)
		go func() {
			defer calls.Done()
			<-startCalls
			errorsByClient <- concurrentClient.Call(context.Background(), domainrpc.CreateMethod, request, nil)
		}()
	}
	close(startCalls)
	calls.Wait()
	close(errorsByClient)
	for callErr := range errorsByClient {
		if callErr != nil {
			t.Fatalf("concurrent mutation retry: %v", callErr)
		}
	}
	for _, concurrentClient := range rawClients {
		concurrentClient.Close()
	}
	rawClient := openRawDomainClient(t, databasePath)
	if err := rawClient.Call(context.Background(), domainrpc.CreateMethod, request, nil); err != nil {
		t.Fatalf("retry committed mutation: %v", err)
	}
	conflict := request
	conflict.Message.Body = "different request"
	conflictErr := rawClient.Call(context.Background(), domainrpc.CreateMethod, conflict, nil)
	var rpcErr *localwire.RPCError
	if !errors.As(conflictErr, &rpcErr) || rpcErr.Code != localwire.CodeInvalidRequest {
		t.Fatalf("mutation key reuse error = %v", conflictErr)
	}
	rawClient.Close()
	stop(done)

	database, err := sql.Open("sqlite", databasePath)
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	var messageFacts, receipts int
	if err := database.QueryRow(`SELECT count(*) FROM canonical_events WHERE event_type IN ('message','question','answer')`).Scan(&messageFacts); err != nil {
		t.Fatal(err)
	}
	if err := database.QueryRow(`SELECT count(*) FROM mutation_receipts WHERE mutation_id=?`, mutationID).Scan(&receipts); err != nil {
		t.Fatal(err)
	}
	if messageFacts != 1 || receipts != 1 {
		t.Fatalf("message facts=%d receipts=%d", messageFacts, receipts)
	}
}

func sendMutationWithoutReadingResponse(t *testing.T, databasePath string, request domainrpc.MessageRequest) net.Conn {
	t.Helper()
	paths, err := syncer.ResolveRuntimePaths(databasePath)
	if err != nil {
		t.Fatal(err)
	}
	connection, err := net.Dial("unix", paths.Socket)
	if err != nil {
		t.Fatal(err)
	}
	codec := localwire.NewCodec(connection, connection, localwire.DefaultMaximumFrameBytes)
	handshake, err := json.Marshal(localwire.HandshakeRequest{
		Mode: localwire.DomainMode, Supported: localwire.DomainVersions,
		Client: localwire.PeerMetadata{Build: "lost-response-test"},
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := codec.Write(localwire.Envelope{Kind: localwire.HandshakeKind, Params: handshake}); err != nil {
		t.Fatal(err)
	}
	if response, err := codec.Read(); err != nil || response.Kind != localwire.HandshakeKind {
		t.Fatalf("mutation handshake = %#v, %v", response, err)
	}
	raw, err := json.Marshal(request)
	if err != nil {
		t.Fatal(err)
	}
	if err := codec.Write(localwire.Envelope{
		Kind: localwire.RequestKind, Version: localwire.CurrentDomainVersion,
		ID: "intentionally-unread-response", Method: domainrpc.CreateMethod, Params: raw,
	}); err != nil {
		t.Fatal(err)
	}
	return connection
}

func openRawDomainClient(t *testing.T, databasePath string) *localwire.Client {
	t.Helper()
	paths, err := syncer.ResolveRuntimePaths(databasePath)
	if err != nil {
		t.Fatal(err)
	}
	connection, err := net.Dial("unix", paths.Socket)
	if err != nil {
		t.Fatal(err)
	}
	client, err := localwire.NewClient(context.Background(), connection, localwire.ClientOptions{
		Mode: localwire.DomainMode, Supported: localwire.DomainVersions,
		Metadata: localwire.PeerMetadata{Build: "mutation-retry-test"},
	})
	if err != nil {
		connection.Close()
		t.Fatal(err)
	}
	return client
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
