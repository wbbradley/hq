package codexbridge

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/wbbradley/hq/internal/harness"
)

type adapterRequestHandler struct {
	instance *codexInstance
	legacy   RequestHandler
}

type adapterNotificationHandler struct {
	instance *codexInstance
	legacy   NotificationHandler
}

type adapterPendingRequest struct {
	server    ServerRequest
	responses chan harness.Response
	completed bool
}

func (h *adapterRequestHandler) HandleRequest(ctx context.Context, request ServerRequest) (any, *RPCError, bool) {
	if h.legacy != nil {
		return h.legacy.HandleRequest(ctx, request)
	}
	payload, rpcErr, handled := normalizedHarnessRequest(request)
	if !handled || rpcErr != nil {
		return nil, rpcErr, handled
	}
	if sessionID := serverRequestSessionID(request); sessionID == "" || sessionID != string(h.instance.session.identity.ID) {
		return failClosedServerRequest(request)
	}
	requestID := harness.RequestID(requestIDText(request.ID))
	pending := &adapterPendingRequest{server: request, responses: make(chan harness.Response, 1)}
	h.instance.pendingMu.Lock()
	if _, exists := h.instance.pendingRequests[requestID]; exists {
		h.instance.pendingMu.Unlock()
		return nil, &RPCError{Code: -32600, Message: "duplicate app-server request ID"}, true
	}
	h.instance.pendingRequests[requestID] = pending
	h.instance.pendingMu.Unlock()
	normalized := harness.Request{ID: requestID, Session: h.instance.session.Identity(), Payload: payload}
	assignRequestCorrelation(&normalized, request)
	h.instance.streamMu.Lock()
	if h.instance.streamsClosed {
		h.instance.streamMu.Unlock()
		return failClosedServerRequest(request)
	}
	select {
	case h.instance.requests <- normalized:
		h.instance.streamMu.Unlock()
	case <-ctx.Done():
		h.instance.streamMu.Unlock()
		h.markCanceled(requestID)
		return failClosedServerRequest(request)
	}
	select {
	case response, open := <-pending.responses:
		if !open {
			return failClosedServerRequest(request)
		}
		return harnessResponseForServerRequest(request, response)
	case <-ctx.Done():
		h.markCanceled(requestID)
		return failClosedServerRequest(request)
	}
}

func (h *adapterRequestHandler) markCanceled(requestID harness.RequestID) {
	h.instance.pendingMu.Lock()
	if pending := h.instance.pendingRequests[requestID]; pending != nil {
		pending.completed = true
	}
	h.instance.pendingMu.Unlock()
}

