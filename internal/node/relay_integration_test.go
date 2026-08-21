package node_test

import (
	"context"
	"database/sql"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/hqclient"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/node"
	"github.com/wbbradley/hq/internal/nostrwire"
	"github.com/wbbradley/hq/internal/syncer"
)

func TestDomainClientResubscribesAcrossLiveNodeRestartAndBuildDrift(t *testing.T) {
	root := t.TempDir()
	t.Setenv("XDG_STATE_HOME", filepath.Join(root, "state"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(root, "config"))
	t.Setenv("XDG_RUNTIME_DIR", filepath.Join(root, "runtime"))
	databasePath := filepath.Join(root, "restart", "hq.db")
	initializeNodeIdentity(t, databasePath)
	stopNode := startTestNode(t, databasePath)
	defer stopNode()
	ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
	defer cancel()
	client, err := hqclient.Open(ctx, databasePath)
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	agent, err := client.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "restart"}, model.RepositoryContext{Directory: "/restart"})
	if err != nil {
		t.Fatal(err)
	}
	subscription, err := client.Subscribe(ctx, domain.TopicMessages)
	if err != nil {
		t.Fatal(err)
	}
	defer subscription.Close()
	oldInstance := client.State().Handshake.Server.InstanceID
	if err := syncer.RestartDaemon(databasePath); err != nil {
		t.Fatal(err)
	}
	select {
	case change := <-subscription.Changes():
		if !change.FullSnapshot || change.Revision == 0 {
			t.Fatalf("restart invalidation = %#v", change)
		}
	case <-time.After(4 * time.Second):
		t.Fatal("domain client did not reconnect and resubscribe")
	}
	waitUntil(t, 2*time.Second, func() bool {
		state := client.State()
		return (state.Phase == hqclient.ConnectionConnected || state.Phase == hqclient.ConnectionDrift) && state.Handshake.Server.InstanceID != "" && state.Handshake.Server.InstanceID != oldInstance
	}, "domain client did not become ready on the restarted node")
	message := model.Message{
		ID: uuid.Must(uuid.NewV7()).String(), SenderMailboxID: agent.ID, RecipientMailboxID: model.HumanMailboxID,
		Body: "after restart", Context: model.RepositoryContext{Directory: "/restart"}, CreatedAt: time.Now().UTC(),
	}
	if err := client.Create(ctx, message); err != nil {
		t.Fatal(err)
	}
	assertMessageInvalidation(t, subscription.Changes(), "post-restart commit")

	drifted := hqclient.New(openRawDomainClient(t, databasePath))
	defer drifted.Close()
	if state := drifted.State(); state.Phase != hqclient.ConnectionDrift || !strings.Contains(state.Diagnostic(), "restart") {
		t.Fatalf("compatible build drift state = %#v, diagnostic=%q", state, state.Diagnostic())
	}
	if human, err := drifted.HumanMailbox(ctx); err != nil || human.ID != model.HumanMailboxID {
		t.Fatalf("build-drift domain call = %#v, %v", human, err)
	}
}

