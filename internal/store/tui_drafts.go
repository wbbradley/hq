package store

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"slices"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/model"
)

func validateTUIDraft(draft domain.TUIDraft) error {
	if _, err := uuid.Parse(draft.ID); err != nil {
		return errors.New("TUI draft ID must be a UUID")
	}
	if len([]byte(draft.Body)) > event.MaxBodyBytes {
		return fmt.Errorf("TUI draft body exceeds %d bytes", event.MaxBodyBytes)
	}
	if draft.ReplyToMessageID == "" && draft.RecipientMailboxID == "" {
		return errors.New("TUI draft requires a reply target or recipient")
	}
	return nil
}

func encodeTUIDraft(draft domain.TUIDraft) (conversation, address, repository, activation string, err error) {
	values := []struct {
		value any
		out   *string
	}{{draft.Conversation, &conversation}, {draft.RecipientAddress, &address}, {draft.Repository, &repository}}
	for _, value := range values {
		raw, marshalErr := json.Marshal(value.value)
		if marshalErr != nil {
			return "", "", "", "", marshalErr
		}
		*value.out = string(raw)
	}
	if draft.Activation != nil {
		raw, marshalErr := json.Marshal(draft.Activation)
		if marshalErr != nil {
			return "", "", "", "", marshalErr
		}
		activation = string(raw)
	}
	return conversation, address, repository, activation, nil
}

type tuiDraftScanner interface{ Scan(...any) error }

type canonicalTransactionContextKey struct{}

type canonicalTransactionContext struct {
	tx     *sql.Tx
	topics map[domain.ChangeTopic]bool
}

func (c *canonicalTransactionContext) addTopics(topics ...domain.ChangeTopic) {
	if c.topics == nil {
		c.topics = make(map[domain.ChangeTopic]bool)
	}
	for _, topic := range topics {
		c.topics[topic] = true
	}
}

func (c *canonicalTransactionContext) sortedTopics() []domain.ChangeTopic {
	topics := make([]domain.ChangeTopic, 0, len(c.topics))
	for topic := range c.topics {
		topics = append(topics, topic)
	}
	slices.Sort(topics)
	return topics
}

func scanTUIDraft(scanner tuiDraftScanner) (domain.TUIDraft, error) {
	var draft domain.TUIDraft
	var conversation, address, repository, activation string
	var createdAt, updatedAt int64
	if err := scanner.Scan(&draft.ID, &draft.Version, &draft.Body, &draft.ReplyToMessageID, &conversation, &draft.RecipientMailboxID, &draft.RecipientLabel, &address, &draft.RecipientNamed, &repository, &activation, &createdAt, &updatedAt); err != nil {
		return draft, err
	}
	if err := json.Unmarshal([]byte(conversation), &draft.Conversation); err != nil {
		return draft, err
	}
	if err := json.Unmarshal([]byte(address), &draft.RecipientAddress); err != nil {
		return draft, err
	}
	if err := json.Unmarshal([]byte(repository), &draft.Repository); err != nil {
		return draft, err
	}
	if activation != "" {
		draft.Activation = new(domain.ProjectActivationIntent)
		if err := json.Unmarshal([]byte(activation), draft.Activation); err != nil {
			return draft, err
		}
	}
	draft.CreatedAt, draft.UpdatedAt = time.UnixMilli(createdAt).UTC(), time.UnixMilli(updatedAt).UTC()
	return draft, nil
}

const tuiDraftColumns = `id,version,body,reply_to_message_id,conversation_json,recipient_mailbox_id,recipient_label,recipient_address_json,recipient_named,repository_json,activation_json,created_at,updated_at`

