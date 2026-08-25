package store

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/model"
)

const harnessActivityQueryLimit = 1000

func (s *SQLite) UpsertHarnessActivity(ctx context.Context, activity domain.HarnessActivity) error {
	activity, err := normalizeHarnessActivity(activity, s.now)
	if err != nil {
		return err
	}
	account, parents, _, err := s.localAccountAction(ctx, "")
	if err != nil {
		return err
	}
	content, err := s.fitHarnessActivityContent(ctx, activity, account.ID, parents)
	if err != nil {
		return err
	}
	return s.appendContents(ctx, []event.Content{content}, []time.Time{activity.OccurredAt}, nil)
}

func (s *SQLite) fitHarnessActivityContent(ctx context.Context, activity domain.HarnessActivity, accountID string, parents []string) (event.Content, error) {
	makeContent := func(body string, truncated bool) (event.Content, error) {
		payload, err := event.MarshalPayload(event.HarnessActivityPayload{
			Correlation: activity.Correlation, Kind: activity.Kind, Status: activity.Status,
			Title: activity.Title, Body: body, Truncated: activity.Truncated || truncated,
			OccurredAt: activity.OccurredAt.UnixMilli(), RuntimeID: activity.RuntimeID, Sequence: activity.Sequence,
		})
		if err != nil {
			return event.Content{}, err
		}
		return event.Content{
			Schema: event.Schema2, Type: event.TypeHarnessActivity, Sender: s.localAddress(activity.MailboxID),
			Audience: &event.Audience{HumanAccountID: accountID}, Parents: uniqueSorted(parents),
			Scope: event.ScopeAccountAddressed, Payload: payload,
		}, nil
	}
	content, err := makeContent(activity.Body, false)
	if err != nil {
		return event.Content{}, err
	}
	if _, err = s.signContents(ctx, []event.Content{content}, []time.Time{activity.OccurredAt}); err == nil {
		return content, nil
	} else if !strings.Contains(err.Error(), "signed event wire") {
		return event.Content{}, fmt.Errorf("sign harness activity: %w", err)
	}

	best := ""
	low, high := 0, len(activity.Body)
	for low <= high {
		middle := low + (high-low)/2
		candidate, _ := truncateUTF8(activity.Body, middle)
		candidateContent, candidateErr := makeContent(candidate, true)
		if candidateErr == nil {
			_, candidateErr = s.signContents(ctx, []event.Content{candidateContent}, []time.Time{activity.OccurredAt})
		}
		if candidateErr == nil {
			best = candidate
			low = middle + 1
			continue
		}
		if !strings.Contains(candidateErr.Error(), "signed event wire") {
			high = middle - 1
			continue
		}
		high = middle - 1
	}
	if best == "" && (activity.Kind == domain.HarnessActivityPlan || activity.Kind == domain.HarnessActivityDiff || activity.Kind == domain.HarnessActivityProgress) {
		return event.Content{}, errors.New("harness activity required body cannot fit signed event wire")
	}
	content, err = makeContent(best, true)
	if err != nil {
		return event.Content{}, err
	}
	if _, err := s.signContents(ctx, []event.Content{content}, []time.Time{activity.OccurredAt}); err != nil {
		return event.Content{}, fmt.Errorf("fit harness activity signed wire: %w", err)
	}
	return content, nil
}

