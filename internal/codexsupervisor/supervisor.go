// Package codexsupervisor owns daemon-local Codex bridge lifecycles.
package codexsupervisor

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/codexbridge"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/logging"
	"github.com/wbbradley/hq/internal/model"
)

type StarterFactory func(environment []string) codexbridge.ProcessStarter

type Supervisor struct {
	Store              domain.Operations
	Ledger             codexbridge.DeliveryLedger
	Starter            StarterFactory
	LoadLaunchDefaults func() (domain.CodexLaunchDefaults, error)
	Sync               func(context.Context) error
	Logger             *slog.Logger

	ctx    context.Context
	cancel context.CancelFunc
	mu     sync.Mutex
	// subsMu stays independent because store changes may publish synchronously
	// while a supervisor operation holds mu.
	subsMu   sync.Mutex
	wakeWG   sync.WaitGroup
	workers  map[string]*worker
	receipts map[string]receipt
	lastGood map[string]domain.CodexLaunchRequest
	waking   map[string]bool
	subs     map[uint64]*localSubscription
	nextSub  uint64
}

type worker struct {
	requestID string
	digest    [32]byte
	cancel    context.CancelFunc
	ready     chan struct{}
	done      chan struct{}
	runtime   domain.CodexRuntime
	launch    domain.CodexLaunchRequest
}

type receipt struct {
	digest  [32]byte
	runtime domain.CodexRuntime
}

func New(ctx context.Context, store domain.Operations, ledger codexbridge.DeliveryLedger) *Supervisor {
	lifetime, cancel := context.WithCancel(ctx)
	if ledger == nil {
		ledger = codexbridge.NewMemoryLedger()
	}
	return &Supervisor{
		Store: store, Ledger: ledger, Logger: slog.New(slog.DiscardHandler), ctx: lifetime, cancel: cancel,
		workers: make(map[string]*worker), receipts: make(map[string]receipt), lastGood: make(map[string]domain.CodexLaunchRequest), waking: make(map[string]bool), subs: make(map[uint64]*localSubscription),
	}
}

func (s *Supervisor) Close() error {
	logger := s.logger().With("component", "codex_supervisor")
	logger.Info("Codex supervisor stopping")
	s.cancel()
	// Wake registration is gated by mu and checks the canceled lifetime. Cross
	// that gate before waiting so no goroutine can call Add concurrently with a
	// zero-count Wait.
	s.mu.Lock()
	s.mu.Unlock()
	s.wakeWG.Wait()
	s.mu.Lock()
	workers := make([]*worker, 0, len(s.workers))
	for _, current := range s.workers {
		workers = append(workers, current)
		current.cancel()
	}
	s.mu.Unlock()
	for _, current := range workers {
		<-current.done
	}
	s.mu.Lock()
	for name, request := range s.lastGood {
		clearLaunchEnvironment(&request)
		delete(s.lastGood, name)
	}
	s.mu.Unlock()
	logger.Info("Codex supervisor stopped", "workers", len(workers))
	return nil
}

