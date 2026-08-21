package codexbridge

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/store"
)

type requestResult struct {
	result  any
	rpcErr  *RPCError
	handled bool
}

type requestTestFixture struct {
	dispatcherFixture
	router *RequestRouter
	seen   map[string]bool
	t      *testing.T
}

func newRequestTestFixture(t *testing.T) *requestTestFixture {
	t.Helper()
	fixture := newDispatcherFixture(t)
	router := NewRequestRouter(fixture.store, fixture.replies)
	router.Bind(fixture.thread, fixture.agent, model.RepositoryContext{Directory: "/work/repo"}, nil, nil, 2*time.Millisecond)
	return &requestTestFixture{dispatcherFixture: fixture, router: router, seen: make(map[string]bool), t: t}
}

func (f *requestTestFixture) call(method string, params any) <-chan requestResult {
	return f.callContext(context.Background(), method, params)
}

func (f *requestTestFixture) callContext(ctx context.Context, method string, params any) <-chan requestResult {
	raw, _ := json.Marshal(params)
	done := make(chan requestResult, 1)
	go func() {
		result, rpcErr, handled := f.router.HandleRequest(ctx, ServerRequest{ID: json.RawMessage(`"request-1"`), Method: method, Params: raw})
		done <- requestResult{result: result, rpcErr: rpcErr, handled: handled}
	}()
	return done
}

func (f *requestTestFixture) question(t *testing.T, body string) model.Message {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		messages, err := f.store.List(context.Background(), model.Filter{RecipientMailboxID: model.HumanMailboxID, Limit: 100})
		if err != nil {
			t.Fatal(err)
		}
		for _, message := range messages {
			if !f.seen[message.ID] && message.Body == body {
				f.seen[message.ID] = true
				return message
			}
		}
		time.Sleep(2 * time.Millisecond)
	}
	f.t.Fatalf("question %q was not published", body)
	return model.Message{}
}

func (f *requestTestFixture) reply(t *testing.T, question model.Message, body string) model.Message {
	t.Helper()
	id, err := uuid.NewV7()
	if err != nil {
		t.Fatal(err)
	}
	reply := model.Message{
		ID: id.String(), Context: question.Context, SenderMailboxID: model.HumanMailboxID,
		RecipientMailboxID: f.agent.ID, Body: body, CreatedAt: time.Now().UTC(),
	}
	if err := f.store.Reply(context.Background(), question.ID, reply); err != nil {
		t.Fatal(err)
	}
	claimed, err := f.replies.ClaimOne(context.Background(), f.store, f.agent.ID)
	if err != nil || !claimed {
		t.Fatalf("reply claim = %t, %v", claimed, err)
	}
	return reply
}

func receiveRequestResult(t *testing.T, done <-chan requestResult) requestResult {
	t.Helper()
	select {
	case result := <-done:
		return result
	case <-time.After(2 * time.Second):
		t.Fatal("server request did not finish")
		return requestResult{}
	}
}

func TestRequestUserInputPublishesMultipleCorrelatedQuestions(t *testing.T) {
	fixture := newRequestTestFixture(t)
	params := RequestUserInputParams{
		ThreadID: fixture.thread, TurnID: "turn-1", ItemID: "item-1", IsBlocking: true,
		Questions: []RequestUserInputQuestion{
			{ID: "color", Header: "Color", Question: "Choose a color", Options: []RequestUserInputOption{{Label: "Red", Description: "Warm"}, {Label: "Blue", Description: "Cool"}}},
			{ID: "note", Header: "Note", Question: "Add a note", IsOther: true},
		},
	}
	done := fixture.call(requestUserInputMethod, params)
	color := fixture.question(t, "Choose a color")
	note := fixture.question(t, "Add a note")
	if !strings.Contains(color.Details, "Question ID: color") || !strings.Contains(color.Details, "Red — Warm") || !strings.Contains(color.Details, "Codex request: \"request-1\"") || !strings.Contains(color.Details, "HQ message: "+color.ID) {
		t.Fatalf("color details = %q", color.Details)
	}
	fixture.reply(t, note, "some context")
	fixture.reply(t, color, "Blue")
	result := receiveRequestResult(t, done)
	if result.rpcErr != nil || !result.handled {
		t.Fatalf("result = %#v", result)
	}
	raw, _ := json.Marshal(result.result)
	var response struct {
		Answers map[string]struct {
			Answers []string `json:"answers"`
		} `json:"answers"`
	}
	if json.Unmarshal(raw, &response) != nil || response.Answers["color"].Answers[0] != "Blue" || response.Answers["note"].Answers[0] != "some context" {
		t.Fatalf("response = %s", raw)
	}
}

