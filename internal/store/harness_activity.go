package store

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/wbbradley/hq/internal/domain"
)

const harnessActivityQueryLimit = 1000

func (s *SQLite) UpsertHarnessActivity(ctx context.Context, activity domain.HarnessActivity) error {
	activity, err := normalizeHarnessActivity(activity, s.now)
	if err != nil {
		return err
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	result, err := tx.ExecContext(ctx, `INSERT INTO harness_activities(
mailbox_id,harness,session_id,operation_id,kind,item_id,status,title,body,truncated,occurred_at
) VALUES(?,?,?,?,?,?,?,?,?,?,?)
ON CONFLICT(harness,session_id,operation_id,kind,item_id) DO UPDATE SET
mailbox_id=excluded.mailbox_id,status=excluded.status,title=excluded.title,body=excluded.body,truncated=excluded.truncated,occurred_at=excluded.occurred_at
WHERE mailbox_id<>excluded.mailbox_id OR status<>excluded.status OR title<>excluded.title OR body<>excluded.body OR truncated<>excluded.truncated OR occurred_at<>excluded.occurred_at`,
		activity.MailboxID, activity.Harness, activity.SessionID, activity.OperationID, activity.Kind, activity.ItemID,
		activity.Status, activity.Title, activity.Body, activity.Truncated, activity.OccurredAt.UnixMilli())
	if err != nil {
		return fmt.Errorf("upsert harness activity: %w", err)
	}
	changed, err := result.RowsAffected()
	if err != nil {
		return err
	}
	if activity.Kind == domain.HarnessActivityProgress {
		pruned, pruneErr := tx.ExecContext(ctx, `DELETE FROM harness_activities WHERE rowid IN (
SELECT rowid FROM harness_activities WHERE harness=? AND session_id=? AND kind='progress'
ORDER BY occurred_at DESC,operation_id DESC,item_id DESC LIMIT -1 OFFSET ?
)`, activity.Harness, activity.SessionID, domain.HarnessActivityProgressRetained)
		if pruneErr != nil {
			return fmt.Errorf("prune harness activity progress: %w", pruneErr)
		}
		prunedCount, rowsErr := pruned.RowsAffected()
		if rowsErr != nil {
			return rowsErr
		}
		changed += prunedCount
	}
	if changed == 0 {
		return tx.Commit()
	}
	change, err := recordChangeTx(ctx, tx, []domain.ChangeTopic{domain.TopicActivities})
	if err != nil {
		return err
	}
	if err := tx.Commit(); err != nil {
		return err
	}
	s.notifyChange(change)
	return nil
}

func (s *SQLite) ListHarnessActivities(ctx context.Context, filter domain.HarnessActivityFilter) ([]domain.HarnessActivity, error) {
	filter.MailboxID = strings.TrimSpace(filter.MailboxID)
	if filter.MailboxID == "" {
		return nil, errors.New("list harness activities: mailbox ID is required")
	}
	where := []string{"mailbox_id=?"}
	args := []any{filter.MailboxID}
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
	rows, err := s.db.QueryContext(ctx, `SELECT mailbox_id,harness,session_id,operation_id,kind,item_id,status,title,body,truncated,occurred_at FROM (
SELECT mailbox_id,harness,session_id,operation_id,kind,item_id,status,title,body,truncated,occurred_at
FROM harness_activities WHERE `+strings.Join(where, " AND ")+`
ORDER BY occurred_at DESC,harness DESC,session_id DESC,operation_id DESC,kind DESC,item_id DESC LIMIT ?
) ORDER BY occurred_at,harness,session_id,operation_id,kind,item_id`, args...)
	if err != nil {
		return nil, fmt.Errorf("list harness activities: %w", err)
	}
	defer rows.Close()
	var activities []domain.HarnessActivity
	for rows.Next() {
		var activity domain.HarnessActivity
		var occurredAt int64
		if err := rows.Scan(&activity.MailboxID, &activity.Harness, &activity.SessionID, &activity.OperationID, &activity.Kind, &activity.ItemID, &activity.Status, &activity.Title, &activity.Body, &activity.Truncated, &occurredAt); err != nil {
			return nil, err
		}
		activity.OccurredAt = time.UnixMilli(occurredAt).UTC()
		activities = append(activities, activity)
	}
	return activities, rows.Err()
}

func normalizeHarnessActivity(activity domain.HarnessActivity, now func() time.Time) (domain.HarnessActivity, error) {
	activity.MailboxID = strings.TrimSpace(activity.MailboxID)
	activity.Harness = strings.TrimSpace(activity.Harness)
	activity.SessionID = strings.TrimSpace(activity.SessionID)
	activity.OperationID = strings.TrimSpace(activity.OperationID)
	activity.ItemID = strings.TrimSpace(activity.ItemID)
	if activity.MailboxID == "" || activity.Harness == "" || activity.SessionID == "" || activity.OperationID == "" {
		return activity, errors.New("harness activity requires mailbox, harness, session, and operation IDs")
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
