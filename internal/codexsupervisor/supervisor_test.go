package codexsupervisor

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/codexbridge"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/store"
)

type scriptedStarter struct {
	mu           sync.Mutex
	starts       int
	environments [][]string
	fail         error
}

type lockedBuffer struct {
	mu sync.Mutex
	b  bytes.Buffer
}

func (b *lockedBuffer) Write(value []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.b.Write(value)
}

func (b *lockedBuffer) String() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.b.String()
}

func (s *scriptedStarter) factory(environment []string) codexbridge.ProcessStarter {
	s.mu.Lock()
	s.environments = append(s.environments, append([]string(nil), environment...))
	s.mu.Unlock()
	return processStarterFunc(func(string) (codexbridge.Process, error) {
		s.mu.Lock()
		defer s.mu.Unlock()
		if s.fail != nil {
			return nil, s.fail
		}
		s.starts++
		return newScriptedProcess("thread-" + string(rune('0'+s.starts))), nil
	})
}

type processStarterFunc func(string) (codexbridge.Process, error)

func (f processStarterFunc) Start(directory string) (codexbridge.Process, error) { return f(directory) }

type scriptedProcess struct {
	clientInput  *io.PipeWriter
	clientOutput *io.PipeReader
	errors       *io.PipeReader
	done         chan struct{}
	kill         sync.Once
}

func newScriptedProcess(startThreadID string) *scriptedProcess {
	serverInput, clientInput := io.Pipe()
	clientOutput, serverOutput := io.Pipe()
	errorsReader, errorsWriter := io.Pipe()
	process := &scriptedProcess{clientInput: clientInput, clientOutput: clientOutput, errors: errorsReader, done: make(chan struct{})}
	go func() {
		defer close(process.done)
		defer serverInput.Close()
		defer serverOutput.Close()
		defer errorsWriter.Close()
		scanner := bufio.NewScanner(serverInput)
		for scanner.Scan() {
			var request struct {
				ID     int64           `json:"id"`
				Method string          `json:"method"`
				Params json.RawMessage `json:"params"`
			}
			if json.Unmarshal(scanner.Bytes(), &request) != nil || request.ID == 0 {
				continue
			}
			result := `{}`
			switch request.Method {
			case "thread/start":
				result = `{"thread":{"id":"` + startThreadID + `"}}`
			case "thread/resume":
				var params struct {
					ThreadID string `json:"threadId"`
				}
				_ = json.Unmarshal(request.Params, &params)
				result = `{"thread":{"id":"` + params.ThreadID + `"}}`
			case "turn/start":
				result = `{"turn":{"id":"turn-1","status":"inProgress"}}`
			}
			_, _ = io.WriteString(serverOutput, `{"jsonrpc":"2.0","id":`+string(mustJSON(request.ID))+`,"result":`+result+`}`+"\n")
		}
	}()
	return process
}

func mustJSON(value any) []byte {
	raw, _ := json.Marshal(value)
	return raw
}

func (p *scriptedProcess) Input() io.WriteCloser { return p.clientInput }
func (p *scriptedProcess) Output() io.ReadCloser { return p.clientOutput }
func (p *scriptedProcess) Errors() io.ReadCloser { return p.errors }
func (p *scriptedProcess) Wait() error           { <-p.done; return nil }
func (p *scriptedProcess) Kill() error {
	p.kill.Do(func() { _ = p.clientInput.Close(); _ = p.clientOutput.Close(); _ = p.errors.Close() })
	return nil
}

