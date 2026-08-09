package syncer

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"sort"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/nostrwire"
	"github.com/wbbradley/hq/internal/store"
)

func TestTwoInstallationsSyncThroughAuthenticatedRelayWithCatchUp(t *testing.T) {
	relay := newFakeRelay(t)
	sender := syncStore(t)
	receiver := syncStore(t)
	senderID, senderKey := sender.InstallationIdentity()
	receiverID, receiverKey := receiver.InstallationIdentity()
	ctx := context.Background()
	if err := sender.TrustPeer(ctx, store.Peer{InstallationID: receiverID, SignerKeyID: receiverKey, Relays: []string{relay.url}}); err != nil {
		t.Fatal(err)
	}
	if err := receiver.TrustPeer(ctx, store.Peer{InstallationID: senderID, SignerKeyID: senderKey, Relays: []string{relay.url}}); err != nil {
		t.Fatal(err)
	}
	if err := receiver.AddRelay(ctx, store.RelayConfig{URL: relay.url, Read: true, RequireAuth: true}); err != nil {
		t.Fatal(err)
	}
	ids := []string{"0198c7ec-73b0-7cc3-a5f7-e31c77140da1", "0198c7ec-73b0-7cc3-a5f7-e31c77140da2"}
	for index, id := range ids {
		message := model.Message{ID: id, SenderMailboxID: model.HumanMailboxID, Body: "relay message " + id, Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC().Add(time.Duration(index) * time.Second)}
		if err := sender.CreatePeerMessage(ctx, message, receiverID, model.HumanMailboxID); err != nil {
			t.Fatal(err)
		}
	}
	senderEngine := &Engine{State: sender, Codec: sender.WireCodec(nil, nil), PageSize: 1, AuthTimeout: time.Second}
	if err := senderEngine.RunOnce(ctx); err != nil {
		t.Fatal(err)
	}
	if relay.eventCount() != 2 {
		t.Fatalf("relay event count = %d", relay.eventCount())
	}
	receiverEngine := &Engine{State: receiver, Codec: receiver.WireCodec(nil, nil), PageSize: 1, AuthTimeout: time.Second}
	if err := receiverEngine.RunOnce(ctx); err != nil {
		t.Fatal(err)
	}
	for _, id := range ids {
		message, err := receiver.Get(ctx, id)
		if err != nil || !strings.HasPrefix(message.Body, "relay message") {
			t.Fatalf("received %s = %#v, %v", id, message, err)
		}
	}
	if relay.authCount() < 2 {
		t.Fatalf("auth count = %d", relay.authCount())
	}
}

func TestCatchUpDoesNotSkipEventsWithTheSameTimestamp(t *testing.T) {
	relay := newFakeRelay(t)
	sender := syncStore(t)
	receiver := syncStore(t)
	senderID, senderKey := sender.InstallationIdentity()
	receiverID, receiverKey := receiver.InstallationIdentity()
	ctx := context.Background()
	if err := sender.TrustPeer(ctx, store.Peer{InstallationID: receiverID, SignerKeyID: receiverKey, Relays: []string{relay.url}}); err != nil {
		t.Fatal(err)
	}
	if err := receiver.TrustPeer(ctx, store.Peer{InstallationID: senderID, SignerKeyID: senderKey, Relays: []string{relay.url}}); err != nil {
		t.Fatal(err)
	}
	if err := receiver.AddRelay(ctx, store.RelayConfig{URL: relay.url, Read: true, RequireAuth: true}); err != nil {
		t.Fatal(err)
	}
	ids := []string{
		"0198c7ec-73b0-7cc3-a5f7-e31c77140db1",
		"0198c7ec-73b0-7cc3-a5f7-e31c77140db2",
		"0198c7ec-73b0-7cc3-a5f7-e31c77140db3",
		"0198c7ec-73b0-7cc3-a5f7-e31c77140db4",
	}
	for _, id := range ids {
		message := model.Message{ID: id, SenderMailboxID: model.HumanMailboxID, Body: "same time " + id, Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}
		if err := sender.CreatePeerMessage(ctx, message, receiverID, model.HumanMailboxID); err != nil {
			t.Fatal(err)
		}
	}
	jobs, err := sender.PendingOutbox(ctx, len(ids))
	if err != nil || len(jobs) != len(ids) {
		t.Fatalf("pending jobs = %#v, %v", jobs, err)
	}
	codec := sender.WireCodec(&sameTimestampReader{}, func() time.Time { return time.Unix(2_000_000_000, 0) })
	for _, job := range jobs {
		inspection := event.Inspect(job.ExactCanonicalBytes)
		if inspection.Status == event.StatusInvalid {
			t.Fatal(inspection.Err)
		}
		wrapped, err := codec.Wrap(inspection.Event, receiverKey)
		if err != nil {
			t.Fatal(err)
		}
		relay.seed(wrapped.ExactWire)
	}
	engine := &Engine{State: receiver, Codec: receiver.WireCodec(nil, nil), PageSize: 2, AuthTimeout: time.Second}
	if err := engine.RunOnce(ctx); err != nil {
		t.Fatal(err)
	}
	for _, id := range ids {
		if _, err := receiver.Get(ctx, id); err != nil {
			t.Fatalf("same-timestamp message %s: %v", id, err)
		}
	}
}