func (h *adapterNotificationHandler) HandleNotification(ctx context.Context, notification Notification) {
	h.instance.threadState.HandleNotification(ctx, notification)
	switch notification.Method {
	case "turn/started", "turn/completed":
		var params TurnNotification
		if json.Unmarshal(notification.Params, &params) == nil && params.ThreadID == string(h.instance.session.identity.ID) && params.Turn.ID != "" {
			status := harness.OperationRunning
			errorMessage := ""
			valid := true
			if notification.Method == "turn/completed" {
				switch params.Turn.Status {
				case "completed":
					status = harness.OperationCompleted
				case "failed":
					status = harness.OperationFailed
					if params.Turn.Error != nil {
						errorMessage = params.Turn.Error.Message
					}
				case "interrupted":
					status = harness.OperationInterrupted
				default:
					valid = false
				}
			}
			if valid {
				_ = h.instance.emit(harness.OperationID(params.Turn.ID), "", harness.OperationStatusEvent{Status: status, Error: errorMessage})
			}
		}
	case "turn/plan/updated":
		var params TurnPlanUpdatedNotification
		if json.Unmarshal(notification.Params, &params) == nil && params.Plan != nil && validActivityContext(h.instance, params.ThreadID, params.TurnID) {
			plan := formatTurnPlan(params)
			if plan == "" {
				plan = "(no plan)"
			}
			_ = h.instance.emit(harness.OperationID(params.TurnID), "", harness.PlanEvent{Text: boundedAdapterText(plan, adapterActivityTextBytes)})
		}
	case "turn/diff/updated":
		var params TurnDiffUpdatedNotification
		if json.Unmarshal(notification.Params, &params) == nil && params.Diff != nil && validActivityContext(h.instance, params.ThreadID, params.TurnID) {
			diff := *params.Diff
			if diff == "" {
				diff = "(no changes)"
			}
			_ = h.instance.emit(harness.OperationID(params.TurnID), "", harness.DiffEvent{Text: boundedAdapterText(diff, adapterActivityTextBytes)})
		}
	case "item/started":
		var params ItemStartedNotification
		if json.Unmarshal(notification.Params, &params) == nil && strings.TrimSpace(params.Item.ID) != "" && validActivityContext(h.instance, params.ThreadID, params.TurnID) {
			if progress := startedItemProgress(params.Item); progress != "" {
				_ = h.instance.emit(harness.OperationID(params.TurnID), params.Item.ID, harness.ProgressEvent{Message: boundedAdapterText(progress, adapterProgressTextBytes)})
			}
		}
	case "item/completed":
		var params ItemCompletedNotification
		if json.Unmarshal(notification.Params, &params) == nil && strings.TrimSpace(params.Item.ID) != "" && validActivityContext(h.instance, params.ThreadID, params.TurnID) {
			if payload := completedItemEvent(params.Item); payload != nil {
				_ = h.instance.emit(harness.OperationID(params.TurnID), params.Item.ID, payload)
			}
		}
	case "item/plan/delta", "item/commandExecution/outputDelta", "item/fileChange/outputDelta":
		var params ItemDeltaNotification
		if json.Unmarshal(notification.Params, &params) == nil && strings.TrimSpace(params.ItemID) != "" && validActivityContext(h.instance, params.ThreadID, params.TurnID) && strings.TrimSpace(params.Delta) != "" {
			_ = h.instance.emit(harness.OperationID(params.TurnID), params.ItemID, harness.ProgressEvent{Message: boundedAdapterText(params.Delta, adapterProgressTextBytes)})
		}
	case "item/mcpToolCall/progress":
		var params ToolProgressNotification
		if json.Unmarshal(notification.Params, &params) == nil && strings.TrimSpace(params.ItemID) != "" && validActivityContext(h.instance, params.ThreadID, params.TurnID) && strings.TrimSpace(params.Message) != "" {
			_ = h.instance.emit(harness.OperationID(params.TurnID), params.ItemID, harness.ProgressEvent{Message: boundedAdapterText(params.Message, adapterProgressTextBytes)})
		}
	}
	if h.legacy != nil {
		h.legacy.HandleNotification(ctx, notification)
	}
}

const (
	adapterTitleTextBytes    = 2 << 10
	adapterCommandTextBytes  = 32 << 10
	adapterProgressTextBytes = 8 << 10
	adapterActivityTextBytes = 128 << 10
)

func validActivityContext(instance *codexInstance, threadID, turnID string) bool {
	return threadID == string(instance.session.identity.ID) && strings.TrimSpace(turnID) != ""
}

func formatTurnPlan(params TurnPlanUpdatedNotification) string {
	parts := make([]string, 0, len(params.Plan)+1)
	if params.Explanation != nil && strings.TrimSpace(*params.Explanation) != "" {
		parts = append(parts, strings.TrimSpace(*params.Explanation))
	}
	for _, step := range params.Plan {
		text := strings.TrimSpace(step.Step)
		if text == "" {
			continue
		}
		marker := "[ ]"
		switch step.Status {
		case "completed":
			marker = "[x]"
		case "inProgress":
			marker = "[~]"
		}
		parts = append(parts, "- "+marker+" "+text)
	}
	return strings.Join(parts, "\n")
}

func startedItemProgress(item ThreadItem) string {
	switch item.Type {
	case "commandExecution":
		if item.Status != "inProgress" {
			return ""
		}
		if command := strings.TrimSpace(item.Command); command != "" {
			return "Running command: " + boundedAdapterText(command, adapterTitleTextBytes)
		}
	case "fileChange":
		if item.Status != "inProgress" {
			return ""
		}
		return "Applying file changes"
	case "mcpToolCall":
		if item.Status == "inProgress" && item.Server != "" && item.Tool != "" {
			return "Calling " + boundedAdapterText(item.Server+"/"+item.Tool, adapterTitleTextBytes)
		}
	case "dynamicToolCall":
		if item.Status == "inProgress" && item.Tool != "" {
			return "Calling " + boundedAdapterText(item.Tool, adapterTitleTextBytes)
		}
	case "collabAgentToolCall":
		if item.Status == "inProgress" && item.Tool != "" {
			return "Running collaboration tool: " + boundedAdapterText(item.Tool, adapterTitleTextBytes)
		}
	case "webSearch":
		if item.Query != "" {
			return "Searching the web: " + boundedAdapterText(item.Query, adapterTitleTextBytes)
		}
	case "plan":
		if strings.TrimSpace(item.Text) != "" {
			return "Updating plan"
		}
	}
	return ""
}