func (s *Supervisor) LaunchCodexAgent(ctx context.Context, request domain.CodexLaunchRequest) (domain.CodexRuntime, error) {
	logger := s.logger().With("component", "codex_supervisor", "agent", request.AgentName, "request_id", request.RequestID)
	logger.Info("Codex agent launch requested", "action", request.Action, "session_id", request.SessionID, "directory", request.Directory, "yolo", request.Yolo, "environment_variables", len(request.Environment), "has_initial_prompt", strings.TrimSpace(request.InitialPrompt) != "")
	digest, err := validateLaunch(&request)
	if err != nil {
		logger.Warn("Codex agent launch rejected", "error", err)
		return domain.CodexRuntime{}, err
	}
	logger = s.logger().With("component", "codex_supervisor", "agent", request.AgentName, "request_id", request.RequestID, "directory", request.Directory)

	s.mu.Lock()
	if s.ctx.Err() != nil {
		s.mu.Unlock()
		return domain.CodexRuntime{}, errors.New("Codex supervisor is stopped")
	}
	if previous, ok := s.receipts[request.RequestID]; ok {
		s.mu.Unlock()
		if previous.digest != digest {
			logger.Warn("Codex launch request ID reused with different options")
			return domain.CodexRuntime{}, errors.New("Codex launch request ID was reused for different options")
		}
		logger.Debug("returning prior Codex launch receipt", "phase", previous.runtime.Phase, "thread_id", previous.runtime.ThreadID)
		return previous.runtime, nil
	}
	if current := s.workers[request.AgentName]; current != nil {
		if current.requestID == request.RequestID {
			if current.digest != digest {
				s.mu.Unlock()
				logger.Warn("active Codex launch request ID reused with different options")
				return domain.CodexRuntime{}, errors.New("Codex launch request ID was reused for different options")
			}
			s.mu.Unlock()
			return s.waitForLaunch(ctx, current, digest)
		}
		if sameDesiredRuntime(current.runtime, request) && current.runtime.Phase == domain.CodexRuntimeRunning {
			result := current.runtime
			s.receipts[request.RequestID] = receipt{digest: digest, runtime: result}
			s.mu.Unlock()
			logger.Info("Codex agent already running in requested session", "thread_id", result.ThreadID)
			return result, nil
		}
		if !request.ConfirmSwitch {
			s.mu.Unlock()
			logger.Warn("Codex session switch requires confirmation", "current_thread_id", current.runtime.ThreadID)
			return domain.CodexRuntime{}, errors.New("named agent is already running; confirm the session switch")
		}
		logger.Info("stopping Codex worker for confirmed session switch", "current_thread_id", current.runtime.ThreadID)
		current.runtime.Phase = domain.CodexRuntimeStopping
		current.cancel()
		s.mu.Unlock()
		select {
		case <-current.done:
		case <-ctx.Done():
			return domain.CodexRuntime{}, ctx.Err()
		}
		s.mu.Lock()
	}

	agent, err := s.Store.CreateNamedAgent(ctx, request.AgentName, "")
	if err != nil {
		s.mu.Unlock()
		logger.Error("resolve named agent for Codex launch", "error", err)
		return domain.CodexRuntime{}, err
	}
	resumeID, newThread, err := s.resolveAction(ctx, agent, request)
	if err != nil {
		s.mu.Unlock()
		logger.Warn("resolve requested Codex session", "error", err)
		return domain.CodexRuntime{}, err
	}
	workerContext, cancel := context.WithCancel(s.ctx)
	started := time.Now().UTC()
	current := &worker{
		requestID: request.RequestID, digest: digest, cancel: cancel, ready: make(chan struct{}), done: make(chan struct{}),
		runtime: domain.CodexRuntime{AgentName: request.AgentName, ThreadID: resumeID, Directory: request.Directory, Phase: domain.CodexRuntimeStarting, StartedAt: &started},
		launch:  cloneLaunchRequest(request),
	}
	s.workers[request.AgentName] = current
	logger.Info("Codex worker registered", "resume_thread_id", resumeID, "new_thread", newThread)
	environment := append([]string(nil), request.Environment...)
	request.Environment = nil
	starterFactory := s.Starter
	if starterFactory == nil {
		starterFactory = func(environment []string) codexbridge.ProcessStarter {
			return &codexbridge.ExecStarter{Environment: environment, UseEnvironment: true, Logger: logger.With("component", "codex_process")}
		}
	}
	starter := starterFactory(append([]string(nil), environment...))
	for index := range environment {
		environment[index] = ""
	}
	environment = nil
	options := codexbridge.Options{
		Directory: request.Directory, ResumeThreadID: resumeID, AgentName: request.AgentName, NewThread: newThread,
		InitialPrompt: request.InitialPrompt, Yolo: request.Yolo, Repository: request.Repository,
		Store: s.Store, Starter: starter, Stderr: logging.NewLineWriter(logger.With("component", "codex_process"), slog.LevelWarn, "Codex app-server stderr"), Sync: s.Sync, Ledger: s.Ledger,
		Logger:         logger.With("component", "codex_bridge"),
		Updates:        domain.ClientUpdates{Subscribe: s.Subscribe},
		SuppressStatus: true,
		OnReady:        func(ready codexbridge.BridgeReady) { s.markReady(current, ready) },
	}
	s.mu.Unlock()
	go s.runWorker(workerContext, current, options)
	return s.waitForLaunch(ctx, current, digest)
}

type localSubscription struct {
	supervisor *Supervisor
	id         uint64
	topics     map[domain.ChangeTopic]bool
	changes    chan domain.Invalidation
	once       sync.Once
}

func (s *localSubscription) Changes() <-chan domain.Invalidation { return s.changes }
func (s *localSubscription) Close() {
	s.once.Do(func() {
		s.supervisor.subsMu.Lock()
		delete(s.supervisor.subs, s.id)
		s.supervisor.subsMu.Unlock()
	})
}

