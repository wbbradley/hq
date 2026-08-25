package codexbridge

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

var ErrHumanCancelled = errors.New("human cancelled the HQ question")

type QuestionStore interface {
	HumanMailbox(context.Context) (model.Mailbox, error)
	Create(context.Context, model.Message) error
	Get(context.Context, string) (model.Message, error)
	List(context.Context, model.Filter) ([]model.Message, error)
	Archive(context.Context, string) error
}

type RequestCorrelation struct {
	ThreadID  string
	TurnID    string
	ItemID    string
	RequestID string
}

type QuestionSpec struct {
	Body        string
	Details     string
	Correlation RequestCorrelation
}

type PendingQuestion struct {
	MessageID    string
	spec         QuestionSpec
	waiter       *ReplyWaiter
	subscription domain.ChangeSubscription
}

type AnswerValidator func(string) (any, error)

type Questioner struct {
	Store          QuestionStore
	Replies        *ReplyRegistry
	Mailbox        model.Mailbox
	ThreadID       string
	Repository     model.RepositoryContext
	Sync           func(context.Context) error
	Subscribe      func(context.Context, ...domain.ChangeTopic) (domain.ChangeSubscription, error)
	RepairInterval time.Duration
}

func (q *Questioner) CorrelationSessionID() string { return q.ThreadID }

func (q *Questioner) Publish(ctx context.Context, spec QuestionSpec) (*PendingQuestion, error) {
	if q.Store == nil || q.Replies == nil || q.Mailbox.ID == "" {
		return nil, errors.New("HQ question publisher is not bound")
	}
	messageID, err := uuid.NewV7()
	if err != nil {
		return nil, err
	}
	waiter, err := q.Replies.Register(messageID.String())
	if err != nil {
		return nil, err
	}
	var subscription domain.ChangeSubscription
	if q.Subscribe != nil {
		subscription, err = q.Subscribe(ctx, domain.TopicMessages, domain.TopicMailboxes)
		if err != nil {
			waiter.Cancel()
			return nil, fmt.Errorf("subscribe to HQ question updates: %w", err)
		}
	}
	closeSubscription := func() {
		if subscription != nil {
			subscription.Close()
		}
	}
	human, err := q.Store.HumanMailbox(ctx)
	if err != nil {
		waiter.Cancel()
		closeSubscription()
		return nil, err
	}
	details := strings.TrimSpace(spec.Details)
	message := model.Message{
		ID: messageID.String(), Context: q.Repository, SenderMailboxID: q.Mailbox.ID,
		RecipientMailboxID: human.ID, Purpose: model.MessagePurposeProtocolQuestion, Body: spec.Body, Details: details,
		Correlation: q.messageCorrelation(spec.Correlation), TechnicalSections: requestTechnicalSections(spec.Correlation), CreatedAt: time.Now().UTC(),
	}
	if err := q.Store.Create(ctx, message); err != nil {
		waiter.Cancel()
		closeSubscription()
		return nil, err
	}
	if q.Sync != nil {
		if err := q.Sync(ctx); err != nil {
			waiter.Cancel()
			closeSubscription()
			_ = q.Store.Archive(context.Background(), message.ID)
			return nil, err
		}
	}
	return &PendingQuestion{MessageID: message.ID, spec: spec, waiter: waiter, subscription: subscription}, nil
}

func (q *Questioner) Ask(ctx context.Context, spec QuestionSpec, validate AnswerValidator) (any, error) {
	pending, err := q.Publish(ctx, spec)
	if err != nil {
		return nil, err
	}
	return q.AwaitValidated(ctx, pending, validate)
}

func (q *Questioner) AwaitValidated(ctx context.Context, pending *PendingQuestion, validate AnswerValidator) (any, error) {
	for {
		reply, err := q.await(ctx, pending)
		if err != nil {
			return nil, err
		}
		value, validationErr := validate(strings.TrimSpace(reply.Message.Body))
		if validationErr == nil {
			if err := reply.Complete(context.Background()); err != nil {
				_ = reply.Release(context.Background())
				return nil, err
			}
			return value, nil
		}
		if err := reply.Complete(context.Background()); err != nil {
			_ = reply.Release(context.Background())
			return nil, err
		}
		reprompt := pending.spec
		reprompt.Body = "Invalid reply; please answer again: " + pending.spec.Body
		reprompt.Details = "Validation error: " + validationErr.Error() + "\n\n" + pending.spec.Details
		pending, err = q.Publish(ctx, reprompt)
		if err != nil {
			return nil, err
		}
	}
}

