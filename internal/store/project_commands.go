package store

import (
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"errors"
	"slices"
	"sort"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/model"
)

type pendingHomeCommand struct {
	command  domain.ProjectCommand
	eventID  string
	issuer   string
	received bool
}

const projectHomeUnreachableAfter = 24 * time.Hour

func annotateProjectCommandReachability(command *domain.ProjectCommand, now time.Time) {
	if command != nil && command.Stage == domain.ProjectCommandQueued && command.Diagnostic == "" && now.Sub(command.CreatedAt) >= projectHomeUnreachableAfter {
		command.Diagnostic = "project home has not acknowledged this command for 24 hours; it may be offline or unreachable"
	}
}

func (s *SQLite) queueRemoteProjectCreation(ctx context.Context, request domain.CreateProjectRequest) (domain.Project, error) {
	if _, err := uuid.Parse(request.HomeInstallation); err != nil {
		return domain.Project{}, errors.New("project home installation must be a UUID")
	}
	if pending, _ := latestProjectCommand(ctx, s.db, request.ID); pending != nil && pending.Stage != domain.ProjectCommandCommitted && pending.Stage != domain.ProjectCommandRejected {
		return domain.Project{}, domain.ErrProjectCommandPending
	}
	operation, body, err := domain.EncodeProjectCommand(domain.ProjectCreateCommand(request))
	if err != nil {
		return domain.Project{}, err
	}
	commandID := ""
	if mutation, ok := domain.MutationFromContext(ctx); ok {
		commandID = mutation.ID
	}
	if commandID == "" {
		commandID = uuid.NewString()
	}
	account, parents, _, err := s.localAccountAction(ctx, "")
	if err != nil {
		return domain.Project{}, err
	}
	payload, _ := event.MarshalPayload(event.ProjectCommandPayload{CommandID: commandID, ProjectID: request.ID, Operation: string(operation), Body: body})
	created := s.now().UTC()
	content := event.Content{Type: event.TypeProjectCommand, Sender: s.localAddress(model.HumanMailboxID), Recipient: &event.MailboxAddress{InstallationID: request.HomeInstallation, MailboxID: model.HumanMailboxID}, Audience: &event.Audience{HumanAccountID: account.ID}, Parents: parents, Scope: event.ScopeAccountAddressed, Payload: payload}
	command := domain.ProjectCommand{ID: commandID, ProjectID: request.ID, HomeInstallation: request.HomeInstallation, Operation: operation, Body: body, Stage: domain.ProjectCommandQueued, CreatedAt: created, UpdatedAt: created}
	placeholder := domain.Project{ID: request.ID, HomeInstallation: request.HomeInstallation, Name: request.Name, Brief: request.Brief, Lifecycle: domain.ProjectPreparing, ReadOnlyReplica: true, PendingCommand: &command, LatestCommand: &command, CreatedAt: created, UpdatedAt: created}
	value, err := s.appendContentsResult(ctx, []event.Content{content}, []time.Time{created}, func(*sql.Tx) (any, error) { return placeholder, nil })
	if err != nil {
		return placeholder, err
	}
	return value.(domain.Project), nil
}

