package codexbridge

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

const (
	requestUserInputMethod = "item/tool/requestUserInput"
	commandApprovalMethod  = "item/commandExecution/requestApproval"
	fileApprovalMethod     = "item/fileChange/requestApproval"
	permissionMethod       = "item/permissions/requestApproval"
	mcpElicitationMethod   = "mcpServer/elicitation/request"
)

type RequestRouter struct {
	mu         sync.RWMutex
	store      QuestionStore
	replies    *ReplyRegistry
	questioner *Questioner
}

func NewRequestRouter(store QuestionStore, replies *ReplyRegistry) *RequestRouter {
	return &RequestRouter{store: store, replies: replies}
}

func (r *RequestRouter) Bind(threadID string, mailbox model.Mailbox, repository model.RepositoryContext, syncMailbox func(context.Context) error, subscribe func(context.Context, ...domain.ChangeTopic) (domain.ChangeSubscription, error), repairInterval time.Duration) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.questioner = &Questioner{
		Store: r.store, Replies: r.replies, Mailbox: mailbox, ThreadID: threadID, Repository: repository,
		Sync: syncMailbox, Subscribe: subscribe, RepairInterval: repairInterval,
	}
}

func (r *RequestRouter) HandleRequest(ctx context.Context, request ServerRequest) (any, *RPCError, bool) {
	r.mu.RLock()
	questioner := r.questioner
	r.mu.RUnlock()
	if questioner == nil {
		return nil, &RPCError{Code: -32000, Message: "HQ bridge is not ready for Codex server requests"}, true
	}
	switch request.Method {
	case requestUserInputMethod:
		return r.handleUserInput(ctx, questioner, request)
	case commandApprovalMethod:
		return r.handleCommandApproval(ctx, questioner, request)
	case fileApprovalMethod:
		return r.handleFileApproval(ctx, questioner, request)
	case permissionMethod:
		return r.handlePermissionApproval(ctx, questioner, request)
	case mcpElicitationMethod:
		return r.handleMCPElicitation(ctx, questioner, request)
	default:
		correlation := RequestCorrelation{RequestID: requestIDText(request.ID)}
		_ = questioner.Notice(context.Background(), "Unsupported Codex request", "HQ cannot safely handle app-server request method "+request.Method+".", correlation)
		return nil, nil, false
	}
}

func (r *RequestRouter) handleUserInput(ctx context.Context, questioner *Questioner, request ServerRequest) (any, *RPCError, bool) {
	var params RequestUserInputParams
	if err := json.Unmarshal(request.Params, &params); err != nil || params.ThreadID == "" || params.TurnID == "" || params.ItemID == "" || len(params.Questions) == 0 {
		r.noticeMalformed(questioner, request, "request_user_input payload is malformed")
		return nil, requestError("HQ rejected malformed request_user_input"), true
	}
	correlation := RequestCorrelation{ThreadID: params.ThreadID, TurnID: params.TurnID, ItemID: params.ItemID, RequestID: requestIDText(request.ID)}
	if !r.matchesThread(questioner, params.ThreadID) {
		r.noticeMalformed(questioner, request, "request thread does not match the bound Codex thread")
		return nil, requestError("HQ rejected a request for another Codex thread"), true
	}
	for _, question := range params.Questions {
		if question.IsSecret {
			_ = questioner.Notice(context.Background(), "Sensitive input request rejected", "Codex requested a confidential answer. HQ stores message content, so it did not display or persist any request fields.", correlation)
			return nil, requestError("HQ has no non-persistent secret input channel"), true
		}
		if question.ID == "" || strings.TrimSpace(question.Question) == "" {
			r.noticeMalformed(questioner, request, "request_user_input contains an invalid question")
			return nil, requestError("HQ rejected malformed request_user_input question"), true
		}
	}

	pending := make([]*PendingQuestion, 0, len(params.Questions))
	for _, question := range params.Questions {
		spec := QuestionSpec{Body: question.Question, Details: userInputDetails(question), Correlation: correlation}
		published, err := questioner.Publish(ctx, spec)
		if err != nil {
			cancelPending(questioner, pending)
			return nil, requestError("HQ could not publish request_user_input: " + err.Error()), true
		}
		pending = append(pending, published)
	}
	answers := make(map[string]any, len(params.Questions))
	for index, question := range params.Questions {
		value, err := questioner.AwaitValidated(ctx, pending[index], userInputValidator(question))
		if err != nil {
			cancelPending(questioner, pending[index+1:])
			return nil, requestError("request_user_input was cancelled"), true
		}
		answers[question.ID] = map[string]any{"answers": []string{value.(string)}}
	}
	return map[string]any{"answers": answers}, nil, true
}