func completedItemEvent(item ThreadItem) harness.EventPayload {
	switch item.Type {
	case "agentMessage":
		if strings.TrimSpace(item.Text) != "" {
			return harness.OutputEvent{Text: item.Text, Final: item.Phase == "final_answer"}
		}
	case "plan":
		if strings.TrimSpace(item.Text) != "" {
			return harness.PlanEvent{Text: boundedAdapterText(item.Text, adapterActivityTextBytes)}
		}
	case "commandExecution":
		status, ok := completedItemStatus(item.Status)
		if !ok || strings.TrimSpace(item.Command) == "" {
			return nil
		}
		output := ""
		if item.AggregatedOutput != nil {
			output = boundedAdapterText(*item.AggregatedOutput, adapterCommandTextBytes)
		}
		return harness.CommandEvent{Command: boundedAdapterText(item.Command, adapterTitleTextBytes), Output: output, ExitCode: item.ExitCode, Status: status}
	case "fileChange":
		status, ok := completedItemStatus(item.Status)
		if !ok {
			return nil
		}
		path, summary := summarizeFileChanges(item.Changes)
		return harness.FileChangeEvent{Path: boundedAdapterText(path, adapterTitleTextBytes), Summary: boundedAdapterText(summary, adapterActivityTextBytes), Status: status}
	case "mcpToolCall":
		status, ok := completedItemStatus(item.Status)
		if !ok || item.Server == "" || item.Tool == "" {
			return nil
		}
		return harness.ToolEvent{Name: boundedAdapterText(item.Server+"/"+item.Tool, adapterTitleTextBytes), Summary: boundedAdapterText(toolCallSummary(item), adapterActivityTextBytes), Status: status}
	case "dynamicToolCall":
		status, ok := completedItemStatus(item.Status)
		if !ok || item.Tool == "" {
			return nil
		}
		return harness.ToolEvent{Name: boundedAdapterText(item.Tool, adapterTitleTextBytes), Summary: boundedAdapterText(toolCallSummary(item), adapterActivityTextBytes), Status: status}
	case "collabAgentToolCall":
		status, ok := completedItemStatus(item.Status)
		if !ok || item.Tool == "" {
			return nil
		}
		summary := ""
		if len(item.ReceiverThreadIDs) > 0 {
			summary = "Receiver threads: " + strings.Join(item.ReceiverThreadIDs, ", ")
		}
		return harness.ToolEvent{Name: boundedAdapterText("collab/"+item.Tool, adapterTitleTextBytes), Summary: boundedAdapterText(summary, adapterActivityTextBytes), Status: status}
	case "webSearch":
		if strings.TrimSpace(item.Query) != "" {
			return harness.ToolEvent{Name: "web search", Summary: boundedAdapterText(item.Query, adapterActivityTextBytes), Status: harness.OperationCompleted}
		}
	}
	return nil
}

func completedItemStatus(status string) (harness.OperationStatus, bool) {
	switch status {
	case "completed":
		return harness.OperationCompleted, true
	case "failed":
		return harness.OperationFailed, true
	case "declined":
		return harness.OperationInterrupted, true
	default:
		return "", false
	}
}

func summarizeFileChanges(changes []FileUpdateChange) (string, string) {
	if len(changes) == 0 {
		return "file changes", "No file details were provided."
	}
	path := strings.TrimSpace(changes[0].Path)
	if path == "" {
		path = "file changes"
	}
	if len(changes) > 1 {
		path = fmt.Sprintf("%s (+%d more)", path, len(changes)-1)
	}
	parts := make([]string, 0, len(changes))
	for _, change := range changes {
		kind := struct {
			Type string `json:"type"`
		}{}
		_ = json.Unmarshal(change.Kind, &kind)
		header := strings.TrimSpace(strings.TrimSpace(kind.Type) + " " + strings.TrimSpace(change.Path))
		if header == "" {
			header = "file change"
		}
		if change.Diff != "" {
			header += "\n" + change.Diff
		}
		parts = append(parts, header)
	}
	return path, strings.Join(parts, "\n\n")
}

