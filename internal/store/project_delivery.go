package store

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"reflect"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/projectstate"
)

const projectDispatchLease = 30 * time.Second

func (s *SQLite) CreateProjectOutput(ctx context.Context, binding domain.ProjectOutputBinding, message model.Message) error {
	if binding.ProjectID == "" || binding.AssignmentID == "" || binding.AgentName == "" || binding.ProjectThreadID == "" || binding.ExternalThreadID == "" {
		return errors.New("project output provenance is incomplete")
	}
	if message.ID == "" || message.SenderMailboxID == "" || message.RecipientMailboxID != model.HumanMailboxID {
		return errors.New("project output message is invalid")
	}
	message.Purpose = model.MessagePurposeProjectOutput
	if err := s.reconcileExistingProjectOutput(ctx, binding, message); err == nil {
		return nil
	} else if !errors.Is(err, domain.ErrNotFound) {
		return err
	}
	account, parents, _, err := s.localAccountAction(ctx, "")
	if err != nil {
		return err
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	var mailboxID, projectName, historicalAgent, externalThread string
	var ended sql.NullString
	var forced bool
	err = tx.QueryRowContext(ctx, `SELECT p.mailbox_id,p.name,e.agent_name,t.external_thread_id,e.ended_event_id,e.forced FROM projects p JOIN project_assignment_epochs e ON e.project_id=p.id JOIN project_threads t ON t.id=? AND t.project_id=p.id AND t.agent_name=e.agent_name WHERE p.id=? AND e.id=?`, binding.ProjectThreadID, binding.ProjectID, binding.AssignmentID).Scan(&mailboxID, &projectName, &historicalAgent, &externalThread, &ended, &forced)
	if errors.Is(err, sql.ErrNoRows) {
		return domain.ErrProjectThreadMismatch
	}
	if err != nil {
		return err
	}
	if mailboxID != message.SenderMailboxID || historicalAgent != binding.AgentName || externalThread != binding.ExternalThreadID {
		return domain.ErrProjectThreadMismatch
	}
	var currentAssignment, currentAgent, currentThread string
	currentErr := tx.QueryRowContext(ctx, `SELECT id,agent_name,COALESCE(selected_thread_id,'') FROM project_assignment_epochs WHERE project_id=? AND ended_event_id IS NULL`, binding.ProjectID).Scan(&currentAssignment, &currentAgent, &currentThread)
	if currentErr != nil && !errors.Is(currentErr, sql.ErrNoRows) {
		return currentErr
	}
	late := ended.Valid || currentAssignment != binding.AssignmentID || currentAgent != binding.AgentName || currentThread != binding.ProjectThreadID
	actorLabel := binding.AgentName + " · " + projectName
	provenance := projectOutputProvenanceSection(binding.ProjectID, binding.AssignmentID, binding.ProjectThreadID, late, currentAssignment, currentAgent, currentThread)
	if late {
		actorLabel += " (late from inactive assignment)"
	}
	message.TechnicalSections = append(append([]model.TechnicalSection(nil), message.TechnicalSections...), provenance)
	payload, _ := event.MarshalPayload(textPayloadForMessage(message, actorLabel))
	content := event.Content{
		Schema: event.MessageSchemaVersion, Type: event.TypeQuestion, Sender: s.localAddress(mailboxID),
		Audience: &event.Audience{HumanAccountID: account.ID}, Parents: parents, Authorities: uniqueSorted(parents),
		Scope: event.ScopeAccountAddressed, Payload: payload,
	}
	signed, err := s.signContents(ctx, []event.Content{content}, []time.Time{message.CreatedAt})
	if err != nil {
		return err
	}
	if _, err := s.ingestCanonicalTx(ctx, tx, signed, true); err != nil {
		return err
	}
	now := s.now().UTC().UnixMilli()
	if _, err := tx.ExecContext(ctx, `INSERT INTO project_output_provenance(message_id,project_id,assignment_id,agent_name,project_thread_id,external_thread_id,late,current_assignment_id,current_agent_name,current_project_thread_id,runtime_owner_token,observed_runtime_state,forced_transition,recorded_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)`, message.ID, binding.ProjectID, binding.AssignmentID, binding.AgentName, binding.ProjectThreadID, binding.ExternalThreadID, boolInt(late), currentAssignment, currentAgent, currentThread, binding.RuntimeOwner, binding.RuntimeState, boolInt(forced), now); err != nil {
		return err
	}
	change, err := recordChangeTx(ctx, tx, canonicalChangeTopics)
	if err != nil {
		return err
	}
	if err := tx.Commit(); err != nil {
		return err
	}
	s.notifyChange(change)
	return nil
}

func (s *SQLite) reconcileExistingProjectOutput(ctx context.Context, binding domain.ProjectOutputBinding, message model.Message) error {
	existing, err := s.Get(ctx, message.ID)
	if err != nil {
		return err
	}
	var projectID, assignmentID, agentName, projectThreadID, externalThreadID, currentAssignment, currentAgent, currentThread string
	var late bool
	err = s.db.QueryRowContext(ctx, `SELECT project_id,assignment_id,agent_name,project_thread_id,external_thread_id,late,current_assignment_id,current_agent_name,current_project_thread_id FROM project_output_provenance WHERE message_id=?`, message.ID).Scan(
		&projectID, &assignmentID, &agentName, &projectThreadID, &externalThreadID, &late, &currentAssignment, &currentAgent, &currentThread,
	)
	if errors.Is(err, sql.ErrNoRows) {
		return fmt.Errorf("project output message ID %s collides with different HQ content", message.ID)
	}
	if err != nil {
		return err
	}
	if projectID != binding.ProjectID || assignmentID != binding.AssignmentID || agentName != binding.AgentName || projectThreadID != binding.ProjectThreadID || externalThreadID != binding.ExternalThreadID {
		return fmt.Errorf("project output message ID %s collides with different project provenance", message.ID)
	}
	message.TechnicalSections = append(append([]model.TechnicalSection(nil), message.TechnicalSections...), projectOutputProvenanceSection(projectID, assignmentID, projectThreadID, late, currentAssignment, currentAgent, currentThread))
	if existing.SenderMailboxID != message.SenderMailboxID || existing.RecipientMailboxID != message.RecipientMailboxID || existing.Purpose != message.Purpose || existing.Body != message.Body || existing.Details != message.Details || existing.Presentation != message.Presentation || existing.Correlation != message.Correlation || existing.Context != message.Context || !reflect.DeepEqual(existing.TechnicalSections, message.TechnicalSections) {
		return fmt.Errorf("project output message ID %s collides with different HQ content", message.ID)
	}
	return nil
}

func projectOutputProvenanceSection(projectID, assignmentID, projectThreadID string, late bool, currentAssignment, currentAgent, currentThread string) model.TechnicalSection {
	section := model.TechnicalSection{Namespace: "hq.project.output_provenance", Fields: []model.TechnicalField{
		{Key: "project_id", Label: "Project", Value: projectID},
		{Key: "assignment_id", Label: "Project assignment", Value: assignmentID},
		{Key: "project_thread_id", Label: "Project thread", Value: projectThreadID},
	}}
	if late {
		section.Fields = append(section.Fields,
			model.TechnicalField{Key: "late", Label: "Late from inactive assignment", Value: "yes"},
			model.TechnicalField{Key: "current_assignment_id", Label: "Current assignment", Value: valueOrNone(currentAssignment)},
			model.TechnicalField{Key: "current_agent", Label: "Current agent", Value: valueOrNone(currentAgent)},
			model.TechnicalField{Key: "current_project_thread_id", Label: "Current project thread", Value: valueOrNone(currentThread)},
		)
	}
	return section
}

func valueOrNone(value string) string {
	if value == "" {
		return "(none)"
	}
	return value
}

func (s *SQLite) ClaimProjectMessage(ctx context.Context, projectID, assignmentID, projectThreadID, token string) (domain.ProjectDelivery, error) {
	now := s.now().UTC()
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return domain.ProjectDelivery{}, err
	}
	defer tx.Rollback()
	delivery := domain.ProjectDelivery{ProjectID: projectID, AssignmentID: assignmentID, ProjectThreadID: projectThreadID}
	err = tx.QueryRowContext(ctx, `SELECT e.agent_name,t.external_thread_id FROM projects p JOIN project_assignment_epochs e ON e.project_id=p.id JOIN project_threads t ON t.id=e.selected_thread_id WHERE p.id=? AND p.lifecycle='open' AND e.id=? AND e.state='runnable' AND e.ended_event_id IS NULL AND t.id=? AND t.project_id=p.id AND t.agent_name=e.agent_name`, projectID, assignmentID, projectThreadID).Scan(&delivery.AgentName, &delivery.ExternalThreadID)
	if errors.Is(err, sql.ErrNoRows) {
		return delivery, domain.ErrNotReady
	}
	if err != nil {
		return delivery, err
	}
	err = tx.QueryRowContext(ctx, `SELECT a.sequence,a.message_id,r.message_id IS NOT NULL FROM project_message_acceptances a JOIN delivery_facts d ON d.message_id=a.message_id LEFT JOIN project_dispatch_records r ON r.message_id=a.message_id WHERE a.project_id=? AND d.completed_at IS NULL ORDER BY a.sequence LIMIT 1`, projectID).Scan(&delivery.Sequence, &delivery.Message.ID, &delivery.Dispatched)
	if errors.Is(err, sql.ErrNoRows) {
		return delivery, domain.ErrNotReady
	}
	if err != nil {
		return delivery, err
	}
	if delivery.Dispatched {
		result, err := tx.ExecContext(ctx, `UPDATE delivery_facts SET delivery_token=?,delivery_lease_until=? WHERE message_id=? AND completed_at IS NULL AND (delivery_token IS NULL OR delivery_lease_until<?)`, token, now.Add(projectDispatchLease).UnixMilli(), delivery.Message.ID, now.UnixMilli())
		if err != nil {
			return delivery, err
		}
		if count, _ := result.RowsAffected(); count != 1 {
			return delivery, domain.ErrClaimed
		}
		delivery.Message, err = getMessageWith(ctx, tx, delivery.Message.ID)
		if err != nil {
			return delivery, err
		}
		if err := tx.Commit(); err != nil {
			return delivery, err
		}
		return delivery, nil
	}
	var priorAssignment, priorThread, state string
	var lease sql.NullInt64
	attemptErr := tx.QueryRowContext(ctx, `SELECT assignment_id,project_thread_id,state,lease_until FROM project_dispatch_attempts WHERE message_id=?`, delivery.Message.ID).Scan(&priorAssignment, &priorThread, &state, &lease)
	switch {
	case attemptErr == nil && (priorAssignment != assignmentID || priorThread != projectThreadID):
		return delivery, domain.ErrClaimed
	case attemptErr == nil && lease.Valid && lease.Int64 >= now.UnixMilli():
		return delivery, domain.ErrClaimed
	case attemptErr == nil:
		if _, err := tx.ExecContext(ctx, `UPDATE project_dispatch_attempts SET owner_token=?,lease_until=?,updated_at=? WHERE message_id=?`, token, now.Add(projectDispatchLease).UnixMilli(), now.UnixMilli(), delivery.Message.ID); err != nil {
			return delivery, err
		}
	case errors.Is(attemptErr, sql.ErrNoRows):
		if _, err := tx.ExecContext(ctx, `INSERT INTO project_dispatch_attempts(message_id,project_id,sequence,assignment_id,agent_name,project_thread_id,external_thread_id,owner_token,lease_until,state,started_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,'pending',?,?)`, delivery.Message.ID, projectID, delivery.Sequence, assignmentID, delivery.AgentName, projectThreadID, delivery.ExternalThreadID, token, now.Add(projectDispatchLease).UnixMilli(), now.UnixMilli(), now.UnixMilli()); err != nil {
			return delivery, err
		}
	default:
		return delivery, attemptErr
	}
	result, err := tx.ExecContext(ctx, `UPDATE delivery_facts SET delivery_token=?,delivery_lease_until=? WHERE message_id=? AND completed_at IS NULL AND (delivery_token IS NULL OR delivery_lease_until<?)`, token, now.Add(projectDispatchLease).UnixMilli(), delivery.Message.ID, now.UnixMilli())
	if err != nil {
		return delivery, err
	}
	if count, _ := result.RowsAffected(); count != 1 {
		return delivery, domain.ErrClaimed
	}
	delivery.Message, err = getMessageWith(ctx, tx, delivery.Message.ID)
	if err != nil {
		return delivery, err
	}
	change, err := recordChangeTx(ctx, tx, []domain.ChangeTopic{domain.TopicMessages, domain.TopicProjects})
	if err != nil {
		return delivery, err
	}
	if err := tx.Commit(); err != nil {
		return delivery, err
	}
	s.notifyChange(change)
	return delivery, nil
}