func (r *RequestRouter) handleCommandApproval(ctx context.Context, questioner *Questioner, request ServerRequest) (any, *RPCError, bool) {
	var params CommandApprovalParams
	if err := json.Unmarshal(request.Params, &params); err != nil || !validApprovalContext(params.ThreadID, params.TurnID, params.ItemID) || !validNetworkAmendments(params.ProposedNetworkPolicyAmendments) {
		r.noticeMalformed(questioner, request, "command approval payload is malformed")
		return commandDecision("cancel"), nil, true
	}
	correlation := RequestCorrelation{ThreadID: params.ThreadID, TurnID: params.TurnID, ItemID: params.ItemID, RequestID: requestIDText(request.ID)}
	if !r.matchesThread(questioner, params.ThreadID) {
		r.noticeMalformed(questioner, request, "command approval belongs to another thread")
		return commandDecision("cancel"), nil, true
	}
	spec := QuestionSpec{Body: commandApprovalBody(params), Details: commandApprovalDetails(params), Correlation: correlation}
	value, err := questioner.Ask(ctx, spec, commandApprovalValidator(params))
	if err != nil {
		return commandDecision("cancel"), nil, true
	}
	return map[string]any{"decision": value}, nil, true
}

func (r *RequestRouter) handleFileApproval(ctx context.Context, questioner *Questioner, request ServerRequest) (any, *RPCError, bool) {
	var params FileChangeApprovalParams
	if err := json.Unmarshal(request.Params, &params); err != nil || !validApprovalContext(params.ThreadID, params.TurnID, params.ItemID) {
		r.noticeMalformed(questioner, request, "file approval payload is malformed")
		return commandDecision("cancel"), nil, true
	}
	correlation := RequestCorrelation{ThreadID: params.ThreadID, TurnID: params.TurnID, ItemID: params.ItemID, RequestID: requestIDText(request.ID)}
	if !r.matchesThread(questioner, params.ThreadID) {
		r.noticeMalformed(questioner, request, "file approval belongs to another thread")
		return commandDecision("cancel"), nil, true
	}
	details := fmt.Sprintf("Reason: %s\nGrant root: %s\n\nLegal replies:\naccept — approve once\nacceptForSession — PERSISTS for matching files in this session\ndecline — deny and continue\ncancel — deny and interrupt the turn", valueOrNone(params.Reason), valueOrNone(params.GrantRoot))
	value, err := questioner.Ask(ctx, QuestionSpec{Body: "Codex requests approval for file changes", Details: details, Correlation: correlation}, exactDecisionValidator("accept", "acceptForSession", "decline", "cancel"))
	if err != nil {
		return commandDecision("cancel"), nil, true
	}
	return commandDecision(value.(string)), nil, true
}

func (r *RequestRouter) handlePermissionApproval(ctx context.Context, questioner *Questioner, request ServerRequest) (any, *RPCError, bool) {
	var params PermissionApprovalParams
	if err := json.Unmarshal(request.Params, &params); err != nil || !validApprovalContext(params.ThreadID, params.TurnID, params.ItemID) || !jsonObject(params.Permissions) {
		r.noticeMalformed(questioner, request, "permission approval payload is malformed")
		return deniedPermissions(), nil, true
	}
	correlation := RequestCorrelation{ThreadID: params.ThreadID, TurnID: params.TurnID, ItemID: params.ItemID, RequestID: requestIDText(request.ID)}
	if !r.matchesThread(questioner, params.ThreadID) {
		r.noticeMalformed(questioner, request, "permission approval belongs to another thread")
		return deniedPermissions(), nil, true
	}
	details := fmt.Sprintf("Working directory: %s\nReason: %s\nRequested permissions:\n%s\n\nLegal replies:\ngrantTurn — grant only for this turn\ngrantSession — PERSISTS for later turns in this session\ndecline — grant nothing", valueOrNone(params.CWD), valueOrNone(params.Reason), prettyJSON(params.Permissions))
	value, err := questioner.Ask(ctx, QuestionSpec{Body: "Codex requests additional permissions", Details: details, Correlation: correlation}, exactDecisionValidator("grantTurn", "grantSession", "decline"))
	if err != nil || value.(string) == "decline" {
		return deniedPermissions(), nil, true
	}
	scope := "turn"
	if value.(string) == "grantSession" {
		scope = "session"
	}
	return map[string]any{"permissions": params.Permissions, "scope": scope}, nil, true
}