func (s *SQLite) QueueProjectWorktreeProvision(ctx context.Context, request domain.ProjectWorktreeRequest) (domain.Project, bool, error) {
	if request.HomeInstallation == "" || request.HomeInstallation == s.signer.InstallationID {
		return domain.Project{}, false, nil
	}
	if _, err := uuid.Parse(request.HomeInstallation); err != nil {
		return domain.Project{}, true, errors.New("project home installation must be a UUID")
	}
	if request.RequestID == "" {
		request.RequestID = uuid.NewString()
	}
	if request.ProjectID == "" {
		request.ProjectID = uuid.NewString()
	}
	if _, err := uuid.Parse(request.RequestID); err != nil {
		return domain.Project{}, true, errors.New("worktree provisioning request ID must be a UUID")
	}
	if _, err := uuid.Parse(request.ProjectID); err != nil {
		return domain.Project{}, true, errors.New("worktree project ID must be a UUID")
	}
	if pending, _ := latestProjectCommand(ctx, s.db, request.ProjectID); pending != nil && pending.Stage != domain.ProjectCommandCommitted && pending.Stage != domain.ProjectCommandRejected {
		return domain.Project{}, true, domain.ErrProjectCommandPending
	}
	operation, body, err := domain.EncodeProjectCommand(domain.ProjectProvisionWorktreeCommand(request))
	if err != nil {
		return domain.Project{}, true, err
	}
	account, parents, _, err := s.localAccountAction(ctx, "")
	if err != nil {
		return domain.Project{}, true, err
	}
	payload, _ := event.MarshalPayload(event.ProjectCommandPayload{CommandID: request.RequestID, ProjectID: request.ProjectID, Operation: string(operation), Body: body})
	created := s.now().UTC()
	content := event.Content{Type: event.TypeProjectCommand, Sender: s.localAddress(model.HumanMailboxID), Recipient: &event.MailboxAddress{InstallationID: request.HomeInstallation, MailboxID: model.HumanMailboxID}, Audience: &event.Audience{HumanAccountID: account.ID}, Parents: parents, Scope: event.ScopeAccountAddressed, Payload: payload}
	command := domain.ProjectCommand{ID: request.RequestID, ProjectID: request.ProjectID, HomeInstallation: request.HomeInstallation, Operation: operation, Body: body, Stage: domain.ProjectCommandQueued, CreatedAt: created, UpdatedAt: created}
	placeholder := domain.Project{ID: request.ProjectID, HomeInstallation: request.HomeInstallation, Name: request.Name, Brief: request.Brief, Lifecycle: domain.ProjectPreparing, ReadOnlyReplica: true, PendingCommand: &command, LatestCommand: &command, CreatedAt: created, UpdatedAt: created}
	value, err := s.appendContentsResult(ctx, []event.Content{content}, []time.Time{created}, func(*sql.Tx) (any, error) { return placeholder, nil })
	if err != nil {
		return placeholder, true, err
	}
	return value.(domain.Project), true, nil
}

func (s *SQLite) QueueProjectCommand(ctx context.Context, command domain.ProjectCommand) (domain.Project, error) {
	project, err := getProjectReplica(ctx, s.db, command.ProjectID)
	if err != nil {
		return project, err
	}
	if command.ExpectedHead == "" || command.ExpectedHead != project.HeadEventID {
		return project, &domain.StaleProjectHead{ProjectID: project.ID, Expected: command.ExpectedHead, Current: project.HeadEventID}
	}
	if pending, _ := latestProjectCommand(ctx, s.db, project.ID); pending != nil && pending.Stage != domain.ProjectCommandCommitted && pending.Stage != domain.ProjectCommandRejected {
		return project, domain.ErrProjectCommandPending
	}
	if command.ID == "" {
		command.ID = uuid.NewString()
	}
	if _, err := uuid.Parse(command.ID); err != nil {
		return project, errors.New("project command ID must be a UUID")
	}
	data, err := domain.DecodeProjectCommand(command.Operation, command.Body)
	if err != nil {
		return project, err
	}
	operation, body, err := domain.EncodeProjectCommand(data)
	if err != nil {
		return project, err
	}
	command.Operation, command.Body = operation, body
	account, membership, _, err := s.localAccountAction(ctx, "")
	if err != nil {
		return project, err
	}
	parents := append(append([]string(nil), membership...), command.ExpectedHead)
	sort.Strings(parents)
	payload, _ := event.MarshalPayload(event.ProjectCommandPayload{CommandID: command.ID, ProjectID: project.ID, ExpectedHead: command.ExpectedHead, Operation: string(command.Operation), Body: command.Body})
	created := s.now().UTC()
	content := event.Content{Type: event.TypeProjectCommand, Sender: s.localAddress(model.HumanMailboxID), Recipient: &event.MailboxAddress{InstallationID: project.HomeInstallation, MailboxID: model.HumanMailboxID}, Audience: &event.Audience{HumanAccountID: account.ID}, Parents: parents, Scope: event.ScopeAccountAddressed, Payload: payload}
	value, err := s.appendContentsResult(ctx, []event.Content{content}, []time.Time{created}, func(tx *sql.Tx) (any, error) {
		current, err := getProjectReplica(ctx, tx, project.ID)
		if err != nil {
			return nil, err
		}
		command.HomeInstallation, command.Stage, command.CreatedAt, command.UpdatedAt = project.HomeInstallation, domain.ProjectCommandQueued, created, created
		current.PendingCommand = &command
		current.LatestCommand = &command
		return current, nil
	})
	if err != nil {
		return project, err
	}
	return value.(domain.Project), nil
}

