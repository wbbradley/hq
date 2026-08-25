package harnesssupervisor

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/codexbridge"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/harness"
	"github.com/wbbradley/hq/internal/harness/fake"
	"github.com/wbbradley/hq/internal/harnessbridge"
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

type recordingHarnessFactory struct {
	harness.Factory
	launched chan harness.Instance
}

func (f *recordingHarnessFactory) Launch(ctx context.Context, config harness.LaunchConfig) (harness.Instance, error) {
	instance, err := f.Factory.Launch(ctx, config)
	if err == nil {
		f.launched <- instance
	}
	return instance, err
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

func codexFactoryResolver(starterFactory func([]string) codexbridge.ProcessStarter) FactoryResolver {
	return func(provider harness.ProviderID, environment []string) (harness.Factory, error) {
		if provider != codexbridge.CodexProviderID {
			return nil, &harness.ProviderError{Provider: provider, Operation: "resolve provider", Cause: harness.ErrUnknownProvider}
		}
		return &codexbridge.HarnessFactory{Starter: starterFactory(environment), Stderr: io.Discard}, nil
	}
}

func codexOptions(yolo bool) json.RawMessage {
	raw, _ := json.Marshal(codexbridge.CodexOptions{Yolo: yolo})
	return raw
}

func decodeCodexTestOptions(provider harness.ProviderID, agentName string, raw json.RawMessage) (harness.ProviderOptions, error) {
	if provider != codexbridge.CodexProviderID {
		return nil, harness.ErrUnknownProvider
	}
	var options codexbridge.CodexOptions
	if len(raw) != 0 {
		if err := json.Unmarshal(raw, &options); err != nil {
			return nil, err
		}
	}
	options.DeveloperInstructions = codexbridge.NamedAgentDeveloperInstructions(agentName)
	return options, nil
}

func codexYolo(raw json.RawMessage) bool {
	var options codexbridge.CodexOptions
	_ = json.Unmarshal(raw, &options)
	return options.Yolo
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

func TestSupervisorLaunchesRegisteredNonCodexProvider(t *testing.T) {
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

	const providerID harness.ProviderID = "home-built"
	provider := fake.NewFactory(providerID)
	factory := &recordingHarnessFactory{Factory: provider, launched: make(chan harness.Instance, 4)}
	registry, err := harness.NewRegistry(factory)
	if err != nil {
		t.Fatal(err)
	}
	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger(), registry)
	database.SetChangeObserver(supervisor.Publish)
	defer supervisor.Close()

	runtime, err := supervisor.LaunchHarnessAgent(context.Background(), domain.HarnessLaunchRequest{
		RequestID: uuid.NewString(), AgentName: "generic-agent", Harness: string(providerID),
		Action: domain.HarnessSessionNew, Directory: t.TempDir(),
	})
	if err != nil {
		t.Fatal(err)
	}
	if runtime.Phase != domain.HarnessRuntimeRunning || runtime.Harness != string(providerID) || runtime.SessionID != "session-1" {
		t.Fatalf("runtime = %#v", runtime)
	}
	instance := receiveHarnessInstance(t, factory.launched)
	agent, err := database.GetNamedAgent(context.Background(), runtime.AgentName)
	if err != nil {
		t.Fatal(err)
	}
	provider.SetNextSubmissionOutcome(harness.DeliveryUncertain, true)
	message := model.Message{
		ID: "019c0000-0000-7000-8000-000000000601", SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: agent.MailboxID,
		Body: "supervisor recovery input", Context: model.RepositoryContext{Directory: runtime.Directory}, CreatedAt: time.Now().UTC(),
	}
	if err := database.Create(context.Background(), message); err != nil {
		t.Fatal(err)
	}
	waitForSupervisorMessage(t, database, message.ID, func(got model.Message) bool { return got.CompletedAt != nil })

	if err := provider.Emit(instance, "operation-output", "output-1", harness.OutputEvent{Text: "supervisor-neutral output", Final: true}); err != nil {
		t.Fatal(err)
	}
	waitForSupervisorBody(t, database, "supervisor-neutral output")
	_, response, err := provider.Ask(instance, "operation-question", "approval-1", harness.ApprovalRequest{Kind: "command", Summary: "Run checks", Choices: []string{"accept", "decline"}})
	if err != nil {
		t.Fatal(err)
	}
	question := waitForSupervisorBody(t, database, "Deterministic fake harness requests command approval")
	reply := model.Message{
		ID: "019c0000-0000-7000-8000-000000000602", SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: agent.MailboxID,
		Body: "accept", Context: question.Context, CreatedAt: time.Now().UTC(),
	}
	if err := database.Reply(context.Background(), question.ID, reply); err != nil {
		t.Fatal(err)
	}
	select {
	case answered := <-response:
		decision, ok := answered.Payload.(harness.DecisionResponse)
		if !ok || decision.Decision != "accept" {
			t.Fatalf("interactive response = %#v", answered)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("interactive request was not answered")
	}
	stopped, err := supervisor.StopHarnessAgent(context.Background(), runtime.AgentName)
	if err != nil {
		t.Fatal(err)
	}
	if stopped.Phase != domain.HarnessRuntimeOffline || stopped.Harness != string(providerID) || stopped.SessionID != runtime.SessionID {
		t.Fatalf("stopped runtime = %#v", stopped)
	}
	resumed, err := supervisor.LaunchHarnessAgent(context.Background(), domain.HarnessLaunchRequest{
		RequestID: uuid.NewString(), AgentName: runtime.AgentName, Harness: string(providerID),
		Action: domain.HarnessSessionResume, SessionID: runtime.SessionID, Directory: runtime.Directory,
	})
	if err != nil || resumed.Phase != domain.HarnessRuntimeRunning || resumed.SessionID != runtime.SessionID {
		t.Fatalf("resumed runtime = %#v, %v", resumed, err)
	}
	_ = receiveHarnessInstance(t, factory.launched)
	if _, err := supervisor.StopHarnessAgent(context.Background(), runtime.AgentName); err != nil {
		t.Fatal(err)
	}

	capabilities := provider.Provider().Capabilities
	capabilities.Resume = false
	provider.SetCapabilities(capabilities)
	failed, err := supervisor.LaunchHarnessAgent(context.Background(), domain.HarnessLaunchRequest{
		RequestID: uuid.NewString(), AgentName: runtime.AgentName, Harness: string(providerID),
		Action: domain.HarnessSessionResume, SessionID: runtime.SessionID, Directory: runtime.Directory,
	})
	if !errors.Is(err, harness.ErrCapabilityUnavailable) || failed.Phase != domain.HarnessRuntimeFailed {
		t.Fatalf("incapable resume = %#v, %v", failed, err)
	}
	_, err = supervisor.LaunchHarnessAgent(context.Background(), domain.HarnessLaunchRequest{
		RequestID: uuid.NewString(), AgentName: "unknown-agent", Harness: "missing-provider",
		Action: domain.HarnessSessionNew, Directory: t.TempDir(),
	})
	if !errors.Is(err, harness.ErrUnknownProvider) {
		t.Fatalf("unknown provider error = %v", err)
	}
}

func TestSharedProviderFactoryCrashIsIsolatedToOneLogicalInstance(t *testing.T) {
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

	provider := fake.NewFactory("shared-provider")
	factory := &recordingHarnessFactory{Factory: provider, launched: make(chan harness.Instance, 2)}
	registry, err := harness.NewRegistry(factory)
	if err != nil {
		t.Fatal(err)
	}
	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger(), registry)
	defer supervisor.Close()

	launch := func(name string) domain.HarnessRuntime {
		runtime, launchErr := supervisor.LaunchHarnessAgent(context.Background(), domain.HarnessLaunchRequest{
			RequestID: uuid.NewString(), AgentName: name, Harness: "shared-provider", Action: domain.HarnessSessionNew, Directory: t.TempDir(),
		})
		if launchErr != nil {
			t.Fatal(launchErr)
		}
		return runtime
	}
	first := launch("isolated-first")
	firstInstance := receiveHarnessInstance(t, factory.launched)
	second := launch("isolated-second")
	secondInstance := receiveHarnessInstance(t, factory.launched)

	if err := provider.Crash(firstInstance, errors.New("isolated instance crash")); err != nil {
		t.Fatal(err)
	}
	waitForHarnessRuntimePhase(t, supervisor, first.AgentName, domain.HarnessRuntimeOffline)
	stillRunning, err := supervisor.HarnessAgentRuntime(context.Background(), second.AgentName)
	if err != nil {
		t.Fatal(err)
	}
	if stillRunning.Phase != domain.HarnessRuntimeRunning || stillRunning.SessionID != second.SessionID {
		t.Fatalf("second runtime changed after first crashed: %#v", stillRunning)
	}
	if err := provider.Emit(secondInstance, "operation-2", "output-2", harness.OutputEvent{Text: "second instance survived", Final: true}); err != nil {
		t.Fatal(err)
	}
	waitForSupervisorBody(t, database, "second instance survived")
	if stopped, err := supervisor.StopHarnessAgent(context.Background(), second.AgentName); err != nil || stopped.Phase != domain.HarnessRuntimeOffline {
		t.Fatalf("stop second runtime = %#v, %v", stopped, err)
	}
}

func TestSupervisorLaunchCanPublishAgentCreationInvalidation(t *testing.T) {
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

	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	supervisor.ResolveFactory = codexFactoryResolver((&scriptedStarter{}).factory)
	database.SetChangeObserver(supervisor.Publish)
	defer supervisor.Close()

	result, err := supervisor.LaunchHarnessAgent(context.Background(), domain.HarnessLaunchRequest{Harness: "codex",
		RequestID: uuid.NewString(), AgentName: "fred", Action: domain.HarnessSessionNew, Directory: t.TempDir(),
	})
	if err != nil || result.Phase != domain.HarnessRuntimeRunning {
		t.Fatalf("launch with synchronous change publication = %#v, %v", result, err)
	}
}

func TestProjectActivationOpensAssignsAndBindsNewThread(t *testing.T) {
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
	if _, err := database.CreateNamedAgent(context.Background(), "fred", ""); err != nil {
		t.Fatal(err)
	}
	directory := t.TempDir()
	project, err := database.CreateProject(context.Background(), domain.CreateProjectRequest{Name: "runtime project", Paths: []domain.ProjectPathInput{{DisplayPath: directory}}})
	if err != nil {
		t.Fatal(err)
	}
	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	supervisor.ResolveFactory = codexFactoryResolver((&scriptedStarter{}).factory)
	defer supervisor.Close()
	activated, err := supervisor.ActivateHarnessProject(context.Background(), domain.ProjectHarnessActivationRequest{ProjectID: project.ID, ExpectedHead: project.HeadEventID, AgentName: "fred", Launch: domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), Action: domain.HarnessSessionNew, Directory: directory}})
	if err != nil {
		t.Fatal(err)
	}
	if activated.Project.Lifecycle != domain.ProjectOpen || activated.Project.Assignment == nil || activated.Project.Assignment.State != domain.AssignmentRunnable || activated.Runtime.SessionID != "thread-1" {
		t.Fatalf("activation = %#v", activated)
	}
	threads, err := database.ListProjectThreads(context.Background(), project.ID)
	if err != nil || len(threads) != 1 || threads[0].ExternalID != "thread-1" || threads[0].AgentName != "fred" {
		t.Fatalf("project threads = %#v, %v", threads, err)
	}
	if sessions, err := database.ListNamedAgentSessions(context.Background(), "fred"); err != nil || len(sessions) != 0 {
		t.Fatalf("project thread leaked into direct sessions: %#v, %v", sessions, err)
	}
	if _, err := supervisor.LaunchHarnessAgent(context.Background(), domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), AgentName: "fred", Action: domain.HarnessSessionCurrent, Directory: directory}); !errors.Is(err, domain.ErrAgentAssigned) {
		t.Fatalf("direct launch of assigned agent = %v", err)
	}
}

