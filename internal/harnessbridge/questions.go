package harnessbridge

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/harness"
	"github.com/wbbradley/hq/internal/model"
)

var errHumanCancelled = errors.New("human cancelled the HQ question")

type requestCorrelation struct {
	sessionID   string
	operationID string
	itemID      string
	requestID   string
}

type questionSpec struct {
	body        string
	details     string
	correlation requestCorrelation
}

type pendingQuestion struct {
	messageID    string
	spec         questionSpec
	registration *replyRegistration
	subscription domain.ChangeSubscription
}

type answerValidator func(string) (harness.ResponsePayload, error)

type questioner struct {
	store          QuestionStore
	replies        *replyRegistry
	mailbox        model.Mailbox
	session        harness.SessionIdentity
	repository     model.RepositoryContext
	sync           func(context.Context) error
	subscribe      func(context.Context, ...domain.ChangeTopic) (domain.ChangeSubscription, error)
	repairInterval time.Duration
	terms          Terminology
}

func (q *questioner) ask(ctx context.Context, spec questionSpec, validate answerValidator) (harness.ResponsePayload, error) {
	pending, err := q.publish(ctx, spec)
	if err != nil {
		return nil, err
	}
	for {
		reply, err := q.await(ctx, pending)
		if err != nil {
			return nil, err
		}
		value, validationErr := validate(strings.TrimSpace(reply.message.Body))
		if validationErr == nil {
			if err := reply.complete(context.Background()); err != nil {
				_ = reply.release(context.Background())
				return nil, err
			}
			return value, nil
		}
		if err := reply.complete(context.Background()); err != nil {
			_ = reply.release(context.Background())
			return nil, err
		}
		reprompt := pending.spec
		reprompt.body = "Invalid reply; please answer again: " + pending.spec.body
		reprompt.details = "Validation error: " + validationErr.Error() + "\n\n" + pending.spec.details
		pending, err = q.publish(ctx, reprompt)
		if err != nil {
			return nil, err
		}
	}
}

func (q *questioner) publish(ctx context.Context, spec questionSpec) (*pendingQuestion, error) {
	if q.store == nil || q.replies == nil || q.mailbox.ID == "" {
		return nil, errors.New("HQ question publisher is not bound")
	}
	messageID, err := uuid.NewV7()
	if err != nil {
		return nil, err
	}
	registration, err := q.replies.register(messageID.String())
	if err != nil {
		return nil, err
	}
	var subscription domain.ChangeSubscription
	if q.subscribe != nil {
		subscription, err = q.subscribe(ctx, domain.TopicMessages, domain.TopicMailboxes)
		if err != nil {
			q.replies.cancel(messageID.String())
			return nil, fmt.Errorf("subscribe to HQ question updates: %w", err)
		}
	}
	human, err := q.store.HumanMailbox(ctx)
	if err != nil {
		q.replies.cancel(messageID.String())
		closeSubscription(subscription)
		return nil, err
	}
	details := strings.TrimSpace(spec.details)
	if details != "" {
		details += "\n\n"
	}
	details += q.correlationDetails(spec.correlation, messageID.String())
	message := model.Message{
		ID: messageID.String(), Context: q.repository, SenderMailboxID: q.mailbox.ID, RecipientMailboxID: human.ID,
		Purpose: model.MessagePurposeProtocolQuestion, Body: spec.body, Details: details, CreatedAt: time.Now().UTC(),
	}
	if err := q.store.Create(ctx, message); err != nil {
		q.replies.cancel(messageID.String())
		closeSubscription(subscription)
		return nil, err
	}
	if q.sync != nil {
		if err := q.sync(ctx); err != nil {
			q.replies.cancel(messageID.String())
			closeSubscription(subscription)
			_ = q.store.Archive(context.Background(), message.ID)
			return nil, err
		}
	}
	return &pendingQuestion{messageID: message.ID, spec: spec, registration: registration, subscription: subscription}, nil
}