func (r *RequestRouter) handleMCPElicitation(ctx context.Context, questioner *Questioner, request ServerRequest) (any, *RPCError, bool) {
	var params MCPElicitationParams
	if err := json.Unmarshal(request.Params, &params); err != nil || params.ThreadID == "" || params.ServerName == "" || params.Message == "" {
		r.noticeMalformed(questioner, request, "MCP elicitation payload is malformed")
		return cancelledElicitation(), nil, true
	}
	correlation := RequestCorrelation{ThreadID: params.ThreadID, TurnID: params.TurnID, RequestID: requestIDText(request.ID)}
	if !r.matchesThread(questioner, params.ThreadID) {
		r.noticeMalformed(questioner, request, "MCP elicitation belongs to another thread")
		return cancelledElicitation(), nil, true
	}
	switch params.Mode {
	case "form":
		if _, err := validateMCPFormSchema(params.RequestedSchema); err != nil {
			_ = questioner.Notice(context.Background(), "Unsupported MCP form", "HQ rejected an invalid typed MCP form schema.", correlation)
			return cancelledElicitation(), nil, true
		}
		details := fmt.Sprintf("MCP server: %s\nSchema:\n%s\n\nLegal replies:\naccept {\"field\":\"value\"} — validate and submit the JSON object\ndecline — decline the request\ncancel — cancel the request", params.ServerName, prettyJSON(params.RequestedSchema))
		value, err := questioner.Ask(ctx, QuestionSpec{Body: params.Message, Details: details, Correlation: correlation}, mcpFormAnswerValidator(params.RequestedSchema))
		if err != nil {
			return cancelledElicitation(), nil, true
		}
		return value, nil, true
	case "url":
		if params.URL == "" || params.ElicitationID == "" {
			return cancelledElicitation(), nil, true
		}
		details := fmt.Sprintf("MCP server: %s\nURL: %s\nElicitation ID: %s\n\nLegal replies: accept, decline, cancel", params.ServerName, params.URL, params.ElicitationID)
		value, err := questioner.Ask(ctx, QuestionSpec{Body: params.Message, Details: details, Correlation: correlation}, exactDecisionValidator("accept", "decline", "cancel"))
		if err != nil {
			return cancelledElicitation(), nil, true
		}
		return map[string]any{"action": value.(string), "content": nil}, nil, true
	default:
		_ = questioner.Notice(context.Background(), "Unsupported MCP elicitation", "HQ rejected MCP elicitation mode "+params.Mode+" because it cannot validate that schema safely.", correlation)
		return cancelledElicitation(), nil, true
	}
}

func (r *RequestRouter) matchesThread(questioner *Questioner, threadID string) bool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return r.questioner == questioner && questioner != nil && questioner.Mailbox.ID != "" && threadID != "" && questioner.CorrelationSessionID() == threadID
}

func (r *RequestRouter) noticeMalformed(questioner *Questioner, request ServerRequest, reason string) {
	_ = questioner.Notice(context.Background(), "Codex request rejected", reason, RequestCorrelation{RequestID: requestIDText(request.ID)})
}

func requestIDText(id json.RawMessage) string {
	return string(id)
}

func requestError(message string) *RPCError {
	return &RPCError{Code: -32000, Message: message}
}

func validApprovalContext(threadID, turnID, itemID string) bool {
	return threadID != "" && turnID != "" && itemID != ""
}

func cancelPending(questioner *Questioner, pending []*PendingQuestion) {
	for _, question := range pending {
		questioner.Cancel(question)
	}
}

func userInputDetails(question RequestUserInputQuestion) string {
	var details strings.Builder
	fmt.Fprintf(&details, "Question ID: %s\nLabel: %s\nFree-form answer allowed: %t", question.ID, question.Header, question.IsOther || len(question.Options) == 0)
	if len(question.Options) > 0 {
		details.WriteString("\nOptions:")
		for _, option := range question.Options {
			fmt.Fprintf(&details, "\n- %s — %s", option.Label, option.Description)
		}
	}
	return details.String()
}

func userInputValidator(question RequestUserInputQuestion) AnswerValidator {
	return func(answer string) (any, error) {
		if answer == "" {
			return nil, errors.New("answer must not be empty")
		}
		for _, option := range question.Options {
			if answer == option.Label {
				return answer, nil
			}
		}
		if question.IsOther || len(question.Options) == 0 {
			return answer, nil
		}
		return nil, errors.New("answer must exactly match one listed option")
	}
}