func TestRequestUserInputRepromptsInvalidOption(t *testing.T) {
	fixture := newRequestTestFixture(t)
	params := RequestUserInputParams{ThreadID: fixture.thread, TurnID: "turn-1", ItemID: "item-1", Questions: []RequestUserInputQuestion{{ID: "choice", Header: "Choice", Question: "Choose", Options: []RequestUserInputOption{{Label: "A", Description: "First"}}}}}
	done := fixture.call(requestUserInputMethod, params)
	question := fixture.question(t, "Choose")
	fixture.reply(t, question, "anything")
	reprompt := fixture.question(t, "Invalid reply; please answer again: Choose")
	if !strings.Contains(reprompt.Details, "must exactly match") {
		t.Fatalf("reprompt details = %q", reprompt.Details)
	}
	fixture.reply(t, reprompt, "A")
	result := receiveRequestResult(t, done)
	if result.rpcErr != nil {
		t.Fatal(result.rpcErr)
	}
}

func TestRequestUserInputSecretIsNeverPersisted(t *testing.T) {
	fixture := newRequestTestFixture(t)
	params := RequestUserInputParams{ThreadID: fixture.thread, TurnID: "turn-secret", ItemID: "item-secret", Questions: []RequestUserInputQuestion{{ID: "password", Header: "Password", Question: "Enter swordfish", IsSecret: true, Options: []RequestUserInputOption{{Label: "swordfish", Description: "the secret"}}}}}
	result := receiveRequestResult(t, fixture.call(requestUserInputMethod, params))
	if result.rpcErr == nil || !strings.Contains(result.rpcErr.Message, "secret") {
		t.Fatalf("result = %#v", result)
	}
	messages, err := fixture.store.List(context.Background(), model.Filter{RecipientMailboxID: model.HumanMailboxID, Limit: 100})
	if err != nil || len(messages) != 1 {
		t.Fatalf("messages = %#v, %v", messages, err)
	}
	persisted := messages[0].Body + "\n" + messages[0].Details
	if !strings.Contains(messages[0].Details, "Kind: notice") {
		t.Fatalf("notice details = %q", messages[0].Details)
	}
	for _, secretField := range []string{"Enter swordfish", "Password", "password", "the secret"} {
		if strings.Contains(persisted, secretField) {
			t.Fatalf("secret field %q was persisted in %q", secretField, persisted)
		}
	}
}

func TestCommandApprovalLegalDecisions(t *testing.T) {
	for _, decision := range []string{"accept", "acceptForSession", "decline", "cancel"} {
		t.Run(decision, func(t *testing.T) {
			fixture := newRequestTestFixture(t)
			params := CommandApprovalParams{ThreadID: fixture.thread, TurnID: "turn", ItemID: "item", Command: "go test ./...", CWD: "/work/repo", Reason: "verify"}
			done := fixture.call(commandApprovalMethod, params)
			question := fixture.question(t, "Codex requests command approval")
			if decision == "acceptForSession" && !strings.Contains(question.Details, "PERSISTS") {
				t.Fatalf("details = %q", question.Details)
			}
			fixture.reply(t, question, decision)
			result := receiveRequestResult(t, done)
			response := result.result.(map[string]any)
			if result.rpcErr != nil || response["decision"] != decision {
				t.Fatalf("result = %#v", result)
			}
		})
	}
}