func (s *SQLite) ListHarnessActivities(ctx context.Context, filter domain.HarnessActivityFilter) ([]domain.HarnessActivity, error) {
	filter.MailboxID = strings.TrimSpace(filter.MailboxID)
	if filter.MailboxID == "" {
		return nil, errors.New("list harness activities: mailbox ID is required")
	}
	where := []string{"mailbox_id=?"}
	args := []any{filter.MailboxID}
	if filter.InstallationID != "" {
		where = append(where, "source_installation_id=?")
		args = append(args, filter.InstallationID)
	}
	if filter.Harness != "" {
		where = append(where, "harness=?")
		args = append(args, filter.Harness)
	}
	if filter.SessionID != "" {
		where = append(where, "session_id=?")
		args = append(args, filter.SessionID)
	}
	limit := filter.Limit
	if limit <= 0 || limit > harnessActivityQueryLimit {
		limit = harnessActivityQueryLimit
	}
	args = append(args, limit)
	rows, err := s.db.QueryContext(ctx, `SELECT event_id,source_installation_id,mailbox_id,audience_account_id,harness,session_id,operation_id,kind,item_id,status,title,body,truncated,occurred_at,runtime_id,source_sequence,display_order FROM (
SELECT event_id,source_installation_id,mailbox_id,audience_account_id,harness,session_id,operation_id,kind,item_id,status,title,body,truncated,occurred_at,runtime_id,source_sequence,display_order
FROM harness_activities WHERE `+strings.Join(where, " AND ")+`
ORDER BY display_order DESC,event_id DESC LIMIT ?
) ORDER BY display_order,event_id`, args...)
	if err != nil {
		return nil, fmt.Errorf("list harness activities: %w", err)
	}
	defer rows.Close()
	var activities []domain.HarnessActivity
	for rows.Next() {
		activity, scanErr := scanHarnessActivity(rows)
		if scanErr != nil {
			return nil, scanErr
		}
		activities = append(activities, activity)
	}
	return activities, rows.Err()
}

type harnessActivityScanner interface {
	Scan(...any) error
}

func scanHarnessActivity(scanner harnessActivityScanner) (domain.HarnessActivity, error) {
	var activity domain.HarnessActivity
	var occurredAt int64
	var sequence string
	if err := scanner.Scan(&activity.EventID, &activity.InstallationID, &activity.MailboxID, &activity.AudienceAccountID, &activity.Harness, &activity.SessionID, &activity.OperationID, &activity.Kind, &activity.ItemID, &activity.Status, &activity.Title, &activity.Body, &activity.Truncated, &occurredAt, &activity.RuntimeID, &sequence, &activity.DisplayOrder); err != nil {
		return domain.HarnessActivity{}, err
	}
	var err error
	activity.Sequence, err = strconv.ParseUint(sequence, 10, 64)
	if err != nil {
		return domain.HarnessActivity{}, fmt.Errorf("decode harness activity sequence: %w", err)
	}
	activity.Correlation = model.MessageCorrelation{Provider: activity.Harness, SessionID: activity.SessionID, OperationID: activity.OperationID, ItemID: activity.ItemID}
	activity.OccurredAt = time.UnixMilli(occurredAt).UTC()
	return activity, nil
}

func (s *SQLite) harnessActivityByEventID(ctx context.Context, eventID string) (domain.HarnessActivity, error) {
	row := s.db.QueryRowContext(ctx, `SELECT event_id,source_installation_id,mailbox_id,audience_account_id,harness,session_id,operation_id,kind,item_id,status,title,body,truncated,occurred_at,runtime_id,source_sequence,display_order FROM harness_activities WHERE event_id=?`, eventID)
	activity, err := scanHarnessActivity(row)
	if errors.Is(err, sql.ErrNoRows) {
		return domain.HarnessActivity{}, domain.ErrNotFound
	}
	return activity, err
}