func TestFailedClosedProjectActivationCompensatesToClosedUnassigned(t *testing.T) {
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
	if _, err := database.CreateNamedAgent(context.Background(), "fred", ""); err != nil {
		t.Fatal(err)
	}
	project, err := database.CreateProject(context.Background(), domain.CreateProjectRequest{Name: "failed activation", Paths: []domain.ProjectPathInput{{DisplayPath: t.TempDir()}}})
	if err != nil {
		t.Fatal(err)
	}
	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	supervisor.ResolveFactory = codexFactoryResolver((&scriptedStarter{fail: errors.New("start failed")}).factory)
	defer supervisor.Close()
	_, err = supervisor.ActivateHarnessProject(context.Background(), domain.ProjectHarnessActivationRequest{ProjectID: project.ID, ExpectedHead: project.HeadEventID, AgentName: "fred", Launch: domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), Action: domain.HarnessSessionNew, Directory: t.TempDir()}})
	if err == nil {
		t.Fatal("failed runtime activation succeeded")
	}
	got, err := database.GetProject(context.Background(), project.ID)
	if err != nil {
		t.Fatal(err)
	}
	if got.Lifecycle != domain.ProjectClosed || got.Assignment != nil {
		t.Fatalf("compensated project = %#v", got)
	}
	if agent, err := database.GetNamedAgent(context.Background(), "fred"); err != nil || !agent.Idle {
		t.Fatalf("compensated agent = %#v, %v", agent, err)
	}
}

