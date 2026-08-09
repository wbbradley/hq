package syncer

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"slices"
	"sync/atomic"
	"time"

	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/nostrwire"
	"github.com/wbbradley/hq/internal/store"
)

type Transport interface {
	Dial(context.Context, string, *nostrwire.Codec) (nostrwire.RelayClient, error)
}

type State interface {
	InstallationIdentity() (string, string)
	ListRelays(context.Context) ([]store.RelayConfig, error)
	OutboundRelays(context.Context) ([]string, error)
	PrepareOutbound(context.Context, int) (int, error)
	RelayJobs(context.Context, string, int, time.Time) ([]store.RelayJob, error)
	RecordPublish(context.Context, string, string, bool, bool, string, time.Time, time.Time) error
	ReceiveGiftWrap(context.Context, []byte, string, time.Time) (store.ReceiveResult, error)
	SetRelaySyncState(context.Context, string, bool, bool, string, *time.Time, *time.Time) error
}

type SyncEngine interface {
	RunOnce(context.Context) error
	Run(context.Context) error
}

type Engine struct {
	State          State
	Transport      Transport
	Codec          *nostrwire.Codec
	Now            func() time.Time
	PageSize       int
	PublishTimeout time.Duration
	AuthTimeout    time.Duration
	PollInterval   time.Duration
	Random         io.Reader
	sequence       atomic.Uint64
}

func (e *Engine) defaults() {
	if e.Transport == nil {
		e.Transport = nostrwire.WebSocketDialer{}
	}
	if e.Now == nil {
		e.Now = time.Now
	}
	if e.PageSize <= 0 {
		e.PageSize = 500
	}
	if e.PublishTimeout <= 0 {
		e.PublishTimeout = 10 * time.Second
	}
	if e.AuthTimeout <= 0 {
		e.AuthTimeout = 5 * time.Second
	}
	if e.PollInterval <= 0 {
		e.PollInterval = 30 * time.Second
	}
}

func (e *Engine) RunOnce(ctx context.Context) error {
	e.defaults()
	if e.State == nil || e.Codec == nil {
		return errors.New("sync engine needs state and an installation codec")
	}
	if _, err := e.State.PrepareOutbound(ctx, 1000); err != nil {
		return err
	}
	configured, err := e.State.ListRelays(ctx)
	if err != nil {
		return err
	}
	type mode struct {
		read, write, requireAuth, unsafe bool
	}
	modes := make(map[string]mode)
	for _, relay := range configured {
		modes[relay.URL] = mode{relay.Read, relay.Write, relay.RequireAuth, relay.UnsafeNoAuth}
	}
	outbound, err := e.State.OutboundRelays(ctx)
	if err != nil {
		return err
	}
	for _, relay := range outbound {
		value := modes[relay]
		value.write = true
		modes[relay] = value
	}
	urls := make([]string, 0, len(modes))
	for relay := range modes {
		urls = append(urls, relay)
	}
	slices.Sort(urls)
	var failures []error
	for _, relay := range urls {
		if err := e.runRelay(ctx, relay, modes[relay]); err != nil {
			failures = append(failures, fmt.Errorf("%s: %w", relay, err))
		}
	}
	return errors.Join(failures...)
}

func (e *Engine) Run(ctx context.Context) error {
	e.defaults()
	attempt := 0
	for {
		err := e.RunOnce(ctx)
		if ctx.Err() != nil {
			return ctx.Err()
		}
		delay := e.PollInterval
		if err != nil {
			delay = nostrwire.BackoffWithJitter(attempt, e.Random)
			attempt++
		} else {
			attempt = 0
		}
		timer := time.NewTimer(delay)
		select {
		case <-ctx.Done():
			timer.Stop()
			return ctx.Err()
		case <-timer.C:
		}
	}
}

func (e *Engine) runRelay(ctx context.Context, relayURL string, mode struct{ read, write, requireAuth, unsafe bool }) (resultErr error) {
	client, err := e.Transport.Dial(ctx, relayURL, e.Codec)
	if err != nil {
		_ = e.State.SetRelaySyncState(context.Background(), relayURL, false, false, err.Error(), nil, nil)
		return err
	}
	defer func() {
		_ = client.Close()
		message := ""
		if resultErr != nil {
			message = resultErr.Error()
		}
		_ = e.State.SetRelaySyncState(context.Background(), relayURL, false, false, message, nil, nil)
	}()
	if mode.read {
		if err := e.read(ctx, client, relayURL, mode.requireAuth, mode.unsafe); err != nil {
			_ = e.State.SetRelaySyncState(context.Background(), relayURL, true, client.Authenticated(), err.Error(), nil, nil)
			return err
		}
	}
	if mode.write {
		if err := e.publish(ctx, client, relayURL); err != nil {
			_ = e.State.SetRelaySyncState(context.Background(), relayURL, true, client.Authenticated(), err.Error(), nil, nil)
			return err
		}
	}
	return e.State.SetRelaySyncState(ctx, relayURL, true, client.Authenticated(), "", nil, nil)
}

