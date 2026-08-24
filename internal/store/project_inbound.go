package store

import (
	"context"
	"database/sql"
	"fmt"
	"sort"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/projectstate"
)

// reconcileProjectInputs commits the project-input invariant for projected
// messages that may predate or have bypassed the current ingress transaction.
func (s *SQLite) reconcileProjectInputs(ctx context.Context) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	accepted, err := s.reconcileProjectInputsTx(ctx, tx)
	if err != nil {
		return err
	}
	var change domain.Invalidation
	if len(accepted) != 0 {
		change, err = recordChangeTx(ctx, tx, append(append([]domain.ChangeTopic(nil), canonicalChangeTopics...), domain.TopicProjects))
		if err != nil {
			return err
		}
	}
	if err := tx.Commit(); err != nil {
		return err
	}
	s.notifyChange(change)
	return nil
}

// reconcileProjectInputsTx is the single commit boundary for accepting human
// conversation into authoritative project history. It is intentionally
// source-agnostic: local create/reply, remote append, and replay converge here.
func (s *SQLite) reconcileProjectInputsTx(ctx context.Context, tx *sql.Tx) ([]string, error) {
	rows, err := tx.QueryContext(ctx, `SELECT p.id,m.id,m.event_id FROM messages m JOIN projects p ON p.mailbox_id=m.recipient_mailbox_id LEFT JOIN project_message_acceptances a ON a.message_id=m.id WHERE a.message_id IS NULL AND m.sender_mailbox_id=? AND m.purpose IN (?,?) ORDER BY p.id,m.created_at,m.event_id`, model.HumanMailboxID, model.MessagePurposeProjectInput, model.MessagePurposeConversation)
	if err != nil {
		return nil, err
	}
	type pending struct{ projectID, messageID, eventID string }
	var messages []pending
	for rows.Next() {
		var item pending
		if err := rows.Scan(&item.projectID, &item.messageID, &item.eventID); err != nil {
			rows.Close()
			return nil, err
		}
		messages = append(messages, item)
	}
	if err := rows.Close(); err != nil {
		return nil, err
	}
	var accepted []string
	for _, message := range messages {
		var head string
		if err := tx.QueryRowContext(ctx, `SELECT head_event_id FROM projects WHERE id=?`, message.projectID).Scan(&head); err != nil {
			return nil, err
		}
		var sequence int64
		if err := tx.QueryRowContext(ctx, `SELECT COALESCE(MAX(sequence),0)+1 FROM project_message_acceptances WHERE project_id=?`, message.projectID).Scan(&sequence); err != nil {
			return nil, err
		}
		now := s.now().UTC()
		acceptance, _, err := s.signProjectEventTx(ctx, tx, message.projectID, head, projectstate.MessageAccepted{MessageID: message.messageID, MessageEventID: message.eventID, Sequence: sequence}, now)
		if err != nil {
			return nil, err
		}
		if _, err := s.ingestCanonicalProjectionTx(ctx, tx, []event.SignedEvent{acceptance}, true); err != nil {
			return nil, err
		}
		if err := s.appendProjectPendingNoticeTx(ctx, tx, message.projectID, message.messageID, acceptance.ID(), now); err != nil {
			return nil, err
		}
		accepted = append(accepted, acceptance.ID())
	}
	return accepted, nil
}

func (s *SQLite) appendProjectPendingNoticeTx(ctx context.Context, tx *sql.Tx, projectID, messageID, parent string, created time.Time) error {
	var lifecycle, name, mailboxID string
	var archived bool
	if err := tx.QueryRowContext(ctx, `SELECT lifecycle,name,mailbox_id,archived FROM projects WHERE id=?`, projectID).Scan(&lifecycle, &name, &mailboxID, &archived); err != nil {
		return err
	}
	if lifecycle != string(domain.ProjectClosed) && !archived {
		return nil
	}
	accountID, parents, err := projectAccountRouteTx(ctx, tx, s.signer.InstallationID)
	if err != nil {
		return err
	}
	parents = append(parents, parent)
	sort.Strings(parents)
	noticeID, err := uuid.NewV7()
	if err != nil {
		return err
	}
	payload, _ := event.MarshalPayload(event.TextPayload{MessageID: noticeID.String(), Body: "New activity is waiting for project " + name, Details: fmt.Sprintf("Kind: notice\nProject: %s\nPending message: %s\nLifecycle: %s\nArchived: %t", projectID, messageID, lifecycle, archived), Purpose: model.MessagePurposeSystemNotice, ActorLabel: "HQ · " + name})
	content := event.Content{Type: event.TypeQuestion, Sender: s.localAddress(mailboxID), Audience: &event.Audience{HumanAccountID: accountID}, Parents: parents, Scope: event.ScopeAccountAddressed, Payload: payload}
	signed, err := s.signContents(ctx, []event.Content{content}, []time.Time{created})
	if err != nil {
		return err
	}
	_, err = s.ingestCanonicalProjectionTx(ctx, tx, signed, true)
	return err
}
