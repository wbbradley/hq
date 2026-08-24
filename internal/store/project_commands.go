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
	body, err := json.Marshal(request)
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
	payload, _ := event.MarshalPayload(event.ProjectCommandPayload{CommandID: commandID, ProjectID: request.ID, Operation: "project.create", Body: body})
	created := s.now().UTC()
	content := event.Content{Type: event.TypeProjectCommand, Sender: s.localAddress(model.HumanMailboxID), Recipient: &event.MailboxAddress{InstallationID: request.HomeInstallation, MailboxID: model.HumanMailboxID}, Audience: &event.Audience{HumanAccountID: account.ID}, Parents: parents, Scope: event.ScopeAccountAddressed, Payload: payload}
	command := domain.ProjectCommand{ID: commandID, ProjectID: request.ID, HomeInstallation: request.HomeInstallation, Operation: "project.create", Body: body, Stage: domain.ProjectCommandQueued, CreatedAt: created, UpdatedAt: created}
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
	body, err := json.Marshal(request)
	if err != nil {
		return domain.Project{}, true, err
	}
	account, parents, _, err := s.localAccountAction(ctx, "")
	if err != nil {
		return domain.Project{}, true, err
	}
	payload, _ := event.MarshalPayload(event.ProjectCommandPayload{CommandID: request.RequestID, ProjectID: request.ProjectID, Operation: "project.provision-worktree", Body: body})
	created := s.now().UTC()
	content := event.Content{Type: event.TypeProjectCommand, Sender: s.localAddress(model.HumanMailboxID), Recipient: &event.MailboxAddress{InstallationID: request.HomeInstallation, MailboxID: model.HumanMailboxID}, Audience: &event.Audience{HumanAccountID: account.ID}, Parents: parents, Scope: event.ScopeAccountAddressed, Payload: payload}
	command := domain.ProjectCommand{ID: request.RequestID, ProjectID: request.ProjectID, HomeInstallation: request.HomeInstallation, Operation: "project.provision-worktree", Body: body, Stage: domain.ProjectCommandQueued, CreatedAt: created, UpdatedAt: created}
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
	if len(command.Body) == 0 {
		command.Body = []byte(`{}`)
	}
	if !json.Valid(command.Body) || command.Operation == "" {
		return project, errors.New("project command operation and JSON body are required")
	}
	account, membership, _, err := s.localAccountAction(ctx, "")
	if err != nil {
		return project, err
	}
	parents := append(append([]string(nil), membership...), command.ExpectedHead)
	sort.Strings(parents)
	payload, _ := event.MarshalPayload(event.ProjectCommandPayload{CommandID: command.ID, ProjectID: project.ID, ExpectedHead: command.ExpectedHead, Operation: command.Operation, Body: command.Body})
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

