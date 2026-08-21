package domainrpc

import (
	"context"
	"encoding/json"
	"net"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/localwire"
)

type revisionOperations struct {
	*recordingOperations
	current func(context.Context) (uint64, error)
}

func (s *revisionOperations) CurrentRevision(ctx context.Context) (uint64, error) {
	return s.current(ctx)
}

func TestSubscriptionQueuesChangesUntilAcknowledgementAndCoalesces(t *testing.T) {
	entered := make(chan struct{})
	release := make(chan struct{})
	operations := &revisionOperations{
		recordingOperations: &recordingOperations{},
		current: func(context.Context) (uint64, error) {
			close(entered)
			<-release
			return 10, nil
		},
	}
	hub := NewSubscriptionHub()
	client, stop := subscriptionTestClient(t, Service{Store: operations, Subscriptions: hub})
	defer stop()
	response := make(chan SubscribeChangesResponse, 1)
	errors := make(chan error, 1)
	go func() {
		var acknowledged SubscribeChangesResponse
		err := client.Call(context.Background(), SubscribeChangesMethod, SubscribeChangesRequest{
			SubscriptionID: "messages", Topics: []domain.ChangeTopic{domain.TopicMessages, domain.TopicNetwork},
		}, &acknowledged)
		response <- acknowledged
		errors <- err
	}()
	<-entered
	hub.Publish(domain.Invalidation{Revision: 11, Topics: []domain.ChangeTopic{domain.TopicMessages}})
	hub.Publish(domain.Invalidation{Revision: 12, Topics: []domain.ChangeTopic{domain.TopicNetwork, domain.TopicPeers}})
	close(release)
	if err := <-errors; err != nil {
		t.Fatal(err)
	}
	if acknowledged := <-response; acknowledged.Revision != 10 {
		t.Fatalf("acknowledged revision = %d", acknowledged.Revision)
	}
	select {
	case notice := <-client.Notifications():
		if notice.SubscriptionID != "messages" || notice.Method != InvalidatedMethod {
			t.Fatalf("notification envelope = %#v", notice)
		}
		var change domain.Invalidation
		if err := json.Unmarshal(notice.Params, &change); err != nil {
			t.Fatal(err)
		}
		if change.Revision != 12 || len(change.Topics) != 2 || change.Topics[0] != domain.TopicMessages || change.Topics[1] != domain.TopicNetwork {
			t.Fatalf("coalesced invalidation = %#v", change)
		}
	case <-time.After(time.Second):
		t.Fatal("subscription did not emit a queued invalidation")
	}
	client.Close()
	deadline := time.Now().Add(time.Second)
	for {
		hub.mu.Lock()
		remaining := len(hub.subscribers)
		hub.mu.Unlock()
		if remaining == 0 {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("subscribers after disconnect = %d", remaining)
		}
		time.Sleep(time.Millisecond)
	}
}

func TestSlowSubscriberQueueStaysBoundedAndKeepsNewestRevision(t *testing.T) {
	hub := NewSubscriptionHub()
	subscriber := &changeSubscriber{
		hub: hub, id: "slow", topics: make(map[domain.ChangeTopic]bool),
		queue: make(chan domain.Invalidation, 1), active: true,
	}
	hub.subscribers[subscriber] = true
	for revision := uint64(1); revision <= 100; revision++ {
		topic := domain.TopicMessages
		if revision%2 == 0 {
			topic = domain.TopicNetwork
		}
		hub.Publish(domain.Invalidation{Revision: revision, Topics: []domain.ChangeTopic{topic}})
	}
	if len(subscriber.queue) != 1 {
		t.Fatalf("slow subscriber queue length = %d", len(subscriber.queue))
	}
	change := <-subscriber.queue
	if change.Revision != 100 || len(change.Topics) != 2 {
		t.Fatalf("coalesced slow subscriber change = %#v", change)
	}
}

func subscriptionTestClient(t *testing.T, service Service) (*localwire.Client, func()) {
	t.Helper()
	server, err := localwire.NewServer(localwire.ServerOptions{
		Modes: map[localwire.HandshakeMode]localwire.ModeConfig{
			localwire.DomainMode: {Supported: localwire.DomainVersions, Handler: service.Handle},
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	clientConnection, serverConnection := net.Pipe()
	done := make(chan error, 1)
	go func() { done <- server.ServeConn(context.Background(), serverConnection) }()
	client, err := localwire.NewClient(context.Background(), clientConnection, localwire.ClientOptions{
		Mode: localwire.DomainMode, Supported: localwire.DomainVersions,
	})
	if err != nil {
		t.Fatal(err)
	}
	return client, func() {
		client.Close()
		select {
		case <-done:
		case <-time.After(time.Second):
			t.Fatal("subscription test server did not stop")
		}
	}
}