func (s *SQLite) ListTUIDrafts(ctx context.Context) ([]domain.TUIDraft, error) {
	rows, err := s.db.QueryContext(ctx, `SELECT `+tuiDraftColumns+` FROM tui_drafts ORDER BY updated_at DESC,id`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var drafts []domain.TUIDraft
	for rows.Next() {
		draft, err := scanTUIDraft(rows)
		if err != nil {
			return nil, err
		}
		drafts = append(drafts, draft)
	}
	return drafts, rows.Err()
}

func (s *SQLite) PutTUIDraft(ctx context.Context, draft domain.TUIDraft) (domain.TUIDraft, error) {
	if err := validateTUIDraft(draft); err != nil {
		return domain.TUIDraft{}, err
	}
	conversation, address, repository, activation, err := encodeTUIDraft(draft)
	if err != nil {
		return domain.TUIDraft{}, err
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return domain.TUIDraft{}, err
	}
	defer tx.Rollback()
	now := s.now().UTC().UnixMilli()
	if draft.Version == 0 {
		_, err = tx.ExecContext(ctx, `INSERT INTO tui_drafts(`+tuiDraftColumns+`) VALUES (?,1,?,?,?,?,?,?,?,?,?,?,?)`, draft.ID, draft.Body, draft.ReplyToMessageID, conversation, draft.RecipientMailboxID, draft.RecipientLabel, address, boolInt(draft.RecipientNamed), repository, activation, now, now)
	} else {
		var result sql.Result
		result, err = tx.ExecContext(ctx, `UPDATE tui_drafts SET version=version+1,body=?,reply_to_message_id=?,conversation_json=?,recipient_mailbox_id=?,recipient_label=?,recipient_address_json=?,recipient_named=?,repository_json=?,activation_json=?,updated_at=? WHERE id=? AND version=?`, draft.Body, draft.ReplyToMessageID, conversation, draft.RecipientMailboxID, draft.RecipientLabel, address, boolInt(draft.RecipientNamed), repository, activation, now, draft.ID, draft.Version)
		if err == nil {
			changed, _ := result.RowsAffected()
			if changed == 0 {
				return domain.TUIDraft{}, s.tuiDraftVersionErrorTx(ctx, tx, draft.ID)
			}
		}
	}
	if err != nil {
		if strings.Contains(strings.ToLower(err.Error()), "unique") {
			return domain.TUIDraft{}, domain.ErrTUIDraftConflict
		}
		return domain.TUIDraft{}, err
	}
	stored, err := scanTUIDraft(tx.QueryRowContext(ctx, `SELECT `+tuiDraftColumns+` FROM tui_drafts WHERE id=?`, draft.ID))
	if err != nil {
		return domain.TUIDraft{}, err
	}
	if err := recordMutationTx(ctx, tx, stored); err != nil {
		return domain.TUIDraft{}, err
	}
	change, err := recordChangeTx(ctx, tx, []domain.ChangeTopic{domain.TopicTUIDrafts})
	if err != nil {
		return domain.TUIDraft{}, err
	}
	if err := tx.Commit(); err != nil {
		return domain.TUIDraft{}, err
	}
	s.notifyChange(change)
	return stored, nil
}

func (s *SQLite) DeleteTUIDraft(ctx context.Context, id string, version uint64) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	result, err := tx.ExecContext(ctx, `DELETE FROM tui_drafts WHERE id=? AND version=?`, id, version)
	if err != nil {
		return err
	}
	changed, _ := result.RowsAffected()
	if changed == 0 {
		return s.tuiDraftVersionErrorTx(ctx, tx, id)
	}
	if err := recordMutationTx(ctx, tx, nil); err != nil {
		return err
	}
	change, err := recordChangeTx(ctx, tx, []domain.ChangeTopic{domain.TopicTUIDrafts})
	if err != nil {
		return err
	}
	if err := tx.Commit(); err != nil {
		return err
	}
	s.notifyChange(change)
	return nil
}

