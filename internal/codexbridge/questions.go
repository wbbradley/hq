package codexbridge

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/google/uuid"
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
	MessageID string
	spec      QuestionSpec
	waiter    *ReplyWaiter
}

type AnswerValidator func(string) (any, error)

type Questioner struct {
	Store        QuestionStore
	Replies      *ReplyRegistry
	Mailbox      model.Mailbox
	ThreadID     string
	Repository   model.RepositoryContext
	Sync         func(context.Context) error
	PollInterval time.Duration
}

func (q *Questioner) CorrelationThreadID() string { return q.ThreadID }

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
	human, err := q.Store.HumanMailbox(ctx)
	if err != nil {
		waiter.Cancel()
		return nil, err
	}
	details := strings.TrimSpace(spec.Details)
	if details != "" {
		details += "\n\n"
	}
	details += correlationDetails(spec.Correlation, messageID.String())
	message := model.Message{
		ID: messageID.String(), Context: q.Repository, SenderMailboxID: q.Mailbox.ID,
		RecipientMailboxID: human.ID, Body: spec.Body, Details: details, CreatedAt: time.Now().UTC(),
	}
	if err := q.Store.Create(ctx, message); err != nil {
		waiter.Cancel()
		return nil, err
	}
	if q.Sync != nil {
		if err := q.Sync(ctx); err != nil {
			waiter.Cancel()
			_ = q.Store.Archive(context.Background(), message.ID)
			return nil, err
		}
	}
	return &PendingQuestion{MessageID: message.ID, spec: spec, waiter: waiter}, nil
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
	if strings.TrimSpace(details) != "" {
		details += "\n\n"
	}
	details += correlationDetails(correlation, messageID.String())
	message := model.Message{
		ID: messageID.String(), Context: q.Repository, SenderMailboxID: q.Mailbox.ID,
		RecipientMailboxID: human.ID, Body: body, Details: details, CreatedAt: time.Now().UTC(),
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
	interval := q.PollInterval
	if interval <= 0 {
		interval = 100 * time.Millisecond
	}
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	for {
		select {
		case reply, open := <-pending.waiter.Replies:
			if !open || reply == nil {
				return nil, ErrHumanCancelled
			}
			return reply, nil
		case <-ctx.Done():
			q.Cancel(pending)
			return nil, ctx.Err()
		case <-ticker.C:
			message, err := q.Store.Get(ctx, pending.MessageID)
			if err != nil {
				q.Cancel(pending)
				return nil, err
			}
			if message.ArchivedAt == nil {
				continue
			}
			replies, err := q.Store.List(ctx, model.Filter{ReplyTo: pending.MessageID, RecipientMailboxID: q.Mailbox.ID, Limit: 1})
			if err != nil {
				q.Cancel(pending)
				return nil, err
			}
			if len(replies) == 0 {
				pending.waiter.Cancel()
				return nil, ErrHumanCancelled
			}
		}
	}
}

func correlationDetails(correlation RequestCorrelation, hqMessageID string) string {
	return fmt.Sprintf("Codex thread: %s\nCodex turn: %s\nCodex item: %s\nCodex request: %s\nHQ message: %s", correlation.ThreadID, correlation.TurnID, correlation.ItemID, correlation.RequestID, hqMessageID)
}