func (s *SQLite) queueReplicaCommand(ctx context.Context, projectID, expected, operation string, data any) (domain.Project, bool, error) {
	project, err := getProjectReplica(ctx, s.db, projectID)
	if errors.Is(err, domain.ErrProjectNotFound) {
		return domain.Project{}, false, nil
	}
	if err != nil {
		return project, true, err
	}
	body, err := json.Marshal(data)
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
			command := &domain.ProjectCommand{ID: payload.CommandID, ProjectID: payload.ProjectID, HomeInstallation: inspection.Event.Content.Recipient.InstallationID, ExpectedHead: payload.ExpectedHead, Operation: payload.Operation, Body: payload.Body, Stage: domain.ProjectCommandQueued, CreatedAt: time.Unix(inspection.Event.Nostr.CreatedAt, 0).UTC(), UpdatedAt: time.Unix(inspection.Event.Nostr.CreatedAt, 0).UTC()}
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
			if json.Unmarshal(item.Content.Payload, &payload) != nil || payload.Operation != "project.create" && payload.Operation != "project.provision-worktree" || item.Content.Recipient == nil {
				continue
			}
			var name, brief string
			if payload.Operation == "project.create" {
				var request domain.CreateProjectRequest
				if json.Unmarshal(payload.Body, &request) != nil {
					continue
				}
				name, brief = request.Name, request.Brief
			} else {
				var request domain.ProjectWorktreeRequest
				if json.Unmarshal(payload.Body, &request) != nil {
					continue
				}
				name, brief = request.Name, request.Brief
			}
			created := time.Unix(item.Nostr.CreatedAt, 0).UTC()
			command := &domain.ProjectCommand{ID: payload.CommandID, ProjectID: payload.ProjectID, HomeInstallation: item.Content.Recipient.InstallationID, Operation: payload.Operation, Body: payload.Body, Stage: domain.ProjectCommandQueued, CreatedAt: created, UpdatedAt: created}
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
		commands[payload.CommandID] = pendingHomeCommand{command: domain.ProjectCommand{ID: payload.CommandID, ProjectID: payload.ProjectID, HomeInstallation: s.signer.InstallationID, ExpectedHead: payload.ExpectedHead, Operation: payload.Operation, Body: payload.Body, Stage: domain.ProjectCommandReceived}, eventID: item.ID(), issuer: item.Content.InstallationID, received: received[payload.CommandID]}
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
	digestBytes := sha256.Sum256(append([]byte(pending.command.Operation+"\x00"+pending.command.ProjectID+"\x00"+pending.command.ExpectedHead+"\x00"), pending.command.Body...))
	mutation := domain.Mutation{ID: pending.command.ID, Method: pending.command.Operation, RequestDigest: hex.EncodeToString(digestBytes[:])}
	mutationCtx := domain.WithMutation(ctx, mutation)
	var project domain.Project
	if raw, found, err := s.MutationResult(ctx, mutation); err != nil {
		return err
	} else if found {
		if pending.command.Operation == "project.resource.check" {
			project, err = s.GetProject(ctx, pending.command.ProjectID)
			if err != nil {
				return err
			}
		} else if err := json.Unmarshal(raw, &project); err != nil {
			return err
		}
	} else {
		var err error
		switch pending.command.Operation {
		case "project.open":
			project, err = s.OpenProject(mutationCtx, pending.command.ProjectID, pending.command.ExpectedHead)
		case "project.create":
			var request domain.CreateProjectRequest
			if json.Unmarshal(pending.command.Body, &request) != nil {
				err = errors.New("invalid project creation command")
			} else {
				request.ID, request.HomeInstallation = pending.command.ProjectID, s.signer.InstallationID
				project, err = s.CreateProject(mutationCtx, request)
			}
		case "project.archive.set":
			var data struct {
				Archived bool `json:"archived"`
			}
			if json.Unmarshal(pending.command.Body, &data) != nil {
				err = errors.New("invalid archive command")
			} else {
				project, err = s.SetProjectArchived(mutationCtx, pending.command.ProjectID, pending.command.ExpectedHead, data.Archived)
			}
		case "project.metadata.update":
			var data struct {
				Name  string `json:"name"`
				Brief string `json:"brief"`
			}
			if json.Unmarshal(pending.command.Body, &data) != nil {
				err = errors.New("invalid metadata command")
			} else {
				project, err = s.UpdateProjectMetadata(mutationCtx, pending.command.ProjectID, pending.command.ExpectedHead, data.Name, data.Brief)
			}
		case "project.close.begin":
			project, err = s.BeginCloseProject(mutationCtx, pending.command.ProjectID, pending.command.ExpectedHead)
		case "project.close.finalize":
			var data struct {
				Forced             bool   `json:"forced"`
				RuntimeObservation string `json:"runtime_observation"`
			}
			if json.Unmarshal(pending.command.Body, &data) != nil {
				err = errors.New("invalid close command")
			} else {
				project, err = s.FinalizeCloseProject(mutationCtx, pending.command.ProjectID, pending.command.ExpectedHead, data.Forced, data.RuntimeObservation)
			}
		case "project.resource.add":
			var data struct {
				Path    domain.ProjectPathInput `json:"path"`
				Primary bool                    `json:"primary"`
			}
			if json.Unmarshal(pending.command.Body, &data) != nil {
				err = errors.New("invalid resource add command")
			} else {
				project, err = s.AddProjectPath(mutationCtx, pending.command.ProjectID, pending.command.ExpectedHead, data.Path, data.Primary)
			}
		case "project.resource.remove", "project.resource.primary":
			var data struct {
				ResourceID string `json:"resource_id"`
			}
			if json.Unmarshal(pending.command.Body, &data) != nil {
				err = errors.New("invalid resource command")
			} else if pending.command.Operation == "project.resource.remove" {
				project, err = s.RemoveProjectResource(mutationCtx, pending.command.ProjectID, pending.command.ExpectedHead, data.ResourceID)
			} else {
				project, err = s.SetProjectPrimaryResource(mutationCtx, pending.command.ProjectID, pending.command.ExpectedHead, data.ResourceID)
			}
		case "project.resource.replace":
			var data struct {
				ResourceID string                  `json:"resource_id"`
				Path       domain.ProjectPathInput `json:"path"`
			}
			if json.Unmarshal(pending.command.Body, &data) != nil {
				err = errors.New("invalid resource replace command")
			} else {
				project, err = s.ReplaceProjectPath(mutationCtx, pending.command.ProjectID, pending.command.ExpectedHead, data.ResourceID, data.Path)
			}
		case "project.resource.check":
			var data struct {
				ResourceID string `json:"resource_id"`
			}
			if json.Unmarshal(pending.command.Body, &data) != nil {
				err = errors.New("invalid resource check command")
			} else if _, err = s.CheckProjectResource(mutationCtx, pending.command.ProjectID, data.ResourceID); err == nil {
				project, err = s.GetProject(mutationCtx, pending.command.ProjectID)
			}
		case "project.assignment.assign":
			var data struct {
				Agent string `json:"agent"`
			}
			if json.Unmarshal(pending.command.Body, &data) != nil {
				err = errors.New("invalid assignment command")
			} else {
				project, err = s.AssignProject(mutationCtx, pending.command.ProjectID, pending.command.ExpectedHead, data.Agent)
			}
		case "project.assignment.activate":
			var data domain.ActivateProjectAssignmentRequest
			if json.Unmarshal(pending.command.Body, &data) != nil {
				err = errors.New("invalid assignment activation command")
			} else {
				project, err = s.ActivateProjectAssignment(mutationCtx, pending.command.ProjectID, pending.command.ExpectedHead, data)
			}
		case "project.assignment.abort", "project.assignment.block":
			var data struct {
				Diagnostic string `json:"diagnostic"`
			}
			if json.Unmarshal(pending.command.Body, &data) != nil {
				err = errors.New("invalid assignment state command")
			} else if pending.command.Operation == "project.assignment.abort" {
				project, err = s.AbortProjectAssignment(mutationCtx, pending.command.ProjectID, pending.command.ExpectedHead, data.Diagnostic)
			} else {
				project, err = s.BlockProjectAssignment(mutationCtx, pending.command.ProjectID, pending.command.ExpectedHead, data.Diagnostic)
			}
		case "project.assignment.unassign":
			var data struct {
				Forced             bool   `json:"forced"`
				RuntimeObservation string `json:"runtime_observation"`
			}
			if json.Unmarshal(pending.command.Body, &data) != nil {
				err = errors.New("invalid unassign command")
			} else {
				project, err = s.UnassignProject(mutationCtx, pending.command.ProjectID, pending.command.ExpectedHead, data.Forced, data.RuntimeObservation)
			}
		default:
			if s.projectCommandHandler == nil {
				err = errors.New("unsupported remote project operation")
			} else {
				// Runtime sagas perform several authoritative mutations and own
				// idempotency through their stable operation IDs; one mutation
				// receipt cannot span those transaction boundaries.
				project, err = s.projectCommandHandler(ctx, pending.command)
			}
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