func TestProjectMessageWakesDurableRunnableAssignmentAfterRestart(t *testing.T) {
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
	if _, err := database.CreateNamedAgent(context.Background(), "fred", ""); err != nil {
		t.Fatal(err)
	}
	directory := t.TempDir()
	project, err := database.CreateProject(context.Background(), domain.CreateProjectRequest{Name: "wake project", Paths: []domain.ProjectPathInput{{DisplayPath: directory}}, Open: true})
	if err != nil {
		t.Fatal(err)
	}
	first := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	first.ResolveFactory = codexFactoryResolver((&scriptedStarter{}).factory)
	activated, err := first.ActivateHarnessProject(context.Background(), domain.ProjectHarnessActivationRequest{ProjectID: project.ID, ExpectedHead: project.HeadEventID, AgentName: "fred", Launch: domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), Action: domain.HarnessSessionNew, Directory: directory}})
	if err != nil {
		t.Fatal(err)
	}
	if err := first.Close(); err != nil {
		t.Fatal(err)
	}
	message := model.Message{ID: "019c0000-0000-7000-8000-000000000401", SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: activated.Project.MailboxID, Body: "resume project", CreatedAt: time.Now().UTC()}
	if err := database.Create(context.Background(), message); err != nil {
		t.Fatal(err)
	}
	secondStarter := &scriptedStarter{}
	second := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	second.ResolveFactory = codexFactoryResolver(secondStarter.factory)
	second.LoadLaunchDefaults = func() (domain.HarnessLaunchDefaults, error) { return domain.HarnessLaunchDefaults{}, nil }
	defer second.Close()
	second.StartWorkReconciliation()
	runtime := waitForRunningRuntime(t, second, "fred")
	if runtime.SessionID != activated.Runtime.SessionID {
		t.Fatalf("woke thread %q, want %q", runtime.SessionID, activated.Runtime.SessionID)
	}
	deadline := time.Now().Add(2 * time.Second)
	for {
		got, err := database.Get(context.Background(), message.ID)
		if err == nil && got.CompletedAt != nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("project message was not dispatched after wake: %#v, %v", got, err)
		}
		time.Sleep(time.Millisecond)
	}
}

func TestProjectReplyWakesOfflineAssignmentAfterDaemonRestart(t *testing.T) {
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
	if _, err := database.CreateNamedAgent(context.Background(), "reply-agent", ""); err != nil {
		t.Fatal(err)
	}
	directory := t.TempDir()
	project, err := database.CreateProject(context.Background(), domain.CreateProjectRequest{Name: "reply wake", Paths: []domain.ProjectPathInput{{DisplayPath: directory}}, Open: true})
	if err != nil {
		t.Fatal(err)
	}
	first := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	first.ResolveFactory = codexFactoryResolver((&scriptedStarter{}).factory)
	activated, err := first.ActivateHarnessProject(context.Background(), domain.ProjectHarnessActivationRequest{ProjectID: project.ID, ExpectedHead: project.HeadEventID, AgentName: "reply-agent", Launch: domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), Action: domain.HarnessSessionNew, Directory: directory}})
	if err != nil {
		t.Fatal(err)
	}
	outputID := "019d0000-0000-7000-8000-000000000051"
	binding := domain.ProjectOutputBinding{
		ProjectID: activated.Project.ID, AssignmentID: activated.Project.Assignment.ID, AgentName: "reply-agent",
		ProjectThreadID: activated.Project.Assignment.SelectedThreadID, ExternalThreadID: activated.Runtime.SessionID, RuntimeState: "connected",
	}
	if err := database.CreateProjectOutput(context.Background(), binding, model.Message{ID: outputID, SenderMailboxID: activated.Project.MailboxID, RecipientMailboxID: model.HumanMailboxID, Body: "reply to continue", CreatedAt: time.Now().UTC()}); err != nil {
		t.Fatal(err)
	}
	if err := first.Close(); err != nil {
		t.Fatal(err)
	}
	replyID := "019d0000-0000-7000-8000-000000000052"
	if err := database.Reply(context.Background(), outputID, model.Message{ID: replyID, SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: activated.Project.MailboxID, Body: "continue after restart", CreatedAt: time.Now().UTC()}); err != nil {
		t.Fatal(err)
	}
	original, err := database.Get(context.Background(), outputID)
	if err != nil || original.ArchivedAt == nil {
		t.Fatalf("replied-to output = %#v, %v", original, err)
	}

	secondStarter := &scriptedStarter{}
	second := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	second.ResolveFactory = codexFactoryResolver(secondStarter.factory)
	second.LoadLaunchDefaults = func() (domain.HarnessLaunchDefaults, error) { return domain.HarnessLaunchDefaults{}, nil }
	defer second.Close()
	second.StartWorkReconciliation()
	runtime := waitForRunningRuntime(t, second, "reply-agent")
	if runtime.SessionID != activated.Runtime.SessionID {
		t.Fatalf("reply woke thread %q, want %q", runtime.SessionID, activated.Runtime.SessionID)
	}
	deadline := time.Now().Add(2 * time.Second)
	for {
		reply, getErr := database.Get(context.Background(), replyID)
		if getErr == nil && reply.CompletedAt != nil {
			if reply.Purpose != model.MessagePurposeProjectInput || reply.ReplyTo == nil || *reply.ReplyTo != outputID {
				t.Fatalf("dispatched reply = %#v", reply)
			}
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("project reply was not dispatched after restart: %#v, %v", reply, getErr)
		}
		time.Sleep(time.Millisecond)
	}
	secondStarter.mu.Lock()
	starts := secondStarter.starts
	secondStarter.mu.Unlock()
	if starts != 1 {
		t.Fatalf("restart reconciliation launched %d workers", starts)
	}
}

func TestSupervisorReconcilesDirectWorkFromStoreInvalidationOnce(t *testing.T) {
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
	first := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	first.ResolveFactory = codexFactoryResolver((&scriptedStarter{}).factory)
	launched, err := first.LaunchHarnessAgent(context.Background(), domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), AgentName: "direct", Action: domain.HarnessSessionNew, Directory: directory})
	if err != nil {
		t.Fatal(err)
	}
	if err := first.Close(); err != nil {
		t.Fatal(err)
	}

	secondStarter := &scriptedStarter{}
	second := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	second.ResolveFactory = codexFactoryResolver(secondStarter.factory)
	second.LoadLaunchDefaults = func() (domain.HarnessLaunchDefaults, error) { return domain.HarnessLaunchDefaults{}, nil }
	database.SetChangeObserver(second.Publish)
	second.StartWorkReconciliation()
	defer second.Close()
	agent, err := database.GetNamedAgent(context.Background(), "direct")
	if err != nil {
		t.Fatal(err)
	}
	message := model.Message{ID: "019c0000-0000-7000-8000-000000000402", SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: agent.MailboxID, Body: "durable direct work", CreatedAt: time.Now().UTC()}
	if err := database.Create(context.Background(), message); err != nil {
		t.Fatal(err)
	}
	for range 10 {
		second.Publish(domain.Invalidation{Topics: []domain.ChangeTopic{domain.TopicMessages}})
	}
	runtime := waitForRunningRuntime(t, second, "direct")
	if runtime.SessionID != launched.SessionID || runtime.Directory != directory {
		t.Fatalf("reconciled runtime = %#v", runtime)
	}
	deadline := time.Now().Add(2 * time.Second)
	for {
		got, getErr := database.Get(context.Background(), message.ID)
		if getErr == nil && got.CompletedAt != nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("direct work was not delivered: %#v, %v", got, getErr)
		}
		time.Sleep(time.Millisecond)
	}
	secondStarter.mu.Lock()
	starts := secondStarter.starts
	secondStarter.mu.Unlock()
	if starts != 1 {
		t.Fatalf("duplicate reconciliation launched %d workers", starts)
	}
}

