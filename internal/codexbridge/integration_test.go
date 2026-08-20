package codexbridge

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/store"
)

type scriptedAppServer struct {
	process   *fakeProcess
	threadID  string
	calls     chan rpcEnvelope
	responses chan rpcEnvelope
	done      chan error
	writeMu   sync.Mutex
	turns     int
}

func newScriptedAppServer(process *fakeProcess, threadID string) *scriptedAppServer {
	server := &scriptedAppServer{
		process: process, threadID: threadID, calls: make(chan rpcEnvelope, 64),
		responses: make(chan rpcEnvelope, 16), done: make(chan error, 1),
	}
	go server.run()
	return server
}

func (s *scriptedAppServer) run() {
	scanner := bufio.NewScanner(s.process.serverInput)
	for scanner.Scan() {
		var envelope rpcEnvelope
		if err := json.Unmarshal(scanner.Bytes(), &envelope); err != nil {
			s.done <- err
			s.process.finish(nil)
			return
		}
		if envelope.Method == "" {
			s.responses <- envelope
			continue
		}
		s.calls <- envelope
		switch envelope.Method {
		case "initialize":
			s.result(envelope.ID, `{}`)
		case "initialized":
		case "thread/start", "thread/resume":
			s.result(envelope.ID, `{"thread":{"id":"`+s.threadID+`","turns":[]}}`)
		case "turn/start":
			s.turns++
			s.result(envelope.ID, fmt.Sprintf(`{"turn":{"id":"turn-%d","status":"inProgress"}}`, s.turns))
		case "turn/steer":
			var params TurnSteerParams
			if err := json.Unmarshal(envelope.Params, &params); err != nil {
				s.done <- err
				s.process.finish(nil)
				return
			}
			s.result(envelope.ID, `{"turnId":"`+params.ExpectedTurnID+`"}`)
		case "thread/read":
			s.result(envelope.ID, `{"thread":{"id":"`+s.threadID+`","turns":[]}}`)
		default:
			s.done <- fmt.Errorf("unexpected client method %q", envelope.Method)
			s.process.finish(nil)
			return
		}
	}
	s.process.finish(nil)
	s.done <- scanner.Err()
}

func (s *scriptedAppServer) result(id json.RawMessage, result string) {
	s.sendRaw(`{"id":` + string(id) + `,"result":` + result + `}`)
}

func (s *scriptedAppServer) sendRaw(raw string) {
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	_, _ = io.WriteString(s.process.serverOutput, raw+"\n")
}

func (s *scriptedAppServer) nextCall(t *testing.T, method string) rpcEnvelope {
	t.Helper()
	deadline := time.After(3 * time.Second)
	for {
		select {
		case call := <-s.calls:
			if call.Method == method {
				return call
			}
		case err := <-s.done:
			t.Fatalf("scripted app-server stopped before %s: %v", method, err)
		case <-deadline:
			t.Fatalf("timed out waiting for %s", method)
		}
	}
}

func (s *scriptedAppServer) nextResponse(t *testing.T, id string) rpcEnvelope {
	t.Helper()
	deadline := time.After(3 * time.Second)
	for {
		select {
		case response := <-s.responses:
			if string(response.ID) == `"`+id+`"` {
				return response
			}
		case err := <-s.done:
			t.Fatalf("scripted app-server stopped before response %s: %v", id, err)
		case <-deadline:
			t.Fatalf("timed out waiting for response %s", id)
		}
	}
}

func runFullSessionBridge(t *testing.T, ctx context.Context, fixture dispatcherFixture, process *fakeProcess, ledgerPath, resumeThreadID string) <-chan error {
	t.Helper()
	done := make(chan error, 1)
	go func() {
		done <- Run(ctx, Options{
			Directory: "/work/repo", ResumeThreadID: resumeThreadID, Starter: fakeStarter{process}, Store: fixture.store,
			Repository: model.RepositoryContext{Directory: "/work/repo"}, Stderr: io.Discard,
			LedgerPath: ledgerPath, PollInterval: 2 * time.Millisecond,
		})
	}()
	return done
}

