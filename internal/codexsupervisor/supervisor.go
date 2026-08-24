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
	"os/exec"
	"path/filepath"
	"slices"
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
	Store              domain.ProjectRuntimeStore
	Ledger             codexbridge.DeliveryLedger
	Starter            StarterFactory
	LoadLaunchDefaults func() (domain.CodexLaunchDefaults, error)
	Sync               func(context.Context) error
	RunGit             GitRunner
	Logger             *slog.Logger

	ctx    context.Context
	cancel context.CancelFunc
	mu     sync.Mutex
	// subsMu stays independent because store changes may publish synchronously
	// while a supervisor operation holds mu.
	subsMu            sync.Mutex
	wakeWG            sync.WaitGroup
	reconcileWG       sync.WaitGroup
	reconcileOnce     sync.Once
	reconcileTrigger  chan struct{}
	ReconcileInterval time.Duration
	workers           map[string]*worker
	receipts          map[string]receipt
	lastGood          map[string]domain.CodexLaunchRequest
	provisionMu       sync.Mutex
	waking            map[string]bool
	subs              map[uint64]*localSubscription
	nextSub           uint64
}

type GitRunner func(context.Context, string, ...string) ([]byte, error)

type worker struct {
	requestID string
	digest    [32]byte
	cancel    context.CancelFunc
	ready     chan struct{}
	done      chan struct{}
	runtime   domain.CodexRuntime
	launch    domain.CodexLaunchRequest
	projectID string
}

type receipt struct {
	digest  [32]byte
	runtime domain.CodexRuntime
}

func New(ctx context.Context, store domain.ProjectRuntimeStore, ledger codexbridge.DeliveryLedger) *Supervisor {
	lifetime, cancel := context.WithCancel(ctx)
	if ledger == nil {
		ledger = codexbridge.NewMemoryLedger()
	}
	supervisor := &Supervisor{
		Store: store, Ledger: ledger, Logger: slog.New(slog.DiscardHandler), ctx: lifetime, cancel: cancel,
		workers: make(map[string]*worker), receipts: make(map[string]receipt), lastGood: make(map[string]domain.CodexLaunchRequest), waking: make(map[string]bool), subs: make(map[uint64]*localSubscription), reconcileTrigger: make(chan struct{}, 1),
	}
	supervisor.recoverIncompleteProjectActivations()
	return supervisor
}

func (s *Supervisor) Close() error {
	logger := s.logger().With("component", "codex_supervisor")
	logger.Info("Codex supervisor stopping")
	s.cancel()
	s.reconcileWG.Wait()
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
	return s.launchCodexAgent(ctx, request, nil)
}

type projectLaunchBinding struct {
	projectID, assignmentID, expectedHead, projectThreadID, mailboxID, projectName string
	runnable                                                                       bool
}