func (s *SQLite) SubmitTUIDraft(ctx context.Context, id string, version uint64) (domain.TUIDraftSubmission, error) {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return domain.TUIDraftSubmission{}, err
	}
	defer tx.Rollback()
	draft, err := scanTUIDraft(tx.QueryRowContext(ctx, `SELECT `+tuiDraftColumns+` FROM tui_drafts WHERE id=?`, id))
	if errors.Is(err, sql.ErrNoRows) {
		return domain.TUIDraftSubmission{}, domain.ErrTUIDraftNotFound
	}
	if err != nil {
		return domain.TUIDraftSubmission{}, err
	}
	if draft.Version != version {
		return domain.TUIDraftSubmission{}, domain.ErrTUIDraftConflict
	}
	if strings.TrimSpace(draft.Body) == "" {
		return domain.TUIDraftSubmission{}, errors.New("cannot submit an empty TUI draft")
	}
	message := draftMessage(draft)
	if draft.RecipientNamed {
		var mailboxID, harness, sessionID string
		err := tx.QueryRowContext(ctx, `SELECT mailbox_id,current_harness,current_session_id FROM named_agents WHERE name=? AND retired=0`, draft.RecipientLabel).Scan(&mailboxID, &harness, &sessionID)
		if errors.Is(err, sql.ErrNoRows) || err == nil && mailboxID != draft.RecipientMailboxID {
			return domain.TUIDraftSubmission{}, fmt.Errorf("%w: named recipient %s", domain.ErrTUIDraftTarget, draft.RecipientLabel)
		}
		if err != nil {
			return domain.TUIDraftSubmission{}, err
		}
		if harness != "" && sessionID != "" {
			message.Correlation = model.MessageCorrelation{Provider: harness, SessionID: sessionID}
		}
	}
	outer := &canonicalTransactionContext{tx: tx}
	txCtx := context.WithValue(ctx, canonicalTransactionContextKey{}, outer)
	if draft.ReplyToMessageID != "" {
		original, err := s.Get(txCtx, draft.ReplyToMessageID)
		if errors.Is(err, domain.ErrNotFound) {
			return domain.TUIDraftSubmission{}, fmt.Errorf("%w: reply message %s", domain.ErrTUIDraftTarget, draft.ReplyToMessageID)
		}
		if err != nil {
			return domain.TUIDraftSubmission{}, err
		}
		message.RecipientMailboxID = original.SenderMailboxID
		message.RecipientLabel = original.SenderLabel
		message.Context = original.Context
		replyTo := draft.ReplyToMessageID
		message.ReplyTo = &replyTo
		message.Correlation = original.Correlation
		if original.Purpose == model.MessagePurposeProtocolQuestion {
			message.Purpose = model.MessagePurposeProtocolAnswer
		} else if original.SenderAddress.Kind == model.MailboxProject {
			message.Purpose = model.MessagePurposeProjectInput
		}
		err = s.Reply(txCtx, draft.ReplyToMessageID, message)
		if err != nil {
			if errors.Is(err, domain.ErrNotFound) || errors.Is(err, ErrAlreadyHandled) {
				return domain.TUIDraftSubmission{}, fmt.Errorf("%w: reply message %s: %v", domain.ErrTUIDraftTarget, draft.ReplyToMessageID, err)
			}
			return domain.TUIDraftSubmission{}, err
		}
	} else {
		if draft.RecipientAddress.Kind == model.MailboxProject || draft.Activation != nil {
			message.Purpose = model.MessagePurposeProjectInput
		}
		if err := s.Create(txCtx, message); err != nil {
			return domain.TUIDraftSubmission{}, err
		}
	}
	result, err := tx.ExecContext(ctx, `DELETE FROM tui_drafts WHERE id=? AND version=?`, id, version)
	if err != nil {
		return domain.TUIDraftSubmission{}, err
	}
	if changed, _ := result.RowsAffected(); changed != 1 {
		return domain.TUIDraftSubmission{}, domain.ErrTUIDraftConflict
	}
	submission := domain.TUIDraftSubmission{MessageID: draft.ID, Activation: draft.Activation}
	if err := recordMutationTx(ctx, tx, submission); err != nil {
		return domain.TUIDraftSubmission{}, err
	}
	outer.addTopics(domain.TopicTUIDrafts)
	change, err := recordChangeTx(ctx, tx, outer.sortedTopics())
	if err != nil {
		return domain.TUIDraftSubmission{}, err
	}
	if err := tx.Commit(); err != nil {
		return domain.TUIDraftSubmission{}, err
	}
	s.notifyChange(change)
	return submission, nil
}

func (s *SQLite) tuiDraftVersionErrorTx(ctx context.Context, tx *sql.Tx, id string) error {
	var exists int
	if err := tx.QueryRowContext(ctx, `SELECT EXISTS(SELECT 1 FROM tui_drafts WHERE id=?)`, id).Scan(&exists); err != nil {
		return err
	}
	if exists == 0 {
		return domain.ErrTUIDraftNotFound
	}
	return domain.ErrTUIDraftConflict
}

func draftMessage(draft domain.TUIDraft) model.Message {
	return model.Message{
		ID: draft.ID, SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: draft.RecipientMailboxID,
		SenderLabel: "human", RecipientLabel: draft.RecipientLabel, RecipientAddress: draft.RecipientAddress, Body: strings.TrimSpace(draft.Body),
		Context: draft.Repository, CreatedAt: draft.UpdatedAt,
	}
}