func (q *Questioner) Cancel(pending *PendingQuestion) {
	if pending == nil {
		return
	}
	pending.waiter.Cancel()
	if pending.subscription != nil {
		pending.subscription.Close()
	}
	_ = q.Store.Archive(context.Background(), pending.MessageID)
	select {
	case reply := <-pending.waiter.Replies:
		if reply != nil {
			_ = reply.Release(context.Background())
		}
	default:
	}
}

func (q *Questioner) Notice(ctx context.Context, body, details string, correlation RequestCorrelation) error {
	human, err := q.Store.HumanMailbox(ctx)
	if err != nil {
		return err
	}
	messageID, err := uuid.NewV7()
	if err != nil {
		return err
	}
	details = strings.TrimSpace(details)
	message := model.Message{
		ID: messageID.String(), Context: q.Repository, SenderMailboxID: q.Mailbox.ID,
		RecipientMailboxID: human.ID, Purpose: model.MessagePurposeSystemNotice, Body: body, Details: details,
		Presentation: model.PresentationNotice, Correlation: q.messageCorrelation(correlation),
		TechnicalSections: requestTechnicalSections(correlation), CreatedAt: time.Now().UTC(),
	}
	if err := q.Store.Create(ctx, message); err != nil {
		return err
	}
	if q.Sync != nil {
		return q.Sync(ctx)
	}
	return nil
}

func (q *Questioner) await(ctx context.Context, pending *PendingQuestion) (*ClaimedReply, error) {
	interval := q.RepairInterval
	if interval <= 0 {
		interval = defaultMailboxRepairInterval
	}
	timer := time.NewTimer(interval)
	defer timer.Stop()
	var changes <-chan domain.Invalidation
	if pending.subscription != nil {
		changes = pending.subscription.Changes()
	}
	for {
		select {
		case reply, open := <-pending.waiter.Replies:
			if pending.subscription != nil {
				pending.subscription.Close()
			}
			if !open || reply == nil {
				return nil, ErrHumanCancelled
			}
			return reply, nil
		case <-ctx.Done():
			q.Cancel(pending)
			return nil, ctx.Err()
		case <-changes:
		case <-timer.C:
		}
		message, err := q.Store.Get(ctx, pending.MessageID)
		if err != nil {
			q.Cancel(pending)
			return nil, err
		}
		if message.ArchivedAt == nil {
			if !timer.Stop() {
				select {
				case <-timer.C:
				default:
				}
			}
			timer.Reset(interval)
			continue
		}
		replies, err := q.Store.List(ctx, model.Filter{ReplyTo: pending.MessageID, RecipientMailboxID: q.Mailbox.ID, Limit: 1})
		if err != nil {
			q.Cancel(pending)
			return nil, err
		}
		if len(replies) == 0 {
			pending.waiter.Cancel()
			if pending.subscription != nil {
				pending.subscription.Close()
			}
			return nil, ErrHumanCancelled
		}
	}
}

func (q *Questioner) messageCorrelation(correlation RequestCorrelation) model.MessageCorrelation {
	sessionID := correlation.ThreadID
	if sessionID == "" {
		sessionID = q.ThreadID
	}
	result := model.MessageCorrelation{Provider: "codex", SessionID: sessionID, OperationID: correlation.TurnID, ItemID: correlation.ItemID, RequestID: correlation.RequestID}
	if !result.Valid() {
		return model.MessageCorrelation{}
	}
	return result
}

func requestTechnicalSections(correlation RequestCorrelation) []model.TechnicalSection {
	if correlation.RequestID == "" || correlation.TurnID != "" {
		return nil
	}
	return []model.TechnicalSection{{Namespace: "hq.harness.request", Fields: []model.TechnicalField{{Key: "request_id", Label: "Request", Value: correlation.RequestID}}}}
}