func (q *questioner) notice(ctx context.Context, body, details string, correlation requestCorrelation) error {
	human, err := q.store.HumanMailbox(ctx)
	if err != nil {
		return err
	}
	id, err := uuid.NewV7()
	if err != nil {
		return err
	}
	if strings.TrimSpace(details) != "" {
		details += "\n\n"
	}
	details += "Kind: notice\n" + q.correlationDetails(correlation, id.String())
	message := model.Message{ID: id.String(), Context: q.repository, SenderMailboxID: q.mailbox.ID, RecipientMailboxID: human.ID, Purpose: model.MessagePurposeSystemNotice, Body: body, Details: details, CreatedAt: time.Now().UTC()}
	if err := q.store.Create(ctx, message); err != nil {
		return err
	}
	if q.sync != nil {
		return q.sync(ctx)
	}
	return nil
}

func (q *questioner) await(ctx context.Context, pending *pendingQuestion) (*claimedReply, error) {
	interval := q.repairInterval
	if interval <= 0 {
		interval = defaultRepairInterval
	}
	timer := time.NewTimer(interval)
	defer timer.Stop()
	var changes <-chan domain.Invalidation
	if pending.subscription != nil {
		changes = pending.subscription.Changes()
	}
	for {
		select {
		case reply, open := <-pending.registration.replies:
			closeSubscription(pending.subscription)
			if !open || reply == nil {
				return nil, errHumanCancelled
			}
			return reply, nil
		case <-ctx.Done():
			q.cancel(pending)
			return nil, ctx.Err()
		case <-changes:
		case <-timer.C:
		}
		message, err := q.store.Get(ctx, pending.messageID)
		if err != nil {
			q.cancel(pending)
			return nil, err
		}
		if message.ArchivedAt == nil {
			resetTimer(timer, interval)
			continue
		}
		replies, err := q.store.List(ctx, model.Filter{ReplyTo: pending.messageID, RecipientMailboxID: q.mailbox.ID, Limit: 1})
		if err != nil {
			q.cancel(pending)
			return nil, err
		}
		if len(replies) == 0 {
			q.replies.cancel(pending.messageID)
			closeSubscription(pending.subscription)
			return nil, errHumanCancelled
		}
	}
}

func (q *questioner) cancel(pending *pendingQuestion) {
	if pending == nil {
		return
	}
	q.replies.cancel(pending.messageID)
	closeSubscription(pending.subscription)
	_ = q.store.Archive(context.Background(), pending.messageID)
	select {
	case reply := <-pending.registration.replies:
		if reply != nil {
			_ = reply.release(context.Background())
		}
	default:
	}
}

func (q *questioner) correlationDetails(correlation requestCorrelation, messageID string) string {
	return fmt.Sprintf("Harness provider: %s\nHarness session: %s\nHarness operation: %s\nHarness item: %s\nHarness request: %s\nHQ message: %s", q.session.Provider, correlation.sessionID, correlation.operationID, correlation.itemID, correlation.requestID, messageID)
}

func closeSubscription(subscription domain.ChangeSubscription) {
	if subscription != nil {
		subscription.Close()
	}
}

func resetTimer(timer *time.Timer, interval time.Duration) {
	if !timer.Stop() {
		select {
		case <-timer.C:
		default:
		}
	}
	timer.Reset(interval)
}

func exactDecision(choices []string) answerValidator {
	return func(answer string) (harness.ResponsePayload, error) {
		for _, choice := range choices {
			if answer == choice {
				return harness.DecisionResponse{Decision: answer}, nil
			}
		}
		return nil, fmt.Errorf("reply must exactly match one of: %s", strings.Join(choices, ", "))
	}
}

func structuredAnswer(answer string) (harness.ResponsePayload, error) {
	if answer == "decline" || answer == "cancel" {
		return harness.CancelResponse{Reason: answer}, nil
	}
	trimmed := strings.TrimSpace(answer)
	if strings.HasPrefix(trimmed, "accept ") {
		trimmed = strings.TrimSpace(strings.TrimPrefix(trimmed, "accept "))
	}
	var object map[string]any
	if json.Unmarshal([]byte(trimmed), &object) != nil || object == nil {
		return nil, errors.New("reply must be accept followed by one JSON object, decline, or cancel")
	}
	return harness.StructuredResponse{MediaType: "application/json", Data: []byte(trimmed)}, nil
}