func (s *Supervisor) Subscribe(_ context.Context, topics ...domain.ChangeTopic) (domain.ChangeSubscription, error) {
	s.subsMu.Lock()
	defer s.subsMu.Unlock()
	s.nextSub++
	subscription := &localSubscription{supervisor: s, id: s.nextSub, topics: make(map[domain.ChangeTopic]bool), changes: make(chan domain.Invalidation, 1)}
	for _, topic := range topics {
		subscription.topics[topic] = true
	}
	s.subs[subscription.id] = subscription
	return subscription, nil
}

func (s *Supervisor) Publish(change domain.Invalidation) {
	s.subsMu.Lock()
	defer s.subsMu.Unlock()
	for _, subscription := range s.subs {
		matched := change.FullSnapshot || len(subscription.topics) == 0
		for _, topic := range change.Topics {
			matched = matched || subscription.topics[topic]
		}
		if matched {
			select {
			case subscription.changes <- change:
			default:
			}
		}
	}
}

func validateLaunch(request *domain.CodexLaunchRequest) ([32]byte, error) {
	if _, err := uuid.Parse(request.RequestID); err != nil {
		return [32]byte{}, errors.New("Codex launch request_id must be a UUID")
	}
	request.AgentName = strings.TrimSpace(request.AgentName)
	if request.AgentName == "" {
		return [32]byte{}, errors.New("Codex launch requires an agent name")
	}
	if request.Action == "" {
		request.Action = domain.CodexSessionCurrent
	}
	if request.Action != domain.CodexSessionCurrent && request.Action != domain.CodexSessionNew && request.Action != domain.CodexSessionResume {
		return [32]byte{}, fmt.Errorf("unknown Codex session action %q", request.Action)
	}
	if request.Action == domain.CodexSessionResume && strings.TrimSpace(request.SessionID) == "" {
		return [32]byte{}, errors.New("resuming Codex requires an exact thread ID")
	}
	request.Directory = filepath.Clean(strings.TrimSpace(request.Directory))
	if !filepath.IsAbs(request.Directory) {
		return [32]byte{}, errors.New("Codex working directory must be absolute")
	}
	info, err := os.Stat(request.Directory)
	if err != nil {
		return [32]byte{}, errors.New("Codex working directory does not exist")
	}
	if !info.IsDir() {
		return [32]byte{}, errors.New("Codex working directory is not a directory")
	}
	request.Repository.Directory = request.Directory
	raw, err := json.Marshal(request)
	if err != nil {
		return [32]byte{}, errors.New("encode Codex launch request")
	}
	return sha256.Sum256(raw), nil
}

func (s *Supervisor) resolveAction(ctx context.Context, agent domain.NamedAgent, request domain.CodexLaunchRequest) (string, bool, error) {
	switch request.Action {
	case domain.CodexSessionNew:
		return "", true, nil
	case domain.CodexSessionResume:
		sessions, err := s.Store.ListNamedAgentSessions(ctx, request.AgentName)
		if err != nil {
			return "", false, err
		}
		for _, session := range sessions {
			if session.Harness == "codex" && session.SessionID == request.SessionID {
				return request.SessionID, false, nil
			}
		}
		return "", false, errors.New("Codex thread is not in this agent's session history")
	default:
		if agent.CurrentSessionID == "" {
			return "", true, nil
		}
		if agent.Harness != "codex" {
			return "", false, errors.New("named agent's current session does not belong to Codex")
		}
		return agent.CurrentSessionID, false, nil
	}
}

func (s *Supervisor) markReady(current *worker, ready codexbridge.BridgeReady) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if current.runtime.Phase != domain.CodexRuntimeStarting {
		return
	}
	current.runtime.Phase = domain.CodexRuntimeRunning
	current.runtime.ThreadID = ready.ThreadID
	current.runtime.Directory = ready.Directory
	relaunch := cloneLaunchRequest(current.launch)
	relaunch.RequestID = ""
	relaunch.Action = domain.CodexSessionResume
	relaunch.SessionID = ready.ThreadID
	relaunch.Directory = ready.Directory
	relaunch.Repository.Directory = ready.Directory
	relaunch.InitialPrompt = ""
	relaunch.ConfirmSwitch = false
	s.replaceLastGoodLocked(current.runtime.AgentName, relaunch)
	clearLaunchEnvironment(&current.launch)
	s.logger().Info("Codex worker ready", "component", "codex_supervisor", "agent", current.runtime.AgentName, "thread_id", ready.ThreadID, "directory", ready.Directory)
	close(current.ready)
}