func TestDuplicateRelayOKRecoversCrashAfterRelayReceipt(t *testing.T) {
	relay := newFakeRelay(t)
	sender := syncStore(t)
	receiver := syncStore(t)
	receiverID, receiverKey := receiver.InstallationIdentity()
	ctx := context.Background()
	if err := sender.TrustPeer(ctx, store.Peer{InstallationID: receiverID, SignerKeyID: receiverKey, Relays: []string{relay.url}}); err != nil {
		t.Fatal(err)
	}
	message := model.Message{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140da3", SenderMailboxID: model.HumanMailboxID, Body: "duplicate accepted", Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}
	if err := sender.CreatePeerMessage(ctx, message, receiverID, model.HumanMailboxID); err != nil {
		t.Fatal(err)
	}
	if _, err := sender.PrepareOutbound(ctx, 10); err != nil {
		t.Fatal(err)
	}
	jobs, err := sender.RelayJobs(ctx, relay.url, 10, time.Now())
	if err != nil || len(jobs) != 1 {
		t.Fatalf("jobs = %#v, %v", jobs, err)
	}
	relay.seed(jobs[0].ExactGiftWrapBytes)
	engine := &Engine{State: sender, Codec: sender.WireCodec(nil, nil), AuthTimeout: time.Second}
	if err := engine.RunOnce(ctx); err != nil {
		t.Fatal(err)
	}
	attempts, err := sender.RelayAttempts(ctx, jobs[0].CanonicalEventID)
	if err != nil || len(attempts) != 1 || attempts[0].State != "accepted" || attempts[0].AcceptedAt == nil {
		t.Fatalf("attempts = %#v, %v", attempts, err)
	}
}

func TestEventAtEOSELiveHandoffIsNotLost(t *testing.T) {
	relay := newFakeRelay(t)
	sender := syncStore(t)
	receiver := syncStore(t)
	senderID, senderKey := sender.InstallationIdentity()
	receiverID, receiverKey := receiver.InstallationIdentity()
	ctx := context.Background()
	if err := sender.TrustPeer(ctx, store.Peer{InstallationID: receiverID, SignerKeyID: receiverKey, Relays: []string{relay.url}}); err != nil {
		t.Fatal(err)
	}
	if err := receiver.TrustPeer(ctx, store.Peer{InstallationID: senderID, SignerKeyID: senderKey, Relays: []string{relay.url}}); err != nil {
		t.Fatal(err)
	}
	if err := receiver.AddRelay(ctx, store.RelayConfig{URL: relay.url, Read: true, RequireAuth: true}); err != nil {
		t.Fatal(err)
	}
	ids := []string{"0198c7ec-73b0-7cc3-a5f7-e31c77140da7", "0198c7ec-73b0-7cc3-a5f7-e31c77140da8"}
	for _, id := range ids {
		message := model.Message{ID: id, SenderMailboxID: model.HumanMailboxID, Body: "handoff " + id, Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}
		if err := sender.CreatePeerMessage(ctx, message, receiverID, model.HumanMailboxID); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := sender.PrepareOutbound(ctx, 10); err != nil {
		t.Fatal(err)
	}
	jobs, err := sender.RelayJobs(ctx, relay.url, 10, time.Now())
	if err != nil || len(jobs) != 2 {
		t.Fatalf("jobs=%#v, %v", jobs, err)
	}
	relay.seed(jobs[0].ExactGiftWrapBytes)
	relay.mu.Lock()
	relay.holdLive = append([]byte(nil), jobs[1].ExactGiftWrapBytes...)
	relay.mu.Unlock()
	engine := &Engine{State: receiver, Codec: receiver.WireCodec(nil, nil), PageSize: 10, AuthTimeout: time.Second}
	if err := engine.RunOnce(ctx); err != nil {
		t.Fatal(err)
	}
	for _, id := range ids {
		if _, err := receiver.Get(ctx, id); err != nil {
			t.Fatalf("handoff message %s: %v", id, err)
		}
	}
}

func TestUnavailableRelayLeavesQueuedWork(t *testing.T) {
	sender := syncStore(t)
	receiver := syncStore(t)
	receiverID, receiverKey := receiver.InstallationIdentity()
	const unavailable = "ws://127.0.0.1:1"
	ctx := context.Background()
	if err := sender.TrustPeer(ctx, store.Peer{InstallationID: receiverID, SignerKeyID: receiverKey, Relays: []string{unavailable}}); err != nil {
		t.Fatal(err)
	}
	message := model.Message{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140da4", SenderMailboxID: model.HumanMailboxID, Body: "offline", Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}
	if err := sender.CreatePeerMessage(ctx, message, receiverID, model.HumanMailboxID); err != nil {
		t.Fatal(err)
	}
	engine := &Engine{State: sender, Codec: sender.WireCodec(nil, nil), AuthTimeout: 50 * time.Millisecond}
	if err := engine.RunOnce(ctx); err == nil {
		t.Fatal("unavailable relay sync succeeded")
	}
	jobs, err := sender.RelayJobs(ctx, unavailable, 10, time.Now())
	if err != nil || len(jobs) != 1 {
		t.Fatalf("queued jobs = %#v, %v", jobs, err)
	}
}

func TestRelayRejectionKeepsRetryState(t *testing.T) {
	relay := newFakeRelay(t)
	relay.rejectEvents = true
	sender := syncStore(t)
	receiver := syncStore(t)
	receiverID, receiverKey := receiver.InstallationIdentity()
	ctx := context.Background()
	if err := sender.TrustPeer(ctx, store.Peer{InstallationID: receiverID, SignerKeyID: receiverKey, Relays: []string{relay.url}}); err != nil {
		t.Fatal(err)
	}
	message := model.Message{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140da5", SenderMailboxID: model.HumanMailboxID, Body: "retry rejection", Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}
	if err := sender.CreatePeerMessage(ctx, message, receiverID, model.HumanMailboxID); err != nil {
		t.Fatal(err)
	}
	engine := &Engine{State: sender, Codec: sender.WireCodec(nil, nil), AuthTimeout: time.Second}
	if err := engine.RunOnce(ctx); err == nil || !strings.Contains(err.Error(), "rate-limited") {
		t.Fatalf("rejection error = %v", err)
	}
	var eventID string
	stored, err := sender.Get(ctx, message.ID)
	if err != nil {
		t.Fatal(err)
	}
	eventID = stored.EventID
	attempts, err := sender.RelayAttempts(ctx, eventID)
	if err != nil || len(attempts) != 1 || attempts[0].State != "retry" {
		t.Fatalf("attempts = %#v, %v", attempts, err)
	}
}

func TestDisconnectAfterRelayStoreRetriesSameWrapper(t *testing.T) {
	relay := newFakeRelay(t)
	relay.closeAfterStore = true
	sender := syncStore(t)
	receiver := syncStore(t)
	receiverID, receiverKey := receiver.InstallationIdentity()
	ctx := context.Background()
	if err := sender.TrustPeer(ctx, store.Peer{InstallationID: receiverID, SignerKeyID: receiverKey, Relays: []string{relay.url}}); err != nil {
		t.Fatal(err)
	}
	message := model.Message{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140da6", SenderMailboxID: model.HumanMailboxID, Body: "disconnect retry", Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}
	if err := sender.CreatePeerMessage(ctx, message, receiverID, model.HumanMailboxID); err != nil {
		t.Fatal(err)
	}
	now := time.Now().UTC()
	engine := &Engine{State: sender, Codec: sender.WireCodec(nil, nil), AuthTimeout: time.Second, Now: func() time.Time { return now }}
	if err := engine.RunOnce(ctx); err == nil {
		t.Fatal("disconnect sync succeeded")
	}
	if relay.eventCount() != 1 {
		t.Fatalf("relay did not retain event before disconnect: %d", relay.eventCount())
	}
	relay.mu.Lock()
	relay.closeAfterStore = false
	relay.mu.Unlock()
	now = now.Add(2 * time.Second)
	if err := engine.RunOnce(ctx); err != nil {
		t.Fatal(err)
	}
	var eventID string
	stored, _ := sender.Get(ctx, message.ID)
	eventID = stored.EventID
	attempts, err := sender.RelayAttempts(ctx, eventID)
	if err != nil || len(attempts) != 1 || attempts[0].State != "accepted" || attempts[0].AttemptCount != 2 {
		t.Fatalf("retry attempts = %#v, %v", attempts, err)
	}
}

func syncStore(t *testing.T) *store.SQLite {
	t.Helper()
	database := t.TempDir() + "/hq.db"
	key, err := identity.KeyPath(database)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := identity.Initialize(key, nil); err != nil {
		t.Fatal(err)
	}
	db, err := store.Open(database)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { db.Close() })
	return db
}

type fakeRelay struct {
	t               *testing.T
	server          *httptest.Server
	url             string
	mu              sync.Mutex
	events          map[string]json.RawMessage
	auths           int
	rejectEvents    bool
	closeAfterStore bool
	holdLive        []byte
}

func newFakeRelay(t *testing.T) *fakeRelay {
	t.Helper()
	relay := &fakeRelay{t: t, events: make(map[string]json.RawMessage)}
	relay.server = httptest.NewServer(http.HandlerFunc(relay.serve))
	relay.url = "ws" + strings.TrimPrefix(relay.server.URL, "http")
	t.Cleanup(relay.server.Close)
	return relay
}

func (r *fakeRelay) serve(writer http.ResponseWriter, request *http.Request) {
	connection, err := websocket.Accept(writer, request, nil)
	if err != nil {
		return
	}
	defer connection.Close(websocket.StatusNormalClosure, "done")
	ctx := request.Context()
	challenge := "hq-test-challenge"
	_ = writeRelay(ctx, connection, []any{"AUTH", challenge})
	authed := false
	for {
		_, raw, err := connection.Read(ctx)
		if err != nil {
			return
		}
		var parts []json.RawMessage
		if json.Unmarshal(raw, &parts) != nil || len(parts) == 0 {
			continue
		}
		var label string
		_ = json.Unmarshal(parts[0], &label)
		switch label {
		case "AUTH":
			if len(parts) != 2 {
				continue
			}
			var auth event.NostrEvent
			_ = json.Unmarshal(parts[1], &auth)
			accepted := auth.Kind == nostrwire.KindClientAuth && auth.VerifySignature() && hasTag(auth.Tags, "challenge", challenge) && hasTag(auth.Tags, "relay", r.url)
			authed = accepted
			if accepted {
				r.mu.Lock()
				r.auths++
				r.mu.Unlock()
			}
			_ = writeRelay(ctx, connection, []any{"OK", auth.ID, accepted, "auth accepted"})
		case "EVENT":
			if len(parts) != 2 {
				continue
			}
			var outer event.NostrEvent
			_ = json.Unmarshal(parts[1], &outer)
			if !authed {
				_ = writeRelay(ctx, connection, []any{"OK", outer.ID, false, "auth-required: authenticate first"})
				continue
			}
			r.mu.Lock()
			reject, closeAfter := r.rejectEvents, r.closeAfterStore
			r.mu.Unlock()
			if reject {
				_ = writeRelay(ctx, connection, []any{"OK", outer.ID, false, "rate-limited: slow down"})
				continue
			}
			accepted, message := r.storeEvent(parts[1], outer)
			if closeAfter {
				_ = connection.Close(websocket.StatusInternalError, "test disconnect")
				return
			}
			_ = writeRelay(ctx, connection, []any{"OK", outer.ID, accepted, message})
		case "REQ":
			if len(parts) != 3 {
				continue
			}
			var id string
			_ = json.Unmarshal(parts[1], &id)
			if !authed {
				_ = writeRelay(ctx, connection, []any{"CLOSED", id, "auth-required: authenticate first"})
				continue
			}
			var filter map[string]json.RawMessage
			_ = json.Unmarshal(parts[2], &filter)
			for _, item := range r.query(filter) {
				_ = writeRelay(ctx, connection, []any{"EVENT", id, item})
			}
			_ = writeRelay(ctx, connection, []any{"EOSE", id})
			r.mu.Lock()
			live := append([]byte(nil), r.holdLive...)
			r.holdLive = nil
			r.mu.Unlock()
			if len(live) > 0 {
				_ = writeRelay(ctx, connection, []any{"EVENT", id, json.RawMessage(live)})
			}
		case "CLOSE":
			continue
		}
	}
}

func (r *fakeRelay) storeEvent(raw json.RawMessage, outer event.NostrEvent) (bool, string) {
	if !outer.VerifySignature() || outer.Kind != nostrwire.KindGiftWrap {
		return false, "invalid: bad event"
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if _, exists := r.events[outer.ID]; exists {
		return false, "duplicate: already saved"
	}
	r.events[outer.ID] = append([]byte(nil), raw...)
	return true, "saved"
}

func (r *fakeRelay) query(filter map[string]json.RawMessage) []json.RawMessage {
	var recipients []string
	var until int64
	var limit int
	_ = json.Unmarshal(filter["#p"], &recipients)
	_ = json.Unmarshal(filter["until"], &until)
	_ = json.Unmarshal(filter["limit"], &limit)
	r.mu.Lock()
	defer r.mu.Unlock()
	type candidate struct {
		raw     json.RawMessage
		created int64
	}
	var items []candidate
	for _, raw := range r.events {
		var outer event.NostrEvent
		_ = json.Unmarshal(raw, &outer)
		if until != 0 && outer.CreatedAt > until {
			continue
		}
		if len(recipients) == 1 && !hasTag(outer.Tags, "p", recipients[0]) {
			continue
		}
		items = append(items, candidate{append([]byte(nil), raw...), outer.CreatedAt})
	}
	sort.Slice(items, func(i, j int) bool {
		if items[i].created == items[j].created {
			return string(items[i].raw) < string(items[j].raw)
		}
		return items[i].created > items[j].created
	})
	if limit > 0 && len(items) > limit {
		items = items[:limit]
	}
	result := make([]json.RawMessage, len(items))
	for i := range items {
		result[i] = items[i].raw
	}
	return result
}

func (r *fakeRelay) seed(raw []byte) {
	var outer event.NostrEvent
	if json.Unmarshal(raw, &outer) != nil {
		r.t.Fatal("seed event is invalid")
	}
	r.mu.Lock()
	r.events[outer.ID] = append([]byte(nil), raw...)
	r.mu.Unlock()
}

func (r *fakeRelay) eventCount() int { r.mu.Lock(); defer r.mu.Unlock(); return len(r.events) }
func (r *fakeRelay) authCount() int  { r.mu.Lock(); defer r.mu.Unlock(); return r.auths }

func writeRelay(ctx context.Context, connection *websocket.Conn, value any) error {
	raw, err := json.Marshal(value)
	if err != nil {
		return err
	}
	return connection.Write(ctx, websocket.MessageText, raw)
}

func hasTag(tags [][]string, name, value string) bool {
	for _, tag := range tags {
		if len(tag) == 2 && tag[0] == name && tag[1] == value {
			return true
		}
	}
	return false
}

type sameTimestampReader struct{ secret byte }

func (r *sameTimestampReader) Read(target []byte) (int, error) {
	if len(target) == 32 {
		r.secret++
		clear(target)
		target[len(target)-1] = r.secret
		return len(target), nil
	}
	for index := range target {
		target[index] = 1
	}
	return len(target), nil
}

func TestBackoffIsBounded(t *testing.T) {
	if BackoffForTest(-1) != time.Second || BackoffForTest(100) != 256*time.Second {
		t.Fatal("backoff is not bounded")
	}
	jittered := nostrwire.BackoffWithJitter(2, bytes.NewReader(make([]byte, 32)))
	if jittered < 3*time.Second || jittered > 5*time.Second {
		t.Fatalf("jittered backoff = %s", jittered)
	}
}

func BackoffForTest(attempt int) time.Duration { return nostrwire.Backoff(attempt) }