func (s *SQLite) MarkProjectDispatchUncertain(ctx context.Context, messageID, token string) error {
	now := s.now().UTC().UnixMilli()
	result, err := s.db.ExecContext(ctx, `UPDATE project_dispatch_attempts SET state='uncertain',updated_at=? WHERE message_id=? AND owner_token=? AND state='pending'`, now, messageID, token)
	if err != nil {
		return err
	}
	if count, _ := result.RowsAffected(); count != 1 {
		return domain.ErrNotReady
	}
	return nil
}

func (s *SQLite) RecordProjectDispatch(ctx context.Context, messageID, token string) error {
	now := s.now().UTC()
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	var projectID, assignmentID, agentName, projectThreadID, externalThreadID, state string
	var sequence int64
	err = tx.QueryRowContext(ctx, `SELECT project_id,sequence,assignment_id,agent_name,project_thread_id,external_thread_id,state FROM project_dispatch_attempts WHERE message_id=? AND owner_token=?`, messageID, token).Scan(&projectID, &sequence, &assignmentID, &agentName, &projectThreadID, &externalThreadID, &state)
	if errors.Is(err, sql.ErrNoRows) {
		var existing int
		if lookupErr := tx.QueryRowContext(ctx, `SELECT count(*) FROM project_dispatch_records WHERE message_id=?`, messageID).Scan(&existing); lookupErr == nil && existing == 1 {
			return nil
		}
		return domain.ErrNotReady
	}
	if err != nil {
		return err
	}
	if state != "uncertain" {
		return fmt.Errorf("record project dispatch: %w", domain.ErrNotReady)
	}
	var head string
	if err := tx.QueryRowContext(ctx, `SELECT head_event_id FROM projects WHERE id=?`, projectID).Scan(&head); err != nil {
		return err
	}
	dispatch, _, err := s.signProjectEventTx(ctx, tx, projectID, head, projectstate.MessageDispatched{MessageID: messageID, Sequence: sequence, AssignmentID: assignmentID, Agent: agentName, ProjectThreadID: projectThreadID, ExternalThreadID: externalThreadID}, now)
	if err != nil {
		return err
	}
	if _, err := s.ingestCanonicalTx(ctx, tx, []event.SignedEvent{dispatch}, true); err != nil {
		return err
	}
	if _, err := tx.ExecContext(ctx, `UPDATE project_dispatch_attempts SET state='accepted',owner_token=NULL,lease_until=NULL,updated_at=? WHERE message_id=?`, now.UnixMilli(), messageID); err != nil {
		return err
	}
	change, err := recordChangeTx(ctx, tx, []domain.ChangeTopic{domain.TopicMessages, domain.TopicProjects})
	if err != nil {
		return err
	}
	if err := tx.Commit(); err != nil {
		return err
	}
	s.notifyChange(change)
	return nil
}