func TestLocalRPCPublishesAndRetainedInboundInvalidates(t *testing.T) {
	root := t.TempDir()
	t.Setenv("XDG_STATE_HOME", filepath.Join(root, "state"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(root, "config"))
	t.Setenv("XDG_RUNTIME_DIR", filepath.Join(root, "runtime"))
	relay := newRetainedRelay(t)
	senderPath := filepath.Join(root, "sender", "hq.db")
	receiverPath := filepath.Join(root, "receiver", "hq.db")
	senderIdentity := initializeNodeIdentity(t, senderPath)
	receiverIdentity := initializeNodeIdentity(t, receiverPath)
	stopSender := startTestNode(t, senderPath)
	defer stopSender()
	stopReceiver := startTestNode(t, receiverPath)
	defer stopReceiver()

	ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
	defer cancel()
	sender, err := hqclient.Open(ctx, senderPath)
	if err != nil {
		t.Fatal(err)
	}
	defer sender.Close()
	receiver, err := hqclient.Open(ctx, receiverPath)
	if err != nil {
		t.Fatal(err)
	}
	defer receiver.Close()
	repository := model.RepositoryContext{Directory: "/integration/repository"}
	senderMailbox, err := sender.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "sender"}, repository)
	if err != nil {
		t.Fatal(err)
	}
	receiverMailbox, err := receiver.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "receiver"}, repository)
	if err != nil {
		t.Fatal(err)
	}
	if err := sender.TrustPeer(ctx, domain.Peer{
		InstallationID: receiverIdentity.InstallationID, SignerKeyID: receiverIdentity.PublicKey(),
		Name: "receiver", Relays: []string{relay.url}, Trusted: true,
	}); err != nil {
		t.Fatal(err)
	}
	if err := receiver.TrustPeer(ctx, domain.Peer{
		InstallationID: senderIdentity.InstallationID, SignerKeyID: senderIdentity.PublicKey(),
		Name: "sender", Relays: []string{relay.url}, Trusted: true,
	}); err != nil {
		t.Fatal(err)
	}
	if err := receiver.SetMailboxShare(ctx, receiverMailbox.ID, senderIdentity.InstallationID, true); err != nil {
		t.Fatal(err)
	}

	senderSubscription, err := sender.Subscribe(ctx, domain.TopicMessages)
	if err != nil {
		t.Fatal(err)
	}
	defer senderSubscription.Close()
	receiverSubscription, err := receiver.Subscribe(ctx, domain.TopicMessages)
	if err != nil {
		t.Fatal(err)
	}
	defer receiverSubscription.Close()
	messageID := uuid.Must(uuid.NewV7()).String()
	if snapshot, err := receiver.List(ctx, model.Filter{RecipientMailboxID: receiverMailbox.ID}); err != nil || len(snapshot) != 0 {
		t.Fatalf("receiver snapshot = %#v, %v", snapshot, err)
	}
	message := model.Message{
		ID: messageID, SenderMailboxID: senderMailbox.ID,
		RecipientInstallationID: receiverIdentity.InstallationID, RecipientMailboxID: receiverMailbox.ID,
		Body: "local RPC through retained Nostr", Context: repository, CreatedAt: time.Now().UTC(),
	}
	if err := sender.Create(ctx, message); err != nil {
		t.Fatal(err)
	}
	assertMessageInvalidation(t, senderSubscription.Changes(), "sender local commit")
	status, err := sender.NetworkStatus(ctx)
	if err != nil || status.Queued != 1 || status.RelayAccepted != 0 || relay.eventCount() != 0 {
		t.Fatalf("durable pre-publish state = %#v, relay events=%d, err=%v", status, relay.eventCount(), err)
	}
	if err := sender.Synchronize(ctx); err != nil {
		t.Fatal(err)
	}
	waitUntil(t, 3*time.Second, func() bool {
		status, statusErr := sender.NetworkStatus(ctx)
		return statusErr == nil && status.Queued == 0 && status.RelayAccepted == 1 && relay.eventCount() == 1
	}, "sender outbox was not published exactly once")

	if err := receiver.AddRelay(ctx, domain.RelayConfig{URL: relay.url, Read: true, RequireAuth: true}); err != nil {
		t.Fatal(err)
	}
	if err := receiver.Synchronize(ctx); err != nil {
		t.Fatal(err)
	}
	assertMessageInvalidation(t, receiverSubscription.Changes(), "receiver retained catch-up")
	got, err := receiver.Get(ctx, messageID)
	if err != nil || got.Body != message.Body || got.SenderInstallationID != senderIdentity.InstallationID {
		t.Fatalf("receiver message = %#v, %v", got, err)
	}
	if relay.eventCount() != 1 {
		t.Fatalf("relay event count = %d", relay.eventCount())
	}

	sender.Close()
	receiver.Close()
	stopSender()
	stopReceiver()
	assertPersistedMessageCounts(t, senderPath, messageID, 1, 1, 0)
	assertPersistedMessageCounts(t, receiverPath, messageID, 1, 0, 1)
}

func initializeNodeIdentity(t *testing.T, databasePath string) identity.Material {
	t.Helper()
	keyPath, err := identity.KeyPath(databasePath)
	if err != nil {
		t.Fatal(err)
	}
	material, err := identity.Initialize(keyPath, nil)
	if err != nil {
		t.Fatal(err)
	}
	return material
}

func startTestNode(t *testing.T, databasePath string) func() {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- node.Run(ctx, databasePath) }()
	waitForNode(t, databasePath)
	var once sync.Once
	return func() {
		once.Do(func() {
			cancel()
			select {
			case err := <-done:
				if err != nil {
					t.Errorf("stop node %s: %v", databasePath, err)
				}
			case <-time.After(2 * time.Second):
				t.Errorf("node %s did not stop", databasePath)
			}
		})
	}
}

func assertMessageInvalidation(t *testing.T, changes <-chan domain.Invalidation, source string) {
	t.Helper()
	select {
	case change := <-changes:
		if change.Revision == 0 || len(change.Topics) != 1 || change.Topics[0] != domain.TopicMessages {
			t.Fatalf("%s invalidation = %#v", source, change)
		}
	case <-time.After(time.Second):
		t.Fatalf("%s did not invalidate promptly", source)
	}
}