func (e *Engine) publish(ctx context.Context, client nostrwire.RelayClient, relayURL string) error {
	jobs, err := e.State.RelayJobs(ctx, relayURL, 1000, e.Now())
	if err != nil {
		return err
	}
	var failures []error
	for _, job := range jobs {
		publishCtx, cancel := context.WithTimeout(ctx, e.PublishTimeout)
		result, publishErr := client.Publish(publishCtx, job.ExactGiftWrapBytes, job.GiftWrapEventID)
		cancel()
		if publishErr == nil && !result.Accepted && len(result.Message) >= len("auth-required:") && result.Message[:len("auth-required:")] == "auth-required:" {
			authCtx, authCancel := context.WithTimeout(ctx, e.AuthTimeout)
			authErr := client.WaitAuth(authCtx)
			authCancel()
			if authErr == nil {
				publishCtx, cancel = context.WithTimeout(ctx, e.PublishTimeout)
				result, publishErr = client.Publish(publishCtx, job.ExactGiftWrapBytes, job.GiftWrapEventID)
				cancel()
			}
		}
		now := e.Now().UTC()
		if publishErr != nil {
			retry := now.Add(nostrwire.BackoffWithJitter(0, e.Random))
			_ = e.State.RecordPublish(context.Background(), job.CanonicalEventID, relayURL, false, false, publishErr.Error(), now, retry)
			failures = append(failures, publishErr)
			continue
		}
		accepted := result.Accepted || isDuplicateOK(result.Message)
		retry := now
		if !accepted {
			retry = now.Add(nostrwire.BackoffWithJitter(0, e.Random))
		}
		if err := e.State.RecordPublish(ctx, job.CanonicalEventID, relayURL, accepted, !accepted, result.Message, now, retry); err != nil {
			return err
		}
		if !accepted {
			failures = append(failures, fmt.Errorf("relay rejected %s: %s", job.GiftWrapEventID, result.Message))
		}
	}
	return errors.Join(failures...)
}

func isDuplicateOK(message string) bool {
	return len(message) >= 10 && (message[:10] == "duplicate:" || message[:10] == "duplicate ")
}

func (e *Engine) read(ctx context.Context, client nostrwire.RelayClient, relayURL string, requireAuth, unsafe bool) error {
	_, publicKey := e.State.InstallationIdentity()
	if requireAuth {
		authCtx, cancel := context.WithTimeout(ctx, e.AuthTimeout)
		err := client.WaitAuth(authCtx)
		cancel()
		if err != nil {
			return fmt.Errorf("NIP-42 authentication required: %w", err)
		}
	} else if !unsafe {
		return errors.New("private subscription without NIP-42 needs the unsafe override")
	}
	sequence := e.sequence.Add(1)
	liveID := fmt.Sprintf("hq-live-%d", sequence)
	live, err := client.Subscribe(ctx, liveID, nostrwire.Filter{Kinds: []int{int(nostrwire.KindGiftWrap)}, Tags: map[string][]string{"p": {publicKey}}, Limit: e.PageSize})
	if err != nil {
		return err
	}
	count, oldest, err := e.consumeUntilEOSE(ctx, live, relayURL)
	if err != nil {
		return err
	}
	until := oldest
	limit := e.PageSize
	page := 0
	more := count == e.PageSize
	for more && until > 0 {
		page++
		id := fmt.Sprintf("hq-page-%d-%d", sequence, page)
		sub, err := client.Subscribe(ctx, id, nostrwire.Filter{Kinds: []int{int(nostrwire.KindGiftWrap)}, Tags: map[string][]string{"p": {publicKey}}, Until: until, Limit: limit})
		if err != nil {
			return err
		}
		previousOldest := oldest
		count, oldest, err = e.consumeUntilEOSE(ctx, sub, relayURL)
		if err != nil {
			return err
		}
		more = count == limit
		if count == limit && oldest == previousOldest {
			if limit > 64_000/2 {
				return errors.New("relay catch-up cannot advance past one timestamp")
			}
			limit *= 2
			continue
		}
		until = oldest
		limit = e.PageSize
	}
	for {
		select {
		case frame := <-live.Frames:
			if frame.EOSE {
				continue
			}
			if err := e.receive(ctx, relayURL, frame.Event); err != nil {
				return err
			}
		default:
			now := e.Now().UTC()
			return e.State.SetRelaySyncState(ctx, relayURL, true, client.Authenticated(), "", &now, nil)
		}
	}
}

func (e *Engine) consumeUntilEOSE(ctx context.Context, sub *nostrwire.Subscription, relayURL string) (int, int64, error) {
	count := 0
	oldest := int64(0)
	for {
		select {
		case frame, ok := <-sub.Frames:
			if !ok {
				return count, oldest, errors.New("subscription ended before EOSE")
			}
			if frame.EOSE {
				now := e.Now().UTC()
				if err := e.State.SetRelaySyncState(ctx, relayURL, true, false, "", &now, nil); err != nil {
					return count, oldest, err
				}
				return count, oldest, nil
			}
			created, err := outerCreatedAt(frame.Event)
			if err != nil {
				return count, oldest, err
			}
			if oldest == 0 || created < oldest {
				oldest = created
			}
			count++
			if err := e.receive(ctx, relayURL, frame.Event); err != nil {
				return count, oldest, err
			}
		case reason := <-sub.Closed:
			return count, oldest, fmt.Errorf("subscription closed: %s", reason)
		case <-ctx.Done():
			return count, oldest, ctx.Err()
		}
	}
}

func (e *Engine) receive(ctx context.Context, relayURL string, raw []byte) error {
	now := e.Now().UTC()
	_, err := e.State.ReceiveGiftWrap(ctx, raw, relayURL, now)
	if err == nil {
		_ = e.State.SetRelaySyncState(ctx, relayURL, true, false, "", nil, &now)
	}
	return err
}

func outerCreatedAt(raw []byte) (int64, error) {
	var outer event.NostrEvent
	if err := json.Unmarshal(raw, &outer); err != nil {
		return 0, err
	}
	return outer.CreatedAt, nil
}
