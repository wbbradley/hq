package codexbridge

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

const canonicalOutputQueueSize = 64

type OutputStore interface {
	Create(context.Context, model.Message) error
	Get(context.Context, string) (model.Message, error)
}

type canonicalOutput struct {
	key     string
	body    string
	details string
}

type OutputRelay struct {
	store         OutputStore
	ledger        DeliveryLedger
	sync          func(context.Context) error
	queue         chan canonicalOutput
	done          chan struct{}
	now           func() time.Time
	lastCreatedAt time.Time

	bindMu     sync.RWMutex
	threadID   string
	mailbox    model.Mailbox
	repository model.RepositoryContext

	acceptMu sync.Mutex
	accept   bool
	enqueue  sync.WaitGroup
	stop     sync.Once
	doneOnce sync.Once

	errMu sync.Mutex
	err   error
}

func NewOutputRelay(store OutputStore, ledger DeliveryLedger, syncMailbox func(context.Context) error) *OutputRelay {
	relay := &OutputRelay{
		store: store, ledger: ledger, sync: syncMailbox, queue: make(chan canonicalOutput, canonicalOutputQueueSize),
		done: make(chan struct{}), accept: true, now: time.Now,
	}
	go relay.run()
	return relay
}

func (r *OutputRelay) Bind(threadID string, mailbox model.Mailbox, repository model.RepositoryContext) {
	r.bindMu.Lock()
	r.threadID, r.mailbox, r.repository = threadID, mailbox, repository
	r.bindMu.Unlock()
}

func (r *OutputRelay) HandleNotification(_ context.Context, notification Notification) {
	threadID, _, _ := r.binding()
	if threadID == "" {
		return
	}
	output, ok := canonicalOutputFromNotification(threadID, notification)
	if !ok {
		return
	}
	r.acceptMu.Lock()
	if !r.accept {
		r.acceptMu.Unlock()
		return
	}
	r.enqueue.Add(1)
	r.acceptMu.Unlock()
	defer r.enqueue.Done()
	select {
	case r.queue <- output:
	case <-r.done:
	}
}

func (r *OutputRelay) Done() <-chan struct{} { return r.done }

func (r *OutputRelay) Err() error {
	r.errMu.Lock()
	defer r.errMu.Unlock()
	return r.err
}

func (r *OutputRelay) StopAndWait() {
	r.stop.Do(func() {
		r.acceptMu.Lock()
		r.accept = false
		r.acceptMu.Unlock()
		r.enqueue.Wait()
		close(r.queue)
	})
	<-r.done
}

func (r *OutputRelay) run() {
	for output := range r.queue {
		if err := r.publish(output); err != nil {
			r.errMu.Lock()
			r.err = err
			r.errMu.Unlock()
			r.acceptMu.Lock()
			r.accept = false
			r.acceptMu.Unlock()
			r.doneOnce.Do(func() { close(r.done) })
			return
		}
	}
	r.doneOnce.Do(func() { close(r.done) })
}

func (r *OutputRelay) publish(output canonicalOutput) error {
	threadID, mailbox, repository := r.binding()
	if r.store == nil || r.ledger == nil || threadID == "" || mailbox.ID == "" {
		return errors.New("Codex output relay is not bound")
	}
	sent, err := r.ledger.OutputSent(threadID, output.key)
	if err != nil {
		return fmt.Errorf("read Codex output ledger: %w", err)
	}
	if sent {
		return nil
	}
	message := model.Message{
		ID: stableOutputMessageID(threadID, output.key), Context: repository,
		SenderMailboxID: mailbox.ID, RecipientMailboxID: model.HumanMailboxID,
		Body: output.body, Details: output.details, CreatedAt: r.nextCreatedAt(),
	}
	existing, err := r.store.Get(context.Background(), message.ID)
	switch {
	case err == nil:
		if !sameCanonicalOutput(existing, message) {
			return fmt.Errorf("Codex output message ID %s collides with different HQ content", message.ID)
		}
		if existing.CreatedAt.After(r.lastCreatedAt) {
			r.lastCreatedAt = existing.CreatedAt
		}
	case errors.Is(err, domain.ErrNotFound):
		if err := r.store.Create(context.Background(), message); err != nil {
			return fmt.Errorf("publish Codex output: %w", err)
		}
	default:
		return fmt.Errorf("reconcile Codex output: %w", err)
	}
	if r.sync != nil {
		if err := r.sync(context.Background()); err != nil {
			return fmt.Errorf("sync Codex output: %w", err)
		}
	}
	if err := r.ledger.MarkOutputSent(threadID, output.key); err != nil {
		return fmt.Errorf("checkpoint Codex output: %w", err)
	}
	return nil
}