func (s *SQLite) queueReplicaCommand(ctx context.Context, projectID, expected string, data domain.ProjectCommandData) (domain.Project, bool, error) {
	project, err := getProjectReplica(ctx, s.db, projectID)
	if errors.Is(err, domain.ErrProjectNotFound) {
		return domain.Project{}, false, nil
	}
	if err != nil {
		return project, true, err
	}
	operation, body, err := domain.EncodeProjectCommand(data)
	if err != nil {
		return project, true, err
	}
	commandID := ""
	if mutation, ok := domain.MutationFromContext(ctx); ok {
		commandID = mutation.ID
	}
	queued, err := s.QueueProjectCommand(ctx, domain.ProjectCommand{ID: commandID, ProjectID: projectID, ExpectedHead: expected, Operation: operation, Body: body})
	return queued, true, err
}

func latestProjectCommand(ctx context.Context, q projectQueryer, projectID string) (*domain.ProjectCommand, error) {
	rows, err := q.QueryContext(ctx, `SELECT raw FROM canonical_events WHERE event_type IN (?,?) AND reduction_status=? ORDER BY created_at,event_id`, event.TypeProjectCommand, event.TypeProjectResult, event.StatusProjected)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	commands := make(map[string]*domain.ProjectCommand)
	results := make(map[string]event.ProjectCommandResultPayload)
	resultTimes := make(map[string]time.Time)
	var order []string
	for rows.Next() {
		var raw []byte
		if err := rows.Scan(&raw); err != nil {
			return nil, err
		}
		inspection := event.Inspect(raw)
		if inspection.Event.Content.Type == event.TypeProjectCommand {
			var payload event.ProjectCommandPayload
			if json.Unmarshal(inspection.Event.Content.Payload, &payload) != nil || payload.ProjectID != projectID {
				continue
			}
			command := &domain.ProjectCommand{ID: payload.CommandID, ProjectID: payload.ProjectID, HomeInstallation: inspection.Event.Content.Recipient.InstallationID, ExpectedHead: payload.ExpectedHead, Operation: domain.ProjectCommandOperation(payload.Operation), Body: payload.Body, Stage: domain.ProjectCommandQueued, CreatedAt: time.Unix(inspection.Event.Nostr.CreatedAt, 0).UTC(), UpdatedAt: time.Unix(inspection.Event.Nostr.CreatedAt, 0).UTC()}
			commands[command.ID], order = command, append(order, command.ID)
		} else {
			var payload event.ProjectCommandResultPayload
			if json.Unmarshal(inspection.Event.Content.Payload, &payload) != nil || payload.ProjectID != projectID {
				continue
			}
			existing := results[payload.CommandID]
			if existing.Stage == "" || existing.Stage == "received" || payload.Stage != "received" && existing.Stage == "received" {
				results[payload.CommandID], resultTimes[payload.CommandID] = payload, time.Unix(inspection.Event.Nostr.CreatedAt, 0).UTC()
			}
		}
	}
	for id, payload := range results {
		if command := commands[id]; command != nil {
			command.CurrentHead, command.Diagnostic, command.UpdatedAt = payload.CurrentHead, payload.Diagnostic, resultTimes[id]
			switch payload.Stage {
			case "received":
				command.Stage = domain.ProjectCommandReceived
			case "committed":
				command.Stage = domain.ProjectCommandCommitted
			case "rejected":
				command.Stage = domain.ProjectCommandRejected
			}
		}
	}
	if len(order) == 0 {
		return nil, nil
	}
	command := commands[order[len(order)-1]]
	annotateProjectCommandReachability(command, time.Now().UTC())
	return command, nil
}