func TestSupervisorStartupRecoversInterruptedProjectActivation(t *testing.T) {
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
	if _, err := database.CreateNamedAgent(context.Background(), "fred", ""); err != nil {
		t.Fatal(err)
	}
	project, err := database.CreateProject(context.Background(), domain.CreateProjectRequest{Name: "interrupted", Paths: []domain.ProjectPathInput{{DisplayPath: t.TempDir()}}})
	if err != nil {
		t.Fatal(err)
	}
	operationID := uuid.NewString()
	operation, err := database.BeginProjectActivation(context.Background(), operationID, project.ID, project.HeadEventID, "fred")
	if err != nil || operation.PriorLifecycle != domain.ProjectClosed {
		t.Fatalf("begin operation = %#v, %v", operation, err)
	}
	project, err = database.OpenProject(context.Background(), project.ID, project.HeadEventID)
	if err != nil {
		t.Fatal(err)
	}
	project, err = database.AssignProject(context.Background(), project.ID, project.HeadEventID, "fred")
	if err != nil {
		t.Fatal(err)
	}
	if err := database.SetProjectActivationAssignment(context.Background(), operationID, project.Assignment.ID); err != nil {
		t.Fatal(err)
	}
	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	defer supervisor.Close()
	got, err := database.GetProject(context.Background(), project.ID)
	if err != nil {
		t.Fatal(err)
	}
	if got.Lifecycle != domain.ProjectClosed || got.Assignment != nil {
		t.Fatalf("recovered project = %#v", got)
	}
	operations, err := database.ListIncompleteProjectActivations(context.Background())
	if err != nil || len(operations) != 0 {
		t.Fatalf("incomplete operations = %#v, %v", operations, err)
	}
}

func TestProjectCloseQuiescesRuntimeAndArchivePreservesFiles(t *testing.T) {
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
	if _, err := database.CreateNamedAgent(context.Background(), "fred", ""); err != nil {
		t.Fatal(err)
	}
	directory := t.TempDir()
	marker := filepath.Join(directory, "keep-me")
	if err := os.WriteFile(marker, []byte("durable"), 0o600); err != nil {
		t.Fatal(err)
	}
	project, err := database.CreateProject(context.Background(), domain.CreateProjectRequest{Name: "close project", Paths: []domain.ProjectPathInput{{DisplayPath: directory}}, Open: true})
	if err != nil {
		t.Fatal(err)
	}
	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	supervisor.ResolveFactory = codexFactoryResolver((&scriptedStarter{}).factory)
	defer supervisor.Close()
	activated, err := supervisor.ActivateHarnessProject(context.Background(), domain.ProjectHarnessActivationRequest{ProjectID: project.ID, ExpectedHead: project.HeadEventID, AgentName: "fred", Launch: domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), Action: domain.HarnessSessionNew, Directory: directory}})
	if err != nil {
		t.Fatal(err)
	}
	closed, err := supervisor.CloseHarnessProject(context.Background(), domain.ProjectHarnessCloseRequest{ProjectID: project.ID, ExpectedHead: activated.Project.HeadEventID, Archive: true})
	if err != nil {
		t.Fatal(err)
	}
	if closed.Lifecycle != domain.ProjectClosed || !closed.Archived || closed.Assignment != nil {
		t.Fatalf("closed project = %#v", closed)
	}
	if raw, err := os.ReadFile(marker); err != nil || string(raw) != "durable" {
		t.Fatalf("close modified underlying file: %q, %v", raw, err)
	}
}

func TestProjectCloseRequiresForceWhenRuntimeOwnershipIsUnknown(t *testing.T) {
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
	if _, err := database.CreateNamedAgent(context.Background(), "fred", ""); err != nil {
		t.Fatal(err)
	}
	project, err := database.CreateProject(context.Background(), domain.CreateProjectRequest{Name: "force close", Open: true})
	if err != nil {
		t.Fatal(err)
	}
	project, err = database.AssignProject(context.Background(), project.ID, project.HeadEventID, "fred")
	if err != nil {
		t.Fatal(err)
	}
	project, err = database.ActivateProjectAssignment(context.Background(), project.ID, project.HeadEventID, domain.ActivateProjectAssignmentRequest{Harness: "codex", ExternalThread: "unknown-thread", LaunchDirectory: t.TempDir()})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := database.AcquireNamedAgent(context.Background(), "fred", "orphan-owner", time.Minute); err != nil {
		t.Fatal(err)
	}
	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	defer supervisor.Close()
	_, err = supervisor.CloseHarnessProject(context.Background(), domain.ProjectHarnessCloseRequest{ProjectID: project.ID, ExpectedHead: project.HeadEventID})
	if !errors.Is(err, domain.ErrProjectRuntimeUnknown) {
		t.Fatalf("normal close with unknown runtime = %v", err)
	}
	closing, err := database.GetProject(context.Background(), project.ID)
	if err != nil {
		t.Fatal(err)
	}
	if closing.Lifecycle != domain.ProjectClosing {
		t.Fatalf("project did not remain closing: %#v", closing)
	}
	closed, err := supervisor.CloseHarnessProject(context.Background(), domain.ProjectHarnessCloseRequest{ProjectID: project.ID, ExpectedHead: closing.HeadEventID, Force: true})
	if err != nil || closed.Lifecycle != domain.ProjectClosed {
		t.Fatalf("force close = %#v, %v", closed, err)
	}
}

