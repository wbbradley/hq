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

const defaultMailboxRepairInterval = 5 * time.Minute

type DeliveryStore interface {
	ClaimStore
}

type Dispatcher struct {
	Client         *Client
	Store          DeliveryStore
	Ledger         DeliveryLedger
	Replies        *ReplyRegistry
	State          *ThreadState
	ThreadID       string
	MailboxID      string
	Invalidations  <-chan domain.Invalidation
	RepairInterval time.Duration
	Sync           func(context.Context) error
}

type claimedDelivery struct {
	message model.Message
	token   string
}

func (d *Dispatcher) Run(ctx context.Context) error {
	if d.Client == nil || d.Store == nil || d.Ledger == nil || d.State == nil || d.ThreadID == "" || d.MailboxID == "" {
		return errors.New("Codex inbound dispatcher is missing a required dependency")
	}
	interval := d.RepairInterval
	if interval <= 0 {
		interval = defaultMailboxRepairInterval
	}
	for {
		if err := ctx.Err(); err != nil {
			return nil
		}
		if d.Sync != nil {
			if err := d.Sync(ctx); err != nil && ctx.Err() == nil {
				return fmt.Errorf("sync HQ mailbox: %w", err)
			}
		}
		if d.Replies != nil {
			claimed, err := d.Replies.ClaimOne(ctx, d.Store, d.MailboxID)
			if err != nil && ctx.Err() == nil {
				return fmt.Errorf("claim structured HQ reply: %w", err)
			}
			if claimed {
				continue
			}
		}
		delivery, err := d.claim(ctx)
		if errors.Is(err, domain.ErrNotReady) {
			if !d.waitForMailbox(ctx, interval) {
				return nil
			}
			continue
		}
		if err != nil {
			if ctx.Err() != nil {
				return nil
			}
			return fmt.Errorf("claim HQ message: %w", err)
		}
		accepted, dispatchErr := d.deliver(ctx, delivery.message)
		if dispatchErr != nil || !accepted {
			d.release(delivery)
			if ctx.Err() != nil {
				return nil
			}
			if !d.waitForMailbox(ctx, interval) {
				return nil
			}
			continue
		}
		if err := d.Store.Complete(ctx, delivery.message.ID, delivery.token); err != nil {
			if ctx.Err() != nil {
				d.release(delivery)
				return nil
			}
			if !errors.Is(err, domain.ErrNotReady) {
				d.release(delivery)
				return fmt.Errorf("complete HQ message %s: %w", delivery.message.ID, err)
			}
		}
	}
}

func (d *Dispatcher) waitForMailbox(ctx context.Context, repairInterval time.Duration) bool {
	timer := time.NewTimer(repairInterval)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return false
	case <-d.Invalidations:
		return true
	case <-timer.C:
		return true
	}
}

func (d *Dispatcher) claim(ctx context.Context) (claimedDelivery, error) {
	token, err := uuid.NewV7()
	if err != nil {
		return claimedDelivery{}, err
	}
	claim := domain.Claim{RecipientMailboxID: d.MailboxID}
	if d.Replies != nil {
		claim.ExcludeReplyTo = d.Replies.OutstandingIDs()
	}
	message, err := d.Store.Claim(ctx, claim, token.String())
	return claimedDelivery{message: message, token: token.String()}, err
}

func (d *Dispatcher) release(delivery claimedDelivery) {
	releaseContext, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	_ = d.Store.Release(releaseContext, delivery.message.ID, delivery.token)
}