func TestSupervisorLaunchIsDetachedIdempotentConcurrentAndEnvironmentPrivate(t *testing.T) {
	databasePath := filepath.Join(t.TempDir(), "hq.db")
	keyPath, _ := identity.KeyPath(databasePath)
	if _, err := identity.Initialize(keyPath, nil); err != nil {
		t.Fatal(err)
	}
	database, err := store.Open(databasePath)
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	lifetime, cancel := context.WithCancel(context.Background())
	defer cancel()
	starter := &scriptedStarter{}
	supervisor := New(lifetime, database, codexbridge.NewMemoryLedger())
	supervisor.Starter = starter.factory
	defer supervisor.Close()
	directory := t.TempDir()
	secret := "environment-secret-must-not-persist"
	request := domain.CodexLaunchRequest{
		RequestID: uuid.NewString(), AgentName: "fred", Action: domain.CodexSessionNew, Directory: directory,
		Environment: []string{"PATH=/caller/bin", "TOKEN=" + secret}, InitialPrompt: "begin",
	}
	result, err := supervisor.LaunchCodexAgent(context.Background(), request)
	if err != nil || result.Phase != domain.CodexRuntimeRunning || result.ThreadID != "thread-1" {
		t.Fatalf("launch = %#v, %v", result, err)
	}
	duplicate, err := supervisor.LaunchCodexAgent(context.Background(), request)
	if err != nil || duplicate.ThreadID != result.ThreadID || starter.starts != 1 {
		t.Fatalf("duplicate = %#v, %v, starts=%d", duplicate, err, starter.starts)
	}
	changed := request
	changed.InitialPrompt = "different"
	if _, err := supervisor.LaunchCodexAgent(context.Background(), changed); err == nil || strings.Contains(err.Error(), secret) {
		t.Fatalf("request ID conflict = %v", err)
	}
	second := domain.CodexLaunchRequest{RequestID: uuid.NewString(), AgentName: "jane", Action: domain.CodexSessionNew, Directory: directory, Environment: []string{"TOKEN=" + secret}}
	secondResult, err := supervisor.LaunchCodexAgent(context.Background(), second)
	if err != nil || secondResult.Phase != domain.CodexRuntimeRunning || starter.starts != 2 {
		t.Fatalf("second agent = %#v, %v, starts=%d", secondResult, err, starter.starts)
	}
	if running, _ := supervisor.CodexAgentRuntime(context.Background(), "fred"); running.Phase != domain.CodexRuntimeRunning {
		t.Fatalf("first agent stopped when second launched: %#v", running)
	}
	if len(starter.environments) != 2 || strings.Join(starter.environments[0], "|") != "PATH=/caller/bin|TOKEN="+secret {
		t.Fatalf("child environments = %#v", starter.environments)
	}
	if network, err := database.NetworkStatus(context.Background()); err != nil || network.Queued != 0 {
		t.Fatalf("runtime control created Nostr outbox traffic: %#v, %v", network, err)
	}
	sessions, err := database.ListNamedAgentSessions(context.Background(), "fred")
	if err != nil || len(sessions) != 1 || sessions[0].SessionID != "thread-1" || sessions[0].Context.Directory != directory || !sessions[0].Current {
		t.Fatalf("session projection = %#v, %v", sessions, err)
	}
	for _, path := range []string{databasePath, databasePath + "-wal"} {
		raw, readErr := os.ReadFile(path)
		if readErr == nil && bytes.Contains(raw, []byte(secret)) {
			t.Fatalf("environment secret persisted in %s", path)
		}
	}
}

func TestMessageWakesOfflineAgentWithLastKnownGoodLaunch(t *testing.T) {
	databasePath := filepath.Join(t.TempDir(), "hq.db")
	keyPath, _ := identity.KeyPath(databasePath)
	if _, err := identity.Initialize(keyPath, nil); err != nil {
		t.Fatal(err)
	}
	database, err := store.Open(databasePath)
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	starter := &scriptedStarter{}
	supervisor := New(context.Background(), database, codexbridge.NewMemoryLedger())
	supervisor.Starter = starter.factory
	defer supervisor.Close()
	directory := t.TempDir()
	originalEnvironment := []string{"PATH=/original/bin", "TOKEN=original-secret"}
	launched, err := supervisor.LaunchCodexAgent(context.Background(), domain.CodexLaunchRequest{
		RequestID: uuid.NewString(), AgentName: "fred", Action: domain.CodexSessionNew,
		Directory: directory, Repository: model.RepositoryContext{Directory: directory, Branch: "main"},
		Environment: originalEnvironment, InitialPrompt: "begin once", Yolo: true,
	})
	if err != nil || launched.Phase != domain.CodexRuntimeRunning {
		t.Fatalf("initial launch = %#v, %v", launched, err)
	}
	supervisor.mu.Lock()
	lastGood := cloneLaunchRequest(supervisor.lastGood["fred"])
	supervisor.mu.Unlock()
	defer clearLaunchEnvironment(&lastGood)
	if lastGood.SessionID != launched.ThreadID || lastGood.InitialPrompt != "" || !lastGood.Yolo || strings.Join(lastGood.Environment, "|") != strings.Join(originalEnvironment, "|") {
		t.Fatalf("last known good launch = %#v", lastGood)
	}
	if _, err := supervisor.StopCodexAgent(context.Background(), "fred"); err != nil {
		t.Fatal(err)
	}
	agent, err := database.GetNamedAgent(context.Background(), "fred")
	if err != nil || agent.Active {
		t.Fatalf("offline agent = %#v, %v", agent, err)
	}
	message := model.Message{SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: agent.MailboxID, Body: "wake up"}
	supervisor.WakeCodexAgent(message, []string{"PATH=/new/sender", "TOKEN=new-secret"})
	supervisor.WakeCodexAgent(message, []string{"PATH=/duplicate"})
	woken := waitForRunningRuntime(t, supervisor, "fred")
	if woken.ThreadID != launched.ThreadID || woken.Directory != directory {
		t.Fatalf("woken runtime = %#v", woken)
	}
	starter.mu.Lock()
	starts := starter.starts
	environments := append([][]string(nil), starter.environments...)
	starter.mu.Unlock()
	if starts != 2 || len(environments) != 2 || strings.Join(environments[1], "|") != strings.Join(originalEnvironment, "|") {
		t.Fatalf("starts=%d environments=%#v", starts, environments)
	}
}