func TestProjectCloseRetryResumesDurableClosingOperation(t *testing.T) {
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
	project, err := database.CreateProject(context.Background(), domain.CreateProjectRequest{Name: "retry close", Open: true})
	if err != nil {
		t.Fatal(err)
	}
	request := domain.ProjectHarnessCloseRequest{RequestID: uuid.NewString(), ProjectID: project.ID, ExpectedHead: project.HeadEventID}
	if _, err := database.BeginProjectRuntimeOperation(context.Background(), domain.ProjectRuntimeOperation{ID: request.RequestID, Kind: "close", ProjectID: project.ID, ExpectedHead: project.HeadEventID}); err != nil {
		t.Fatal(err)
	}
	closing, err := database.BeginCloseProject(context.Background(), project.ID, project.HeadEventID)
	if err != nil {
		t.Fatal(err)
	}
	if err := database.AdvanceProjectRuntimeOperation(context.Background(), request.RequestID, "closing", closing.HeadEventID, ""); err != nil {
		t.Fatal(err)
	}
	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	defer supervisor.Close()
	closed, err := supervisor.CloseHarnessProject(context.Background(), request)
	if err != nil || closed.Lifecycle != domain.ProjectClosed {
		t.Fatalf("retried close = %#v, %v", closed, err)
	}
}

func TestProjectHandoffRetryResumesAfterDurableUnassign(t *testing.T) {
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
	for _, name := range []string{"alice", "bob"} {
		if _, err := database.CreateNamedAgent(context.Background(), name, ""); err != nil {
			t.Fatal(err)
		}
	}
	directory := t.TempDir()
	project, err := database.CreateProject(context.Background(), domain.CreateProjectRequest{Name: "retry handoff", Paths: []domain.ProjectPathInput{{DisplayPath: directory}}, Open: true})
	if err != nil {
		t.Fatal(err)
	}
	project, err = database.AssignProject(context.Background(), project.ID, project.HeadEventID, "alice")
	if err != nil {
		t.Fatal(err)
	}
	project, err = database.ActivateProjectAssignment(context.Background(), project.ID, project.HeadEventID, domain.ActivateProjectAssignmentRequest{Harness: "codex", ExternalThread: "old-thread", LaunchDirectory: directory})
	if err != nil {
		t.Fatal(err)
	}
	request := domain.ProjectHarnessHandoffRequest{RequestID: uuid.NewString(), ProjectID: project.ID, ExpectedHead: project.HeadEventID, NewAgentName: "bob", Force: true, Launch: domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), Action: domain.HarnessSessionNew, Directory: directory}}
	if _, err := database.BeginProjectRuntimeOperation(context.Background(), domain.ProjectRuntimeOperation{ID: request.RequestID, Kind: "handoff", ProjectID: project.ID, ExpectedHead: project.HeadEventID, TargetAgent: "bob", Force: true}); err != nil {
		t.Fatal(err)
	}
	unassigned, err := database.UnassignProject(context.Background(), project.ID, project.HeadEventID, true, "unknown")
	if err != nil {
		t.Fatal(err)
	}
	if err := database.AdvanceProjectRuntimeOperation(context.Background(), request.RequestID, "unassigned", unassigned.HeadEventID, ""); err != nil {
		t.Fatal(err)
	}
	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	supervisor.ResolveFactory = codexFactoryResolver((&scriptedStarter{}).factory)
	defer supervisor.Close()
	activated, err := supervisor.HandoffHarnessProject(context.Background(), request)
	if err != nil || activated.Project.Assignment == nil || activated.Project.Assignment.AgentName != "bob" || activated.Project.Assignment.State != domain.AssignmentRunnable {
		t.Fatalf("retried handoff = %#v, %v", activated, err)
	}
}

func TestProjectWorktreeProvisioningReservesCreatesAndNeverDeletes(t *testing.T) {
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
	repository := filepath.Join(t.TempDir(), "repository")
	if err := os.Mkdir(repository, 0o700); err != nil {
		t.Fatal(err)
	}
	runTestGit(t, repository, "init")
	runTestGit(t, repository, "config", "user.email", "hq@example.invalid")
	runTestGit(t, repository, "config", "user.name", "HQ Test")
	if err := os.WriteFile(filepath.Join(repository, "README"), []byte("seed\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	runTestGit(t, repository, "add", "README")
	runTestGit(t, repository, "commit", "-m", "seed")
	destination := filepath.Join(filepath.Dir(repository), "feature-worktree")
	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	defer supervisor.Close()
	request := domain.ProjectWorktreeRequest{RequestID: uuid.NewString(), ProjectID: uuid.NewString(), Name: "feature", Repository: repository, MergeBase: "HEAD", Destination: destination, Branch: "feature-test", Open: true}
	project, err := supervisor.ProvisionProjectWorktree(context.Background(), request)
	if err != nil {
		t.Fatal(err)
	}
	if project.Lifecycle != domain.ProjectOpen || len(project.Resources) != 1 || project.PrimaryResourceID != project.Resources[0].ID {
		t.Fatalf("provisioned project = %#v", project)
	}
	if got := strings.TrimSpace(runTestGit(t, destination, "branch", "--show-current")); got != request.Branch {
		t.Fatalf("worktree branch = %q", got)
	}
	closed, err := supervisor.CloseHarnessProject(context.Background(), domain.ProjectHarnessCloseRequest{RequestID: uuid.NewString(), ProjectID: project.ID, ExpectedHead: project.HeadEventID, Archive: true})
	if err != nil || !closed.Archived {
		t.Fatalf("archive provisioned project = %#v, %v", closed, err)
	}
	if _, err := os.Stat(filepath.Join(destination, "README")); err != nil {
		t.Fatalf("archive deleted worktree: %v", err)
	}
	retried, err := supervisor.ProvisionProjectWorktree(context.Background(), request)
	if err != nil || retried.ID != project.ID {
		t.Fatalf("retried provisioning = %#v, %v", retried, err)
	}
}

func runTestGit(t *testing.T, directory string, args ...string) string {
	t.Helper()
	commandArgs := append([]string{"-C", directory}, args...)
	output, err := exec.Command("git", commandArgs...).CombinedOutput()
	if err != nil {
		t.Fatalf("git %v: %v: %s", args, err, output)
	}
	return string(output)
}

func TestProjectHandoffQuiescesOldAgentAndStartsFreshScopedThread(t *testing.T) {
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
	for _, name := range []string{"alice", "bob"} {
		if _, err := database.CreateNamedAgent(context.Background(), name, ""); err != nil {
			t.Fatal(err)
		}
	}
	directory := t.TempDir()
	project, err := database.CreateProject(context.Background(), domain.CreateProjectRequest{Name: "handoff", Open: true})
	if err != nil {
		t.Fatal(err)
	}
	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	supervisor.ResolveFactory = codexFactoryResolver((&scriptedStarter{}).factory)
	defer supervisor.Close()
	first, err := supervisor.ActivateHarnessProject(context.Background(), domain.ProjectHarnessActivationRequest{ProjectID: project.ID, ExpectedHead: project.HeadEventID, AgentName: "alice", Launch: domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), Action: domain.HarnessSessionNew, Directory: directory}})
	if err != nil {
		t.Fatal(err)
	}
	second, err := supervisor.HandoffHarnessProject(context.Background(), domain.ProjectHarnessHandoffRequest{ProjectID: project.ID, ExpectedHead: first.Project.HeadEventID, NewAgentName: "bob", Launch: domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), Action: domain.HarnessSessionNew, Directory: directory}})
	if err != nil {
		t.Fatal(err)
	}
	if second.Project.Assignment == nil || second.Project.Assignment.AgentName != "bob" || second.Runtime.SessionID == first.Runtime.SessionID {
		t.Fatalf("handoff = first %#v second %#v", first, second)
	}
	threads, err := database.ListProjectThreads(context.Background(), project.ID)
	if err != nil || len(threads) != 2 {
		t.Fatalf("handoff threads = %#v, %v", threads, err)
	}
	if alice, err := database.GetNamedAgent(context.Background(), "alice"); err != nil || !alice.Idle {
		t.Fatalf("old agent = %#v, %v", alice, err)
	}
}