func listPendingProjectCreations(ctx context.Context, q projectQueryer) ([]domain.Project, error) {
	rows, err := q.QueryContext(ctx, `SELECT raw FROM canonical_events WHERE event_type IN (?,?) AND reduction_status=? ORDER BY created_at,event_id`, event.TypeProjectCommand, event.TypeProjectResult, event.StatusProjected)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	projects := make(map[string]domain.Project)
	commands := make(map[string]string)
	type timedResult struct {
		payload event.ProjectCommandResultPayload
		at      time.Time
	}
	results := make(map[string][]timedResult)
	for rows.Next() {
		var raw []byte
		if err := rows.Scan(&raw); err != nil {
			return nil, err
		}
		item := event.Inspect(raw).Event
		if item.Content.Type == event.TypeProjectCommand {
			var payload event.ProjectCommandPayload
			if json.Unmarshal(item.Content.Payload, &payload) != nil || item.Content.Recipient == nil {
				continue
			}
			operation := domain.ProjectCommandOperation(payload.Operation)
			if !domain.ProjectCommandCreatesProject(operation) {
				continue
			}
			var name, brief string
			data, decodeErr := domain.DecodeProjectCommand(operation, payload.Body)
			if decodeErr != nil {
				continue
			}
			switch value := data.(type) {
			case *domain.ProjectCreateCommand:
				request := domain.CreateProjectRequest(*value)
				name, brief = request.Name, request.Brief
			case *domain.ProjectProvisionWorktreeCommand:
				request := domain.ProjectWorktreeRequest(*value)
				name, brief = request.Name, request.Brief
			default:
				continue
			}
			created := time.Unix(item.Nostr.CreatedAt, 0).UTC()
			command := &domain.ProjectCommand{ID: payload.CommandID, ProjectID: payload.ProjectID, HomeInstallation: item.Content.Recipient.InstallationID, Operation: operation, Body: payload.Body, Stage: domain.ProjectCommandQueued, CreatedAt: created, UpdatedAt: created}
			projects[payload.ProjectID] = domain.Project{ID: payload.ProjectID, HomeInstallation: item.Content.Recipient.InstallationID, Name: name, Brief: brief, Lifecycle: domain.ProjectPreparing, ReadOnlyReplica: true, PendingCommand: command, LatestCommand: command, CreatedAt: created, UpdatedAt: created}
			commands[payload.CommandID] = payload.ProjectID
		} else {
			var payload event.ProjectCommandResultPayload
			if json.Unmarshal(item.Content.Payload, &payload) != nil {
				continue
			}
			results[payload.CommandID] = append(results[payload.CommandID], timedResult{payload: payload, at: time.Unix(item.Nostr.CreatedAt, 0).UTC()})
		}
	}
	for commandID, items := range results {
		projectID := commands[commandID]
		project, ok := projects[projectID]
		if !ok || project.LatestCommand == nil {
			continue
		}
		for _, item := range items {
			project.LatestCommand.Stage, project.LatestCommand.CurrentHead, project.LatestCommand.Diagnostic, project.LatestCommand.UpdatedAt = domain.ProjectCommandStage(item.payload.Stage), item.payload.CurrentHead, item.payload.Diagnostic, item.at
			if item.payload.Stage == "committed" {
				delete(projects, projectID)
				break
			} else if item.payload.Stage == "rejected" {
				project.PendingCommand = nil
			} else {
				project.PendingCommand = project.LatestCommand
			}
			projects[projectID] = project
		}
	}
	result := make([]domain.Project, 0, len(projects))
	for _, project := range projects {
		annotateProjectCommandReachability(project.PendingCommand, time.Now().UTC())
		result = append(result, project)
	}
	sort.Slice(result, func(i, j int) bool { return result[i].CreatedAt.Before(result[j].CreatedAt) })
	return result, nil
}

func (s *SQLite) ProcessProjectCommands(ctx context.Context) error {
	commands, err := s.pendingHomeProjectCommands(ctx)
	if err != nil {
		return err
	}
	for _, pending := range commands {
		if err := s.processProjectCommand(ctx, pending); err != nil {
			return err
		}
	}
	return nil
}

func (s *SQLite) pendingHomeProjectCommands(ctx context.Context) ([]pendingHomeCommand, error) {
	rows, err := s.db.QueryContext(ctx, `SELECT raw FROM canonical_events WHERE event_type IN (?,?) AND reduction_status=? ORDER BY created_at,event_id`, event.TypeProjectCommand, event.TypeProjectResult, event.StatusProjected)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	commands := make(map[string]pendingHomeCommand)
	resolved, received := make(map[string]bool), make(map[string]bool)
	var order []string
	for rows.Next() {
		var raw []byte
		if err := rows.Scan(&raw); err != nil {
			return nil, err
		}
		item := event.Inspect(raw).Event
		if item.Content.Type == event.TypeProjectResult {
			var payload event.ProjectCommandResultPayload
			if json.Unmarshal(item.Content.Payload, &payload) == nil {
				if payload.Stage == "received" {
					received[payload.CommandID] = true
				} else {
					resolved[payload.CommandID] = true
				}
			}
			continue
		}
		var payload event.ProjectCommandPayload
		if json.Unmarshal(item.Content.Payload, &payload) != nil || item.Content.Recipient == nil || item.Content.Recipient.InstallationID != s.signer.InstallationID {
			continue
		}
		commands[payload.CommandID] = pendingHomeCommand{command: domain.ProjectCommand{ID: payload.CommandID, ProjectID: payload.ProjectID, HomeInstallation: s.signer.InstallationID, ExpectedHead: payload.ExpectedHead, Operation: domain.ProjectCommandOperation(payload.Operation), Body: payload.Body, Stage: domain.ProjectCommandReceived}, eventID: item.ID(), issuer: item.Content.InstallationID, received: received[payload.CommandID]}
		order = append(order, payload.CommandID)
	}
	var result []pendingHomeCommand
	for id, item := range commands {
		item.received = received[id]
		commands[id] = item
	}
	for _, id := range order {
		if !resolved[id] {
			result = append(result, commands[id])
		}
	}
	return result, nil
}