func TestCommandApprovalReturnsExactProposedAmendments(t *testing.T) {
	tests := []struct {
		name   string
		answer string
		params CommandApprovalParams
		want   string
	}{
		{name: "execpolicy", answer: "acceptWithExecpolicyAmendment", params: CommandApprovalParams{ProposedExecpolicyAmendment: []string{"prefix_rule", "go", "test"}}, want: `{"decision":{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["prefix_rule","go","test"]}}}`},
		{name: "network", answer: "applyNetworkPolicyAmendment:1", params: CommandApprovalParams{ProposedNetworkPolicyAmendments: []json.RawMessage{json.RawMessage(`{"action":"allow","host":"example.com","ports":[443]}`)}}, want: `{"decision":{"applyNetworkPolicyAmendment":{"network_policy_amendment":{"action":"allow","host":"example.com","ports":[443]}}}}`},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			fixture := newRequestTestFixture(t)
			test.params.ThreadID, test.params.TurnID, test.params.ItemID = fixture.thread, "turn", "item"
			done := fixture.call(commandApprovalMethod, test.params)
			question := fixture.question(t, "Codex requests command approval")
			if !strings.Contains(question.Details, "PERSISTS") {
				t.Fatalf("details = %q", question.Details)
			}
			fixture.reply(t, question, test.answer)
			result := receiveRequestResult(t, done)
			raw, _ := json.Marshal(result.result)
			if string(raw) != test.want {
				t.Fatalf("response = %s", raw)
			}
		})
	}
}

func TestApprovalInvalidReplyRepromptsAndHQCancelFailsClosed(t *testing.T) {
	fixture := newRequestTestFixture(t)
	params := CommandApprovalParams{ThreadID: fixture.thread, TurnID: "turn", ItemID: "item", Command: "make release"}
	done := fixture.call(commandApprovalMethod, params)
	question := fixture.question(t, "Codex requests command approval")
	fixture.reply(t, question, "sure")
	reprompt := fixture.question(t, "Invalid reply; please answer again: Codex requests command approval")
	if err := fixture.store.Archive(context.Background(), reprompt.ID); err != nil {
		t.Fatal(err)
	}
	result := receiveRequestResult(t, done)
	if result.result.(map[string]any)["decision"] != "cancel" {
		t.Fatalf("result = %#v", result)
	}
}

func TestFileApprovalAndPermissionScopes(t *testing.T) {
	fixture := newRequestTestFixture(t)
	fileDone := fixture.call(fileApprovalMethod, FileChangeApprovalParams{ThreadID: fixture.thread, TurnID: "turn-file", ItemID: "item-file", GrantRoot: "/work"})
	fileQuestion := fixture.question(t, "Codex requests approval for file changes")
	fixture.reply(t, fileQuestion, "acceptForSession")
	fileResult := receiveRequestResult(t, fileDone)
	if fileResult.result.(map[string]any)["decision"] != "acceptForSession" {
		t.Fatalf("file result = %#v", fileResult)
	}

	permissions := json.RawMessage(`{"network":{"enabled":true},"fileSystem":{"write":["/work/repo"]}}`)
	for _, test := range []struct {
		answer string
		scope  string
		grant  bool
	}{{"grantTurn", "turn", true}, {"grantSession", "session", true}, {"decline", "turn", false}} {
		done := fixture.call(permissionMethod, PermissionApprovalParams{ThreadID: fixture.thread, TurnID: "turn-perm", ItemID: "item-perm-" + test.answer, CWD: "/work/repo", Permissions: permissions})
		question := fixture.question(t, "Codex requests additional permissions")
		fixture.reply(t, question, test.answer)
		result := receiveRequestResult(t, done)
		raw, _ := json.Marshal(result.result)
		if !strings.Contains(string(raw), `"scope":"`+test.scope+`"`) {
			t.Fatalf("response = %s", raw)
		}
		if test.grant != strings.Contains(string(raw), `"enabled":true`) {
			t.Fatalf("response = %s", raw)
		}
	}
}