func TestProjectHandoffBlocksUntilExplicitForceTakeover(t *testing.T) {
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
	for _, name := range []string{"alice", "bob"} {
		if _, err := database.CreateNamedAgent(context.Background(), name, ""); err != nil {
			t.Fatal(err)
		}
	}
	directory := t.TempDir()
	project, err := database.CreateProject(context.Background(), domain.CreateProjectRequest{Name: "blocked handoff", Open: true})
	if err != nil {
		t.Fatal(err)
	}
	project, err = database.AssignProject(context.Background(), project.ID, project.HeadEventID, "alice")
	if err != nil {
		t.Fatal(err)
	}
	project, err = database.ActivateProjectAssignment(context.Background(), project.ID, project.HeadEventID, domain.ActivateProjectAssignmentRequest{Harness: "codex", ExternalThread: "orphan-thread", LaunchDirectory: directory})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := database.AcquireNamedAgent(context.Background(), "alice", "orphan-owner", time.Minute); err != nil {
		t.Fatal(err)
	}
	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	supervisor.ResolveFactory = codexFactoryResolver((&scriptedStarter{}).factory)
	defer supervisor.Close()
	_, err = supervisor.HandoffHarnessProject(context.Background(), domain.ProjectHarnessHandoffRequest{ProjectID: project.ID, ExpectedHead: project.HeadEventID, NewAgentName: "bob", Launch: domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), Action: domain.HarnessSessionNew, Directory: directory}})
	if !errors.Is(err, domain.ErrProjectRuntimeUnknown) {
		t.Fatalf("normal handoff = %v", err)
	}
	blocked, err := database.GetProject(context.Background(), project.ID)
	if err != nil {
		t.Fatal(err)
	}
	if blocked.Assignment == nil || blocked.Assignment.State != domain.AssignmentBlocked || blocked.Assignment.AgentName != "alice" {
		t.Fatalf("blocked project = %#v", blocked)
	}
	taken, err := supervisor.HandoffHarnessProject(context.Background(), domain.ProjectHarnessHandoffRequest{ProjectID: project.ID, ExpectedHead: blocked.HeadEventID, NewAgentName: "bob", Force: true, Launch: domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), Action: domain.HarnessSessionNew, Directory: directory}})
	if err != nil {
		t.Fatal(err)
	}
	if taken.Project.Assignment == nil || taken.Project.Assignment.AgentName != "bob" || taken.Project.Assignment.State != domain.AssignmentRunnable {
		t.Fatalf("forced takeover = %#v", taken)
	}
}

func TestRetiringAssignedAgentQuiescesAndLeavesProjectOpen(t *testing.T) {
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
	if _, err := database.CreateNamedAgent(context.Background(), "fred", ""); err != nil {
		t.Fatal(err)
	}
	directory := t.TempDir()
	project, err := database.CreateProject(context.Background(), domain.CreateProjectRequest{Name: "retire", Paths: []domain.ProjectPathInput{{DisplayPath: directory}}, Open: true})
	if err != nil {
		t.Fatal(err)
	}
	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	supervisor.ResolveFactory = codexFactoryResolver((&scriptedStarter{}).factory)
	defer supervisor.Close()
	activated, err := supervisor.ActivateHarnessProject(context.Background(), domain.ProjectHarnessActivationRequest{ProjectID: project.ID, ExpectedHead: project.HeadEventID, AgentName: "fred", Launch: domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), Action: domain.HarnessSessionNew, Directory: directory}})
	if err != nil {
		t.Fatal(err)
	}
	if err := supervisor.RetireHarnessAgent(context.Background(), domain.HarnessRetireAgentRequest{AgentName: "fred"}); err != nil {
		t.Fatal(err)
	}
	got, err := database.GetProject(context.Background(), activated.Project.ID)
	if err != nil {
		t.Fatal(err)
	}
	if got.Lifecycle != domain.ProjectOpen || got.Assignment != nil {
		t.Fatalf("project after retirement = %#v", got)
	}
	agent, err := database.GetNamedAgent(context.Background(), "fred")
	if err != nil || !agent.Retired {
		t.Fatalf("retired agent = %#v, %v", agent, err)
	}
	if _, err := database.CreateProject(context.Background(), domain.CreateProjectRequest{Name: "conflict", Paths: []domain.ProjectPathInput{{DisplayPath: directory}}, Open: true}); !errors.Is(err, domain.ErrResourceConflict) {
		t.Fatalf("retirement released project claim: %v", err)
	}
}