func stopFullSessionBridge(t *testing.T, cancel context.CancelFunc, bridgeDone <-chan error, server *scriptedAppServer) {
	t.Helper()
	cancel()
	select {
	case err := <-bridgeDone:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("full-session bridge did not stop")
	}
	select {
	case err := <-server.done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("scripted app-server did not stop")
	}
}

func TestCodexBridgeFullSession(t *testing.T) {
	fixture := newDispatcherFixture(t)
	ledgerPath := t.TempDir() + "/codexbridge.json"
	process := newFakeProcess()
	server := newScriptedAppServer(process, fixture.thread)
	ctx, cancel := context.WithCancel(context.Background())
	bridgeDone := runFullSessionBridge(t, ctx, fixture, process, ledgerPath, "")
	server.nextCall(t, "initialize")
	server.nextCall(t, "initialized")
	server.nextCall(t, "thread/start")
	waitForStoreBody(t, fixture.store, model.HumanMailboxID, "Codex bridge ready")

	firstInputID := "019c0000-0000-7000-8000-000000000121"
	fixture.addHumanMessage(t, firstInputID, "Implement the feature", time.Now().UTC())
	firstTurn := server.nextCall(t, "turn/start")
	var firstTurnParams TurnStartParams
	if json.Unmarshal(firstTurn.Params, &firstTurnParams) != nil || firstTurnParams.ThreadID != fixture.thread || firstTurnParams.ClientUserMessageID != firstInputID || firstTurnParams.Input[0].Text != "Implement the feature" {
		t.Fatalf("first turn = %s", firstTurn.Params)
	}

	server.sendRaw(`{"id":"approval-full-session","method":"item/commandExecution/requestApproval","params":{"threadId":"` + fixture.thread + `","turnId":"turn-1","itemId":"command-1","command":"go test ./...","cwd":"/work/repo","reason":"verify the implementation"}}`)
	question := waitForStoreMessage(t, fixture.store, model.HumanMailboxID, "Codex requests command approval")
	if !strings.Contains(question.Details, "verify the implementation") || !strings.Contains(question.Details, "approval-full-session") {
		t.Fatalf("approval question = %#v", question)
	}
	approvalReply := model.Message{
		ID: "019c0000-0000-7000-8000-000000000122", Context: question.Context,
		SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: fixture.agent.ID,
		Body: "accept", CreatedAt: time.Now().UTC(),
	}
	if err := fixture.store.Reply(context.Background(), question.ID, approvalReply); err != nil {
		t.Fatal(err)
	}
	approvalResponse := server.nextResponse(t, "approval-full-session")
	var decision map[string]string
	if approvalResponse.Error != nil || json.Unmarshal(approvalResponse.Result, &decision) != nil || decision["decision"] != "accept" {
		t.Fatalf("approval response = %#v", approvalResponse)
	}

	server.sendRaw(`{"method":"item/agentMessage/delta","params":{"threadId":"` + fixture.thread + `","turnId":"turn-1","itemId":"agent-full-session","delta":"partial"}}`)
	server.sendRaw(`{"method":"item/completed","params":{"threadId":"` + fixture.thread + `","turnId":"turn-1","item":{"type":"reasoning","id":"reason-full-session","summary":["internal"]}}}`)
	canonical := `{"method":"item/completed","params":{"threadId":"` + fixture.thread + `","turnId":"turn-1","item":{"type":"agentMessage","id":"agent-full-session","text":"Feature complete","phase":"final_answer"}}}`
	server.sendRaw(canonical)
	server.sendRaw(`{"method":"turn/completed","params":{"threadId":"` + fixture.thread + `","turn":{"id":"turn-1","status":"completed"}}}`)
	output := waitForStoreMessage(t, fixture.store, model.HumanMailboxID, "Feature complete")

	followUp := model.Message{
		ID: "019c0000-0000-7000-8000-000000000123", Context: output.Context,
		SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: fixture.agent.ID,
		Body: "Please add one more test", CreatedAt: time.Now().UTC(),
	}
	if err := fixture.store.Reply(context.Background(), output.ID, followUp); err != nil {
		t.Fatal(err)
	}
	followUpTurn := server.nextCall(t, "turn/start")
	var followUpParams TurnStartParams
	if json.Unmarshal(followUpTurn.Params, &followUpParams) != nil || followUpParams.ClientUserMessageID != followUp.ID || followUpParams.Input[0].Text != followUp.Body {
		t.Fatalf("follow-up turn = %s", followUpTurn.Params)
	}
	server.sendRaw(`{"method":"turn/completed","params":{"threadId":"` + fixture.thread + `","turn":{"id":"turn-2","status":"completed"}}}`)
	waitForCompleted(t, fixture.store, firstInputID)
	waitForCompleted(t, fixture.store, followUp.ID)
	stopFullSessionBridge(t, cancel, bridgeDone, server)

	restartProcess := newFakeProcess()
	restartServer := newScriptedAppServer(restartProcess, fixture.thread)
	restartContext, cancelRestart := context.WithCancel(context.Background())
	restartDone := runFullSessionBridge(t, restartContext, fixture, restartProcess, ledgerPath, fixture.thread)
	restartServer.nextCall(t, "initialize")
	restartServer.nextCall(t, "initialized")
	restartServer.nextCall(t, "thread/resume")
	waitForMessageCount(t, fixture.store, model.HumanMailboxID, "Codex bridge ready", 2)
	restartServer.sendRaw(canonical)

	restartInputID := "019c0000-0000-7000-8000-000000000124"
	fixture.addHumanMessage(t, restartInputID, "Continue after restart", time.Now().UTC())
	restartTurn := restartServer.nextCall(t, "turn/start")
	var restartParams TurnStartParams
	if json.Unmarshal(restartTurn.Params, &restartParams) != nil || restartParams.ClientUserMessageID != restartInputID {
		t.Fatalf("restart turn = %s", restartTurn.Params)
	}
	restartServer.sendRaw(`{"method":"item/completed","params":{"threadId":"` + fixture.thread + `","turnId":"turn-1","item":{"type":"agentMessage","id":"agent-after-restart","text":"Restart complete","phase":"final_answer"}}}`)
	restartServer.sendRaw(`{"method":"turn/completed","params":{"threadId":"` + fixture.thread + `","turn":{"id":"turn-1","status":"completed"}}}`)
	waitForStoreBody(t, fixture.store, model.HumanMailboxID, "Restart complete")
	waitForCompleted(t, fixture.store, restartInputID)
	stopFullSessionBridge(t, cancelRestart, restartDone, restartServer)

	messages, err := fixture.store.List(context.Background(), model.Filter{RecipientMailboxID: model.HumanMailboxID, Limit: 200})
	if err != nil {
		t.Fatal(err)
	}
	counts := make(map[string]int)
	for _, message := range messages {
		counts[message.Body]++
	}
	if counts["Feature complete"] != 1 || counts["Restart complete"] != 1 || counts["partial"] != 0 || counts["Codex requests command approval"] != 1 {
		t.Fatalf("message counts = %#v", counts)
	}
	if len(fixture.replies.OutstandingIDs()) != 0 {
		t.Fatalf("outstanding replies = %#v", fixture.replies.OutstandingIDs())
	}
}

func waitForMessageCount(t *testing.T, databaseStore *store.SQLite, recipientID, body string, count int) {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		messages, err := databaseStore.List(context.Background(), model.Filter{RecipientMailboxID: recipientID, Limit: 200})
		if err != nil {
			t.Fatal(err)
		}
		found := 0
		for _, message := range messages {
			if message.Body == body {
				found++
			}
		}
		if found >= count {
			return
		}
		time.Sleep(2 * time.Millisecond)
	}
	t.Fatalf("message %q did not reach count %d", body, count)
}
