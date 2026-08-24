package codexbridge

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

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
			if notification.Method == "turn/completed" {
				switch params.Turn.Status {
				case "failed":
					status = harness.OperationFailed
					if params.Turn.Error != nil {
						errorMessage = params.Turn.Error.Message
					}
				case "interrupted":
					status = harness.OperationInterrupted
				default:
					status = harness.OperationCompleted
				}
			}
			_ = h.instance.emit(harness.OperationID(params.Turn.ID), "", harness.OperationStatusEvent{Status: status, Error: errorMessage})
		}
	case "item/completed":
		var params ItemCompletedNotification
		if json.Unmarshal(notification.Params, &params) == nil && params.ThreadID == string(h.instance.session.identity.ID) && params.TurnID != "" && params.Item.Type == "agentMessage" && params.Item.ID != "" && strings.TrimSpace(params.Item.Text) != "" {
			_ = h.instance.emit(harness.OperationID(params.TurnID), params.Item.ID, harness.OutputEvent{Text: params.Item.Text, Final: params.Item.Phase == "final_answer"})
		}
	}
	if h.legacy != nil {
		h.legacy.HandleNotification(ctx, notification)
	}
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