func TestMessageWakeAfterDaemonRestartUsesPersistedThreadAndSenderEnvironment(t *testing.T) {
	databasePath := filepath.Join(t.TempDir(), "hq.db")
	keyPath, _ := identity.KeyPath(databasePath)
	if _, err := identity.Initialize(keyPath, nil); err != nil {
		t.Fatal(err)
	}
	database, err := store.Open(databasePath)
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	directory := t.TempDir()
	firstStarter := &scriptedStarter{}
	first := New(context.Background(), database, codexbridge.NewMemoryLedger())
	first.Starter = firstStarter.factory
	launched, err := first.LaunchCodexAgent(context.Background(), domain.CodexLaunchRequest{
		RequestID: uuid.NewString(), AgentName: "fred", Action: domain.CodexSessionNew,
		Directory: directory, Repository: model.RepositoryContext{Directory: directory}, Environment: []string{"TOKEN=old-daemon-secret"}, Yolo: true,
	})
	if err != nil || launched.Phase != domain.CodexRuntimeRunning {
		t.Fatalf("initial launch = %#v, %v", launched, err)
	}
	if err := first.Close(); err != nil {
		t.Fatal(err)
	}

	secondStarter := &scriptedStarter{}
	second := New(context.Background(), database, codexbridge.NewMemoryLedger())
	second.Starter = secondStarter.factory
	defer second.Close()
	agent, err := database.GetNamedAgent(context.Background(), "fred")
	if err != nil || agent.Active || agent.CurrentSessionID != launched.ThreadID {
		t.Fatalf("persisted agent = %#v, %v", agent, err)
	}
	senderEnvironment := []string{"PATH=/sender/bin", "TOKEN=sender-secret"}
	second.WakeCodexAgent(model.Message{SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: agent.MailboxID}, senderEnvironment)
	woken := waitForRunningRuntime(t, second, "fred")
	if woken.ThreadID != launched.ThreadID || woken.Directory != directory {
		t.Fatalf("woken runtime = %#v", woken)
	}
	secondStarter.mu.Lock()
	environments := append([][]string(nil), secondStarter.environments...)
	secondStarter.mu.Unlock()
	if len(environments) != 1 || strings.Join(environments[0], "|") != strings.Join(senderEnvironment, "|") {
		t.Fatalf("restart environments = %#v", environments)
	}
}

func waitForRunningRuntime(t *testing.T, supervisor *Supervisor, name string) domain.CodexRuntime {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for {
		runtime, err := supervisor.CodexAgentRuntime(context.Background(), name)
		if err != nil {
			t.Fatal(err)
		}
		if runtime.Phase == domain.CodexRuntimeRunning {
			return runtime
		}
		if time.Now().After(deadline) {
			t.Fatalf("agent %s did not wake; last runtime %#v", name, runtime)
		}
		time.Sleep(time.Millisecond)
	}
}