func (s *SQLite) processProjectCommand(ctx context.Context, pending pendingHomeCommand) error {
	if !pending.received {
		current, _ := s.GetProject(ctx, pending.command.ProjectID)
		if err := s.publishProjectCommandResult(ctx, pending, current, domain.ProjectCommandReceived, ""); err != nil {
			return err
		}
	}
	data, decodeErr := domain.DecodeProjectCommand(pending.command.Operation, pending.command.Body)
	if decodeErr != nil {
		current, _ := s.GetProject(ctx, pending.command.ProjectID)
		return s.publishProjectCommandResult(ctx, pending, current, domain.ProjectCommandRejected, decodeErr.Error())
	}
	digestBytes := sha256.Sum256(append([]byte(string(pending.command.Operation)+"\x00"+pending.command.ProjectID+"\x00"+pending.command.ExpectedHead+"\x00"), pending.command.Body...))
	mutation := domain.Mutation{ID: pending.command.ID, Method: string(pending.command.Operation), RequestDigest: hex.EncodeToString(digestBytes[:])}
	mutationCtx := domain.WithMutation(ctx, mutation)
	var project domain.Project
	if raw, found, err := s.MutationResult(ctx, mutation); err != nil {
		return err
	} else if found {
		if pending.command.Operation == domain.ProjectCommandResourceCheck {
			project, err = s.GetProject(ctx, pending.command.ProjectID)
			if err != nil {
				return err
			}
		} else if err := json.Unmarshal(raw, &project); err != nil {
			return err
		}
	} else {
		var err error
		if domain.ProjectCommandRequiresRuntime(pending.command.Operation) {
			if s.projectCommandHandler == nil {
				err = errors.New("project runtime command handler is unavailable")
			} else {
				// Runtime sagas perform several authoritative mutations and own
				// idempotency through their stable operation IDs; one mutation
				// receipt cannot span those transaction boundaries.
				project, err = s.projectCommandHandler(ctx, pending.command, data)
			}
		} else {
			project, err = domain.ExecuteProjectCommand(mutationCtx, s, pending.command, data)
		}
		if err != nil {
			current, _ := s.GetProject(ctx, pending.command.ProjectID)
			return s.publishProjectCommandResult(ctx, pending, current, domain.ProjectCommandRejected, err.Error())
		}
	}
	return s.publishProjectCommandResult(ctx, pending, project, domain.ProjectCommandCommitted, "")
}

func (s *SQLite) publishProjectCommandResult(ctx context.Context, pending pendingHomeCommand, project domain.Project, stage domain.ProjectCommandStage, diagnostic string) error {
	account, membership, _, err := s.localAccountAction(ctx, "")
	if err != nil {
		return err
	}
	parents := append(append([]string(nil), membership...), pending.eventID)
	if project.HeadEventID != "" {
		parents = append(parents, project.HeadEventID)
	}
	sort.Strings(parents)
	parents = slices.Compact(parents)
	body, _ := json.Marshal(project)
	payload, _ := event.MarshalPayload(event.ProjectCommandResultPayload{CommandID: pending.command.ID, ProjectID: pending.command.ProjectID, Stage: string(stage), Committed: stage == domain.ProjectCommandCommitted, CurrentHead: project.HeadEventID, Diagnostic: diagnostic, Body: body})
	content := event.Content{Type: event.TypeProjectResult, Sender: s.localAddress(model.HumanMailboxID), Recipient: &event.MailboxAddress{InstallationID: pending.issuer, MailboxID: model.HumanMailboxID}, Audience: &event.Audience{HumanAccountID: account.ID}, Parents: parents, Scope: event.ScopeAccountAddressed, Payload: payload}
	return s.appendContents(ctx, []event.Content{content}, []time.Time{s.now().UTC()}, nil)
}
