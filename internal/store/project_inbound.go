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

// acceptInboundProjectMessagesTx gives causally usable account traffic the
// same home-issued sequence it would receive through local Create.
func (s *SQLite) acceptInboundProjectMessagesTx(ctx context.Context, tx *sql.Tx) ([]string, error) {
	return s.acceptProjectMessagesTx(ctx, tx, `m.sender_installation_id<>p.home_installation_id`)
}

// repairLocalProjectReplies accepts replies written by versions that routed a
// local reply to a project mailbox through the generic answer path. It runs
// outside canonical ingestion so normal Create/CreateProjectMessage cannot
// race their own transactional acceptance.
func (s *SQLite) repairLocalProjectReplies(ctx context.Context) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	accepted, err := s.acceptProjectMessagesTx(ctx, tx, `m.sender_installation_id=p.home_installation_id AND m.event_type='answer' AND m.reply_to IS NOT NULL`)
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

func (s *SQLite) acceptProjectMessagesTx(ctx context.Context, tx *sql.Tx, sourceCondition string) ([]string, error) {
	query := `SELECT p.id,m.id,m.event_id FROM messages m JOIN projects p ON p.mailbox_id=m.recipient_mailbox_id LEFT JOIN project_message_acceptances a ON a.message_id=m.id WHERE a.message_id IS NULL AND m.sender_mailbox_id=? AND m.purpose IN (?,?) AND ` + sourceCondition + ` ORDER BY m.created_at,m.event_id`
	rows, err := tx.QueryContext(ctx, query, model.HumanMailboxID, model.MessagePurposeProjectInput, model.MessagePurposeConversation)
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
		if _, err := s.ingestCanonicalTx(ctx, tx, []event.SignedEvent{acceptance}, true); err != nil {
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
	_, err = s.ingestCanonicalTx(ctx, tx, signed, true)
	return err
}