func TestSupervisorFailureDoesNotSelectAndDoesNotEchoEnvironment(t *testing.T) {
	databasePath := filepath.Join(t.TempDir(), "hq.db")
	keyPath, _ := identity.KeyPath(databasePath)
	if _, err := identity.Initialize(keyPath, nil); err != nil {
		t.Fatal(err)
	}
	database, err := store.Open(databasePath)
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	secret := "do-not-echo-this"
	starter := &scriptedStarter{fail: errors.New("failed with " + secret)}
	supervisor := New(context.Background(), database, codexbridge.NewMemoryLedger())
	supervisor.Starter = starter.factory
	var diagnostics lockedBuffer
	supervisor.Logger = slog.New(slog.NewTextHandler(&diagnostics, &slog.HandlerOptions{Level: slog.LevelDebug}))
	defer supervisor.Close()
	result, err := supervisor.LaunchCodexAgent(context.Background(), domain.CodexLaunchRequest{
		RequestID: uuid.NewString(), AgentName: "fred", Action: domain.CodexSessionNew, Directory: t.TempDir(), Environment: []string{"TOKEN=" + secret},
	})
	if err != nil || result.Phase != domain.CodexRuntimeFailed || strings.Contains(result.Error, secret) {
		t.Fatalf("failed launch = %#v, %v", result, err)
	}
	agent, getErr := database.GetNamedAgent(context.Background(), "fred")
	if getErr != nil || agent.CurrentSessionID != "" {
		t.Fatalf("failed launch selected a session: %#v, %v", agent, getErr)
	}
	deadline := time.Now().Add(time.Second)
	for !strings.Contains(diagnostics.String(), `msg="Codex worker exited"`) && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	log := diagnostics.String()
	for _, expected := range []string{`msg="Codex agent launch requested"`, `msg="Codex worker registered"`, `msg="Codex worker exited"`} {
		if !strings.Contains(log, expected) {
			t.Fatalf("supervisor log omitted %q: %s", expected, log)
		}
	}
	if strings.Contains(log, "TOKEN="+secret) {
		t.Fatalf("supervisor log exposed the environment entry: %s", log)
	}
}

func TestSupervisorShutdownStopsWorkersAndKeepsSelection(t *testing.T) {
	databasePath := filepath.Join(t.TempDir(), "hq.db")
	keyPath, _ := identity.KeyPath(databasePath)
	_, _ = identity.Initialize(keyPath, nil)
	database, err := store.Open(databasePath)
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	starter := &scriptedStarter{}
	lifetime, cancel := context.WithCancel(context.Background())
	supervisor := New(lifetime, database, codexbridge.NewMemoryLedger())
	supervisor.Starter = starter.factory
	directory := t.TempDir()
	result, err := supervisor.LaunchCodexAgent(context.Background(), domain.CodexLaunchRequest{RequestID: uuid.NewString(), AgentName: "fred", Action: domain.CodexSessionNew, Directory: directory})
	if err != nil {
		t.Fatal(err)
	}
	cancel()
	done := make(chan error, 1)
	go func() { done <- supervisor.Close() }()
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("supervisor shutdown did not stop workers")
	}
	agent, err := database.GetNamedAgent(context.Background(), "fred")
	if err != nil || agent.CurrentSessionID != result.ThreadID || agent.Active {
		t.Fatalf("offline selection = %#v, %v", agent, err)
	}
}

func TestSupervisorFailedLiveReplacementKeepsPriorSelection(t *testing.T) {
	databasePath := filepath.Join(t.TempDir(), "hq.db")
	keyPath, _ := identity.KeyPath(databasePath)
	_, _ = identity.Initialize(keyPath, nil)
	database, err := store.Open(databasePath)
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	starter := &scriptedStarter{}
	supervisor := New(context.Background(), database, codexbridge.NewMemoryLedger())
	supervisor.Starter = starter.factory
	defer supervisor.Close()
	directory := t.TempDir()
	first, err := supervisor.LaunchCodexAgent(context.Background(), domain.CodexLaunchRequest{RequestID: uuid.NewString(), AgentName: "fred", Action: domain.CodexSessionNew, Directory: directory})
	if err != nil || first.ThreadID == "" {
		t.Fatalf("first launch = %#v, %v", first, err)
	}
	if _, err := supervisor.LaunchCodexAgent(context.Background(), domain.CodexLaunchRequest{RequestID: uuid.NewString(), AgentName: "fred", Action: domain.CodexSessionNew, Directory: directory}); err == nil || !strings.Contains(err.Error(), "confirm") {
		t.Fatalf("unconfirmed replacement = %v", err)
	}
	if running, _ := supervisor.CodexAgentRuntime(context.Background(), "fred"); running.Phase != domain.CodexRuntimeRunning || running.ThreadID != first.ThreadID {
		t.Fatalf("unconfirmed replacement disturbed worker: %#v", running)
	}
	starter.mu.Lock()
	starter.fail = errors.New("replacement unavailable")
	starter.mu.Unlock()
	replacement, err := supervisor.LaunchCodexAgent(context.Background(), domain.CodexLaunchRequest{RequestID: uuid.NewString(), AgentName: "fred", Action: domain.CodexSessionNew, Directory: directory, ConfirmSwitch: true})
	if err != nil || replacement.Phase != domain.CodexRuntimeFailed {
		t.Fatalf("replacement = %#v, %v", replacement, err)
	}
	agent, err := database.GetNamedAgent(context.Background(), "fred")
	if err != nil || agent.CurrentSessionID != first.ThreadID || agent.Active {
		t.Fatalf("selection after failed replacement = %#v, %v", agent, err)
	}
}
