package codexbridge

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/harness"
)

func TestCodexHarnessAdapterSessionLifecycleAndProtocolTranslation(t *testing.T) {
	process := newFakeProcess()
	server := newScriptedAppServer(process, "session-adapter")
	factory := &HarnessFactory{Starter: fakeStarter{process}, Stderr: io.Discard}
	launched, err := factory.Launch(context.Background(), harness.LaunchConfig{
		InstanceID: "instance-adapter", AgentName: "agent", Directory: "/work/repo", SessionMode: harness.SessionNew,
		Options: &CodexOptions{Yolo: true, DeveloperInstructions: "adapter instructions"},
	})
	if err != nil {
		t.Fatal(err)
	}
	instance := launched.(*codexInstance)
	if instance.ID() != "instance-adapter" || instance.Provider() != CodexProviderID || instance.State().Phase != harness.RuntimeRunning {
		t.Fatalf("instance = %q %q %#v", instance.ID(), instance.Provider(), instance.State())
	}
	if identity := instance.Session().Identity(); identity != (harness.SessionIdentity{Provider: CodexProviderID, ID: "session-adapter"}) {
		t.Fatalf("identity = %#v", identity)
	}
	server.nextCall(t, "initialize")
	server.nextCall(t, "initialized")
	start := server.nextCall(t, "thread/start")
	var startParams ThreadStartParams
	if json.Unmarshal(start.Params, &startParams) != nil || startParams.CWD != "/work/repo" || startParams.DeveloperInstructions != "adapter instructions" || startParams.ApprovalPolicy != approvalPolicyNever || startParams.Sandbox != sandboxModeDangerFullAccess {
		t.Fatalf("thread/start params = %s", start.Params)
	}

	submission := harness.Submission{ID: "submission-adapter", Input: []harness.InputPart{harness.TextInput{Text: "implement it"}}}
	delivery, err := instance.Session().Submit(context.Background(), submission)
	if err != nil || delivery.State != harness.DeliveryAccepted || delivery.OperationID != "turn-1" {
		t.Fatalf("delivery = %#v, %v", delivery, err)
	}
	turnStart := server.nextCall(t, "turn/start")
	var turnStartParams TurnStartParams
	if json.Unmarshal(turnStart.Params, &turnStartParams) != nil || turnStartParams.ClientUserMessageID != "submission-adapter" || turnStartParams.Input[0].Text != "implement it" {
		t.Fatalf("turn/start params = %s", turnStart.Params)
	}

	steerer := instance.Session().(harness.ActiveOperationSubmitter)
	steered, err := steerer.SubmitToActive(context.Background(), "turn-1", harness.Submission{ID: "submission-steered", Input: []harness.InputPart{harness.TextInput{Text: "more context"}}})
	if err != nil || steered != (harness.DeliveryResult{State: harness.DeliveryAccepted, OperationID: "turn-1"}) {
		t.Fatalf("steered delivery = %#v, %v", steered, err)
	}
	turnSteer := server.nextCall(t, "turn/steer")
	var turnSteerParams TurnSteerParams
	if json.Unmarshal(turnSteer.Params, &turnSteerParams) != nil || turnSteerParams.ExpectedTurnID != "turn-1" || turnSteerParams.ClientUserMessageID != "submission-steered" {
		t.Fatalf("turn/steer params = %s", turnSteer.Params)
	}

	server.setThreadReadResult(`{"thread":{"id":"session-adapter","turns":[{"id":"turn-1","status":"inProgress","items":[{"type":"userMessage","id":"vendor-item","clientId":"submission-adapter"}]}]}}`)
	recovered, err := instance.Session().(harness.SubmissionReconciler).Reconcile(context.Background(), "submission-adapter")
	if err != nil || recovered != (harness.RecoveryResult{State: harness.RecoveryAccepted, OperationID: "turn-1"}) {
		t.Fatalf("recovery = %#v, %v", recovered, err)
	}
	server.nextCall(t, "thread/read")

	if err := instance.Session().(harness.Interrupter).Interrupt(context.Background(), "turn-1"); err != nil {
		t.Fatal(err)
	}
	interrupt := server.nextCall(t, "turn/interrupt")
	var interruptParams TurnInterruptParams
	if json.Unmarshal(interrupt.Params, &interruptParams) != nil || interruptParams.ThreadID != "session-adapter" || interruptParams.TurnID != "turn-1" {
		t.Fatalf("turn/interrupt params = %s", interrupt.Params)
	}

	server.sendRaw(`{"method":"future/additive","params":{"ignored":true}}`)
	server.sendRaw(`{"method":"turn/started","params":{"threadId":"session-adapter","turn":{"id":"turn-events","status":"inProgress"}}}`)
	server.sendRaw(`{"method":"item/completed","params":{"threadId":"session-adapter","turnId":"turn-events","item":{"type":"agentMessage","id":"item-output","text":"Done","phase":"final_answer"}}}`)
	server.sendRaw(`{"method":"turn/completed","params":{"threadId":"session-adapter","turn":{"id":"turn-events","status":"completed"}}}`)
	for sequence, expectedPayload := range []any{
		harness.OperationStatusEvent{Status: harness.OperationRunning},
		harness.OutputEvent{Text: "Done", Final: true},
		harness.OperationStatusEvent{Status: harness.OperationCompleted},
	} {
		select {
		case event := <-instance.Events():
			if event.Sequence != uint64(sequence+1) || event.Operation != "turn-events" || event.Payload != expectedPayload {
				t.Fatalf("event %d = %#v; payload=%#v", sequence+1, event, event.Payload)
			}
		case <-time.After(time.Second):
			t.Fatalf("timed out waiting for event %d", sequence+1)
		}
	}

	server.sendRaw(`{"id":"approval-wrong-session","method":"item/fileChange/requestApproval","params":{"threadId":"other-session","turnId":"turn-events","itemId":"wrong-file","reason":"wrong","grantRoot":"/work/repo"}}`)
	wrongSession := server.nextResponse(t, "approval-wrong-session")
	var wrongSessionResult map[string]string
	if wrongSession.Error != nil || json.Unmarshal(wrongSession.Result, &wrongSessionResult) != nil || wrongSessionResult["decision"] != "cancel" {
		t.Fatalf("wrong-session response = %#v", wrongSession)
	}
	select {
	case request := <-instance.Requests():
		t.Fatalf("wrong-session request was exposed: %#v", request)
	default:
	}

	server.sendRaw(`{"id":"approval-adapter","method":"item/fileChange/requestApproval","params":{"threadId":"session-adapter","turnId":"turn-events","itemId":"file-item","reason":"update file","grantRoot":"/work/repo"}}`)
	var request harness.Request
	select {
	case request = <-instance.Requests():
	case <-time.After(time.Second):
		t.Fatal("normalized approval request was not delivered")
	}
	if request.Operation != "turn-events" || request.ItemID != "file-item" {
		t.Fatalf("request = %#v", request)
	}
	if err := instance.Session().Respond(context.Background(), harness.Response{RequestID: request.ID, Payload: harness.DecisionResponse{Decision: "accept"}}); err != nil {
		t.Fatal(err)
	}
	if err := instance.Session().Respond(context.Background(), harness.Response{RequestID: request.ID, Payload: harness.DecisionResponse{Decision: "accept"}}); !errors.Is(err, harness.ErrRequestCompleted) {
		t.Fatalf("duplicate response error = %v", err)
	}
	approval := server.nextResponse(t, "approval-adapter")
	var approvalResult map[string]string
	if approval.Error != nil || json.Unmarshal(approval.Result, &approvalResult) != nil || approvalResult["decision"] != "accept" {
		t.Fatalf("approval response = %#v", approval)
	}

	if err := instance.Shutdown(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := instance.Wait(context.Background()); err != nil {
		t.Fatal(err)
	}
	if state := instance.State(); state.Phase != harness.RuntimeStopped || state.Err != nil {
		t.Fatalf("stopped state = %#v", state)
	}
}

func TestCodexHarnessAdapterRejectsMismatchedResume(t *testing.T) {
	process := newFakeProcess()
	server := newScriptedAppServer(process, "different-session")
	factory := &HarnessFactory{Starter: fakeStarter{process}, Stderr: io.Discard}
	_, err := factory.Launch(context.Background(), harness.LaunchConfig{
		InstanceID: "instance-resume", AgentName: "agent", Directory: "/work/repo",
		SessionMode: harness.SessionResume, RequestedSession: "expected-session",
	})
	if err == nil || !strings.Contains(err.Error(), "different-session") || !strings.Contains(err.Error(), "expected-session") {
		t.Fatalf("resume error = %v", err)
	}
	server.nextCall(t, "initialize")
	server.nextCall(t, "initialized")
	resume := server.nextCall(t, "thread/resume")
	var params ThreadResumeParams
	if json.Unmarshal(resume.Params, &params) != nil || params.ThreadID != "expected-session" {
		t.Fatalf("thread/resume params = %s", resume.Params)
	}
}

func TestCodexHarnessAdapterExitOrdering(t *testing.T) {
	t.Run("process before transport", func(t *testing.T) {
		process := newFakeProcess()
		server := newScriptedAppServer(process, "session-process-first")
		instance := launchAdapterTestInstance(t, process, "instance-process-first")
		server.nextCall(t, "initialize")
		server.nextCall(t, "initialized")
		server.nextCall(t, "thread/start")
		process.finish(errors.New("exit status 9"))
		waitContext, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		if err := instance.Wait(waitContext); err == nil || !strings.Contains(err.Error(), "exit status 9") {
			t.Fatalf("wait error = %v", err)
		}
	})

	t.Run("transport before process", func(t *testing.T) {
		process := newFakeProcess()
		server := newScriptedAppServer(process, "session-transport-first")
		instance := launchAdapterTestInstance(t, process, "instance-transport-first")
		server.nextCall(t, "initialize")
		server.nextCall(t, "initialized")
		server.nextCall(t, "thread/start")
		_ = process.serverOutput.Close()
		waitContext, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		if err := instance.Wait(waitContext); err == nil || !strings.Contains(err.Error(), "protocol stream") {
			t.Fatalf("wait error = %v", err)
		}
	})
}

func TestCodexHarnessAdapterCancelsBlockingRequestDuringShutdown(t *testing.T) {
	process := newFakeProcess()
	server := newScriptedAppServer(process, "session-request-cancel")
	instance := launchAdapterTestInstance(t, process, "instance-request-cancel")
	server.nextCall(t, "initialize")
	server.nextCall(t, "initialized")
	server.nextCall(t, "thread/start")
	server.sendRaw(`{"id":"approval-cancel","method":"item/commandExecution/requestApproval","params":{"threadId":"session-request-cancel","turnId":"turn-1","itemId":"command-1","command":"go test ./...","cwd":"/work/repo","reason":"verify"}}`)
	select {
	case <-instance.Requests():
	case <-time.After(time.Second):
		t.Fatal("blocking request was not delivered")
	}
	if err := instance.Shutdown(context.Background()); err != nil {
		t.Fatal(err)
	}
	response := server.nextResponse(t, "approval-cancel")
	var result map[string]string
	if response.Error != nil || json.Unmarshal(response.Result, &result) != nil || result["decision"] != "cancel" {
		t.Fatalf("shutdown response = %#v", response)
	}
}

func launchAdapterTestInstance(t *testing.T, process *fakeProcess, instanceID harness.InstanceID) *codexInstance {
	t.Helper()
	factory := &HarnessFactory{Starter: fakeStarter{process}, Stderr: io.Discard}
	launched, err := factory.Launch(context.Background(), harness.LaunchConfig{
		InstanceID: instanceID, AgentName: "agent", Directory: "/work/repo", SessionMode: harness.SessionNew,
	})
	if err != nil {
		t.Fatal(err)
	}
	return launched.(*codexInstance)
}