func TestAgentRetirementRetryResumesAfterDurableUnassign(t *testing.T) {
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
	if _, err := database.CreateNamedAgent(context.Background(), "alice", ""); err != nil {
		t.Fatal(err)
	}
	project, err := database.CreateProject(context.Background(), domain.CreateProjectRequest{Name: "retirement retry", Open: true})
	if err != nil {
		t.Fatal(err)
	}
	project, err = database.AssignProject(context.Background(), project.ID, project.HeadEventID, "alice")
	if err != nil {
		t.Fatal(err)
	}
	request := domain.HarnessRetireAgentRequest{RequestID: uuid.NewString(), AgentName: "alice", Force: true}
	if _, err := database.BeginAgentRetirement(context.Background(), domain.AgentRetirementOperation{ID: request.RequestID, AgentName: "alice", ProjectID: project.ID, Force: true}); err != nil {
		t.Fatal(err)
	}
	if err := database.AdvanceAgentRetirement(context.Background(), request.RequestID, "quiesced", ""); err != nil {
		t.Fatal(err)
	}
	project, err = database.UnassignProject(context.Background(), project.ID, project.HeadEventID, true, "unknown")
	if err != nil {
		t.Fatal(err)
	}
	if err := database.AdvanceAgentRetirement(context.Background(), request.RequestID, "unassigned", ""); err != nil {
		t.Fatal(err)
	}
	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	defer supervisor.Close()
	if err := supervisor.RetireHarnessAgent(context.Background(), request); err != nil {
		t.Fatal(err)
	}
	agent, err := database.GetNamedAgent(context.Background(), "alice")
	if err != nil || !agent.Retired {
		t.Fatalf("retired agent = %#v, %v", agent, err)
	}
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
	supervisor := New(lifetime, database, harnessbridge.NewMemoryLedger())
	supervisor.ResolveFactory = codexFactoryResolver(starter.factory)
	defer supervisor.Close()
	directory := t.TempDir()
	secret := "environment-secret-must-not-persist"
	request := domain.HarnessLaunchRequest{Harness: "codex",
		RequestID: uuid.NewString(), AgentName: "fred", Action: domain.HarnessSessionNew, Directory: directory,
		Environment: []string{"PATH=/caller/bin", "TOKEN=" + secret}, InitialPrompt: "begin",
	}
	result, err := supervisor.LaunchHarnessAgent(context.Background(), request)
	if err != nil || result.Phase != domain.HarnessRuntimeRunning || result.SessionID != "thread-1" {
		t.Fatalf("launch = %#v, %v", result, err)
	}
	duplicate, err := supervisor.LaunchHarnessAgent(context.Background(), request)
	if err != nil || duplicate.SessionID != result.SessionID || starter.starts != 1 {
		t.Fatalf("duplicate = %#v, %v, starts=%d", duplicate, err, starter.starts)
	}
	changed := request
	changed.InitialPrompt = "different"
	if _, err := supervisor.LaunchHarnessAgent(context.Background(), changed); err == nil || strings.Contains(err.Error(), secret) {
		t.Fatalf("request ID conflict = %v", err)
	}
	second := domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), AgentName: "jane", Action: domain.HarnessSessionNew, Directory: directory, Environment: []string{"TOKEN=" + secret}}
	secondResult, err := supervisor.LaunchHarnessAgent(context.Background(), second)
	if err != nil || secondResult.Phase != domain.HarnessRuntimeRunning || starter.starts != 2 {
		t.Fatalf("second agent = %#v, %v, starts=%d", secondResult, err, starter.starts)
	}
	if running, _ := supervisor.HarnessAgentRuntime(context.Background(), "fred"); running.Phase != domain.HarnessRuntimeRunning {
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
	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	supervisor.ResolveFactory = codexFactoryResolver(starter.factory)
	supervisor.DecodeOptions = decodeCodexTestOptions
	defaultLoads := 0
	supervisor.LoadLaunchDefaults = func() (domain.HarnessLaunchDefaults, error) {
		defaultLoads++
		return domain.HarnessLaunchDefaults{}, nil
	}
	defer supervisor.Close()
	directory := t.TempDir()
	originalEnvironment := []string{"PATH=/original/bin", "TOKEN=original-secret"}
	launched, err := supervisor.LaunchHarnessAgent(context.Background(), domain.HarnessLaunchRequest{Harness: "codex",
		RequestID: uuid.NewString(), AgentName: "fred", Action: domain.HarnessSessionNew,
		Directory: directory, Repository: model.RepositoryContext{Directory: directory, Branch: "main"},
		Environment: originalEnvironment, InitialPrompt: "begin once", ProviderOptions: codexOptions(true),
	})
	if err != nil || launched.Phase != domain.HarnessRuntimeRunning {
		t.Fatalf("initial launch = %#v, %v", launched, err)
	}
	supervisor.mu.Lock()
	lastGood := cloneLaunchRequest(supervisor.lastGood["fred"])
	supervisor.mu.Unlock()
	defer clearLaunchEnvironment(&lastGood)
	if lastGood.SessionID != launched.SessionID || lastGood.InitialPrompt != "" || !codexYolo(lastGood.ProviderOptions) || strings.Join(lastGood.Environment, "|") != strings.Join(originalEnvironment, "|") {
		t.Fatalf("last known good launch = %#v", lastGood)
	}
	if _, err := supervisor.StopHarnessAgent(context.Background(), "fred"); err != nil {
		t.Fatal(err)
	}
	agent, err := database.GetNamedAgent(context.Background(), "fred")
	if err != nil || agent.Active {
		t.Fatalf("offline agent = %#v, %v", agent, err)
	}
	message := model.Message{SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: agent.MailboxID, Body: "wake up"}
	supervisor.WakeHarnessAgent(message, []string{"PATH=/new/sender", "TOKEN=new-secret"})
	supervisor.WakeHarnessAgent(message, []string{"PATH=/duplicate"})
	woken := waitForRunningRuntime(t, supervisor, "fred")
	if woken.SessionID != launched.SessionID || woken.Directory != directory {
		t.Fatalf("woken runtime = %#v", woken)
	}
	starter.mu.Lock()
	starts := starter.starts
	environments := append([][]string(nil), starter.environments...)
	starter.mu.Unlock()
	if starts != 2 || len(environments) != 2 || strings.Join(environments[1], "|") != strings.Join(originalEnvironment, "|") {
		t.Fatalf("starts=%d environments=%#v", starts, environments)
	}
	if defaultLoads != 0 {
		t.Fatalf("last-known-good wake loaded defaults %d times", defaultLoads)
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
	first := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	first.ResolveFactory = codexFactoryResolver(firstStarter.factory)
	first.DecodeOptions = decodeCodexTestOptions
	launched, err := first.LaunchHarnessAgent(context.Background(), domain.HarnessLaunchRequest{Harness: "codex",
		RequestID: uuid.NewString(), AgentName: "fred", Action: domain.HarnessSessionNew,
		Directory: directory, Repository: model.RepositoryContext{Directory: directory}, Environment: []string{"TOKEN=old-daemon-secret"}, ProviderOptions: codexOptions(true),
	})
	if err != nil || launched.Phase != domain.HarnessRuntimeRunning {
		t.Fatalf("initial launch = %#v, %v", launched, err)
	}
	if err := first.Close(); err != nil {
		t.Fatal(err)
	}

	secondStarter := &scriptedStarter{}
	second := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	second.ResolveFactory = codexFactoryResolver(secondStarter.factory)
	second.DecodeOptions = decodeCodexTestOptions
	defaultLoads := 0
	second.LoadLaunchDefaults = func() (domain.HarnessLaunchDefaults, error) {
		defaultLoads++
		return domain.HarnessLaunchDefaults{Harness: "codex", ProviderOptions: codexOptions(true)}, nil
	}
	defer second.Close()
	agent, err := database.GetNamedAgent(context.Background(), "fred")
	if err != nil || agent.Active || agent.CurrentSessionID != launched.SessionID {
		t.Fatalf("persisted agent = %#v, %v", agent, err)
	}
	senderEnvironment := []string{"PATH=/sender/bin", "TOKEN=sender-secret"}
	second.WakeHarnessAgent(model.Message{SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: agent.MailboxID}, senderEnvironment)
	woken := waitForRunningRuntime(t, second, "fred")
	if woken.SessionID != launched.SessionID || woken.Directory != directory {
		t.Fatalf("woken runtime = %#v", woken)
	}
	secondStarter.mu.Lock()
	environments := append([][]string(nil), secondStarter.environments...)
	secondStarter.mu.Unlock()
	if len(environments) != 1 || strings.Join(environments[0], "|") != strings.Join(senderEnvironment, "|") {
		t.Fatalf("restart environments = %#v", environments)
	}
	second.mu.Lock()
	lastGood := cloneLaunchRequest(second.lastGood["fred"])
	second.mu.Unlock()
	defer clearLaunchEnvironment(&lastGood)
	if defaultLoads != 1 || !codexYolo(lastGood.ProviderOptions) {
		t.Fatalf("restart launch defaults: loads=%d request=%#v", defaultLoads, lastGood)
	}
}

func waitForRunningRuntime(t *testing.T, supervisor *Supervisor, name string) domain.HarnessRuntime {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for {
		runtime, err := supervisor.HarnessAgentRuntime(context.Background(), name)
		if err != nil {
			t.Fatal(err)
		}
		if runtime.Phase == domain.HarnessRuntimeRunning {
			return runtime
		}
		if time.Now().After(deadline) {
			t.Fatalf("agent %s did not wake; last runtime %#v", name, runtime)
		}
		time.Sleep(time.Millisecond)
	}
}

func receiveHarnessInstance(t *testing.T, launched <-chan harness.Instance) harness.Instance {
	t.Helper()
	select {
	case instance := <-launched:
		return instance
	case <-time.After(3 * time.Second):
		t.Fatal("harness instance did not launch")
		return nil
	}
}

func waitForSupervisorMessage(t *testing.T, database *store.SQLite, id string, ready func(model.Message) bool) model.Message {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		message, err := database.Get(context.Background(), id)
		if err == nil && ready(message) {
			return message
		}
		time.Sleep(2 * time.Millisecond)
	}
	t.Fatalf("message %s did not reach expected state", id)
	return model.Message{}
}