func (r *OutputRelay) nextCreatedAt() time.Time {
	createdAt := r.now().UTC()
	if !r.lastCreatedAt.IsZero() && createdAt.Unix() <= r.lastCreatedAt.Unix() {
		createdAt = time.Unix(r.lastCreatedAt.Unix()+1, 0).UTC()
	}
	r.lastCreatedAt = createdAt
	return createdAt
}

func (r *OutputRelay) binding() (string, model.Mailbox, model.RepositoryContext) {
	r.bindMu.RLock()
	defer r.bindMu.RUnlock()
	return r.threadID, r.mailbox, r.repository
}

func canonicalOutputFromNotification(threadID string, notification Notification) (canonicalOutput, bool) {
	switch notification.Method {
	case "item/completed":
		var params ItemCompletedNotification
		if err := json.Unmarshal(notification.Params, &params); err != nil || params.ThreadID != threadID || params.TurnID == "" || params.Item.Type != "agentMessage" || params.Item.ID == "" || strings.TrimSpace(params.Item.Text) == "" {
			return canonicalOutput{}, false
		}
		details := fmt.Sprintf("Codex thread: %s\nCodex turn: %s\nCodex item: %s\nPhase: %s", params.ThreadID, params.TurnID, params.Item.ID, valueOrNone(params.Item.Phase))
		return canonicalOutput{key: params.Item.ID, body: params.Item.Text, details: details}, true
	case "turn/completed":
		var params TurnNotification
		if err := json.Unmarshal(notification.Params, &params); err != nil || params.ThreadID != threadID || params.Turn.ID == "" {
			return canonicalOutput{}, false
		}
		key := "turn-status:" + params.Turn.ID
		switch params.Turn.Status {
		case "failed":
			errorMessage := "(not provided)"
			additionalDetails := ""
			if params.Turn.Error != nil {
				errorMessage = valueOrNone(params.Turn.Error.Message)
				additionalDetails = strings.TrimSpace(params.Turn.Error.AdditionalDetails)
			}
			details := fmt.Sprintf("Codex thread: %s\nCodex turn: %s\nStatus: failed\nError: %s", params.ThreadID, params.Turn.ID, errorMessage)
			if additionalDetails != "" {
				details += "\nAdditional details: " + additionalDetails
			}
			return canonicalOutput{key: key, body: "Codex turn failed", details: details}, true
		case "interrupted":
			details := fmt.Sprintf("Codex thread: %s\nCodex turn: %s\nStatus: interrupted", params.ThreadID, params.Turn.ID)
			return canonicalOutput{key: key, body: "Codex turn interrupted", details: details}, true
		default:
			return canonicalOutput{}, false
		}
	default:
		return canonicalOutput{}, false
	}
}

func stableOutputMessageID(threadID, key string) string {
	return uuid.NewSHA1(uuid.NameSpaceURL, []byte("hq-codex-output\x00"+threadID+"\x00"+key)).String()
}

func sameCanonicalOutput(existing, expected model.Message) bool {
	return existing.ID == expected.ID && existing.SenderMailboxID == expected.SenderMailboxID && existing.RecipientMailboxID == expected.RecipientMailboxID && existing.Body == expected.Body && existing.Details == expected.Details
}