func TestPermissionHQCancelGrantsNothing(t *testing.T) {
	fixture := newRequestTestFixture(t)
	done := fixture.call(permissionMethod, PermissionApprovalParams{ThreadID: fixture.thread, TurnID: "turn", ItemID: "item", CWD: "/work", Permissions: json.RawMessage(`{"network":{"enabled":true}}`)})
	question := fixture.question(t, "Codex requests additional permissions")
	if err := fixture.store.Archive(context.Background(), question.ID); err != nil {
		t.Fatal(err)
	}
	result := receiveRequestResult(t, done)
	raw, _ := json.Marshal(result.result)
	if string(raw) != `{"permissions":{},"scope":"turn"}` {
		t.Fatalf("response = %s", raw)
	}
}

func TestPermissionRejectsNonObjectProfile(t *testing.T) {
	for _, permissions := range []json.RawMessage{json.RawMessage(`null`), json.RawMessage(`[]`), json.RawMessage(`"all"`)} {
		fixture := newRequestTestFixture(t)
		result := receiveRequestResult(t, fixture.call(permissionMethod, PermissionApprovalParams{ThreadID: fixture.thread, TurnID: "turn", ItemID: "item", Permissions: permissions}))
		raw, _ := json.Marshal(result.result)
		if result.rpcErr != nil || string(raw) != `{"permissions":{},"scope":"turn"}` {
			t.Fatalf("permissions %s result = %#v", permissions, result)
		}
	}
}

func TestRequestCancellationReleasesReplyClaimedAfterWaiterStops(t *testing.T) {
	fixture := newRequestTestFixture(t)
	ctx, cancel := context.WithCancel(context.Background())
	done := fixture.callContext(ctx, fileApprovalMethod, FileChangeApprovalParams{ThreadID: fixture.thread, TurnID: "turn", ItemID: "item"})
	question := fixture.question(t, "Codex requests approval for file changes")

	replyID, err := uuid.NewV7()
	if err != nil {
		t.Fatal(err)
	}
	reply := model.Message{ID: replyID.String(), Context: question.Context, SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: fixture.agent.ID, Body: "accept", CreatedAt: time.Now().UTC()}
	if err := fixture.store.Reply(context.Background(), question.ID, reply); err != nil {
		t.Fatal(err)
	}
	cancel()
	result := receiveRequestResult(t, done)
	if result.rpcErr != nil || result.result.(map[string]any)["decision"] != "cancel" {
		t.Fatalf("result = %#v", result)
	}
	claimed, err := fixture.replies.ClaimOne(context.Background(), fixture.store, fixture.agent.ID)
	if err != nil || claimed {
		t.Fatalf("reserved claim after cancellation = %t, %v", claimed, err)
	}
	token := "019c0000-0000-7000-8000-000000000119"
	message, err := fixture.store.Claim(context.Background(), store.Claim{MessageID: reply.ID}, token)
	if err != nil || message.ID != reply.ID {
		t.Fatalf("released reply claim = %#v, %v", message, err)
	}
	_ = fixture.store.Release(context.Background(), reply.ID, token)
}