func waitUntil(t *testing.T, timeout time.Duration, ready func() bool, failure string) {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for !ready() {
		if time.Now().After(deadline) {
			t.Fatal(failure)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func assertPersistedMessageCounts(t *testing.T, databasePath, messageID string, messages, outbox, wrappers int) {
	t.Helper()
	database, err := sql.Open("sqlite", databasePath)
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	database.SetMaxOpenConns(1)
	if _, err := database.Exec(`PRAGMA busy_timeout=5000`); err != nil {
		t.Fatal(err)
	}
	var messageCount, outboxCount, wrapperCount int
	if err := database.QueryRow(`SELECT count(*) FROM messages WHERE id=?`, messageID).Scan(&messageCount); err != nil {
		t.Fatal(err)
	}
	if err := database.QueryRow(`SELECT count(*) FROM outbox o JOIN messages m ON m.event_id=o.event_id WHERE m.id=?`, messageID).Scan(&outboxCount); err != nil {
		t.Fatal(err)
	}
	if err := database.QueryRow(`SELECT count(*) FROM inbound_wrappers w JOIN messages m ON m.event_id=w.canonical_event_id WHERE m.id=?`, messageID).Scan(&wrapperCount); err != nil {
		t.Fatal(err)
	}
	if messageCount != messages || outboxCount != outbox || wrapperCount != wrappers {
		t.Fatalf("%s persisted counts messages=%d outbox=%d wrappers=%d", databasePath, messageCount, outboxCount, wrapperCount)
	}
}

type retainedRelay struct {
	t      *testing.T
	server *httptest.Server
	url    string
	mu     sync.Mutex
	events map[string]json.RawMessage
}

func newRetainedRelay(t *testing.T) *retainedRelay {
	t.Helper()
	relay := &retainedRelay{t: t, events: make(map[string]json.RawMessage)}
	relay.server = httptest.NewServer(http.HandlerFunc(relay.serve))
	relay.url = "ws" + strings.TrimPrefix(relay.server.URL, "http")
	t.Cleanup(relay.server.Close)
	return relay
}

func (r *retainedRelay) serve(writer http.ResponseWriter, request *http.Request) {
	connection, err := websocket.Accept(writer, request, nil)
	if err != nil {
		return
	}
	defer connection.Close(websocket.StatusNormalClosure, "done")
	ctx := request.Context()
	challenge := "hq-node-integration"
	_ = writeRetainedRelay(ctx, connection, []any{"AUTH", challenge})
	authenticated := false
	for {
		_, raw, err := connection.Read(ctx)
		if err != nil {
			return
		}
		var frame []json.RawMessage
		if json.Unmarshal(raw, &frame) != nil || len(frame) == 0 {
			continue
		}
		var kind string
		_ = json.Unmarshal(frame[0], &kind)
		switch kind {
		case "AUTH":
			var auth event.NostrEvent
			if len(frame) == 2 {
				_ = json.Unmarshal(frame[1], &auth)
			}
			authenticated = auth.Kind == nostrwire.KindClientAuth && auth.VerifySignature() && relayTag(auth.Tags, "challenge", challenge) && relayTag(auth.Tags, "relay", r.url)
			_ = writeRetainedRelay(ctx, connection, []any{"OK", auth.ID, authenticated, "auth accepted"})
		case "EVENT":
			if len(frame) != 2 {
				continue
			}
			var outer event.NostrEvent
			_ = json.Unmarshal(frame[1], &outer)
			if !authenticated {
				_ = writeRetainedRelay(ctx, connection, []any{"OK", outer.ID, false, "auth-required: authenticate first"})
				continue
			}
			accepted := outer.Kind == nostrwire.KindGiftWrap && outer.VerifySignature()
			message := "invalid: bad gift wrap"
			if accepted {
				r.mu.Lock()
				if _, duplicate := r.events[outer.ID]; duplicate {
					accepted, message = false, "duplicate: already saved"
				} else {
					r.events[outer.ID], message = append([]byte(nil), frame[1]...), "saved"
				}
				r.mu.Unlock()
			}
			_ = writeRetainedRelay(ctx, connection, []any{"OK", outer.ID, accepted, message})
		case "REQ":
			if len(frame) != 3 {
				continue
			}
			var subscriptionID string
			_ = json.Unmarshal(frame[1], &subscriptionID)
			if !authenticated {
				_ = writeRetainedRelay(ctx, connection, []any{"CLOSED", subscriptionID, "auth-required: authenticate first"})
				continue
			}
			var filter map[string]json.RawMessage
			_ = json.Unmarshal(frame[2], &filter)
			for _, retained := range r.query(filter) {
				_ = writeRetainedRelay(ctx, connection, []any{"EVENT", subscriptionID, retained})
			}
			_ = writeRetainedRelay(ctx, connection, []any{"EOSE", subscriptionID})
		}
	}
}

func (r *retainedRelay) query(filter map[string]json.RawMessage) []json.RawMessage {
	var recipients []string
	_ = json.Unmarshal(filter["#p"], &recipients)
	r.mu.Lock()
	defer r.mu.Unlock()
	result := make([]json.RawMessage, 0, len(r.events))
	for _, raw := range r.events {
		var outer event.NostrEvent
		_ = json.Unmarshal(raw, &outer)
		if len(recipients) == 1 && !relayTag(outer.Tags, "p", recipients[0]) {
			continue
		}
		result = append(result, append([]byte(nil), raw...))
	}
	sort.Slice(result, func(i, j int) bool { return string(result[i]) < string(result[j]) })
	return result
}

func (r *retainedRelay) eventCount() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return len(r.events)
}

func relayTag(tags [][]string, name, value string) bool {
	for _, tag := range tags {
		if len(tag) == 2 && tag[0] == name && tag[1] == value {
			return true
		}
	}
	return false
}

func writeRetainedRelay(ctx context.Context, connection *websocket.Conn, value any) error {
	raw, err := json.Marshal(value)
	if err != nil {
		return err
	}
	return connection.Write(ctx, websocket.MessageText, raw)
}
