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
	"sort"
	"strings"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

type conversationEntryCursor struct {
	AfterEventID string `json:"after_event_id"`
}

func decodeConversationEntryCursor(raw string) (conversationEntryCursor, error) {
	decoded, err := base64.RawURLEncoding.DecodeString(raw)
	if err != nil {
		return conversationEntryCursor{}, fmt.Errorf("decode cursor: %w", err)
	}
	var wire struct {
		AfterEventID *string `json:"after_event_id"`
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
	if wire.AfterEventID == nil || len(*wire.AfterEventID) != 64 || strings.ToLower(*wire.AfterEventID) != *wire.AfterEventID {
		return conversationEntryCursor{}, errors.New("decode cursor: incomplete or invalid fields")
	}
	if _, err := hex.DecodeString(*wire.AfterEventID); err != nil {
		return conversationEntryCursor{}, errors.New("decode cursor: invalid event ID")
	}
	return conversationEntryCursor{AfterEventID: *wire.AfterEventID}, nil
}

type conversationEntryCandidate struct {
	kind      domain.ConversationEntryKind
	recordID  string
	eventID   string
	createdAt int64
}

func (s *SQLite) ListConversationEntries(ctx context.Context, filter model.ConversationHistoryFilter) (domain.ConversationEntryPage, error) {
	if !filter.Key.Valid() {
		return domain.ConversationEntryPage{}, errors.New("list conversation entries: invalid conversation key")
	}
	messageWhere := []string{`((sender_mailbox_id=? AND recipient_mailbox_id=?) OR (sender_mailbox_id=? AND recipient_mailbox_id=?))`}
	messageArgs := []any{filter.Key.CounterpartyMailboxID, model.HumanMailboxID, model.HumanMailboxID, filter.Key.CounterpartyMailboxID}
	if filter.Key.HarnessSessionID != "" {
		messageWhere = append(messageWhere, `harness_provider=?`, `harness_session_id=?`)
		messageArgs = append(messageArgs, filter.Key.HarnessProvider, filter.Key.HarnessSessionID)
	} else {
		messageWhere = append(messageWhere, `harness_session_id=''`, `thread_event_id=?`)
		messageArgs = append(messageArgs, filter.Key.ThreadID)
	}
	union := `SELECT 'message' AS entry_kind,id AS record_id,event_id,created_at FROM messages WHERE ` + strings.Join(messageWhere, " AND ")
	args := append([]any(nil), messageArgs...)
	if filter.Key.HarnessSessionID != "" {
		union += ` UNION ALL SELECT 'activity',event_id,event_id,occurred_at FROM harness_activities WHERE mailbox_id=? AND harness=? AND session_id=?`
		args = append(args, filter.Key.CounterpartyMailboxID, filter.Key.HarnessProvider, filter.Key.HarnessSessionID)
	}
	rows, err := s.db.QueryContext(ctx, union, args...)
	if err != nil {
		return domain.ConversationEntryPage{}, fmt.Errorf("list conversation entries: %w", err)
	}
	var candidates []conversationEntryCandidate
	for rows.Next() {
		var candidate conversationEntryCandidate
		if err := rows.Scan(&candidate.kind, &candidate.recordID, &candidate.eventID, &candidate.createdAt); err != nil {
			rows.Close()
			return domain.ConversationEntryPage{}, err
		}
		candidates = append(candidates, candidate)
	}
	if err := rows.Close(); err != nil {
		return domain.ConversationEntryPage{}, err
	}
	candidates, err = s.orderConversationCandidates(ctx, candidates)
	if err != nil {
		return domain.ConversationEntryPage{}, err
	}

	start := 0
	if filter.Cursor != "" {
		cursor, err := decodeConversationEntryCursor(filter.Cursor)
		if err != nil {
			return domain.ConversationEntryPage{}, fmt.Errorf("list conversation entries: %w", err)
		}
		start = -1
		for index, candidate := range candidates {
			if candidate.eventID == cursor.AfterEventID {
				start = index + 1
				break
			}
		}
		if start < 0 {
			return domain.ConversationEntryPage{}, errors.New("list conversation entries: cursor is not in this conversation")
		}
	}
	limit := pageLimit(filter.Limit)
	end := min(start+limit, len(candidates))
	page := domain.ConversationEntryPage{}
	if end < len(candidates) {
		page.NextCursor, err = encodePageCursor(conversationEntryCursor{AfterEventID: candidates[end-1].eventID})
		if err != nil {
			return domain.ConversationEntryPage{}, fmt.Errorf("list conversation entries: %w", err)
		}
	}
	page.Entries = make([]domain.ConversationEntry, 0, end-start)
	for index := start; index < end; index++ {
		candidate := candidates[index]
		entry := domain.ConversationEntry{Kind: candidate.kind, EventID: candidate.eventID, DisplayOrder: index}
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
			activity.DisplayOrder = index
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

func (s *SQLite) orderConversationCandidates(ctx context.Context, candidates []conversationEntryCandidate) ([]conversationEntryCandidate, error) {
	byID := make(map[string]conversationEntryCandidate, len(candidates))
	children := make(map[string][]string)
	indegree := make(map[string]int, len(candidates))
	for _, candidate := range candidates {
		byID[candidate.eventID] = candidate
		indegree[candidate.eventID] = 0
	}
	for start := 0; start < len(candidates); start += 500 {
		end := min(start+500, len(candidates))
		placeholders := strings.TrimRight(strings.Repeat("?,", end-start), ",")
		args := make([]any, 0, end-start)
		for _, candidate := range candidates[start:end] {
			args = append(args, candidate.eventID)
		}
		rows, err := s.db.QueryContext(ctx, `SELECT child_event_id,parent_event_id FROM causal_edges WHERE child_event_id IN (`+placeholders+`)`, args...)
		if err != nil {
			return nil, fmt.Errorf("load conversation causal edges: %w", err)
		}
		for rows.Next() {
			var child, parent string
			if err := rows.Scan(&child, &parent); err != nil {
				rows.Close()
				return nil, err
			}
			if _, exists := byID[parent]; exists {
				children[parent] = append(children[parent], child)
				indegree[child]++
			}
		}
		if err := rows.Close(); err != nil {
			return nil, err
		}
	}
	less := func(a, b conversationEntryCandidate) bool {
		if a.createdAt == b.createdAt {
			return a.eventID < b.eventID
		}
		return a.createdAt < b.createdAt
	}
	var ready []conversationEntryCandidate
	for id, degree := range indegree {
		if degree == 0 {
			ready = append(ready, byID[id])
		}
	}
	var ordered []conversationEntryCandidate
	for len(ready) != 0 {
		sort.Slice(ready, func(i, j int) bool { return less(ready[i], ready[j]) })
		current := ready[0]
		ready = ready[1:]
		ordered = append(ordered, current)
		for _, child := range children[current.eventID] {
			indegree[child]--
			if indegree[child] == 0 {
				ready = append(ready, byID[child])
			}
		}
	}
	if len(ordered) != len(candidates) {
		return nil, errors.New("conversation entries contain a causal cycle")
	}
	return ordered, nil
}