func (d *Dispatcher) deliver(ctx context.Context, message model.Message) (bool, error) {
	record, exists, err := d.Ledger.Delivery(d.ThreadID, message.ID)
	if err != nil {
		return false, err
	}
	if exists && record.State == DeliveryAccepted {
		return true, nil
	}
	if exists && record.State == DeliveryUncertain {
		found, err := d.reconcile(ctx, message.ID)
		if err != nil {
			return false, err
		}
		if found {
			if err := d.Ledger.SetDelivery(d.ThreadID, message.ID, DeliveryAccepted); err != nil {
				return false, err
			}
			return true, nil
		}
	}
	if !exists {
		if err := d.Ledger.SetDelivery(d.ThreadID, message.ID, DeliveryPending); err != nil {
			return false, err
		}
	}
	if err := d.Ledger.SetDelivery(d.ThreadID, message.ID, DeliveryUncertain); err != nil {
		return false, err
	}
	if err := d.dispatch(ctx, message); err != nil {
		return false, err
	}
	if err := d.Ledger.SetDelivery(d.ThreadID, message.ID, DeliveryAccepted); err != nil {
		return false, err
	}
	return true, nil
}

func (d *Dispatcher) dispatch(ctx context.Context, message model.Message) error {
	input := []TextInput{{Type: "text", Text: message.Body}}
	for {
		activeTurnID := d.State.ActiveTurnID()
		if activeTurnID == "" {
			var response TurnResponse
			params := TurnStartParams{ThreadID: d.ThreadID, Input: input, ClientUserMessageID: message.ID}
			if err := d.Client.Call(ctx, "turn/start", params, &response); err != nil {
				return err
			}
			if response.Turn.ID == "" {
				return errors.New("turn/start returned an empty turn ID")
			}
			d.State.SetActive(response.Turn.ID)
			return nil
		}
		params := TurnSteerParams{ThreadID: d.ThreadID, ExpectedTurnID: activeTurnID, Input: input, ClientUserMessageID: message.ID}
		var response TurnSteerResponse
		err := d.Client.Call(ctx, "turn/steer", params, &response)
		if err == nil {
			if response.TurnID != activeTurnID {
				return fmt.Errorf("turn/steer accepted turn %q instead of expected turn %q", response.TurnID, activeTurnID)
			}
			return nil
		}
		cannotSteer := nonSteerableError(err)
		thread, readErr := d.readThread(ctx)
		if readErr != nil {
			if cannotSteer {
				if waitErr := d.State.WaitForChange(ctx, activeTurnID); waitErr != nil {
					return waitErr
				}
				continue
			}
			return err
		}
		if threadHasClientID(thread, message.ID) {
			return nil
		}
		d.State.UpdateThread(thread)
		refreshedTurnID := d.State.ActiveTurnID()
		if refreshedTurnID == "" || refreshedTurnID != activeTurnID {
			continue
		}
		if !cannotSteer {
			return err
		}
		if err := d.State.WaitForChange(ctx, activeTurnID); err != nil {
			return err
		}
	}
}

func (d *Dispatcher) reconcile(ctx context.Context, messageID string) (bool, error) {
	thread, err := d.readThread(ctx)
	if err != nil {
		return false, err
	}
	d.State.UpdateThread(thread)
	return threadHasClientID(thread, messageID), nil
}

func (d *Dispatcher) readThread(ctx context.Context) (Thread, error) {
	var response ThreadResponse
	if err := d.Client.Call(ctx, "thread/read", ThreadReadParams{ThreadID: d.ThreadID, IncludeTurns: true}, &response); err != nil {
		return Thread{}, err
	}
	if response.Thread.ID != d.ThreadID {
		return Thread{}, fmt.Errorf("thread/read returned thread %q instead of %q", response.Thread.ID, d.ThreadID)
	}
	return response.Thread, nil
}

func threadHasClientID(thread Thread, clientID string) bool {
	for _, turn := range thread.Turns {
		for _, item := range turn.Items {
			if item.Type == "userMessage" && item.ClientID == clientID {
				return true
			}
		}
	}
	return false
}

func nonSteerableError(err error) bool {
	message := strings.ToLower(err.Error())
	for _, fragment := range []string{"cannot steer", "not steerable", "does not accept steering", "cannot accept steering", "active operation"} {
		if strings.Contains(message, fragment) {
			return true
		}
	}
	return false
}