func TestMCPElicitationFormAndURLActions(t *testing.T) {
	fixture := newRequestTestFixture(t)
	schema := json.RawMessage(`{"type":"object","properties":{"name":{"type":"string","minLength":2},"count":{"type":"integer","minimum":1}},"required":["name"]}`)
	formDone := fixture.call(mcpElicitationMethod, MCPElicitationParams{ThreadID: fixture.thread, TurnID: "turn", ServerName: "forms", Mode: "form", Message: "Complete the form", RequestedSchema: schema})
	form := fixture.question(t, "Complete the form")
	fixture.reply(t, form, `accept {"name":"Ada","count":2}`)
	formResult := receiveRequestResult(t, formDone)
	raw, _ := json.Marshal(formResult.result)
	if string(raw) != `{"action":"accept","content":{"count":2,"name":"Ada"}}` {
		t.Fatalf("form response = %s", raw)
	}
	for _, action := range []string{"decline", "cancel"} {
		done := fixture.call(mcpElicitationMethod, MCPElicitationParams{ThreadID: fixture.thread, TurnID: "turn", ServerName: "forms", Mode: "form", Message: "Complete optional form", RequestedSchema: schema})
		question := fixture.question(t, "Complete optional form")
		fixture.reply(t, question, action)
		result := receiveRequestResult(t, done)
		if result.result.(map[string]any)["action"] != action || result.result.(map[string]any)["content"] != nil {
			t.Fatalf("%s form result = %#v", action, result)
		}
	}

	for _, action := range []string{"accept", "decline", "cancel"} {
		done := fixture.call(mcpElicitationMethod, MCPElicitationParams{ThreadID: fixture.thread, TurnID: "turn", ServerName: "oauth", Mode: "url", Message: "Open authorization", URL: "https://example.com/auth", ElicitationID: "elicit-" + action})
		question := fixture.question(t, "Open authorization")
		fixture.reply(t, question, action)
		result := receiveRequestResult(t, done)
		if result.result.(map[string]any)["action"] != action {
			t.Fatalf("%s result = %#v", action, result)
		}
	}
}

func TestMCPInvalidFormRepromptsAndOpenAIFormFailsClosed(t *testing.T) {
	fixture := newRequestTestFixture(t)
	schema := json.RawMessage(`{"type":"object","properties":{"count":{"type":"integer","minimum":1}},"required":["count"]}`)
	done := fixture.call(mcpElicitationMethod, MCPElicitationParams{ThreadID: fixture.thread, TurnID: "turn", ServerName: "forms", Mode: "form", Message: "Count", RequestedSchema: schema})
	question := fixture.question(t, "Count")
	fixture.reply(t, question, `accept {"count":0}`)
	reprompt := fixture.question(t, "Invalid reply; please answer again: Count")
	if !strings.Contains(reprompt.Details, "at least") {
		t.Fatalf("reprompt = %#v", reprompt)
	}
	fixture.reply(t, reprompt, "decline")
	result := receiveRequestResult(t, done)
	if result.result.(map[string]any)["action"] != "decline" {
		t.Fatalf("result = %#v", result)
	}

	unsupported := receiveRequestResult(t, fixture.call(mcpElicitationMethod, MCPElicitationParams{ThreadID: fixture.thread, TurnID: "turn", ServerName: "forms", Mode: "openai/form", Message: "Unsafe extension", RequestedSchema: schema}))
	raw, _ := json.Marshal(unsupported.result)
	if string(raw) != `{"action":"cancel","content":null}` {
		t.Fatalf("unsupported result = %s", raw)
	}
}

func TestUnsupportedAndMalformedRequestsFailClosedWithDiagnostic(t *testing.T) {
	fixture := newRequestTestFixture(t)
	unknown := receiveRequestResult(t, fixture.call("future/request", map[string]any{}))
	if unknown.handled {
		t.Fatalf("unknown request = %#v", unknown)
	}
	malformed := receiveRequestResult(t, fixture.call(commandApprovalMethod, map[string]any{"threadId": fixture.thread}))
	if !malformed.handled || malformed.result.(map[string]any)["decision"] != "cancel" {
		t.Fatalf("malformed result = %#v", malformed)
	}
	messages, err := fixture.store.List(context.Background(), model.Filter{RecipientMailboxID: model.HumanMailboxID, Limit: 100})
	if err != nil {
		t.Fatal(err)
	}
	var diagnostics int
	for _, message := range messages {
		if strings.Contains(message.Body, "Unsupported") || strings.Contains(message.Body, "rejected") {
			diagnostics++
		}
	}
	if diagnostics < 2 {
		t.Fatalf("diagnostics = %d, messages=%s", diagnostics, fmt.Sprint(messages))
	}
}