func waitForSupervisorBody(t *testing.T, database *store.SQLite, body string) model.Message {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		messages, err := database.List(context.Background(), model.Filter{Limit: 100})
		if err != nil {
			t.Fatal(err)
		}
		for _, message := range messages {
			if message.Body == body {
				return message
			}
		}
		time.Sleep(2 * time.Millisecond)
	}
	t.Fatalf("message body %q was not stored", body)
	return model.Message{}
}

func waitForHarnessRuntimePhase(t *testing.T, supervisor *Supervisor, name string, phase domain.HarnessRuntimePhase) domain.HarnessRuntime {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		runtime, err := supervisor.HarnessAgentRuntime(context.Background(), name)
		if err == nil && runtime.Phase == phase {
			return runtime
		}
		time.Sleep(2 * time.Millisecond)
	}
	t.Fatalf("agent %s did not reach runtime phase %s", name, phase)
	return domain.HarnessRuntime{}
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
	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	supervisor.ResolveFactory = codexFactoryResolver(starter.factory)
	var diagnostics lockedBuffer
	supervisor.Logger = slog.New(slog.NewTextHandler(&diagnostics, &slog.HandlerOptions{Level: slog.LevelDebug}))
	defer supervisor.Close()
	result, err := supervisor.LaunchHarnessAgent(context.Background(), domain.HarnessLaunchRequest{Harness: "codex",
		RequestID: uuid.NewString(), AgentName: "fred", Action: domain.HarnessSessionNew, Directory: t.TempDir(), Environment: []string{"TOKEN=" + secret},
	})
	if !errors.Is(err, harness.ErrProviderUnavailable) || result.Phase != domain.HarnessRuntimeFailed || strings.Contains(result.Error, secret) || strings.Contains(err.Error(), secret) {
		t.Fatalf("failed launch = %#v, %v", result, err)
	}
	agent, getErr := database.GetNamedAgent(context.Background(), "fred")
	if getErr != nil || agent.CurrentSessionID != "" {
		t.Fatalf("failed launch selected a session: %#v, %v", agent, getErr)
	}
	deadline := time.Now().Add(time.Second)
	for !strings.Contains(diagnostics.String(), `msg="harness worker exited"`) && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	log := diagnostics.String()
	for _, expected := range []string{`msg="harness agent launch requested"`, `msg="harness worker registered"`, `msg="harness worker exited"`} {
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
	supervisor := New(lifetime, database, harnessbridge.NewMemoryLedger())
	supervisor.ResolveFactory = codexFactoryResolver(starter.factory)
	directory := t.TempDir()
	result, err := supervisor.LaunchHarnessAgent(context.Background(), domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), AgentName: "fred", Action: domain.HarnessSessionNew, Directory: directory})
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
	if err != nil || agent.CurrentSessionID != result.SessionID || agent.Active {
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
	supervisor := New(context.Background(), database, harnessbridge.NewMemoryLedger())
	supervisor.ResolveFactory = codexFactoryResolver(starter.factory)
	defer supervisor.Close()
	directory := t.TempDir()
	first, err := supervisor.LaunchHarnessAgent(context.Background(), domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), AgentName: "fred", Action: domain.HarnessSessionNew, Directory: directory})
	if err != nil || first.SessionID == "" {
		t.Fatalf("first launch = %#v, %v", first, err)
	}
	if _, err := supervisor.LaunchHarnessAgent(context.Background(), domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), AgentName: "fred", Action: domain.HarnessSessionNew, Directory: directory}); err == nil || !strings.Contains(err.Error(), "confirm") {
		t.Fatalf("unconfirmed replacement = %v", err)
	}
	if running, _ := supervisor.HarnessAgentRuntime(context.Background(), "fred"); running.Phase != domain.HarnessRuntimeRunning || running.SessionID != first.SessionID {
		t.Fatalf("unconfirmed replacement disturbed worker: %#v", running)
	}
	starter.mu.Lock()
	starter.fail = errors.New("replacement unavailable")
	starter.mu.Unlock()
	replacement, err := supervisor.LaunchHarnessAgent(context.Background(), domain.HarnessLaunchRequest{Harness: "codex", RequestID: uuid.NewString(), AgentName: "fred", Action: domain.HarnessSessionNew, Directory: directory, ConfirmSwitch: true})
	if !errors.Is(err, harness.ErrProviderUnavailable) || replacement.Phase != domain.HarnessRuntimeFailed {
		t.Fatalf("replacement = %#v, %v", replacement, err)
	}
	agent, err := database.GetNamedAgent(context.Background(), "fred")
	if err != nil || agent.CurrentSessionID != first.SessionID || agent.Active {
		t.Fatalf("selection after failed replacement = %#v, %v", agent, err)
	}
}