func normalizeHarnessActivity(activity domain.HarnessActivity, now func() time.Time) (domain.HarnessActivity, error) {
	activity.MailboxID = strings.TrimSpace(activity.MailboxID)
	activity.RuntimeID = strings.TrimSpace(activity.RuntimeID)
	correlation := activity.Correlation
	correlation.Provider = strings.TrimSpace(correlation.Provider)
	correlation.SessionID = strings.TrimSpace(correlation.SessionID)
	correlation.OperationID = strings.TrimSpace(correlation.OperationID)
	correlation.ItemID = strings.TrimSpace(correlation.ItemID)
	correlation.RequestID = strings.TrimSpace(correlation.RequestID)
	if correlation == (model.MessageCorrelation{}) {
		correlation = model.MessageCorrelation{Provider: strings.TrimSpace(activity.Harness), SessionID: strings.TrimSpace(activity.SessionID), OperationID: strings.TrimSpace(activity.OperationID), ItemID: strings.TrimSpace(activity.ItemID)}
	}
	if activity.MailboxID == "" || correlation.Provider == "" || correlation.SessionID == "" || correlation.OperationID == "" {
		return activity, errors.New("harness activity requires mailbox, harness, session, and operation IDs")
	}
	if correlation.RequestID != "" {
		return activity, errors.New("harness activity cannot carry a request ID")
	}
	for label, pair := range map[string][2]string{
		"harness": {strings.TrimSpace(activity.Harness), correlation.Provider}, "session": {strings.TrimSpace(activity.SessionID), correlation.SessionID},
		"operation": {strings.TrimSpace(activity.OperationID), correlation.OperationID}, "item": {strings.TrimSpace(activity.ItemID), correlation.ItemID},
	} {
		if pair[0] != "" && pair[0] != pair[1] {
			return activity, fmt.Errorf("harness activity %s field conflicts with typed correlation", label)
		}
	}
	activity.Correlation = correlation
	activity.Harness, activity.SessionID, activity.OperationID, activity.ItemID = correlation.Provider, correlation.SessionID, correlation.OperationID, correlation.ItemID
	if activity.RuntimeID == "" || activity.Sequence == 0 {
		return activity, errors.New("harness activity requires runtime identity and positive sequence")
	}
	switch activity.Kind {
	case domain.HarnessActivityOperation, domain.HarnessActivityPlan, domain.HarnessActivityDiff:
		activity.ItemID = ""
	case domain.HarnessActivityCommand, domain.HarnessActivityFile, domain.HarnessActivityTool, domain.HarnessActivityProgress:
		if activity.ItemID == "" {
			return activity, fmt.Errorf("harness activity %s requires an item ID", activity.Kind)
		}
	default:
		return activity, fmt.Errorf("unknown harness activity kind %q", activity.Kind)
	}
	switch activity.Status {
	case "", domain.HarnessActivityRunning, domain.HarnessActivityCompleted, domain.HarnessActivityFailed, domain.HarnessActivityInterrupted:
	default:
		return activity, fmt.Errorf("unknown harness activity status %q", activity.Status)
	}
	if activity.Kind == domain.HarnessActivityOperation && activity.Status == "" {
		return activity, errors.New("operation activity requires a status")
	}
	if (activity.Kind == domain.HarnessActivityPlan || activity.Kind == domain.HarnessActivityDiff || activity.Kind == domain.HarnessActivityProgress) && strings.TrimSpace(activity.Body) == "" {
		return activity, fmt.Errorf("harness activity %s requires a body", activity.Kind)
	}
	if activity.Kind == domain.HarnessActivityCommand || activity.Kind == domain.HarnessActivityFile || activity.Kind == domain.HarnessActivityTool {
		if strings.TrimSpace(activity.Title) == "" {
			return activity, fmt.Errorf("harness activity %s requires a title", activity.Kind)
		}
		if activity.Status != domain.HarnessActivityCompleted && activity.Status != domain.HarnessActivityFailed && activity.Status != domain.HarnessActivityInterrupted {
			return activity, fmt.Errorf("harness activity %s requires a terminal status", activity.Kind)
		}
	}
	var truncated bool
	activity.Title, truncated = truncateUTF8(activity.Title, domain.HarnessActivityTitleBytes)
	activity.Truncated = activity.Truncated || truncated
	bodyLimit := domain.HarnessActivityBodyBytes
	if activity.Kind == domain.HarnessActivityCommand {
		bodyLimit = domain.HarnessActivityCommandBodyBytes
	} else if activity.Kind == domain.HarnessActivityProgress {
		bodyLimit = domain.HarnessActivityProgressBytes
	}
	activity.Body, truncated = truncateUTF8(activity.Body, bodyLimit)
	activity.Truncated = activity.Truncated || truncated
	if activity.OccurredAt.IsZero() {
		if now == nil {
			now = time.Now
		}
		activity.OccurredAt = now().UTC()
	} else {
		activity.OccurredAt = activity.OccurredAt.UTC()
	}
	return activity, nil
}

func truncateUTF8(value string, limit int) (string, bool) {
	if len(value) <= limit {
		return value, false
	}
	end := limit
	for end > 0 && !utf8.ValidString(value[:end]) {
		end--
	}
	return value[:end], true
}

var _ domain.HarnessActivityOperations = (*SQLite)(nil)