func (s *SQLite) ReleaseProjectMessage(ctx context.Context, messageID, token string) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	var state string
	if err := tx.QueryRowContext(ctx, `SELECT state FROM project_dispatch_attempts WHERE message_id=? AND owner_token=?`, messageID, token).Scan(&state); errors.Is(err, sql.ErrNoRows) {
		return domain.ErrNotReady
	} else if err != nil {
		return err
	}
	if state == "pending" {
		if _, err := tx.ExecContext(ctx, `DELETE FROM project_dispatch_attempts WHERE message_id=? AND owner_token=?`, messageID, token); err != nil {
			return err
		}
	} else {
		if _, err := tx.ExecContext(ctx, `UPDATE project_dispatch_attempts SET owner_token=NULL,lease_until=NULL,updated_at=? WHERE message_id=? AND owner_token=?`, s.now().UTC().UnixMilli(), messageID, token); err != nil {
			return err
		}
	}
	if _, err := tx.ExecContext(ctx, `UPDATE delivery_facts SET delivery_token=NULL,delivery_lease_until=NULL WHERE message_id=? AND delivery_token=? AND completed_at IS NULL`, messageID, token); err != nil {
		return err
	}
	change, err := recordChangeTx(ctx, tx, []domain.ChangeTopic{domain.TopicMessages, domain.TopicProjects})
	if err != nil {
		return err
	}
	if err := tx.Commit(); err != nil {
		return err
	}
	s.notifyChange(change)
	return nil
}