func toolCallSummary(item ThreadItem) string {
	parts := make([]string, 0, 3)
	if arguments := compactJSON(item.Arguments); arguments != "" && arguments != "null" {
		parts = append(parts, "Arguments: "+arguments)
	}
	if item.Error != nil && strings.TrimSpace(item.Error.Message) != "" {
		parts = append(parts, "Error: "+strings.TrimSpace(item.Error.Message))
	} else if result := compactJSON(item.Result); result != "" && result != "null" {
		parts = append(parts, "Result: "+result)
	} else if content := compactJSON(item.ContentItems); content != "" && content != "null" {
		parts = append(parts, "Result: "+content)
	}
	return strings.Join(parts, "\n")
}

func compactJSON(value json.RawMessage) string {
	if len(value) == 0 || !json.Valid(value) {
		return ""
	}
	var compact bytes.Buffer
	if err := json.Compact(&compact, value); err != nil {
		return ""
	}
	return compact.String()
}

func boundedAdapterText(value string, limit int) string {
	if len(value) <= limit {
		return value
	}
	end := limit
	for end > 0 && !utf8.ValidString(value[:end]) {
		end--
	}
	return value[:end]
}

func (i *codexInstance) emit(operation harness.OperationID, itemID string, payload harness.EventPayload) error {
	if payload == nil {
		return fmt.Errorf("normalized event payload is required")
	}
	i.streamMu.Lock()
	defer i.streamMu.Unlock()
	if i.streamsClosed {
		return harness.ErrInstanceStopped
	}
	i.sequence++
	event := harness.Event{
		Sequence: i.sequence, Session: i.session.Identity(), Operation: operation, ItemID: itemID,
		OccurredAt: timeNowUTC(), Payload: payload,
	}
	select {
	case i.events <- event:
		return nil
	case <-i.ctx.Done():
		return harness.ErrInstanceStopped
	}
}

func (s *codexSession) Respond(ctx context.Context, response harness.Response) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	if response.RequestID == "" || response.Payload == nil {
		return fmt.Errorf("interactive response requires request ID and payload")
	}
	if err := s.instance.running(); err != nil {
		return err
	}
	s.instance.pendingMu.Lock()
	pending := s.instance.pendingRequests[response.RequestID]
	if pending == nil {
		s.instance.pendingMu.Unlock()
		return harness.ErrRequestNotFound
	}
	if pending.completed {
		s.instance.pendingMu.Unlock()
		return harness.ErrRequestCompleted
	}
	pending.completed = true
	pending.responses <- response
	s.instance.pendingMu.Unlock()
	return nil
}