func (s *Supervisor) launchCodexAgent(ctx context.Context, request domain.CodexLaunchRequest, project *projectLaunchBinding) (domain.CodexRuntime, error) {
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
		if (project == nil && current.projectID != "") || (project != nil && current.projectID != "" && current.projectID != project.projectID) {
			s.mu.Unlock()
			return domain.CodexRuntime{}, fmt.Errorf("named agent is running for project %s: %w", current.projectID, domain.ErrAgentAssigned)
		}
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
	if agent.AssignedProjectID != "" && (project == nil || agent.AssignedProjectID != project.projectID) {
		s.mu.Unlock()
		logger.Warn("direct Codex launch rejected for project-assigned agent", "project_id", agent.AssignedProjectID)
		return domain.CodexRuntime{}, fmt.Errorf("launch direct thread for assigned agent: %w", domain.ErrAgentAssigned)
	}
	var resumeID string
	var newThread bool
	if project != nil {
		resumeID, newThread = request.SessionID, request.Action != domain.CodexSessionResume
	} else {
		resumeID, newThread, err = s.resolveAction(ctx, agent, request)
	}
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
		launch:  cloneLaunchRequest(request), projectID: func() string {
			if project != nil {
				return project.projectID
			}
			return ""
		}(),
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
	if project != nil {
		options.ProjectStore = s.Store
		options.ProjectID = project.projectID
		options.ProjectReady = func(ready codexbridge.BridgeReady) (codexbridge.ProjectBinding, error) {
			if project.runnable {
				current, err := s.Store.GetProject(s.ctx, project.projectID)
				if err != nil {
					return codexbridge.ProjectBinding{}, err
				}
				if current.Lifecycle != domain.ProjectOpen || current.Assignment == nil || current.Assignment.ID != project.assignmentID || current.Assignment.State != domain.AssignmentRunnable || current.Assignment.SelectedThreadID != project.projectThreadID {
					return codexbridge.ProjectBinding{}, domain.ErrProjectThreadMismatch
				}
				threads, err := s.Store.ListProjectThreads(s.ctx, current.ID)
				if err != nil {
					return codexbridge.ProjectBinding{}, err
				}
				matched := false
				for _, thread := range threads {
					if thread.ID == project.projectThreadID && thread.ExternalID == ready.ThreadID && thread.AgentName == current.Assignment.AgentName && !thread.RetiredAgent {
						matched = true
						break
					}
				}
				if !matched {
					return codexbridge.ProjectBinding{}, domain.ErrProjectThreadMismatch
				}
				return codexbridge.ProjectBinding{ProjectID: current.ID, AssignmentID: current.Assignment.ID, ProjectThreadID: current.Assignment.SelectedThreadID, MailboxID: current.MailboxID, ProjectName: current.Name}, nil
			}
			activation := domain.ActivateProjectAssignmentRequest{ThreadID: project.projectThreadID, Harness: "codex", ExternalThread: ready.ThreadID, LaunchDirectory: ready.Directory}
			activated, err := s.Store.ActivateProjectAssignment(s.ctx, project.projectID, project.expectedHead, activation)
			if err != nil {
				return codexbridge.ProjectBinding{}, err
			}
			return codexbridge.ProjectBinding{ProjectID: activated.ID, AssignmentID: activated.Assignment.ID, ProjectThreadID: activated.Assignment.SelectedThreadID, MailboxID: activated.MailboxID, ProjectName: activated.Name}, nil
		}
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
	if change.FullSnapshot || slices.Contains(change.Topics, domain.TopicMessages) || slices.Contains(change.Topics, domain.TopicProjects) || slices.Contains(change.Topics, domain.TopicAgents) {
		s.triggerWorkReconciliation()
	}
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

// StartWorkReconciliation begins the durable pending-work loop. Call it only
// after the store observer is installed; the initial trigger then closes the
// startup scan/observer race.
func (s *Supervisor) StartWorkReconciliation() {
	s.reconcileOnce.Do(func() {
		s.reconcileWG.Add(1)
		go s.runWorkReconciliation()
		s.triggerWorkReconciliation()
	})
}

func (s *Supervisor) triggerWorkReconciliation() {
	select {
	case s.reconcileTrigger <- struct{}{}:
	default:
	}
}

func (s *Supervisor) runWorkReconciliation() {
	defer s.reconcileWG.Done()
	interval := s.ReconcileInterval
	if interval <= 0 {
		interval = 30 * time.Second
	}
	timer := time.NewTimer(interval)
	defer timer.Stop()
	for {
		select {
		case <-s.ctx.Done():
			return
		case <-s.reconcileTrigger:
		case <-timer.C:
		}
		s.reconcilePendingWork()
		timer.Reset(interval)
	}
}

func (s *Supervisor) reconcilePendingWork() {
	work, err := s.Store.ListCodexPendingWork(s.ctx)
	if err != nil {
		if s.ctx.Err() == nil {
			s.logger().Warn("scan durable Codex pending work", "component", "codex_supervisor", "error", err)
		}
		return
	}
	for _, item := range work {
		s.WakeCodexAgent(model.Message{SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: item.MailboxID}, nil)
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
	if current.projectID == "" {
		s.replaceLastGoodLocked(current.runtime.AgentName, relaunch)
	}
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
	if agent.Name == "" {
		s.wakeCodexProject(message, environment)
		return
	}
	if agent.Retired || agent.Active || agent.Harness != "codex" || agent.CurrentSessionID == "" {
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

func (s *Supervisor) wakeCodexProject(message model.Message, environment []string) {
	projects, err := s.Store.ListProjects(s.ctx, false)
	if err != nil {
		s.logger().Error("resolve project for message wake", "component", "codex_supervisor", "error", err)
		return
	}
	var project domain.Project
	for _, candidate := range projects {
		if candidate.MailboxID == message.RecipientMailboxID {
			project = candidate
			break
		}
	}
	if project.ID == "" || project.Lifecycle != domain.ProjectOpen || project.Assignment == nil || project.Assignment.State != domain.AssignmentRunnable {
		return
	}
	threads, err := s.Store.ListProjectThreads(s.ctx, project.ID)
	if err != nil {
		return
	}
	var selected domain.ProjectThread
	for _, thread := range threads {
		if thread.ID == project.Assignment.SelectedThreadID {
			selected = thread
			break
		}
	}
	if selected.ID == "" || selected.RetiredAgent || selected.Harness != "codex" {
		return
	}
	if err := projectResumeDirectorySafe(project, selected); err != nil {
		s.logger().Warn("automatic project wake requires explicit directory decision", "component", "codex_supervisor", "project_id", project.ID, "thread_id", selected.ID, "directory", selected.LaunchDir, "error", err)
		return
	}
	s.mu.Lock()
	if s.ctx.Err() != nil || s.workers[project.Assignment.AgentName] != nil || s.waking[project.Assignment.AgentName] {
		s.mu.Unlock()
		return
	}
	s.waking[project.Assignment.AgentName] = true
	s.wakeWG.Add(1)
	s.mu.Unlock()
	request := domain.CodexLaunchRequest{RequestID: uuid.NewString(), AgentName: project.Assignment.AgentName, Action: domain.CodexSessionResume, SessionID: selected.ExternalID, Directory: selected.LaunchDir, Repository: model.RepositoryContext{Directory: selected.LaunchDir}, Environment: append([]string(nil), environment...)}
	if len(request.Environment) == 0 {
		request.Environment = os.Environ()
	}
	if defaults, defaultsErr := s.launchDefaults(); defaultsErr == nil {
		applyLaunchDefaults(&request, defaults)
	} else {
		clearLaunchEnvironment(&request)
		s.mu.Lock()
		delete(s.waking, project.Assignment.AgentName)
		s.mu.Unlock()
		s.wakeWG.Done()
		return
	}
	binding := &projectLaunchBinding{projectID: project.ID, assignmentID: project.Assignment.ID, expectedHead: project.HeadEventID, projectThreadID: selected.ID, mailboxID: project.MailboxID, projectName: project.Name, runnable: true}
	go func() {
		defer s.wakeWG.Done()
		runtime, launchErr := s.launchCodexAgent(s.ctx, request, binding)
		clearLaunchEnvironment(&request)
		s.mu.Lock()
		delete(s.waking, project.Assignment.AgentName)
		s.mu.Unlock()
		if launchErr != nil || runtime.Phase != domain.CodexRuntimeRunning {
			s.logger().Warn("automatic project wake failed", "component", "codex_supervisor", "project_id", project.ID, "agent", project.Assignment.AgentName, "error", launchErr)
		}
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
		if agent, err := s.Store.GetNamedAgent(ctx, strings.TrimSpace(name)); err == nil && agent.Active {
			return domain.CodexRuntime{AgentName: name, Phase: domain.CodexRuntimeConflict, Error: "runtime ownership remains active but is not controlled by this daemon"}, domain.ErrAgentOwned
		}
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

func (s *Supervisor) CloseCodexProject(ctx context.Context, request domain.ProjectCodexCloseRequest) (domain.Project, error) {
	project, err := s.Store.GetProject(ctx, request.ProjectID)
	if err != nil {
		return project, err
	}
	if project.ReadOnlyReplica {
		if request.RequestID == "" {
			request.RequestID = uuid.NewString()
		}
		return s.queueRemoteProjectRuntime(ctx, project, request.ExpectedHead, domain.ProjectCodexCloseCommand(request))
	}
	if request.RequestID == "" {
		request.RequestID = uuid.NewString()
	}
	operation, err := s.Store.BeginProjectRuntimeOperation(ctx, domain.ProjectRuntimeOperation{ID: request.RequestID, Kind: "close", ProjectID: request.ProjectID, ExpectedHead: request.ExpectedHead, Force: request.Force, Archive: request.Archive})
	if err != nil {
		return project, err
	}
	if operation.State == "blocked" || operation.State == "failed" {
		return project, errors.New(operation.LastError)
	}
	project, err = s.Store.GetProject(ctx, request.ProjectID)
	if err != nil {
		return project, err
	}
	if project.Lifecycle == domain.ProjectClosed {
		if request.Archive && !project.Archived {
			project, err = s.Store.SetProjectArchived(ctx, project.ID, project.HeadEventID, true)
			if err != nil {
				return project, err
			}
		}
		_ = s.Store.AdvanceProjectRuntimeOperation(ctx, request.RequestID, "completed", project.HeadEventID, "")
		return project, nil
	}
	if project.Lifecycle == domain.ProjectOpen {
		project, err = s.Store.BeginCloseProject(ctx, project.ID, project.HeadEventID)
		if err != nil {
			return project, err
		}
		if err := s.Store.AdvanceProjectRuntimeOperation(ctx, request.RequestID, "closing", project.HeadEventID, ""); err != nil {
			return project, err
		}
	} else if project.Lifecycle != domain.ProjectClosing {
		return project, fmt.Errorf("close project: %w", domain.ErrProjectState)
	}
	observation := "stopped"
	if project.Assignment != nil {
		managed := s.hasManagedWorker(project.Assignment.AgentName)
		var stopErr error
		if managed {
			_, stopErr = s.StopCodexAgent(ctx, project.Assignment.AgentName)
		} else {
			stopErr = domain.ErrProjectRuntimeUnknown
		}
		if stopErr != nil {
			if !request.Force {
				diagnostic := fmt.Sprintf("quiesce project runtime: %v", stopErr)
				_ = s.Store.AdvanceProjectRuntimeOperation(context.Background(), request.RequestID, "blocked", project.HeadEventID, diagnostic)
				return project, fmt.Errorf("quiesce project runtime: %w", stopErr)
			}
			observation = "unknown"
		}
	}
	project, err = s.Store.FinalizeCloseProject(ctx, project.ID, project.HeadEventID, request.Force, observation)
	if err != nil {
		return project, err
	}
	if request.Archive {
		project, err = s.Store.SetProjectArchived(ctx, project.ID, project.HeadEventID, true)
		if err != nil {
			return project, err
		}
	}
	_ = s.Store.AdvanceProjectRuntimeOperation(ctx, request.RequestID, "completed", project.HeadEventID, "")
	return project, nil
}

func (s *Supervisor) HandoffCodexProject(ctx context.Context, request domain.ProjectCodexHandoffRequest) (domain.ProjectCodexActivation, error) {
	project, err := s.Store.GetProject(ctx, request.ProjectID)
	if err != nil {
		return domain.ProjectCodexActivation{}, err
	}
	if project.ReadOnlyReplica {
		if request.RequestID == "" {
			request.RequestID = request.Launch.RequestID
		}
		if request.RequestID == "" {
			request.RequestID = uuid.NewString()
		}
		request.Launch.Environment = nil
		queued, queueErr := s.queueRemoteProjectRuntime(ctx, project, request.ExpectedHead, domain.ProjectCodexHandoffCommand(request))
		return domain.ProjectCodexActivation{Project: queued, Runtime: domain.CodexRuntime{AgentName: request.NewAgentName, Phase: domain.CodexRuntimePending}}, queueErr
	}
	if request.RequestID == "" {
		request.RequestID = request.Launch.RequestID
	}
	if request.RequestID == "" {
		request.RequestID = uuid.NewString()
	}
	operation, err := s.Store.BeginProjectRuntimeOperation(ctx, domain.ProjectRuntimeOperation{ID: request.RequestID, Kind: "handoff", ProjectID: request.ProjectID, ExpectedHead: request.ExpectedHead, TargetAgent: request.NewAgentName, Force: request.Force})
	if err != nil {
		return domain.ProjectCodexActivation{}, err
	}
	if operation.State == "blocked" || operation.State == "failed" {
		return domain.ProjectCodexActivation{}, errors.New(operation.LastError)
	}
	project, err = s.Store.GetProject(ctx, request.ProjectID)
	if err != nil {
		return domain.ProjectCodexActivation{}, err
	}
	if project.Assignment != nil && project.Assignment.AgentName == request.NewAgentName && project.Assignment.State == domain.AssignmentRunnable {
		_ = s.Store.AdvanceProjectRuntimeOperation(ctx, request.RequestID, "completed", project.HeadEventID, "")
		runtime, _ := s.CodexAgentRuntime(ctx, request.NewAgentName)
		return domain.ProjectCodexActivation{Project: project, Runtime: runtime}, nil
	}
	if project.Lifecycle != domain.ProjectOpen || (project.Assignment == nil && operation.State == "started") {
		return domain.ProjectCodexActivation{}, fmt.Errorf("handoff project: %w", domain.ErrProjectState)
	}
	if project.Assignment != nil {
		oldAgent := project.Assignment.AgentName
		var stopErr error
		if s.hasManagedWorker(oldAgent) {
			_, stopErr = s.StopCodexAgent(ctx, oldAgent)
		} else {
			stopErr = domain.ErrProjectRuntimeUnknown
		}
		observation := "stopped"
		if stopErr != nil {
			if !request.Force {
				project, _ = s.Store.BlockProjectAssignment(ctx, project.ID, project.HeadEventID, "runtime quiescence could not be confirmed")
				diagnostic := fmt.Sprintf("quiesce project handoff: %v", stopErr)
				_ = s.Store.AdvanceProjectRuntimeOperation(context.Background(), request.RequestID, "blocked", project.HeadEventID, diagnostic)
				return domain.ProjectCodexActivation{}, fmt.Errorf("quiesce project handoff: %w", stopErr)
			}
			observation = "unknown"
		}
		project, err = s.Store.UnassignProject(ctx, project.ID, project.HeadEventID, request.Force, observation)
		if err != nil {
			return domain.ProjectCodexActivation{}, err
		}
		if err := s.Store.AdvanceProjectRuntimeOperation(ctx, request.RequestID, "unassigned", project.HeadEventID, ""); err != nil {
			return domain.ProjectCodexActivation{}, err
		}
	}
	if request.Launch.RequestID == "" {
		request.Launch.RequestID = request.RequestID
	}
	_ = s.Store.AdvanceProjectRuntimeOperation(ctx, request.RequestID, "activating", project.HeadEventID, "")
	activated, err := s.ActivateCodexProject(ctx, domain.ProjectCodexActivationRequest{ProjectID: project.ID, ExpectedHead: project.HeadEventID, AgentName: request.NewAgentName, Launch: request.Launch})
	if err != nil {
		_ = s.Store.AdvanceProjectRuntimeOperation(context.Background(), request.RequestID, "failed", project.HeadEventID, err.Error())
		return activated, err
	}
	_ = s.Store.AdvanceProjectRuntimeOperation(ctx, request.RequestID, "completed", activated.Project.HeadEventID, "")
	return activated, nil
}

func (s *Supervisor) RetireCodexAgent(ctx context.Context, request domain.CodexRetireAgentRequest) error {
	if request.RequestID == "" {
		request.RequestID = uuid.NewString()
	}
	agent, err := s.Store.GetNamedAgent(ctx, request.AgentName)
	if err != nil {
		return err
	}
	operation, err := s.Store.BeginAgentRetirement(ctx, domain.AgentRetirementOperation{ID: request.RequestID, AgentName: agent.Name, ProjectID: agent.AssignedProjectID, Force: request.Force})
	if err != nil {
		return err
	}
	if operation.State == "completed" {
		return nil
	}
	if operation.State == "blocked" || operation.State == "failed" {
		return errors.New(operation.LastError)
	}
	if agent.Retired {
		return s.Store.AdvanceAgentRetirement(ctx, operation.ID, "completed", "")
	}
	if operation.State == "started" {
		var stopErr error
		if s.hasManagedWorker(agent.Name) {
			_, stopErr = s.StopCodexAgent(ctx, agent.Name)
		} else if agent.AssignedProjectID != "" {
			stopErr = domain.ErrProjectRuntimeUnknown
		} else {
			_, stopErr = s.StopCodexAgent(ctx, agent.Name)
		}
		if stopErr != nil {
			if !request.Force {
				if agent.AssignedProjectID != "" {
					if project, getErr := s.Store.GetProject(ctx, agent.AssignedProjectID); getErr == nil {
						_, _ = s.Store.BlockProjectAssignment(ctx, project.ID, project.HeadEventID, "runtime quiescence could not be confirmed before retirement")
					}
				}
				diagnostic := fmt.Sprintf("quiesce retiring agent: %v", stopErr)
				_ = s.Store.AdvanceAgentRetirement(context.Background(), operation.ID, "blocked", diagnostic)
				return fmt.Errorf("quiesce retiring agent: %w", stopErr)
			}
		}
		if err := s.Store.AdvanceAgentRetirement(ctx, operation.ID, "quiesced", ""); err != nil {
			return err
		}
		operation.State = "quiesced"
	}
	agent, err = s.Store.GetNamedAgent(ctx, request.AgentName)
	if err != nil {
		return err
	}
	if operation.State == "quiesced" && agent.AssignedProjectID != "" {
		project, getErr := s.Store.GetProject(ctx, agent.AssignedProjectID)
		if getErr != nil {
			return getErr
		}
		observation := "stopped"
		if request.Force {
			observation = "unknown"
		}
		if _, err = s.Store.UnassignProject(ctx, project.ID, project.HeadEventID, request.Force, observation); err != nil {
			return err
		}
		if err := s.Store.AdvanceAgentRetirement(ctx, operation.ID, "unassigned", ""); err != nil {
			return err
		}
	}
	if err := s.Store.RetireNamedAgent(ctx, agent.Name); err != nil && !errors.Is(err, domain.ErrAgentRetired) {
		return err
	}
	return s.Store.AdvanceAgentRetirement(ctx, operation.ID, "completed", "")
}

func (s *Supervisor) ProvisionProjectWorktree(ctx context.Context, request domain.ProjectWorktreeRequest) (domain.Project, error) {
	if request.RequestID == "" {
		request.RequestID = uuid.NewString()
	}
	if request.ProjectID == "" {
		request.ProjectID = uuid.NewString()
	}
	if queued, remote, err := s.Store.QueueProjectWorktreeProvision(ctx, request); remote {
		return queued, err
	}
	operation, err := s.Store.BeginProjectWorktreeProvision(ctx, request)
	if err != nil {
		return domain.Project{}, err
	}
	if operation.State == "completed" {
		return s.Store.GetProject(ctx, operation.ProjectID)
	}
	if operation.State == "failed" {
		return domain.Project{}, errors.New(operation.LastError)
	}
	s.provisionMu.Lock()
	defer s.provisionMu.Unlock()
	if project, getErr := s.Store.GetProject(ctx, operation.ProjectID); getErr == nil {
		_ = s.Store.AdvanceProjectWorktreeProvision(ctx, operation.ID, "completed", "")
		return project, nil
	} else if !errors.Is(getErr, domain.ErrProjectNotFound) {
		return domain.Project{}, getErr
	}
	run := s.RunGit
	if run == nil {
		run = runGitCommand
	}
	if operation.State == "reserved" {
		created := false
		if _, statErr := os.Stat(operation.Request.Destination); statErr == nil {
			created, err = verifyProvisionedWorktree(ctx, run, operation)
			if err == nil && !created {
				err = errors.New("worktree destination already exists but does not match the reserved repository and branch")
			}
		} else if !errors.Is(statErr, os.ErrNotExist) {
			err = statErr
		} else {
			_, err = run(ctx, operation.Request.Repository, "worktree", "add", "-b", operation.Request.Branch, operation.Request.Destination, operation.Request.MergeBase)
			created = err == nil
		}
		if err != nil || !created {
			diagnostic := safeGitFailure(err)
			_ = s.Store.AdvanceProjectWorktreeProvision(context.Background(), operation.ID, "failed", diagnostic)
			return domain.Project{}, errors.New(diagnostic)
		}
		if err := s.Store.AdvanceProjectWorktreeProvision(ctx, operation.ID, "worktree-created", ""); err != nil {
			return domain.Project{}, err
		}
	}
	create := domain.CreateProjectRequest{ID: operation.ProjectID, HomeInstallation: operation.Request.HomeInstallation, Name: operation.Request.Name, Brief: operation.Request.Brief, PredecessorProjectID: operation.Request.PredecessorProjectID, PrimaryPath: operation.Request.PrimaryPath, Open: operation.Request.Open}
	create.Paths = append(create.Paths, domain.ProjectPathInput{DisplayPath: operation.Request.Destination})
	create.Paths = append(create.Paths, operation.Request.AdditionalPaths...)
	project, err := s.Store.CreateProject(domain.WithProjectProvisioning(ctx, operation.ID), create)
	if err != nil {
		return project, err
	}
	if err := s.Store.AdvanceProjectWorktreeProvision(ctx, operation.ID, "completed", ""); err != nil {
		return project, err
	}
	return project, nil
}

func runGitCommand(ctx context.Context, directory string, args ...string) ([]byte, error) {
	commandArgs := append([]string{"-C", directory}, args...)
	command := exec.CommandContext(ctx, "git", commandArgs...)
	output, err := command.CombinedOutput()
	if err != nil {
		return output, fmt.Errorf("git %s: %w: %s", strings.Join(args, " "), err, strings.TrimSpace(string(output)))
	}
	return output, nil
}

func verifyProvisionedWorktree(ctx context.Context, run GitRunner, operation domain.ProjectWorktreeOperation) (bool, error) {
	top, err := run(ctx, operation.Request.Destination, "rev-parse", "--show-toplevel")
	if err != nil {
		return false, err
	}
	canonicalTop, err := filepath.EvalSymlinks(strings.TrimSpace(string(top)))
	if err != nil || filepath.Clean(canonicalTop) != operation.CanonicalDestination {
		return false, err
	}
	branch, err := run(ctx, operation.Request.Destination, "branch", "--show-current")
	if err != nil {
		return false, err
	}
	if strings.TrimSpace(string(branch)) != operation.Request.Branch {
		return false, nil
	}
	repositoryCommon, err := gitCommonDirectory(ctx, run, operation.Request.Repository)
	if err != nil {
		return false, err
	}
	worktreeCommon, err := gitCommonDirectory(ctx, run, operation.Request.Destination)
	return repositoryCommon == worktreeCommon, err
}

func gitCommonDirectory(ctx context.Context, run GitRunner, directory string) (string, error) {
	raw, err := run(ctx, directory, "rev-parse", "--git-common-dir")
	if err != nil {
		return "", err
	}
	value := strings.TrimSpace(string(raw))
	if !filepath.IsAbs(value) {
		value = filepath.Join(directory, value)
	}
	value, err = filepath.Abs(value)
	if err != nil {
		return "", err
	}
	if resolved, resolveErr := filepath.EvalSymlinks(value); resolveErr == nil {
		value = resolved
	}
	return filepath.Clean(value), nil
}

func safeGitFailure(err error) string {
	if err == nil {
		return "Git worktree provisioning did not produce the reserved destination"
	}
	message := strings.TrimSpace(err.Error())
	if len(message) > 1000 {
		message = message[:1000]
	}
	return message
}

func (s *Supervisor) hasManagedWorker(name string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.workers[strings.TrimSpace(name)] != nil
}

func (s *Supervisor) CodexAgentRuntime(_ context.Context, name string) (domain.CodexRuntime, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if current := s.workers[strings.TrimSpace(name)]; current != nil {
		return current.runtime, nil
	}
	return domain.CodexRuntime{AgentName: name, Phase: domain.CodexRuntimeOffline}, nil
}

func (s *Supervisor) ActivateCodexProject(ctx context.Context, request domain.ProjectCodexActivationRequest) (result domain.ProjectCodexActivation, resultErr error) {
	request.AgentName = strings.TrimSpace(request.AgentName)
	if request.ProjectID == "" || request.ExpectedHead == "" || request.AgentName == "" {
		return result, errors.New("project activation requires project, expected head, and agent")
	}
	project, err := s.Store.GetProject(ctx, request.ProjectID)
	if err != nil {
		return result, err
	}
	if project.ReadOnlyReplica {
		request.Launch.Environment = nil
		queued, queueErr := s.queueRemoteProjectRuntime(ctx, project, request.ExpectedHead, domain.ProjectCodexActivateCommand(request))
		return domain.ProjectCodexActivation{Project: queued, Runtime: domain.CodexRuntime{AgentName: request.AgentName, Phase: domain.CodexRuntimePending}}, queueErr
	}
	if request.Launch.RequestID == "" {
		request.Launch.RequestID = uuid.NewString()
	}
	operation, err := s.Store.BeginProjectActivation(ctx, request.Launch.RequestID, request.ProjectID, request.ExpectedHead, request.AgentName)
	if err != nil {
		return result, err
	}
	if operation.State == "failed" {
		return result, errors.New(operation.LastError)
	}
	if operation.State == "runnable" {
		return s.resumeCompletedProjectActivation(ctx, operation, request.Launch)
	}
	if operation.State == "configuring" {
		_ = s.compensateProjectActivation(context.Background(), operation, "interrupted activation recovered before retry")
		return result, errors.New("prior project activation was interrupted and compensated; retry with a new request")
	}
	project, err = s.Store.GetProject(ctx, request.ProjectID)
	if err != nil {
		_ = s.Store.FailProjectActivation(ctx, operation.ID, err.Error())
		return result, err
	}
	if operation.PriorLifecycle == domain.ProjectClosed {
		project, err = s.Store.OpenProject(ctx, project.ID, project.HeadEventID)
		if err != nil {
			_ = s.Store.FailProjectActivation(ctx, operation.ID, err.Error())
			return result, err
		}
	} else {
		project, err = s.Store.ObserveProjectResources(ctx, project.ID, project.HeadEventID)
		if err != nil {
			_ = s.Store.FailProjectActivation(ctx, operation.ID, err.Error())
			return result, err
		}
	}
	defer func() {
		if resultErr != nil {
			_ = s.compensateProjectActivation(context.Background(), operation, "activation failed: "+resultErr.Error())
		}
	}()
	project, err = s.Store.AssignProject(ctx, project.ID, project.HeadEventID, request.AgentName)
	if err != nil {
		return result, err
	}
	operation.AssignmentID = project.Assignment.ID
	operation.State = "configuring"
	if err := s.Store.SetProjectActivationAssignment(ctx, operation.ID, project.Assignment.ID); err != nil {
		return result, err
	}
	launch := request.Launch
	launch.AgentName = request.AgentName
	launch.RequestID = operation.ID
	threads, err := s.Store.ListProjectThreads(ctx, project.ID)
	if err != nil {
		return result, err
	}
	projectThreadID := ""
	switch launch.Action {
	case "", domain.CodexSessionCurrent:
		launch.Action = domain.CodexSessionNew
		for _, thread := range threads {
			if thread.AgentName == request.AgentName && thread.Harness == "codex" && !thread.RetiredAgent {
				launch.Action, launch.SessionID, projectThreadID = domain.CodexSessionResume, thread.ExternalID, thread.ID
				break
			}
		}
	case domain.CodexSessionResume:
		for _, thread := range threads {
			if thread.AgentName == request.AgentName && thread.Harness == "codex" && thread.ExternalID == launch.SessionID && !thread.RetiredAgent {
				projectThreadID = thread.ID
				break
			}
		}
		if projectThreadID == "" {
			return result, domain.ErrProjectThreadMismatch
		}
	case domain.CodexSessionNew:
		launch.SessionID = ""
	default:
		return result, fmt.Errorf("unknown Codex session action %q", launch.Action)
	}
	binding := &projectLaunchBinding{projectID: project.ID, assignmentID: project.Assignment.ID, expectedHead: project.HeadEventID, projectThreadID: projectThreadID, mailboxID: project.MailboxID, projectName: project.Name}
	runtime, err := s.launchCodexAgent(ctx, launch, binding)
	if err != nil {
		return result, err
	}
	if runtime.Phase != domain.CodexRuntimeRunning {
		return result, errors.New(runtime.Error)
	}
	project, err = s.Store.GetProject(ctx, project.ID)
	if err != nil {
		return result, err
	}
	if project.Assignment == nil || project.Assignment.State != domain.AssignmentRunnable {
		return result, errors.New("project runtime became ready without a runnable assignment")
	}
	if err := s.Store.CompleteProjectActivation(ctx, operation.ID); err != nil {
		return result, err
	}
	return domain.ProjectCodexActivation{Project: project, Runtime: runtime}, nil
}

func (s *Supervisor) queueRemoteProjectRuntime(ctx context.Context, project domain.Project, expected string, data domain.ProjectCommandData) (domain.Project, error) {
	operation, body, err := domain.EncodeProjectCommand(data)
	if err != nil {
		return project, err
	}
	commandID := ""
	if mutation, ok := domain.MutationFromContext(ctx); ok {
		commandID = mutation.ID
	}
	return s.Store.QueueProjectCommand(ctx, domain.ProjectCommand{ID: commandID, ProjectID: project.ID, ExpectedHead: expected, Operation: operation, Body: body})
}

func (s *Supervisor) compensateProjectActivation(parent context.Context, operation domain.ProjectActivationOperation, diagnostic string) error {
	ctx, cancel := context.WithTimeout(parent, 10*time.Second)
	defer cancel()
	_, _ = s.StopCodexAgent(ctx, operation.AgentName)
	project, err := s.Store.GetProject(ctx, operation.ProjectID)
	if err != nil {
		return err
	}
	if project.Assignment != nil && project.Assignment.AgentName == operation.AgentName && (operation.AssignmentID == "" || project.Assignment.ID == operation.AssignmentID) {
		project, err = s.Store.AbortProjectAssignment(ctx, project.ID, project.HeadEventID, diagnostic)
		if err != nil {
			return err
		}
	}
	if operation.PriorLifecycle == domain.ProjectClosed && project.Lifecycle == domain.ProjectOpen && project.Assignment == nil {
		project, err = s.Store.BeginCloseProject(ctx, project.ID, project.HeadEventID)
		if err != nil {
			return err
		}
		project, err = s.Store.FinalizeCloseProject(ctx, project.ID, project.HeadEventID, false, "runtime unavailable during activation recovery")
		if err != nil {
			return err
		}
	}
	if operation.PriorLifecycle == domain.ProjectOpen && project.Lifecycle != domain.ProjectOpen {
		return domain.ErrProjectState
	}
	if operation.PriorLifecycle == domain.ProjectClosed && project.Lifecycle != domain.ProjectClosed {
		return domain.ErrProjectState
	}
	return s.Store.FailProjectActivation(ctx, operation.ID, diagnostic)
}

func (s *Supervisor) recoverIncompleteProjectActivations() {
	operations, err := s.Store.ListIncompleteProjectActivations(s.ctx)
	if err != nil {
		s.logger().Error("list incomplete project activations", "error", err)
		return
	}
	for _, operation := range operations {
		if err := s.compensateProjectActivation(s.ctx, operation, "daemon restarted during project activation"); err != nil {
			s.logger().Error("recover incomplete project activation", "project_id", operation.ProjectID, "operation_id", operation.ID, "error", err)
		}
	}
}

func (s *Supervisor) resumeCompletedProjectActivation(ctx context.Context, operation domain.ProjectActivationOperation, launch domain.CodexLaunchRequest) (domain.ProjectCodexActivation, error) {
	project, err := s.Store.GetProject(ctx, operation.ProjectID)
	if err != nil {
		return domain.ProjectCodexActivation{}, err
	}
	if project.Lifecycle != domain.ProjectOpen || project.Assignment == nil || project.Assignment.ID != operation.AssignmentID || project.Assignment.State != domain.AssignmentRunnable {
		return domain.ProjectCodexActivation{}, domain.ErrProjectState
	}
	if running, _ := s.CodexAgentRuntime(ctx, operation.AgentName); running.Phase == domain.CodexRuntimeRunning {
		return domain.ProjectCodexActivation{Project: project, Runtime: running}, nil
	}
	threads, err := s.Store.ListProjectThreads(ctx, project.ID)
	if err != nil {
		return domain.ProjectCodexActivation{}, err
	}
	var selected domain.ProjectThread
	for _, thread := range threads {
		if thread.ID == project.Assignment.SelectedThreadID {
			selected = thread
			break
		}
	}
	if selected.ID == "" || selected.RetiredAgent || selected.Harness != "codex" {
		return domain.ProjectCodexActivation{}, domain.ErrProjectThreadMismatch
	}
	if err := projectResumeDirectorySafe(project, selected); err != nil {
		return domain.ProjectCodexActivation{}, fmt.Errorf("resume project thread requires explicit directory decision: %w", err)
	}
	launch.RequestID, launch.AgentName, launch.Action, launch.SessionID = uuid.NewString(), operation.AgentName, domain.CodexSessionResume, selected.ExternalID
	if launch.Directory == "" {
		launch.Directory = selected.LaunchDir
	}
	binding := &projectLaunchBinding{projectID: project.ID, assignmentID: project.Assignment.ID, expectedHead: project.HeadEventID, projectThreadID: selected.ID, mailboxID: project.MailboxID, projectName: project.Name, runnable: true}
	runtime, err := s.launchCodexAgent(ctx, launch, binding)
	if err != nil {
		return domain.ProjectCodexActivation{}, err
	}
	return domain.ProjectCodexActivation{Project: project, Runtime: runtime}, nil
}

func projectResumeDirectorySafe(project domain.Project, thread domain.ProjectThread) error {
	info, err := os.Stat(thread.LaunchDir)
	if err != nil {
		return err
	}
	if !info.IsDir() {
		return errors.New("recorded launch path is not a directory")
	}
	resolved, err := filepath.EvalSymlinks(thread.LaunchDir)
	if err != nil {
		return err
	}
	resolved = filepath.Clean(resolved)
	for _, resource := range project.Resources {
		if resource.Kind != "path" {
			continue
		}
		relative, relErr := filepath.Rel(filepath.Clean(resource.CanonicalLocator), resolved)
		if relErr == nil && relative != ".." && !strings.HasPrefix(relative, ".."+string(filepath.Separator)) {
			probe, openErr := os.Open(resolved)
			if openErr != nil {
				return openErr
			}
			_ = probe.Close()
			return nil
		}
	}
	return errors.New("recorded launch directory is no longer covered by a project claim")
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
var _ domain.ProjectCodexRuntimeController = (*Supervisor)(nil)
var _ domain.CodexRuntimeAutoStarter = (*Supervisor)(nil)
var _ domain.ProjectWorktreeProvisioner = (*Supervisor)(nil)