func (s *Supervisor) runWorker(ctx context.Context, current *worker, options codexbridge.Options) {
	logger := s.logger().With("component", "codex_supervisor", "agent", current.runtime.AgentName, "request_id", current.requestID)
	logger.Info("Codex worker starting", "directory", current.runtime.Directory, "requested_thread_id", current.runtime.ThreadID)
	err := codexbridge.Run(ctx, options)
	s.mu.Lock()
	if current.runtime.Phase == domain.CodexRuntimeStarting {
		if ctx.Err() != nil {
			current.runtime.Phase = domain.CodexRuntimeOffline
			current.runtime.Error = ""
		} else if errors.Is(err, domain.ErrAgentOwned) {
			current.runtime.Phase = domain.CodexRuntimeConflict
			current.runtime.Error = "named agent is owned by another process"
		} else {
			current.runtime.Phase = domain.CodexRuntimeFailed
			current.runtime.Error = safeWorkerFailure(err)
		}
		close(current.ready)
	} else {
		current.runtime.Phase = domain.CodexRuntimeOffline
		current.runtime.Error = ""
	}
	if s.workers[current.runtime.AgentName] == current {
		delete(s.workers, current.runtime.AgentName)
	}
	clearLaunchEnvironment(&current.launch)
	close(current.done)
	s.mu.Unlock()
	if err != nil {
		logger.Error("Codex worker exited", "phase", current.runtime.Phase, "thread_id", current.runtime.ThreadID, "error", err)
	} else {
		logger.Info("Codex worker exited", "phase", current.runtime.Phase, "thread_id", current.runtime.ThreadID, "reason", "context canceled")
	}
}

// WakeCodexAgent asynchronously resumes the offline named Codex agent addressed
// by a newly committed local human message. The exact last successful launch
// configuration wins while this daemon remains alive. After a daemon restart,
// the durable selected thread and cwd are combined with the sending client's
// environment.
func (s *Supervisor) WakeCodexAgent(message model.Message, environment []string) {
	if message.SenderMailboxID != model.HumanMailboxID || strings.TrimSpace(message.RecipientMailboxID) == "" {
		return
	}
	agents, err := s.Store.ListNamedAgents(s.ctx)
	if err != nil {
		s.logger().Error("resolve named agent for message wake", "component", "codex_supervisor", "recipient_mailbox_id", message.RecipientMailboxID, "error", err)
		return
	}
	var agent domain.NamedAgent
	for _, candidate := range agents {
		if candidate.MailboxID == message.RecipientMailboxID {
			agent = candidate
			break
		}
	}
	if agent.Name == "" || agent.Retired || agent.Active || agent.Harness != "codex" || agent.CurrentSessionID == "" {
		return
	}

	s.mu.Lock()
	if s.ctx.Err() != nil || s.workers[agent.Name] != nil || s.waking[agent.Name] {
		s.mu.Unlock()
		return
	}
	request, found := s.lastGood[agent.Name]
	if found && request.SessionID == agent.CurrentSessionID {
		request = cloneLaunchRequest(request)
	} else {
		found = false
		request = domain.CodexLaunchRequest{
			Directory: agent.Context.Directory, Repository: agent.Context,
			Environment: append([]string(nil), environment...),
		}
		if len(request.Environment) == 0 {
			request.Environment = os.Environ()
		}
	}
	request.RequestID = uuid.NewString()
	request.AgentName = agent.Name
	request.Action = domain.CodexSessionResume
	request.SessionID = agent.CurrentSessionID
	request.InitialPrompt = ""
	request.ConfirmSwitch = false
	s.waking[agent.Name] = true
	s.wakeWG.Add(1)
	s.mu.Unlock()
	if !found {
		defaults, defaultsErr := s.launchDefaults()
		if defaultsErr != nil {
			clearLaunchEnvironment(&request)
			s.mu.Lock()
			delete(s.waking, agent.Name)
			s.mu.Unlock()
			s.wakeWG.Done()
			s.logger().Warn("automatic Codex agent wake could not load launch defaults", "component", "codex_supervisor", "agent", agent.Name, "error", defaultsErr)
			return
		}
		applyLaunchDefaults(&request, defaults)
	}

	go func() {
		defer s.wakeWG.Done()
		runtime, launchErr := s.LaunchCodexAgent(s.ctx, request)
		clearLaunchEnvironment(&request)
		s.mu.Lock()
		delete(s.waking, agent.Name)
		s.mu.Unlock()
		logger := s.logger().With("component", "codex_supervisor", "agent", agent.Name, "thread_id", agent.CurrentSessionID)
		if launchErr != nil || runtime.Phase != domain.CodexRuntimeRunning {
			logger.Warn("automatic Codex agent wake failed", "phase", runtime.Phase, "error", launchErr)
			return
		}
		logger.Info("automatic Codex agent wake succeeded", "directory", runtime.Directory)
	}()
}