func normalizedHarnessRequest(request ServerRequest) (harness.RequestPayload, *RPCError, bool) {
	switch request.Method {
	case requestUserInputMethod:
		var params RequestUserInputParams
		if json.Unmarshal(request.Params, &params) != nil || params.ThreadID == "" || params.TurnID == "" || params.ItemID == "" || len(params.Questions) == 0 {
			return nil, requestError("HQ rejected malformed request_user_input"), true
		}
		questions := make([]harness.Question, 0, len(params.Questions))
		for _, question := range params.Questions {
			if question.ID == "" || strings.TrimSpace(question.Question) == "" {
				return nil, requestError("HQ rejected malformed request_user_input question"), true
			}
			options := make([]harness.QuestionOption, 0, len(question.Options))
			for _, option := range question.Options {
				options = append(options, harness.QuestionOption{Label: option.Label, Description: option.Description})
			}
			questions = append(questions, harness.Question{
				ID: question.ID, Header: question.Header, Prompt: question.Question, Options: options,
				AllowOther: question.IsOther || len(question.Options) == 0, Secret: question.IsSecret,
			})
		}
		return harness.QuestionSetRequest{Questions: questions}, nil, true
	case commandApprovalMethod:
		var params CommandApprovalParams
		if json.Unmarshal(request.Params, &params) != nil || !validApprovalContext(params.ThreadID, params.TurnID, params.ItemID) || !validNetworkAmendments(params.ProposedNetworkPolicyAmendments) {
			return harness.ApprovalRequest{Kind: "command", Summary: "Malformed command approval", Choices: []string{"cancel"}}, nil, true
		}
		choices := []string{"accept", "acceptForSession", "decline", "cancel"}
		if len(params.ProposedExecpolicyAmendment) > 0 {
			choices = append(choices, "acceptWithExecpolicyAmendment")
		}
		for index := range params.ProposedNetworkPolicyAmendments {
			choices = append(choices, fmt.Sprintf("applyNetworkPolicyAmendment:%d", index+1))
		}
		return harness.ApprovalRequest{Kind: "command", Summary: commandApprovalDetails(params), Choices: choices, Persistent: len(choices) > 4}, nil, true
	case fileApprovalMethod:
		var params FileChangeApprovalParams
		if json.Unmarshal(request.Params, &params) != nil || !validApprovalContext(params.ThreadID, params.TurnID, params.ItemID) {
			return harness.ApprovalRequest{Kind: "file-change", Summary: "Malformed file approval", Choices: []string{"cancel"}}, nil, true
		}
		return harness.ApprovalRequest{Kind: "file-change", Summary: fmt.Sprintf("Reason: %s\nGrant root: %s", valueOrNone(params.Reason), valueOrNone(params.GrantRoot)), Choices: []string{"accept", "acceptForSession", "decline", "cancel"}, Persistent: true}, nil, true
	case permissionMethod:
		var params PermissionApprovalParams
		if json.Unmarshal(request.Params, &params) != nil || !validApprovalContext(params.ThreadID, params.TurnID, params.ItemID) || !jsonObject(params.Permissions) {
			return harness.ApprovalRequest{Kind: "permissions", Summary: "Malformed permission approval", Choices: []string{"decline"}}, nil, true
		}
		return harness.ApprovalRequest{Kind: "permissions", Summary: fmt.Sprintf("Working directory: %s\nReason: %s\nPermissions: %s", valueOrNone(params.CWD), valueOrNone(params.Reason), prettyJSON(params.Permissions)), Choices: []string{"grantTurn", "grantSession", "decline"}, Persistent: true}, nil, true
	case mcpElicitationMethod:
		var params MCPElicitationParams
		if json.Unmarshal(request.Params, &params) != nil || params.ThreadID == "" || params.ServerName == "" || params.Message == "" {
			return harness.ApprovalRequest{Kind: "elicitation", Summary: "Malformed elicitation", Choices: []string{"cancel"}}, nil, true
		}
		switch params.Mode {
		case "form":
			if _, err := validateMCPFormSchema(params.RequestedSchema); err != nil {
				return harness.ApprovalRequest{Kind: "elicitation", Summary: "Unsupported form", Choices: []string{"cancel"}}, nil, true
			}
			return harness.StructuredQuestionRequest{Prompt: params.Message, SchemaMediaType: "application/schema+json", Schema: append([]byte(nil), params.RequestedSchema...)}, nil, true
		case "url":
			if params.URL == "" || params.ElicitationID == "" {
				return harness.ApprovalRequest{Kind: "elicitation", Summary: "Malformed URL elicitation", Choices: []string{"cancel"}}, nil, true
			}
			return harness.ApprovalRequest{Kind: "elicitation-url", Summary: params.Message + "\nURL: " + params.URL, Choices: []string{"accept", "decline", "cancel"}}, nil, true
		default:
			return harness.ApprovalRequest{Kind: "elicitation", Summary: "Unsupported elicitation mode", Choices: []string{"cancel"}}, nil, true
		}
	default:
		return nil, nil, false
	}
}

func assignRequestCorrelation(destination *harness.Request, request ServerRequest) {
	switch request.Method {
	case requestUserInputMethod:
		var params RequestUserInputParams
		if json.Unmarshal(request.Params, &params) == nil {
			destination.Operation, destination.ItemID = harness.OperationID(params.TurnID), params.ItemID
		}
	case commandApprovalMethod:
		var params CommandApprovalParams
		if json.Unmarshal(request.Params, &params) == nil {
			destination.Operation, destination.ItemID = harness.OperationID(params.TurnID), params.ItemID
		}
	case fileApprovalMethod:
		var params FileChangeApprovalParams
		if json.Unmarshal(request.Params, &params) == nil {
			destination.Operation, destination.ItemID = harness.OperationID(params.TurnID), params.ItemID
		}
	case permissionMethod:
		var params PermissionApprovalParams
		if json.Unmarshal(request.Params, &params) == nil {
			destination.Operation, destination.ItemID = harness.OperationID(params.TurnID), params.ItemID
		}
	case mcpElicitationMethod:
		var params MCPElicitationParams
		if json.Unmarshal(request.Params, &params) == nil {
			destination.Operation = harness.OperationID(params.TurnID)
		}
	}
}

