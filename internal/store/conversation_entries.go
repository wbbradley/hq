package store

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strings"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

type conversationEntryCursor struct {
	DisplayOrder int    `json:"display_order"`
	EventID      string `json:"event_id"`
}

func decodeConversationEntryCursor(raw string) (conversationEntryCursor, error) {
	decoded, err := base64.RawURLEncoding.DecodeString(raw)
	if err != nil {
		return conversationEntryCursor{}, fmt.Errorf("decode cursor: %w", err)
	}
	var wire struct {
		DisplayOrder *int    `json:"display_order"`
		EventID      *string `json:"event_id"`
	}
	decoder := json.NewDecoder(bytes.NewReader(decoded))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&wire); err != nil {
		return conversationEntryCursor{}, fmt.Errorf("decode cursor: %w", err)
	}
	if err := decoder.Decode(&struct{}{}); !errors.Is(err, io.EOF) {
		if err == nil {
			err = errors.New("multiple JSON values")
		}
		return conversationEntryCursor{}, fmt.Errorf("decode cursor: %w", err)
	}
	if wire.DisplayOrder == nil || wire.EventID == nil || *wire.DisplayOrder < 0 || len(*wire.EventID) != 64 || strings.ToLower(*wire.EventID) != *wire.EventID {
		return conversationEntryCursor{}, errors.New("decode cursor: incomplete or invalid fields")
	}
	if _, err := hex.DecodeString(*wire.EventID); err != nil {
		return conversationEntryCursor{}, errors.New("decode cursor: invalid event ID")
	}
	return conversationEntryCursor{DisplayOrder: *wire.DisplayOrder, EventID: *wire.EventID}, nil
}

type conversationEntryCandidate struct {
	kind         domain.ConversationEntryKind
	recordID     string
	eventID      string
	displayOrder int
}

func (s *SQLite) ListConversationEntries(ctx context.Context, filter model.ConversationHistoryFilter) (domain.ConversationEntryPage, error) {
	if !filter.Key.Valid() {
		return domain.ConversationEntryPage{}, errors.New("list conversation entries: invalid conversation key")
	}
	limit := pageLimit(filter.Limit)
	messageWhere := []string{`((sender_mailbox_id=? AND recipient_mailbox_id=?) OR (sender_mailbox_id=? AND recipient_mailbox_id=?))`}
	messageArgs := []any{filter.Key.CounterpartyMailboxID, model.HumanMailboxID, model.HumanMailboxID, filter.Key.CounterpartyMailboxID}
	if filter.Key.HarnessSessionID != "" {
		messageWhere = append(messageWhere, `harness_provider=?`, `harness_session_id=?`)
		messageArgs = append(messageArgs, filter.Key.HarnessProvider, filter.Key.HarnessSessionID)
	} else {
		messageWhere = append(messageWhere, `harness_session_id=''`, `thread_event_id=?`)
		messageArgs = append(messageArgs, filter.Key.ThreadID)
	}
	union := `SELECT 'message' AS entry_kind,id AS record_id,event_id,display_order FROM messages WHERE ` + strings.Join(messageWhere, " AND ")
	args := append([]any(nil), messageArgs...)
	if filter.Key.HarnessSessionID != "" {
		union += ` UNION ALL SELECT 'activity',event_id,event_id,display_order FROM harness_activities WHERE mailbox_id=? AND harness=? AND session_id=?`
		args = append(args, filter.Key.CounterpartyMailboxID, filter.Key.HarnessProvider, filter.Key.HarnessSessionID)
	}
	where := ""
	if filter.Cursor != "" {
		cursor, err := decodeConversationEntryCursor(filter.Cursor)
		if err != nil {
			return domain.ConversationEntryPage{}, fmt.Errorf("list conversation entries: %w", err)
		}
		where = ` WHERE (display_order>? OR (display_order=? AND event_id>?))`
		args = append(args, cursor.DisplayOrder, cursor.DisplayOrder, cursor.EventID)
	}
	args = append(args, limit+1)
	rows, err := s.db.QueryContext(ctx, `SELECT entry_kind,record_id,event_id,display_order FROM (`+union+`)`+where+` ORDER BY display_order,event_id LIMIT ?`, args...)
	if err != nil {
		return domain.ConversationEntryPage{}, fmt.Errorf("list conversation entries: %w", err)
	}
	var candidates []conversationEntryCandidate
	for rows.Next() {
		var candidate conversationEntryCandidate
		if err := rows.Scan(&candidate.kind, &candidate.recordID, &candidate.eventID, &candidate.displayOrder); err != nil {
			rows.Close()
			return domain.ConversationEntryPage{}, err
		}
		candidates = append(candidates, candidate)
	}
	if err := rows.Err(); err != nil {
		rows.Close()
		return domain.ConversationEntryPage{}, err
	}
	rows.Close()

	page := domain.ConversationEntryPage{}
	if len(candidates) > limit {
		last := candidates[limit-1]
		page.NextCursor, err = encodePageCursor(conversationEntryCursor{DisplayOrder: last.displayOrder, EventID: last.eventID})
		if err != nil {
			return domain.ConversationEntryPage{}, fmt.Errorf("list conversation entries: %w", err)
		}
		candidates = candidates[:limit]
	}
	page.Entries = make([]domain.ConversationEntry, 0, len(candidates))
	for _, candidate := range candidates {
		entry := domain.ConversationEntry{Kind: candidate.kind, EventID: candidate.eventID, DisplayOrder: candidate.displayOrder}
		switch candidate.kind {
		case domain.ConversationEntryMessage:
			message, err := s.Get(ctx, candidate.recordID)
			if err != nil {
				return domain.ConversationEntryPage{}, fmt.Errorf("hydrate conversation message: %w", err)
			}
			entry.Message = &message
		case domain.ConversationEntryActivity:
			activity, err := s.harnessActivityByEventID(ctx, candidate.recordID)
			if err != nil {
				return domain.ConversationEntryPage{}, fmt.Errorf("hydrate conversation activity: %w", err)
			}
			entry.Activity = &activity
		default:
			return domain.ConversationEntryPage{}, fmt.Errorf("unknown conversation entry kind %q", candidate.kind)
		}
		if !entry.Valid() {
			return domain.ConversationEntryPage{}, errors.New("projected conversation entry is invalid")
		}
		page.Entries = append(page.Entries, entry)
	}
	return page, nil
}