func exactDecisionValidator(decisions ...string) AnswerValidator {
	return func(answer string) (any, error) {
		for _, decision := range decisions {
			if answer == decision {
				return answer, nil
			}
		}
		return nil, fmt.Errorf("reply must exactly match one of: %s", strings.Join(decisions, ", "))
	}
}

func commandApprovalValidator(params CommandApprovalParams) AnswerValidator {
	return func(answer string) (any, error) {
		switch answer {
		case "accept", "acceptForSession", "decline", "cancel":
			return answer, nil
		case "acceptWithExecpolicyAmendment":
			if len(params.ProposedExecpolicyAmendment) == 0 {
				return nil, errors.New("no execpolicy amendment was proposed")
			}
			return map[string]any{"acceptWithExecpolicyAmendment": map[string]any{"execpolicy_amendment": params.ProposedExecpolicyAmendment}}, nil
		default:
			for index, amendment := range params.ProposedNetworkPolicyAmendments {
				if answer == fmt.Sprintf("applyNetworkPolicyAmendment:%d", index+1) {
					return map[string]any{"applyNetworkPolicyAmendment": map[string]any{"network_policy_amendment": amendment}}, nil
				}
			}
			return nil, errors.New("reply must exactly match one legal approval choice")
		}
	}
}

func commandApprovalBody(params CommandApprovalParams) string {
	if params.NetworkApprovalContext != nil {
		return "Codex requests managed network access"
	}
	return "Codex requests command approval"
}

func commandApprovalDetails(params CommandApprovalParams) string {
	var details strings.Builder
	fmt.Fprintf(&details, "Command: %s\nWorking directory: %s\nReason: %s", valueOrNone(params.Command), valueOrNone(params.CWD), valueOrNone(params.Reason))
	if params.NetworkApprovalContext != nil {
		fmt.Fprintf(&details, "\nNetwork target: %s via %s", params.NetworkApprovalContext.Host, params.NetworkApprovalContext.Protocol)
	}
	if len(params.CommandActions) > 0 && string(params.CommandActions) != "null" {
		fmt.Fprintf(&details, "\nCommand actions:\n%s", prettyJSON(params.CommandActions))
	}
	details.WriteString("\n\nLegal replies:\naccept — approve once\nacceptForSession — PERSISTS in the session approval cache\ndecline — deny and continue\ncancel — deny and interrupt the turn")
	if len(params.ProposedExecpolicyAmendment) > 0 {
		fmt.Fprintf(&details, "\nacceptWithExecpolicyAmendment — PERSISTS this exact policy amendment: %s", strings.Join(params.ProposedExecpolicyAmendment, " "))
	}
	for index, amendment := range params.ProposedNetworkPolicyAmendments {
		fmt.Fprintf(&details, "\napplyNetworkPolicyAmendment:%d — PERSISTS exact network rule %s", index+1, networkAmendmentSummary(amendment))
	}
	return details.String()
}

func commandDecision(decision any) map[string]any {
	return map[string]any{"decision": decision}
}

func deniedPermissions() map[string]any {
	return map[string]any{"permissions": map[string]any{}, "scope": "turn"}
}

func cancelledElicitation() map[string]any {
	return map[string]any{"action": "cancel", "content": nil}
}

func mcpFormAnswerValidator(schema json.RawMessage) AnswerValidator {
	return func(answer string) (any, error) {
		switch answer {
		case "decline", "cancel":
			return map[string]any{"action": answer, "content": nil}, nil
		}
		if !strings.HasPrefix(answer, "accept ") {
			return nil, errors.New("reply must be decline, cancel, or accept followed by one JSON object")
		}
		content, err := validateMCPForm(schema, strings.TrimSpace(strings.TrimPrefix(answer, "accept ")))
		if err != nil {
			return nil, err
		}
		return map[string]any{"action": "accept", "content": content}, nil
	}
}

func valueOrNone(value string) string {
	if strings.TrimSpace(value) == "" {
		return "(not provided)"
	}
	return value
}

func jsonObject(raw json.RawMessage) bool {
	var object map[string]json.RawMessage
	return len(raw) != 0 && json.Unmarshal(raw, &object) == nil && object != nil
}

func validNetworkAmendments(amendments []json.RawMessage) bool {
	for _, amendment := range amendments {
		if !jsonObject(amendment) {
			return false
		}
	}
	return true
}

func networkAmendmentSummary(raw json.RawMessage) string {
	var amendment NetworkPolicyAmendment
	if json.Unmarshal(raw, &amendment) == nil && (amendment.Action != "" || amendment.Host != "") {
		return strings.TrimSpace(amendment.Action + " " + amendment.Host)
	}
	return string(raw)
}