func serverRequestSessionID(request ServerRequest) string {
	switch request.Method {
	case requestUserInputMethod:
		var params RequestUserInputParams
		_ = json.Unmarshal(request.Params, &params)
		return params.ThreadID
	case commandApprovalMethod:
		var params CommandApprovalParams
		_ = json.Unmarshal(request.Params, &params)
		return params.ThreadID
	case fileApprovalMethod:
		var params FileChangeApprovalParams
		_ = json.Unmarshal(request.Params, &params)
		return params.ThreadID
	case permissionMethod:
		var params PermissionApprovalParams
		_ = json.Unmarshal(request.Params, &params)
		return params.ThreadID
	case mcpElicitationMethod:
		var params MCPElicitationParams
		_ = json.Unmarshal(request.Params, &params)
		return params.ThreadID
	default:
		return ""
	}
}

func harnessResponseForServerRequest(request ServerRequest, response harness.Response) (any, *RPCError, bool) {
	switch request.Method {
	case requestUserInputMethod:
		var params RequestUserInputParams
		answers, ok := response.Payload.(harness.AnswerResponse)
		if json.Unmarshal(request.Params, &params) != nil || !ok || len(answers.Answers) != len(params.Questions) {
			return nil, requestError("request_user_input response is invalid"), true
		}
		result := make(map[string]any, len(params.Questions))
		for index, question := range params.Questions {
			result[question.ID] = map[string]any{"answers": []string{answers.Answers[index]}}
		}
		return map[string]any{"answers": result}, nil, true
	case commandApprovalMethod:
		var params CommandApprovalParams
		if json.Unmarshal(request.Params, &params) != nil {
			return commandDecision("cancel"), nil, true
		}
		decision, ok := responseDecision(response.Payload)
		if !ok {
			return commandDecision("cancel"), nil, true
		}
		value, err := commandApprovalValidator(params)(decision)
		if err != nil {
			return commandDecision("cancel"), nil, true
		}
		return commandDecision(value), nil, true
	case fileApprovalMethod:
		decision, ok := responseDecision(response.Payload)
		if !ok {
			decision = "cancel"
		}
		if _, err := exactDecisionValidator("accept", "acceptForSession", "decline", "cancel")(decision); err != nil {
			decision = "cancel"
		}
		return commandDecision(decision), nil, true
	case permissionMethod:
		var params PermissionApprovalParams
		decision, ok := responseDecision(response.Payload)
		if json.Unmarshal(request.Params, &params) != nil || !ok || decision == "decline" {
			return deniedPermissions(), nil, true
		}
		if _, err := exactDecisionValidator("grantTurn", "grantSession")(decision); err != nil {
			return deniedPermissions(), nil, true
		}
		scope := "turn"
		if decision == "grantSession" {
			scope = "session"
		}
		return map[string]any{"permissions": params.Permissions, "scope": scope}, nil, true
	case mcpElicitationMethod:
		var params MCPElicitationParams
		if json.Unmarshal(request.Params, &params) != nil {
			return cancelledElicitation(), nil, true
		}
		if params.Mode == "form" {
			if _, cancel := response.Payload.(harness.CancelResponse); cancel {
				return cancelledElicitation(), nil, true
			}
			structured, ok := response.Payload.(harness.StructuredResponse)
			if !ok || structured.MediaType != "application/json" {
				return cancelledElicitation(), nil, true
			}
			content, err := validateMCPForm(params.RequestedSchema, string(structured.Data))
			if err != nil {
				return cancelledElicitation(), nil, true
			}
			return map[string]any{"action": "accept", "content": content}, nil, true
		}
		decision, ok := responseDecision(response.Payload)
		if !ok {
			decision = "cancel"
		}
		if _, err := exactDecisionValidator("accept", "decline", "cancel")(decision); err != nil {
			decision = "cancel"
		}
		return map[string]any{"action": decision, "content": nil}, nil, true
	default:
		return nil, nil, false
	}
}

func responseDecision(payload harness.ResponsePayload) (string, bool) {
	switch value := payload.(type) {
	case harness.DecisionResponse:
		return value.Decision, value.Decision != ""
	case harness.CancelResponse:
		return "cancel", true
	default:
		return "", false
	}
}

func failClosedServerRequest(request ServerRequest) (any, *RPCError, bool) {
	switch request.Method {
	case requestUserInputMethod:
		return nil, requestError("request_user_input was cancelled"), true
	case commandApprovalMethod, fileApprovalMethod:
		return commandDecision("cancel"), nil, true
	case permissionMethod:
		return deniedPermissions(), nil, true
	case mcpElicitationMethod:
		return cancelledElicitation(), nil, true
	default:
		return nil, nil, false
	}
}

var timeNowUTC = func() time.Time { return time.Now().UTC() }
