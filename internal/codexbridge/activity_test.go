package codexbridge

import (
	"context"
	"encoding/json"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/harness"
)

func TestAdapterMapsBoundedActivityAndTreatsCompletionAsAuthoritative(t *testing.T) {
	instance := newActivityTestInstance(t, "activity-session")
	handler := &adapterNotificationHandler{instance: instance}
	send := func(method, params string) {
		t.Helper()
		handler.HandleNotification(context.Background(), Notification{Method: method, Params: json.RawMessage(params)})
	}

	send("turn/plan/updated", `{"threadId":"activity-session","turnId":"turn-activity","explanation":"Execution plan","plan":[{"step":"inspect","status":"completed"},{"step":"implement","status":"inProgress"}]}`)
	send("turn/diff/updated", `{"threadId":"activity-session","turnId":"turn-activity","diff":"diff --git a/a b/a"}`)
	send("item/started", `{"threadId":"activity-session","turnId":"turn-activity","item":{"type":"commandExecution","id":"command-1","command":"go test ./...","status":"inProgress"}}`)
	send("item/commandExecution/outputDelta", `{"threadId":"activity-session","turnId":"turn-activity","itemId":"command-1","delta":"partial output"}`)
	oversizedOutput, _ := json.Marshal(map[string]any{
		"threadId": "activity-session", "turnId": "turn-activity",
		"item": map[string]any{"type": "commandExecution", "id": "command-1", "command": "go test ./...", "status": "completed", "aggregatedOutput": strings.Repeat("x", adapterCommandTextBytes+512), "exitCode": 0},
	})
	handler.HandleNotification(context.Background(), Notification{Method: "item/completed", Params: oversizedOutput})
	send("item/completed", `{"threadId":"activity-session","turnId":"turn-activity","item":{"type":"fileChange","id":"file-1","status":"failed","changes":[{"path":"main.go","kind":{"type":"update","move_path":null},"diff":"@@ -1 +1 @@"},{"path":"new.go","kind":{"type":"add"},"diff":"+package main"}]}}`)
	send("item/mcpToolCall/progress", `{"threadId":"activity-session","turnId":"turn-activity","itemId":"tool-1","message":"Searching docs"}`)
	send("item/completed", `{"threadId":"activity-session","turnId":"turn-activity","item":{"type":"mcpToolCall","id":"tool-1","server":"docs","tool":"search","arguments":{"q":"activity"},"status":"failed","error":{"message":"offline"},"result":null}}`)
	send("item/completed", `{"threadId":"activity-session","turnId":"turn-activity","item":{"type":"dynamicToolCall","id":"tool-2","tool":"analyze","arguments":{"path":"."},"status":"completed","contentItems":[{"type":"text","text":"done"}],"success":true}}`)
	send("item/completed", `{"threadId":"activity-session","turnId":"turn-activity","item":{"type":"collabAgentToolCall","id":"tool-3","tool":"wait","status":"completed","receiverThreadIds":["thread-child"]}}`)
	send("item/completed", `{"threadId":"activity-session","turnId":"turn-activity","item":{"type":"webSearch","id":"tool-4","query":"Codex activity"}}`)
	send("item/completed", `{"threadId":"activity-session","turnId":"turn-activity","item":{"type":"plan","id":"plan-1","text":"Authoritative completed plan"}}`)

	expected := []harness.EventPayload{
		harness.PlanEvent{Text: "Execution plan\n- [x] inspect\n- [~] implement"},
		harness.DiffEvent{Text: "diff --git a/a b/a"},
		harness.ProgressEvent{Message: "Running command: go test ./..."},
		harness.ProgressEvent{Message: "partial output"},
		harness.CommandEvent{Command: "go test ./...", Output: strings.Repeat("x", adapterCommandTextBytes), ExitCode: intPointer(0), Status: harness.OperationCompleted},
		harness.FileChangeEvent{Path: "main.go (+1 more)", Summary: "update main.go\n@@ -1 +1 @@\n\nadd new.go\n+package main", Status: harness.OperationFailed},
		harness.ProgressEvent{Message: "Searching docs"},
		harness.ToolEvent{Name: "docs/search", Summary: "Arguments: {\"q\":\"activity\"}\nError: offline", Status: harness.OperationFailed},
		harness.ToolEvent{Name: "analyze", Summary: "Arguments: {\"path\":\".\"}\nResult: [{\"type\":\"text\",\"text\":\"done\"}]", Status: harness.OperationCompleted},
		harness.ToolEvent{Name: "collab/wait", Summary: "Receiver threads: thread-child", Status: harness.OperationCompleted},
		harness.ToolEvent{Name: "web search", Summary: "Codex activity", Status: harness.OperationCompleted},
		harness.PlanEvent{Text: "Authoritative completed plan"},
	}
	for index, payload := range expected {
		select {
		case event := <-instance.Events():
			if event.Sequence != uint64(index+1) || event.Operation != "turn-activity" || !reflect.DeepEqual(event.Payload, payload) {
				t.Fatalf("event %d = %#v, payload %#v", index+1, event, event.Payload)
			}
		case <-time.After(time.Second):
			t.Fatalf("timed out waiting for activity event %d", index+1)
		}
	}
}

func TestAdapterIgnoresMalformedAdditiveAndReasoningActivity(t *testing.T) {
	instance := newActivityTestInstance(t, "activity-session")
	handler := &adapterNotificationHandler{instance: instance}
	for _, notification := range []Notification{
		{Method: "turn/completed", Params: json.RawMessage(`{"threadId":"activity-session","turn":{"id":"turn","status":"futureStatus"}}`)},
		{Method: "turn/plan/updated", Params: json.RawMessage(`{"threadId":"other","turnId":"turn","plan":[{"step":"leak","status":"completed"}]}`)},
		{Method: "turn/diff/updated", Params: json.RawMessage(`{"threadId":"activity-session","diff":"missing turn"}`)},
		{Method: "item/completed", Params: json.RawMessage(`{"threadId":"activity-session","turnId":"turn","item":{"type":"reasoning","id":"reasoning","summary":["secret"]}}`)},
		{Method: "item/reasoning/textDelta", Params: json.RawMessage(`{"threadId":"activity-session","turnId":"turn","itemId":"reasoning","delta":"secret"}`)},
		{Method: "item/agentMessage/delta", Params: json.RawMessage(`{"threadId":"activity-session","turnId":"turn","itemId":"answer","delta":"raw model response"}`)},
		{Method: "item/completed", Params: json.RawMessage(`{"threadId":"activity-session","turnId":"turn","item":{"type":"futureTool","id":"future","newField":true}}`)},
		{Method: "future/additive", Params: json.RawMessage(`{"anything":true}`)},
		{Method: "item/completed", Params: json.RawMessage(`not-json`)},
	} {
		handler.HandleNotification(context.Background(), notification)
	}
	select {
	case event := <-instance.Events():
		t.Fatalf("excluded notification emitted %#v", event)
	default:
	}
}

func newActivityTestInstance(t *testing.T, sessionID harness.SessionID) *codexInstance {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	instance := &codexInstance{ctx: ctx, cancel: cancel, events: make(chan harness.Event, 32), threadState: NewThreadState(string(sessionID))}
	instance.session = &codexSession{instance: instance, identity: harness.SessionIdentity{Provider: CodexProviderID, ID: sessionID}}
	return instance
}

func intPointer(value int) *int { return &value }