func (s *Supervisor) launchDefaults() (domain.CodexLaunchDefaults, error) {
	if s.LoadLaunchDefaults == nil {
		return domain.CodexLaunchDefaults{}, nil
	}
	return s.LoadLaunchDefaults()
}

func applyLaunchDefaults(request *domain.CodexLaunchRequest, defaults domain.CodexLaunchDefaults) {
	request.Yolo = defaults.Yolo
}

func (s *Supervisor) replaceLastGoodLocked(name string, request domain.CodexLaunchRequest) {
	if previous, ok := s.lastGood[name]; ok {
		clearLaunchEnvironment(&previous)
	}
	s.lastGood[name] = request
}

func cloneLaunchRequest(request domain.CodexLaunchRequest) domain.CodexLaunchRequest {
	request.Environment = append([]string(nil), request.Environment...)
	return request
}

func clearLaunchEnvironment(request *domain.CodexLaunchRequest) {
	for index := range request.Environment {
		request.Environment[index] = ""
	}
	request.Environment = nil
}

func (s *Supervisor) logger() *slog.Logger {
	if s.Logger == nil {
		return slog.New(slog.DiscardHandler)
	}
	return s.Logger
}

func safeWorkerFailure(err error) string {
	message := strings.ToLower(fmt.Sprint(err))
	switch {
	case strings.Contains(message, "no rollout found"), strings.Contains(message, "resume codex thread"):
		return "requested Codex thread is unavailable on this node; the durable selection was not changed"
	case strings.Contains(message, "start codex app-server"):
		return "Codex app-server process could not be started"
	case strings.Contains(message, "initialize codex app-server"):
		return "Codex app-server initialization failed"
	case strings.Contains(message, "start codex thread"):
		return "Codex thread could not be started; the durable selection was not changed"
	default:
		return "Codex worker failed before becoming ready"
	}
}

func (s *Supervisor) waitForLaunch(ctx context.Context, current *worker, digest [32]byte) (domain.CodexRuntime, error) {
	select {
	case <-current.ready:
	case <-ctx.Done():
		return domain.CodexRuntime{}, ctx.Err()
	}
	s.mu.Lock()
	result := current.runtime
	s.receipts[current.requestID] = receipt{digest: digest, runtime: result}
	s.mu.Unlock()
	return result, nil
}

func (s *Supervisor) StopCodexAgent(ctx context.Context, name string) (domain.CodexRuntime, error) {
	s.mu.Lock()
	current := s.workers[strings.TrimSpace(name)]
	if current == nil {
		s.mu.Unlock()
		return domain.CodexRuntime{AgentName: name, Phase: domain.CodexRuntimeOffline}, nil
	}
	current.runtime.Phase = domain.CodexRuntimeStopping
	current.cancel()
	s.mu.Unlock()
	select {
	case <-current.done:
		return domain.CodexRuntime{AgentName: name, ThreadID: current.runtime.ThreadID, Directory: current.runtime.Directory, Phase: domain.CodexRuntimeOffline}, nil
	case <-ctx.Done():
		return domain.CodexRuntime{}, ctx.Err()
	}
}

func (s *Supervisor) CodexAgentRuntime(_ context.Context, name string) (domain.CodexRuntime, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if current := s.workers[strings.TrimSpace(name)]; current != nil {
		return current.runtime, nil
	}
	return domain.CodexRuntime{AgentName: name, Phase: domain.CodexRuntimeOffline}, nil
}

func sameDesiredRuntime(runtime domain.CodexRuntime, request domain.CodexLaunchRequest) bool {
	if runtime.AgentName != request.AgentName || runtime.Directory != request.Directory {
		return false
	}
	switch request.Action {
	case domain.CodexSessionResume:
		return runtime.ThreadID == request.SessionID
	case domain.CodexSessionNew:
		return false
	default:
		return true
	}
}

var _ domain.CodexRuntimeController = (*Supervisor)(nil)
var _ domain.CodexRuntimeAutoStarter = (*Supervisor)(nil)
