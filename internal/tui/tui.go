package tui

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"sort"
	"strings"
	"time"

	"charm.land/bubbles/v2/key"
	"charm.land/bubbles/v2/textarea"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/google/uuid"
	hqconfig "github.com/wbbradley/hq/internal/config"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/repoctx"
)

const repairInterval = 5 * time.Minute

var (
	titleStyle   = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("212"))
	finalStyle   = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("42"))
	selected     = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("230")).Background(lipgloss.Color("62"))
	inputCursor  = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("212"))
	dim          = lipgloss.NewStyle().Foreground(lipgloss.Color("241"))
	panelEdge    = lipgloss.NewStyle().Foreground(lipgloss.Color("63"))
	dimPanelEdge = lipgloss.NewStyle().Foreground(lipgloss.Color("59"))
	panel        = lipgloss.NewStyle().Border(lipgloss.RoundedBorder()).BorderForeground(lipgloss.Color("63")).Padding(0, 1)
	dimPanel     = lipgloss.NewStyle().Border(lipgloss.RoundedBorder()).BorderForeground(lipgloss.Color("59")).Padding(0, 1)
)

type app struct {
	ctx                 context.Context
	store               domain.Store
	repo                repoctx.Provider
	messages            []model.Message
	groups              []messageGroup
	conversations       []model.ConversationSummary
	conversationMode    bool
	histories           map[string][]model.Message
	activities          map[string][]domain.HarnessActivity
	expandedActivities  map[string]bool
	inbox               []model.Message
	sent                []model.Message
	archived            []model.Message
	showSent            bool
	showArchived        bool
	showStatus          bool
	showTechnical       bool
	cursor              int
	width               int
	height              int
	answering           bool
	answerID            string
	answerGroupKey      string
	answerQ             model.Message
	drafts              map[string]messageDraft
	activeDraftKey      string
	composeTo           string
	composeName         string
	composeContext      model.RepositoryContext
	composeNamed        bool
	agents              []domain.NamedAgent
	projects            []domain.Project
	devices             []domain.HumanDevice
	account             domain.HumanAccount
	threadSessions      map[string]domain.AgentSession
	pickingRecipient    bool
	pickerQuery         string
	pickerCursor        int
	editor              textarea.Model
	err                 error
	contextID           string
	branch              string
	remotes             string
	pull                string
	sync                func(context.Context) error
	syncErr             error
	network             domain.NetworkStatus
	changes             <-chan domain.Invalidation
	states              <-chan domain.ConnectionUpdate
	connection          domain.ConnectionUpdate
	loadGeneration      uint64
	undoStack           []undoAction
	nextUndoID          uint64
	undoing             bool
	undoNotice          string
	messageScroll       int
	messageScrollManual bool
	messageViewportKey  string
	messageLiveAnchorID string
	messageAnchorID     string
	messageAnchorOffset int
	paneFocus           paneFocus
	markdown            *messageMarkdownRenderer
	launchDirectory     string
	launchEnvironment   []string
	defaultYolo         bool
	managingAgents      bool
	agentManager        agentManager
	projectSetup        *projectComposeSetup
	composeActivation   *projectActivationPlan
}

type agentManagerStage int

const (
	chooseRuntimeAgent agentManagerStage = iota
	chooseRuntimeSession
	enterRuntimeHarness
	enterRuntimeDirectory
	confirmRuntimeSwitch
	enterThreadName
)

type agentManager struct {
	stage         agentManagerStage
	query         string
	cursor        int
	agent         domain.NamedAgent
	sessions      []domain.AgentSession
	runtime       domain.HarnessRuntime
	harness       string
	directory     string
	threadName    string
	renameSession domain.AgentSession
	pending       domain.HarnessLaunchRequest
	yolo          bool
	busy          bool
	status        string
}

type paneFocus int

const (
	focusInbox paneFocus = iota
	focusMessage
	focusReply
)

type messageGroup struct {
	key             string
	conversationKey model.ConversationKey
	messages        []model.Message
	activities      []domain.HarnessActivity
	draft           *messageDraft
}

type messageDraft struct {
	key            string
	body           string
	answerID       string
	answerGroupKey string
	answerQ        model.Message
	composeTo      string
	composeName    string
	composeContext model.RepositoryContext
	composeNamed   bool
	activation     *projectActivationPlan
	updatedAt      time.Time
}

type recipientChoice struct {
	name         string
	mailboxID    string
	active       bool
	lastActiveAt *time.Time
	context      model.RepositoryContext
	named        bool
	project      bool
	projectID    string
	newProject   bool
	status       string
}

type projectSetupStage int

const (
	enterProjectName projectSetupStage = iota
	chooseProjectHome
	enterProjectBrief
	enterProjectPaths
	chooseProjectPrimary
	enterWorktreeRepository
	enterWorktreeBase
	enterWorktreeDestination
	enterWorktreeBranch
	chooseWorktreePrimary
	chooseProjectAgent
	enterProjectHarness
	chooseProjectThread
	enterProjectDirectory
)

type projectComposeSetup struct {
	project             domain.Project
	stage               projectSetupStage
	agents              []domain.NamedAgent
	agent               domain.NamedAgent
	harness             string
	threads             []domain.ProjectThread
	cursor              int
	query               string
	directory           string
	force               bool
	busy                bool
	status              string
	name                string
	brief               string
	home                string
	pathsText           string
	paths               []string
	worktreeRepository  string
	worktreeBase        string
	worktreeDestination string
	worktreeBranch      string
}

type projectActivationPlan struct {
	projectID string
	agentName string
	harness   string
	action    domain.HarnessSessionAction
	sessionID string
	directory string
	force     bool
}

type paneLayout struct {
	width         int
	height        int
	inboxHeight   int
	messageWidth  int
	messageHeight int
	replyWidth    int
	replyHeight   int
}

type messageLineSpan struct {
	messageID  string
	actionUnit string
	start      int
	end        int
}

type activityLineSpan struct {
	key   string
	start int
	end   int
}

type renderedMessageGroup struct {
	panel         string
	spans         []messageLineSpan
	activitySpans []activityLineSpan
}

func (g messageGroup) latest() model.Message {
	if len(g.messages) == 0 {
		return model.Message{}
	}
	return g.messages[len(g.messages)-1]
}

type loadedMsg struct {
	generation    uint64
	inbox         []model.Message
	sent          []model.Message
	archived      []model.Message
	conversations []model.ConversationSummary
	histories     map[string][]model.Message
	activities    map[string][]domain.HarnessActivity
	network       domain.NetworkStatus
	agents        []domain.NamedAgent
	projects      []domain.Project
	devices       []domain.HumanDevice
	account       domain.HumanAccount
	sessions      map[string]domain.AgentSession
	err           error
}

type historyLoadedMsg struct {
	key        string
	messages   []model.Message
	activities []domain.HarnessActivity
	err        error
}

type answeredMsg struct {
	err  error
	sent bool
}

type projectThreadsMsg struct {
	threads []domain.ProjectThread
	err     error
}

type projectCreatedMsg struct {
	project domain.Project
	err     error
}

type undoAction struct {
	id         uint64
	messageIDs []string
}

type archivedMsg struct {
	messageIDs []string
	err        error
}

type restoredMsg struct {
	action undoAction
	err    error
}

type repairMsg struct{}

type invalidatedMsg struct{}

type connectionMsg struct{ state domain.ConnectionUpdate }

type agentSessionsMsg struct {
	agent    domain.NamedAgent
	sessions []domain.AgentSession
	runtime  domain.HarnessRuntime
	err      error
}

type harnessRuntimeMsg struct {
	runtime domain.HarnessRuntime
	err     error
}

type renamedAgentSessionMsg struct {
	session domain.AgentSession
	err     error
}

type syncMsg struct{ err error }

type branchMsg struct {
	message model.Message
	branch  string
	err     error
}

type pullMsg struct {
	questionID string
	pull       *repoctx.PullRequest
	err        error
}

type remotesMsg struct {
	message model.Message
	branch  string
	remotes []repoctx.Remote
	err     error
}

func Run(ctx context.Context, s domain.Store, in io.Reader, out io.Writer) error {
	return RunWithClient(ctx, s, in, out, domain.ClientUpdates{}, nil)
}

func RunWithSync(ctx context.Context, s domain.Store, in io.Reader, out io.Writer, sync func(context.Context) error) error {
	return RunWithClient(ctx, s, in, out, domain.ClientUpdates{}, sync)
}

func RunWithClient(ctx context.Context, s domain.Store, in io.Reader, out io.Writer, updates domain.ClientUpdates, sync func(context.Context) error) error {
	settings, err := hqconfig.Load()
	if err != nil {
		return err
	}
	return runWithClient(ctx, s, in, out, updates, sync, settings.Codex.Yolo)
}

func runWithClient(ctx context.Context, s domain.Store, in io.Reader, out io.Writer, updates domain.ClientUpdates, sync func(context.Context) error, defaultYolo bool) error {
	var subscription domain.ChangeSubscription
	var err error
	if updates.Subscribe != nil {
		subscription, err = updates.Subscribe(ctx, tuiChangeTopics()...)
		if err != nil {
			return fmt.Errorf("subscribe to HQ updates: %w", err)
		}
		defer subscription.Close()
	}
	editor := textarea.New()
	editor.Placeholder = "Type a message"
	editor.KeyMap.InsertNewline = key.NewBinding(
		key.WithKeys("shift+enter", "ctrl+j"),
		key.WithHelp("shift+enter/ctrl+j", "insert newline"),
	)
	editor.SetWidth(72)
	editor.SetHeight(6)
	launchDirectory, err := os.Getwd()
	if err != nil {
		return fmt.Errorf("read TUI launch directory: %w", err)
	}
	m := app{ctx: ctx, store: s, repo: repoctx.GitHub{}, editor: editor, sync: sync, states: updates.States, connection: updates.Initial, markdown: newMessageMarkdownRenderer(nil), launchDirectory: launchDirectory, launchEnvironment: os.Environ(), defaultYolo: defaultYolo}
	if subscription != nil {
		m.changes = subscription.Changes()
	}
	_, err = tea.NewProgram(m, tea.WithInput(in), tea.WithOutput(out), tea.WithContext(ctx)).Run()
	return err
}

func tuiChangeTopics() []domain.ChangeTopic {
	return []domain.ChangeTopic{domain.TopicMessages, domain.TopicActivities, domain.TopicMailboxes, domain.TopicNetwork, domain.TopicPeers, domain.TopicHuman, domain.TopicRelays, domain.TopicAgents}
}

func (m app) Init() tea.Cmd {
	return tea.Batch(m.load, m.syncNow(), m.waitInvalidation(), m.waitConnection(), scheduleRepair())
}

func scheduleRepair() tea.Cmd {
	return tea.Tick(repairInterval, func(time.Time) tea.Msg { return repairMsg{} })
}

func (m app) waitInvalidation() tea.Cmd {
	if m.changes == nil {
		return nil
	}
	return func() tea.Msg {
		select {
		case <-m.ctx.Done():
			return nil
		case <-m.changes:
			return invalidatedMsg{}
		}
	}
}

func (m app) waitConnection() tea.Cmd {
	if m.states == nil {
		return nil
	}
	return func() tea.Msg {
		select {
		case <-m.ctx.Done():
			return nil
		case state := <-m.states:
			return connectionMsg{state: state}
		}
	}
}

func (m app) syncNow() tea.Cmd {
	if m.sync == nil {
		return nil
	}
	return func() tea.Msg {
		ctx, cancel := context.WithTimeout(m.ctx, 3*time.Second)
		defer cancel()
		return syncMsg{err: m.sync(ctx)}
	}
}

func (m app) load() tea.Msg {
	conversations, err := m.loadAllConversations()
	if err != nil {
		return loadedMsg{err: err}
	}
	histories := make(map[string][]model.Message)
	activities := make(map[string][]domain.HarnessActivity)
	selectedKey := m.selectedGroupKey()
	selectedConversation, found := conversationSummaryByString(conversations, selectedKey)
	if !found && len(conversations) > 0 {
		selectedConversation = conversations[0]
		selectedKey = conversationKeyString(selectedConversation.Key)
		found = true
	}
	if found {
		history, historyErr := m.loadAllConversationHistory(selectedConversation.Key)
		if historyErr != nil {
			return loadedMsg{err: historyErr}
		}
		histories[selectedKey] = history
	}
	agents, err := m.store.ListNamedAgents(m.ctx)
	if err != nil {
		return loadedMsg{err: err}
	}
	projects, err := m.store.ListProjects(m.ctx, false)
	if err != nil {
		return loadedMsg{err: err}
	}
	account, _ := m.store.HumanAccount(m.ctx)
	devices, _ := m.store.HumanDevices(m.ctx)
	sessions := make(map[string]domain.AgentSession)
	for _, agent := range agents {
		history, historyErr := m.store.ListNamedAgentSessions(m.ctx, agent.Name)
		if historyErr != nil {
			return loadedMsg{err: historyErr}
		}
		for _, session := range history {
			sessions[session.Harness+"\x00"+session.SessionID] = session
		}
	}
	if found {
		projected, activityErr := m.loadConversationActivities(selectedConversation.Key, agents, sessions)
		if activityErr != nil {
			return loadedMsg{err: activityErr}
		}
		activities[selectedKey] = projected
	}
	network, err := m.store.NetworkStatus(m.ctx)
	return loadedMsg{conversations: conversations, histories: histories, activities: activities, agents: agents, projects: projects, devices: devices, account: account, sessions: sessions, network: network, err: err}
}

func (m *app) reload() tea.Cmd {
	m.loadGeneration++
	generation := m.loadGeneration
	snapshot := *m
	return func() tea.Msg {
		loaded := snapshot.load().(loadedMsg)
		loaded.generation = generation
		return loaded
	}
}

func (m app) loadAllConversations() ([]model.ConversationSummary, error) {
	filter := model.ConversationFilter{IncludeSent: m.showSent, IncludeArchived: m.showArchived, Limit: 200}
	var conversations []model.ConversationSummary
	for {
		page, err := m.store.ListConversations(m.ctx, filter)
		if err != nil {
			return nil, err
		}
		conversations = append(conversations, page.Conversations...)
		if page.NextCursor == "" {
			return conversations, nil
		}
		filter.Cursor = page.NextCursor
	}
}

func (m app) loadAllConversationHistory(key model.ConversationKey) ([]model.Message, error) {
	filter := model.ConversationHistoryFilter{Key: key, Limit: 200}
	var messages []model.Message
	for {
		page, err := m.store.ListConversationHistory(m.ctx, filter)
		if err != nil {
			return nil, err
		}
		messages = append(messages, page.Messages...)
		if page.NextCursor == "" {
			return messages, nil
		}
		filter.Cursor = page.NextCursor
	}
}

func (m app) loadConversationHistory(key model.ConversationKey) tea.Cmd {
	stableKey := conversationKeyString(key)
	_, historyLoaded := m.histories[stableKey]
	_, activityLoaded := m.activities[stableKey]
	if (historyLoaded && activityLoaded) || !key.Valid() {
		return nil
	}
	return func() tea.Msg {
		messages, err := m.loadAllConversationHistory(key)
		var activities []domain.HarnessActivity
		if err == nil {
			activities, err = m.loadConversationActivities(key, m.agents, m.threadSessions)
		}
		return historyLoadedMsg{key: stableKey, messages: messages, activities: activities, err: err}
	}
}

func (m app) loadConversationActivities(key model.ConversationKey, agents []domain.NamedAgent, sessions map[string]domain.AgentSession) ([]domain.HarnessActivity, error) {
	reader, ok := m.store.(domain.HarnessActivityReader)
	if !ok || key.CounterpartyMailboxID == "" || key.CounterpartyMailboxID == model.HumanMailboxID {
		return nil, nil
	}
	filter := domain.HarnessActivityFilter{MailboxID: key.CounterpartyMailboxID, Limit: 1000}
	if key.HarnessSessionID != "" {
		filter.Harness, filter.SessionID = key.HarnessProvider, key.HarnessSessionID
	} else {
		for _, agent := range agents {
			if agent.MailboxID == filter.MailboxID && agent.CurrentSessionID != "" {
				filter.Harness, filter.SessionID = agent.Harness, agent.CurrentSessionID
				break
			}
		}
	}
	return reader.ListHarnessActivities(m.ctx, filter)
}

func (m app) answer() tea.Msg {
	if m.answerID == "" && m.composeTo == "" {
		return answeredMsg{err: errors.New("message has no recipient")}
	}
	id, err := uuid.NewV7()
	if err != nil {
		return answeredMsg{err: err}
	}
	recipient := m.answerQ.SenderMailboxID
	message := model.Message{ID: id.String(), Context: m.answerQ.Context, SenderMailboxID: model.HumanMailboxID,
		RecipientMailboxID: recipient, SenderLabel: "human", RecipientLabel: m.answerQ.SenderLabel, Body: strings.TrimSpace(m.editor.Value()), CreatedAt: time.Now().UTC()}
	if m.answerQ.Purpose == model.MessagePurposeProtocolQuestion {
		message.Purpose = model.MessagePurposeProtocolAnswer
	} else if m.answerQ.SenderAddress.Kind == model.MailboxProject {
		message.Purpose = model.MessagePurposeProjectInput
	}
	if m.composeTo != "" {
		if m.composeNamed {
			agent, lookupErr := m.store.GetNamedAgent(m.ctx, m.composeName)
			if lookupErr != nil || agent.Retired || agent.MailboxID != m.composeTo {
				cause := lookupErr
				if cause == nil {
					cause = domain.ErrAgentRetired
				}
				return answeredMsg{err: fmt.Errorf("recipient %s is no longer available; choose a recipient again: %w", m.composeName, cause)}
			}
			if agent.Harness != "" && agent.CurrentSessionID != "" {
				message.Correlation = model.MessageCorrelation{Provider: agent.Harness, SessionID: agent.CurrentSessionID}
				message.HarnessProvider, message.HarnessSessionID = agent.Harness, agent.CurrentSessionID
			}
		}
		message.RecipientMailboxID = m.composeTo
		message.Context = m.composeContext
		message.RecipientLabel = m.composeName
		err = m.store.Create(m.ctx, message)
		if err == nil && m.composeActivation != nil {
			err = m.activateComposedProject(*m.composeActivation)
			if err != nil {
				return answeredMsg{err: fmt.Errorf("message is pending in the project; activation failed: %w", err), sent: true}
			}
		}
	} else {
		replyTo := m.answerID
		message.ReplyTo = &replyTo
		message.Correlation = correlationForMessage(m.answerQ)
		err = m.store.Reply(m.ctx, m.answerID, message)
		if err == nil {
			err = m.archiveAnsweredGroup()
			return answeredMsg{err: err, sent: true}
		}
	}
	return answeredMsg{err: err, sent: err == nil}
}

func (m app) activateComposedProject(plan projectActivationPlan) error {
	controller, ok := m.store.(domain.ProjectHarnessRuntimeController)
	if !ok {
		return errors.New("project harness runtime control is unavailable")
	}
	project, err := m.store.GetProject(m.ctx, plan.projectID)
	if err != nil {
		return err
	}
	if project.Lifecycle == domain.ProjectOpen && project.Assignment != nil && project.Assignment.AgentName == plan.agentName && project.Assignment.State == domain.AssignmentRunnable {
		return nil
	}
	repository := model.RepositoryContext{Directory: plan.directory}
	if snapshotter, ok := m.repo.(interface {
		Snapshot(context.Context, string) model.RepositoryContext
	}); ok {
		repository = snapshotter.Snapshot(m.ctx, plan.directory)
	}
	launch := domain.HarnessLaunchRequest{RequestID: uuid.NewString(), AgentName: plan.agentName, Harness: plan.harness, Action: plan.action, SessionID: plan.sessionID, Directory: plan.directory, Repository: repository, Environment: append([]string(nil), m.launchEnvironment...), ProviderOptions: providerOptions(plan.harness, m.defaultYolo)}
	defer func() {
		for index := range launch.Environment {
			launch.Environment[index] = ""
		}
	}()
	if project.Assignment != nil {
		_, err = controller.HandoffHarnessProject(m.ctx, domain.ProjectHarnessHandoffRequest{RequestID: uuid.NewString(), ProjectID: project.ID, ExpectedHead: project.HeadEventID, NewAgentName: plan.agentName, Force: plan.force, Launch: launch})
	} else {
		_, err = controller.ActivateHarnessProject(m.ctx, domain.ProjectHarnessActivationRequest{ProjectID: project.ID, ExpectedHead: project.HeadEventID, AgentName: plan.agentName, Launch: launch})
	}
	return err
}

func (m app) archiveAnsweredGroup() error {
	group, ok := m.groupByKey(m.answerGroupKey)
	if !ok {
		return nil
	}
	targetUnit := actionUnitKey(m.answerQ)
	for _, message := range group.messages {
		if message.ID == m.answerID || !canArchive(message) || actionUnitKey(message) != targetUnit {
			continue
		}
		if err := m.store.Archive(m.ctx, message.ID); err != nil && !errors.Is(err, domain.ErrAlreadyHandled) {
			return fmt.Errorf("reply sent, but archive turn message %s: %w", message.ID, err)
		}
	}
	return nil
}

func correlationForMessage(message model.Message) model.MessageCorrelation {
	correlation := message.Correlation
	if correlation.Empty() {
		correlation = model.MessageCorrelation{Provider: message.HarnessProvider, SessionID: message.HarnessSessionID, OperationID: message.HarnessOperationID}
	}
	return correlation
}

func (m app) archiveGroup(group messageGroup) tea.Cmd {
	return func() tea.Msg {
		var archived []string
		target := archiveTarget(group)
		targetUnit := actionUnitKey(target)
		for _, message := range group.messages {
			if canArchive(message) && actionUnitKey(message) == targetUnit {
				if err := m.store.Archive(m.ctx, message.ID); err != nil {
					return archivedMsg{messageIDs: archived, err: err}
				}
				archived = append(archived, message.ID)
			}
		}
		return archivedMsg{messageIDs: archived}
	}
}

func (m app) restoreAction(action undoAction) tea.Cmd {
	return func() tea.Msg {
		for i := len(action.messageIDs) - 1; i >= 0; i-- {
			id := action.messageIDs[i]
			if err := m.store.Restore(m.ctx, id); err != nil {
				if errors.Is(err, domain.ErrAlreadyHandled) {
					message, getErr := m.store.Get(m.ctx, id)
					if getErr == nil && message.ArchivedAt == nil {
						continue
					}
				}
				return restoredMsg{action: action, err: err}
			}
		}
		return restoredMsg{action: action}
	}
}

func (m app) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width, m.height = msg.Width, msg.Height
		layout := responsivePaneLayout(msg.Width, msg.Height, m.answering)
		m.editor.SetWidth(max(1, layout.replyWidth-panel.GetHorizontalFrameSize()))
		m.editor.SetHeight(max(1, layout.replyHeight-4))
		m.reconcileMessageViewport(true)
	case loadedMsg:
		if msg.generation < m.loadGeneration {
			return m, nil
		}
		selectedKey := m.selectedGroupKey()
		knownSelectedMessages := make(map[string]bool)
		if group, found := m.groupByKey(selectedKey); found {
			for _, message := range group.messages {
				knownSelectedMessages[message.ID] = true
			}
		}
		if msg.conversations != nil || (msg.err == nil && m.store != nil) {
			m.conversationMode = true
			m.conversations = msg.conversations
			m.histories = msg.histories
			m.activities = msg.activities
		} else {
			m.inbox, m.sent, m.archived = msg.inbox, msg.sent, msg.archived
		}
		m.agents, m.projects, m.devices, m.account, m.threadSessions, m.network, m.err = msg.agents, msg.projects, msg.devices, msg.account, msg.sessions, msg.network, msg.err
		if choices := m.filteredRecipients(); m.pickerCursor >= len(choices) {
			m.pickerCursor = max(0, len(choices)-1)
		}
		m.setMessages()
		visibleGroups := m.visibleGroups()
		automaticallySelected := selectedKey == "" && len(visibleGroups) > 0
		if index := groupIndex(visibleGroups, selectedKey); index >= 0 {
			m.cursor = index
		} else if m.cursor >= len(visibleGroups) {
			m.cursor = max(0, len(visibleGroups)-1)
		}
		if automaticallySelected {
			group := visibleGroups[m.cursor]
			m.messageViewportKey = group.key
			m.messageLiveAnchorID = ""
			m.messageScrollManual = false
			m.messageAnchorID = ""
			m.messageAnchorOffset = 0
			for _, message := range group.messages {
				if canArchive(message) {
					m.messageLiveAnchorID = message.ID
				}
			}
		} else if len(knownSelectedMessages) > 0 {
			if group, found := m.groupByKey(selectedKey); found {
				for _, message := range group.messages {
					if !knownSelectedMessages[message.ID] && canArchive(message) {
						m.messageLiveAnchorID = message.ID
						m.messageScrollManual = false
						m.messageAnchorID = ""
						m.messageAnchorOffset = 0
					}
				}
			}
		}
		m.reconcileMessageViewport(true)
		return m.withContextCommand()
	case historyLoadedMsg:
		if msg.err != nil {
			m.err = msg.err
			return m, nil
		}
		if m.histories == nil {
			m.histories = make(map[string][]model.Message)
		}
		m.histories[msg.key] = msg.messages
		if m.activities == nil {
			m.activities = make(map[string][]domain.HarnessActivity)
		}
		m.activities[msg.key] = msg.activities
		selectedKey := m.selectedGroupKey()
		m.setMessages()
		if index := groupIndex(m.visibleGroups(), selectedKey); index >= 0 {
			m.cursor = index
		}
		m.reconcileMessageViewport(true)
		return m.withContextCommand()
	case repairMsg:
		load := m.reload()
		return m, tea.Batch(load, m.syncNow(), scheduleRepair())
	case invalidatedMsg:
		load := m.reload()
		return m, tea.Batch(load, m.waitInvalidation())
	case connectionMsg:
		m.connection = msg.state
		return m, m.waitConnection()
	case agentSessionsMsg:
		m.agentManager.busy = false
		m.agentManager.agent, m.agentManager.sessions, m.agentManager.runtime = msg.agent, msg.sessions, msg.runtime
		if msg.err != nil {
			m.agentManager.status = msg.err.Error()
			return m, nil
		}
		m.agentManager.stage, m.agentManager.cursor, m.agentManager.status = chooseRuntimeSession, 0, ""
		return m, nil
	case harnessRuntimeMsg:
		m.agentManager.busy = false
		m.agentManager.runtime = msg.runtime
		if msg.err != nil {
			m.agentManager.status = msg.err.Error()
		} else {
			m.agentManager.status = fmt.Sprintf("%s · %s · %s", msg.runtime.Phase, threadLabel(m.managedThreadName(msg.runtime.Harness, msg.runtime.SessionID), msg.runtime.SessionID), msg.runtime.Directory)
		}
		return m, m.reload()
	case renamedAgentSessionMsg:
		m.agentManager.busy = false
		if msg.err != nil {
			m.agentManager.status = msg.err.Error()
			return m, nil
		}
		for index := range m.agentManager.sessions {
			if m.agentManager.sessions[index].Harness == msg.session.Harness && m.agentManager.sessions[index].SessionID == msg.session.SessionID {
				m.agentManager.sessions[index] = msg.session
			}
		}
		if msg.session.Current {
			m.agentManager.agent.CurrentThreadName = msg.session.ThreadName
		}
		m.agentManager.stage = chooseRuntimeSession
		m.agentManager.status = "session renamed"
		return m, m.reload()
	case syncMsg:
		m.syncErr = msg.err
		if msg.err == nil {
			return m, m.reload()
		}
	case branchMsg:
		if msg.message.ID != m.contextID {
			return m, nil
		}
		if msg.err != nil {
			m.branch = "[git unavailable]"
			m.remotes = ""
			m.pull = ""
			return m, nil
		}
		m.branch = msg.branch
		m.remotes = "remotes loading…"
		m.pull = ""
		return m, m.loadRemotes(msg.message, msg.branch)
	case remotesMsg:
		if msg.message.ID != m.contextID {
			return m, nil
		}
		if msg.err != nil {
			m.remotes = "[remotes unavailable]"
			m.pull = ""
			return m, nil
		}
		m.remotes = formatRemotes(msg.remotes)
		if len(msg.remotes) == 0 {
			m.remotes = "no remotes"
			m.pull = ""
			return m, nil
		}
		m.pull = "PR loading…"
		return m, m.loadPull(msg.message, msg.branch)
	case pullMsg:
		if msg.questionID != m.contextID {
			return m, nil
		}
		switch {
		case msg.err != nil:
			m.pull = "[gh unavailable]"
		case msg.pull == nil:
			m.pull = "no open PR"
		default:
			m.pull = fmt.Sprintf("PR #%d · %s", msg.pull.Number, msg.pull.Title)
		}
	case answeredMsg:
		m.err = msg.err
		if msg.sent {
			delete(m.drafts, m.activeDraftKey)
			m.activeDraftKey = ""
			m.answering = false
			m.answerID = ""
			m.answerGroupKey = ""
			m.answerQ = model.Message{}
			m.composeTo = ""
			m.composeName = ""
			m.composeContext = model.RepositoryContext{}
			m.composeNamed = false
			m.composeActivation = nil
			m.paneFocus = focusInbox
			m.editor.Reset()
			m.reconcileMessageViewport(true)
			load := m.reload()
			return m, tea.Batch(load, m.syncNow())
		}
		if errors.Is(msg.err, domain.ErrAgentRetired) || errors.Is(msg.err, domain.ErrAgentNotFound) {
			m.answering = false
			m.pickingRecipient = true
			m.pickerQuery = ""
			m.pickerCursor = 0
			m.composeTo = ""
			m.composeName = ""
			m.composeContext = model.RepositoryContext{}
			m.composeNamed = false
			m.composeActivation = nil
			m.paneFocus = focusReply
			m.editor.Blur()
		}
	case projectThreadsMsg:
		if m.projectSetup != nil {
			m.projectSetup.busy = false
			m.projectSetup.threads, m.projectSetup.status = msg.threads, ""
			if msg.err != nil {
				m.projectSetup.status = msg.err.Error()
			} else {
				m.projectSetup.harness = m.projectSetup.agent.Harness
				if m.projectSetup.harness == "" {
					m.projectSetup.harness = "codex"
				}
				m.projectSetup.stage, m.projectSetup.cursor = enterProjectHarness, 0
			}
		}
	case projectCreatedMsg:
		if m.projectSetup != nil {
			m.projectSetup.busy = false
			if msg.err != nil {
				m.projectSetup.status = msg.err.Error()
			} else if msg.project.PendingCommand != nil || msg.project.MailboxID == "" {
				m.projectSetup = nil
				m.pickingRecipient = true
				m.err = fmt.Errorf("project creation queued on %s; compose after the home commits it", msg.project.HomeInstallation)
				return m, m.reload()
			} else {
				m.projectSetup.project = msg.project
				m.prepareProjectAgents()
			}
		}
	case archivedMsg:
		m.err = msg.err
		if len(msg.messageIDs) > 0 {
			m.nextUndoID++
			m.undoStack = append(m.undoStack, undoAction{id: m.nextUndoID, messageIDs: msg.messageIDs})
			if len(m.undoStack) > 20 {
				m.undoStack = append([]undoAction(nil), m.undoStack[len(m.undoStack)-20:]...)
			}
			m.undoNotice = fmt.Sprintf("archived %d message(s) · press u to undo", len(msg.messageIDs))
		}
		if msg.err == nil || len(msg.messageIDs) > 0 {
			load := m.reload()
			return m, tea.Batch(load, m.syncNow())
		}
	case restoredMsg:
		m.undoing = false
		m.err = msg.err
		if msg.err == nil {
			for i := len(m.undoStack) - 1; i >= 0; i-- {
				if m.undoStack[i].id == msg.action.id {
					m.undoStack = append(m.undoStack[:i], m.undoStack[i+1:]...)
					break
				}
			}
			m.undoNotice = fmt.Sprintf("restored %d message(s)", len(msg.action.messageIDs))
			load := m.reload()
			return m, tea.Batch(load, m.syncNow())
		}
	case tea.PasteMsg:
		if m.answering && m.paneFocus == focusReply {
			var cmd tea.Cmd
			m.editor, cmd = m.editor.Update(msg)
			return m, cmd
		}
	case tea.KeyPressMsg:
		if m.connection.Blocking {
			if msg.String() == "q" || msg.String() == "ctrl+c" {
				return m, tea.Quit
			}
			return m, nil
		}
		if m.pickingRecipient {
			return m.updateRecipientPicker(msg)
		}
		if m.projectSetup != nil {
			return m.updateProjectSetup(msg)
		}
		if m.managingAgents {
			return m.updateAgentManager(msg)
		}
		pageKey := msg.String()
		pageMsg := msg
		switch pageKey {
		case "ctrl+u":
			pageKey = "pgup"
			pageMsg = tea.KeyPressMsg{Code: tea.KeyPgUp}
		case "ctrl+d":
			pageKey = "pgdown"
			pageMsg = tea.KeyPressMsg{Code: tea.KeyPgDown}
		}
		switch msg.String() {
		case "tab":
			wasReply := m.answering && m.paneFocus == focusReply
			m.cyclePaneFocus(1)
			if wasReply {
				m.stowActiveDraft()
				return m.withContextCommand()
			}
			if m.paneFocus == focusReply {
				return m.beginComposeForSelection()
			}
			m.editor.Blur()
			return m, nil
		case "shift+tab":
			wasReply := m.answering && m.paneFocus == focusReply
			m.cyclePaneFocus(-1)
			if wasReply {
				m.stowActiveDraft()
				return m.withContextCommand()
			}
			if m.paneFocus == focusReply {
				return m.beginComposeForSelection()
			}
			m.editor.Blur()
			return m, nil
		}
		switch pageKey {
		case "pgup":
			layout := responsivePaneLayout(m.width, m.height, m.answering)
			switch m.paneFocus {
			case focusInbox:
				m.cursor = max(0, m.cursor-max(1, layout.inboxHeight-3))
				m.resetMessageViewport()
				return m.withContextCommand()
			case focusMessage:
				m.scrollMessagePane(-max(1, layout.messageHeight-3))
				return m, nil
			case focusReply:
				if m.answering {
					var cmd tea.Cmd
					m.editor, cmd = m.editor.Update(pageMsg)
					return m, cmd
				}
			}
			return m, nil
		case "pgdown":
			layout := responsivePaneLayout(m.width, m.height, m.answering)
			switch m.paneFocus {
			case focusInbox:
				m.cursor = min(max(0, len(m.visibleGroups())-1), m.cursor+max(1, layout.inboxHeight-3))
				m.resetMessageViewport()
				return m.withContextCommand()
			case focusMessage:
				m.scrollMessagePane(max(1, layout.messageHeight-3))
				return m, nil
			case focusReply:
				if m.answering {
					var cmd tea.Cmd
					m.editor, cmd = m.editor.Update(pageMsg)
					return m, cmd
				}
			}
			return m, nil
		}
		if m.answering {
			switch msg.String() {
			case "ctrl+c", "esc":
				delete(m.drafts, m.activeDraftKey)
				m.activeDraftKey = ""
				m.answering = false
				m.answerID = ""
				m.answerGroupKey = ""
				m.answerQ = model.Message{}
				m.composeTo = ""
				m.composeName = ""
				m.composeContext = model.RepositoryContext{}
				m.composeNamed = false
				m.composeActivation = nil
				m.paneFocus = focusInbox
				m.editor.Blur()
				m.editor.Reset()
				m.reconcileMessageViewport(true)
				return m, nil
			case "j", "down":
				if m.paneFocus == focusMessage {
					m.scrollMessagePane(1)
					return m, nil
				}
				if m.paneFocus == focusInbox && m.cursor+1 < len(m.visibleGroups()) {
					m.cursor++
					return m, nil
				}
			case "k", "up":
				if m.paneFocus == focusMessage {
					m.scrollMessagePane(-1)
					return m, nil
				}
				if m.paneFocus == focusInbox && m.cursor > 0 {
					m.cursor--
					return m, nil
				}
			case "enter":
				if m.paneFocus != focusReply {
					return m, nil
				}
				if strings.TrimSpace(m.editor.Value()) != "" {
					return m, m.answer
				}
				return m, nil
			case "e":
				if m.paneFocus == focusMessage && m.toggleSelectedActivities() {
					m.reconcileMessageViewport(true)
					return m, nil
				}
			}
			if m.paneFocus != focusReply {
				return m, nil
			}
			var cmd tea.Cmd
			m.editor, cmd = m.editor.Update(msg)
			return m, cmd
		}
		switch msg.String() {
		case "q", "ctrl+c":
			return m, tea.Quit
		case "j", "down":
			if m.paneFocus == focusMessage {
				m.scrollMessagePane(1)
				return m, nil
			}
			if m.paneFocus != focusInbox {
				return m, nil
			}
			if m.cursor+1 < len(m.visibleGroups()) {
				m.cursor++
				m.resetMessageViewport()
				return m.withContextCommand()
			}
		case "k", "up":
			if m.paneFocus == focusMessage {
				m.scrollMessagePane(-1)
				return m, nil
			}
			if m.paneFocus != focusInbox {
				return m, nil
			}
			if m.cursor > 0 {
				m.cursor--
				m.resetMessageViewport()
				return m.withContextCommand()
			}
		case "s":
			m.showSent = !m.showSent
			m.cursor = 0
			m.resetMessageViewport()
			if m.conversationMode {
				return m, m.reload()
			}
			m.setMessages()
			return m.withContextCommand()
		case "x":
			m.showArchived = !m.showArchived
			m.cursor = 0
			m.resetMessageViewport()
			if m.conversationMode {
				return m, m.reload()
			}
			m.setMessages()
			return m.withContextCommand()
		case "v":
			m.showStatus = !m.showStatus
			return m, nil
		case "i":
			m.showTechnical = !m.showTechnical
			m.reconcileMessageViewport(true)
			return m, nil
		case "enter", "a":
			if group, ok := m.groupAtCursor(); ok && (group.draft != nil || canReplyGroup(group)) {
				return m.beginComposeForSelection()
			}
		case "d":
			if group, ok := m.groupAtCursor(); ok && canArchiveGroup(group) {
				return m, m.archiveGroup(group)
			}
		case "u":
			if !m.undoing && len(m.undoStack) > 0 {
				action := m.undoStack[len(m.undoStack)-1]
				m.undoing = true
				m.undoNotice = "restoring archived messages…"
				return m, m.restoreAction(action)
			}
			if len(m.undoStack) == 0 {
				m.undoNotice = "nothing to undo"
			}
		case "n":
			m.pickingRecipient = true
			m.pickerQuery = ""
			m.pickerCursor = 0
			m.paneFocus = focusReply
			m.editor.Blur()
			return m, nil
		case "g":
			m.managingAgents = true
			m.agentManager = agentManager{stage: chooseRuntimeAgent, yolo: m.defaultYolo}
			m.editor.Blur()
			return m, nil
		case "r":
			return m, m.reload()
		case "e":
			if m.paneFocus == focusMessage && m.toggleSelectedActivities() {
				m.reconcileMessageViewport(true)
			}
			return m, nil
		}
	}
	return m, nil
}

func (m app) recipients() []recipientChoice {
	choices := make([]recipientChoice, 0, len(m.projects)+len(m.agents)+2)
	for _, project := range m.projects {
		status := string(project.Lifecycle)
		runnable := project.Lifecycle == domain.ProjectOpen && project.Assignment != nil && project.Assignment.State == domain.AssignmentRunnable
		if project.Assignment == nil {
			status += " · unassigned"
			if project.SuggestedAgentName != "" {
				status += " · suggest " + project.SuggestedAgentName
			}
		} else {
			status += " · " + project.Assignment.AgentName + " · " + string(project.Assignment.State)
		}
		if project.PendingCommand != nil {
			status += " · command " + string(project.PendingCommand.Stage)
			if project.PendingCommand.Diagnostic != "" {
				status += " · " + project.PendingCommand.Diagnostic
			}
		} else if project.LatestCommand != nil && project.LatestCommand.Stage == domain.ProjectCommandRejected {
			status += " · command rejected"
		}
		name := fmt.Sprintf("%s · %s/%s", project.Name, short(project.HomeInstallation, 8), short(project.ID, 8))
		choices = append(choices, recipientChoice{name: name, mailboxID: project.MailboxID, active: runnable, project: true, projectID: project.ID, status: status})
	}
	if m.account.LocalInstallationID != "" {
		choices = append(choices, recipientChoice{name: "+ new project", project: true, newProject: true, status: "choose home, resources, agent, and thread"})
	}
	choices = append(choices, recipientChoice{name: "self", mailboxID: model.HumanMailboxID, active: true, status: "personal"})
	for _, agent := range m.agents {
		if agent.Retired {
			continue
		}
		choices = append(choices, recipientChoice{
			name: agent.Name, mailboxID: agent.MailboxID, active: agent.Active,
			lastActiveAt: agent.LastActiveAt, context: agent.Context, named: true, status: "direct agent",
		})
	}
	sort.SliceStable(choices, func(left, right int) bool {
		if choices[left].project != choices[right].project {
			return choices[left].project
		}
		if choices[left].active != choices[right].active {
			return choices[left].active
		}
		return choices[left].name < choices[right].name
	})
	return choices
}

func (m app) runtimeAgents() []domain.NamedAgent {
	query := strings.ToLower(strings.TrimSpace(m.agentManager.query))
	filtered := make([]domain.NamedAgent, 0, len(m.agents))
	for _, agent := range m.agents {
		if agent.Retired || (query != "" && !strings.Contains(strings.ToLower(agent.Name), query)) {
			continue
		}
		filtered = append(filtered, agent)
	}
	sort.Slice(filtered, func(i, j int) bool {
		if filtered[i].Active != filtered[j].Active {
			return filtered[i].Active
		}
		return filtered[i].Name < filtered[j].Name
	})
	return filtered
}

func (m app) updateAgentManager(key tea.KeyPressMsg) (tea.Model, tea.Cmd) {
	if m.agentManager.busy {
		if key.String() == "esc" {
			m.managingAgents = false
		}
		return m, nil
	}
	switch key.String() {
	case "ctrl+c", "esc":
		for index := range m.agentManager.pending.Environment {
			m.agentManager.pending.Environment[index] = ""
		}
		if m.agentManager.stage == chooseRuntimeAgent {
			m.managingAgents = false
		} else if m.agentManager.stage == enterRuntimeHarness || m.agentManager.stage == enterRuntimeDirectory || m.agentManager.stage == confirmRuntimeSwitch || m.agentManager.stage == enterThreadName {
			m.agentManager.stage = chooseRuntimeSession
			m.agentManager.pending = domain.HarnessLaunchRequest{}
			m.agentManager.renameSession = domain.AgentSession{}
			m.agentManager.status = ""
		} else {
			m.agentManager = agentManager{stage: chooseRuntimeAgent, yolo: m.defaultYolo}
		}
		return m, nil
	}
	switch m.agentManager.stage {
	case chooseRuntimeAgent:
		agents := m.runtimeAgents()
		switch key.String() {
		case "j", "down":
			m.agentManager.cursor = min(max(0, len(agents)-1), m.agentManager.cursor+1)
		case "k", "up":
			m.agentManager.cursor = max(0, m.agentManager.cursor-1)
		case "enter":
			if len(agents) > 0 {
				agent := agents[min(m.agentManager.cursor, len(agents)-1)]
				m.agentManager.busy = true
				return m, m.loadAgentSessions(agent)
			}
		case "backspace":
			runes := []rune(m.agentManager.query)
			if len(runes) > 0 {
				m.agentManager.query, m.agentManager.cursor = string(runes[:len(runes)-1]), 0
			}
		default:
			if key.Text != "" && !strings.ContainsAny(key.Text, "\r\n\t") {
				m.agentManager.query += key.Text
				m.agentManager.cursor = 0
			}
		}
	case chooseRuntimeSession:
		rowCount := len(m.agentManager.sessions) + 1
		switch key.String() {
		case "j", "down":
			m.agentManager.cursor = min(rowCount-1, m.agentManager.cursor+1)
		case "k", "up":
			m.agentManager.cursor = max(0, m.agentManager.cursor-1)
		case "n":
			m.beginNewRuntimeHarness()
		case "s":
			m.agentManager.busy = true
			return m, m.stopManagedAgent()
		case "y":
			m.agentManager.yolo = !m.agentManager.yolo
			m.agentManager.status = ""
		case "r":
			if m.agentManager.cursor > 0 && m.agentManager.cursor <= len(m.agentManager.sessions) {
				m.agentManager.renameSession = m.agentManager.sessions[m.agentManager.cursor-1]
				m.agentManager.threadName = m.agentManager.renameSession.ThreadName
				m.agentManager.stage = enterThreadName
				m.agentManager.status = ""
			}
		case "enter":
			if m.agentManager.cursor == 0 {
				m.beginNewRuntimeHarness()
				return m, nil
			}
			session := m.agentManager.sessions[m.agentManager.cursor-1]
			directory := session.Context.Directory
			if directory == "" {
				directory = m.defaultRuntimeDirectory()
			}
			var err error
			directory, err = m.validRuntimeDirectory(directory)
			if err != nil {
				m.agentManager.status = err.Error()
				return m, nil
			}
			request := m.runtimeRequest(domain.HarnessSessionResume, session.SessionID, directory)
			return m.confirmOrLaunch(request)
		}
	case enterRuntimeHarness:
		switch key.String() {
		case "enter":
			m.agentManager.harness = strings.TrimSpace(m.agentManager.harness)
			if m.agentManager.harness == "" {
				m.agentManager.status = "Harness provider is required."
				return m, nil
			}
			m.agentManager.stage = enterRuntimeDirectory
			m.agentManager.directory = m.defaultRuntimeDirectory()
			m.agentManager.status = ""
		case "backspace":
			m.agentManager.harness = trimLastRune(m.agentManager.harness)
		default:
			m.agentManager.harness += printableKeyText(key)
		}
	case enterRuntimeDirectory:
		switch key.String() {
		case "enter":
			directory, err := m.validRuntimeDirectory(m.agentManager.directory)
			if err != nil {
				m.agentManager.status = err.Error()
				return m, nil
			}
			request := m.runtimeRequest(domain.HarnessSessionNew, "", directory)
			return m.confirmOrLaunch(request)
		case "backspace":
			runes := []rune(m.agentManager.directory)
			if len(runes) > 0 {
				m.agentManager.directory = string(runes[:len(runes)-1])
			}
		default:
			if key.Text != "" && !strings.ContainsAny(key.Text, "\r\n\t") {
				m.agentManager.directory += key.Text
			}
		}
	case confirmRuntimeSwitch:
		switch strings.ToLower(key.String()) {
		case "y", "enter":
			request := m.agentManager.pending
			request.ConfirmSwitch = true
			m.agentManager.busy = true
			m.agentManager.status = "switching harness runtime…"
			return m, m.launchManagedAgent(request)
		case "n":
			for index := range m.agentManager.pending.Environment {
				m.agentManager.pending.Environment[index] = ""
			}
			m.agentManager.stage = chooseRuntimeSession
			m.agentManager.pending = domain.HarnessLaunchRequest{}
		}
	case enterThreadName:
		switch key.String() {
		case "enter":
			m.agentManager.busy = true
			m.agentManager.status = "renaming session…"
			return m, m.renameManagedThread(m.agentManager.renameSession, m.agentManager.threadName)
		case "backspace":
			runes := []rune(m.agentManager.threadName)
			if len(runes) > 0 {
				m.agentManager.threadName = string(runes[:len(runes)-1])
			}
		default:
			if key.Text != "" && !strings.ContainsAny(key.Text, "\r\n\t") {
				m.agentManager.threadName += key.Text
			}
		}
	}
	return m, nil
}

func (m *app) beginNewRuntimeHarness() {
	m.agentManager.stage = enterRuntimeHarness
	m.agentManager.harness = m.agentManager.agent.Harness
	if m.agentManager.harness == "" {
		m.agentManager.harness = "codex"
	}
	m.agentManager.status = ""
}

func (m app) defaultRuntimeDirectory() string {
	if m.agentManager.agent.Context.Directory != "" {
		return m.agentManager.agent.Context.Directory
	}
	return m.launchDirectory
}

func (m app) validRuntimeDirectory(raw string) (string, error) {
	directory, err := m.expandClientPath(raw)
	if err != nil {
		return "", err
	}
	info, err := os.Stat(directory)
	if err != nil {
		return "", errors.New("directory does not exist")
	}
	if !info.IsDir() {
		return "", errors.New("path is not a directory")
	}
	return directory, nil
}

func (m app) runtimeRequest(action domain.HarnessSessionAction, sessionID, directory string) domain.HarnessLaunchRequest {
	repository := model.RepositoryContext{Directory: directory}
	if snapshotter, ok := m.repo.(interface {
		Snapshot(context.Context, string) model.RepositoryContext
	}); ok {
		repository = snapshotter.Snapshot(m.ctx, directory)
	}
	harnessID := m.agentManager.agent.Harness
	if action == domain.HarnessSessionNew {
		harnessID = m.agentManager.harness
	}
	for _, session := range m.agentManager.sessions {
		if session.SessionID == sessionID {
			harnessID = session.Harness
			break
		}
	}
	return domain.HarnessLaunchRequest{
		RequestID: uuid.NewString(), AgentName: m.agentManager.agent.Name, Harness: harnessID, Action: action, SessionID: sessionID,
		Directory: directory, Repository: repository, Environment: append([]string(nil), m.launchEnvironment...),
		ProviderOptions: providerOptions(harnessID, m.agentManager.yolo),
	}
}

func providerOptions(harnessID string, yolo bool) json.RawMessage {
	if harnessID != "codex" {
		return nil
	}
	return codexProviderOptions(yolo)
}

func codexProviderOptions(yolo bool) json.RawMessage {
	raw, _ := json.Marshal(map[string]any{"yolo": yolo})
	return raw
}

func providerYolo(raw json.RawMessage) bool {
	var options struct {
		Yolo bool `json:"yolo"`
	}
	_ = json.Unmarshal(raw, &options)
	return options.Yolo
}

func (m app) confirmOrLaunch(request domain.HarnessLaunchRequest) (tea.Model, tea.Cmd) {
	if m.agentManager.runtime.Phase == domain.HarnessRuntimeRunning && (request.Harness != m.agentManager.runtime.Harness || request.Action == domain.HarnessSessionNew || request.SessionID != m.agentManager.runtime.SessionID) {
		m.agentManager.stage = confirmRuntimeSwitch
		m.agentManager.pending = request
		m.agentManager.status = "replace the running harness worker? y/n"
		return m, nil
	}
	m.agentManager.busy = true
	m.agentManager.status = "starting harness runtime…"
	return m, m.launchManagedAgent(request)
}

func (m app) loadAgentSessions(agent domain.NamedAgent) tea.Cmd {
	return func() tea.Msg {
		sessions, err := m.store.ListNamedAgentSessions(m.ctx, agent.Name)
		controller, ok := m.store.(domain.HarnessRuntimeController)
		if err == nil && !ok {
			err = errors.New("harness runtime control is unavailable")
		}
		var runtime domain.HarnessRuntime
		if err == nil {
			runtime, err = controller.HarnessAgentRuntime(m.ctx, agent.Name)
		}
		return agentSessionsMsg{agent: agent, sessions: sessions, runtime: runtime, err: err}
	}
}

func (m app) launchManagedAgent(request domain.HarnessLaunchRequest) tea.Cmd {
	return func() tea.Msg {
		controller, ok := m.store.(domain.HarnessRuntimeController)
		if !ok {
			return harnessRuntimeMsg{err: errors.New("harness runtime control is unavailable")}
		}
		runtime, err := controller.LaunchHarnessAgent(m.ctx, request)
		for index := range request.Environment {
			request.Environment[index] = ""
		}
		return harnessRuntimeMsg{runtime: runtime, err: err}
	}
}

func (m app) stopManagedAgent() tea.Cmd {
	return func() tea.Msg {
		controller, ok := m.store.(domain.HarnessRuntimeController)
		if !ok {
			return harnessRuntimeMsg{err: errors.New("harness runtime control is unavailable")}
		}
		runtime, err := controller.StopHarnessAgent(m.ctx, m.agentManager.agent.Name)
		return harnessRuntimeMsg{runtime: runtime, err: err}
	}
}

func (m app) renameManagedThread(session domain.AgentSession, threadName string) tea.Cmd {
	return func() tea.Msg {
		renamed, err := m.store.RenameNamedAgentSession(m.ctx, session.AgentName, model.SessionIdentity{Harness: session.Harness, ExternalSessionID: session.SessionID}, threadName)
		return renamedAgentSessionMsg{session: renamed, err: err}
	}
}

func shortThreadID(id string) string {
	if len(id) <= 12 {
		return id
	}
	return id[:8] + "…" + id[len(id)-4:]
}

func (m app) managedThreadName(provider, id string) string {
	for _, session := range m.agentManager.sessions {
		if session.Harness == provider && session.SessionID == id {
			return session.ThreadName
		}
	}
	return ""
}

func threadLabel(name, id string) string {
	if strings.TrimSpace(name) == "" {
		return shortThreadID(id)
	}
	return name + " (" + shortThreadID(id) + ")"
}

func (m app) filteredRecipients() []recipientChoice {
	query := strings.ToLower(strings.TrimSpace(m.pickerQuery))
	choices := m.recipients()
	if query == "" {
		return choices
	}
	filtered := make([]recipientChoice, 0, len(choices))
	for _, choice := range choices {
		if choice.newProject {
			if strings.HasPrefix("new project", query) {
				filtered = append(filtered, choice)
			}
		} else if strings.Contains(strings.ToLower(choice.name), query) {
			filtered = append(filtered, choice)
		}
	}
	return filtered
}

func (m app) updateRecipientPicker(msg tea.KeyPressMsg) (tea.Model, tea.Cmd) {
	choices := m.filteredRecipients()
	switch msg.String() {
	case "ctrl+c", "esc":
		m.pickingRecipient = false
		m.pickerQuery = ""
		m.pickerCursor = 0
		m.paneFocus = focusInbox
		return m, nil
	case "j", "down":
		if m.pickerCursor+1 < len(choices) {
			m.pickerCursor++
		}
		return m, nil
	case "k", "up":
		if m.pickerCursor > 0 {
			m.pickerCursor--
		}
		return m, nil
	case "enter":
		if len(choices) == 0 {
			return m, nil
		}
		choice := choices[min(m.pickerCursor, len(choices)-1)]
		if choice.newProject {
			m.pickingRecipient = false
			m.projectSetup = &projectComposeSetup{stage: enterProjectName, home: m.account.LocalInstallationID}
			m.editor.Blur()
			return m, nil
		}
		if choice.project && !choice.active {
			for _, project := range m.projects {
				if project.ID == choice.projectID {
					if project.PendingCommand != nil {
						m.err = domain.ErrProjectCommandPending
						return m, nil
					}
					return m.beginProjectSetup(project)
				}
			}
		}
		m.pickingRecipient = false
		m.answering = true
		m.answerID = ""
		m.answerGroupKey = ""
		m.answerQ = model.Message{}
		m.composeTo, m.composeName = choice.mailboxID, choice.name
		m.composeContext, m.composeNamed = choice.context, choice.named
		m.composeActivation = nil
		m.activeDraftKey = "draft:" + uuid.NewString()
		m.paneFocus = focusReply
		m.resizeEditor()
		m.editor.Focus()
		m.reconcileMessageViewport(true)
		return m, textarea.Blink
	case "backspace":
		runes := []rune(m.pickerQuery)
		if len(runes) > 0 {
			m.pickerQuery = string(runes[:len(runes)-1])
			m.pickerCursor = 0
		}
		return m, nil
	}
	if msg.Text != "" && !strings.ContainsAny(msg.Text, "\r\n\t") {
		m.pickerQuery += msg.Text
		m.pickerCursor = 0
	}
	return m, nil
}

func (m app) beginProjectSetup(project domain.Project) (tea.Model, tea.Cmd) {
	agents := make([]domain.NamedAgent, 0, len(m.agents))
	for _, agent := range m.agents {
		if !agent.Retired && agent.Idle {
			agents = append(agents, agent)
		}
	}
	if project.ReadOnlyReplica && project.SuggestedAgentName != "" {
		found := false
		for _, agent := range agents {
			found = found || agent.Name == project.SuggestedAgentName
		}
		if !found {
			agents = append(agents, domain.NamedAgent{Name: project.SuggestedAgentName, Idle: true})
		}
	}
	sort.SliceStable(agents, func(i, j int) bool {
		if agents[i].Name == project.SuggestedAgentName {
			return true
		}
		if agents[j].Name == project.SuggestedAgentName {
			return false
		}
		return agents[i].Name < agents[j].Name
	})
	m.pickingRecipient = false
	m.projectSetup = &projectComposeSetup{project: project, stage: chooseProjectAgent, agents: agents}
	m.editor.Blur()
	return m, nil
}

func (m app) updateProjectSetup(key tea.KeyPressMsg) (tea.Model, tea.Cmd) {
	setup := m.projectSetup
	if setup == nil {
		return m, nil
	}
	if setup.busy {
		if key.String() == "esc" {
			m.projectSetup = nil
			m.pickingRecipient = true
		}
		return m, nil
	}
	if key.String() == "ctrl+c" || key.String() == "esc" {
		if setup.stage == enterProjectName || setup.stage == chooseProjectAgent && setup.project.ID != "" {
			m.projectSetup = nil
			m.pickingRecipient = true
			return m, nil
		}
		switch setup.stage {
		case chooseProjectHome:
			setup.stage = enterProjectName
		case enterProjectBrief:
			setup.stage = chooseProjectHome
		case enterProjectPaths:
			setup.stage = enterProjectBrief
		case chooseProjectPrimary:
			setup.stage = enterProjectPaths
		case enterWorktreeRepository:
			setup.stage = enterProjectPaths
		case enterWorktreeBase:
			setup.stage = enterWorktreeRepository
		case enterWorktreeDestination:
			setup.stage = enterWorktreeBase
		case enterWorktreeBranch:
			setup.stage = enterWorktreeDestination
		case chooseWorktreePrimary:
			setup.stage = enterWorktreeBranch
		case enterProjectHarness:
			setup.stage = chooseProjectAgent
		case chooseProjectThread:
			setup.stage = enterProjectHarness
		case enterProjectDirectory:
			setup.stage = chooseProjectThread
		default:
			setup.stage = enterProjectName
		}
		setup.cursor, setup.status = 0, ""
		return m, nil
	}
	switch setup.stage {
	case enterProjectName:
		switch key.String() {
		case "enter":
			if strings.TrimSpace(setup.name) == "" {
				setup.status = "Project name is required."
			} else {
				setup.stage, setup.cursor, setup.status = chooseProjectHome, 0, ""
			}
		case "backspace":
			setup.name = trimLastRune(setup.name)
		default:
			setup.name += printableKeyText(key)
		}
	case chooseProjectHome:
		devices := m.activeProjectHomes()
		switch key.String() {
		case "j", "down":
			setup.cursor = min(max(0, len(devices)-1), setup.cursor+1)
		case "k", "up":
			setup.cursor = max(0, setup.cursor-1)
		case "enter":
			if len(devices) != 0 {
				setup.home = devices[min(setup.cursor, len(devices)-1)].InstallationID
				setup.stage, setup.status = enterProjectBrief, ""
			}
		}
	case enterProjectBrief:
		switch key.String() {
		case "enter":
			setup.stage, setup.status = enterProjectPaths, ""
		case "backspace":
			setup.brief = trimLastRune(setup.brief)
		default:
			setup.brief += printableKeyText(key)
		}
	case enterProjectPaths:
		switch key.String() {
		case "tab":
			paths, err := m.expandProjectPaths(setup.pathsText)
			if err != nil {
				setup.status = err.Error()
				return m, nil
			}
			setup.paths = paths
			setup.pathsText = strings.Join(paths, ", ")
			setup.stage, setup.worktreeBase, setup.status = enterWorktreeRepository, "HEAD", ""
		case "enter":
			paths, err := m.expandProjectPaths(setup.pathsText)
			if err != nil {
				setup.status = err.Error()
				return m, nil
			}
			setup.paths = paths
			setup.pathsText = strings.Join(paths, ", ")
			if len(setup.paths) > 1 {
				setup.stage, setup.cursor, setup.status = chooseProjectPrimary, 0, ""
			} else {
				setup.busy, setup.status = true, "creating project…"
				return m, m.createProjectFromSetup(*setup, 0)
			}
		case "backspace":
			setup.pathsText = trimLastRune(setup.pathsText)
		default:
			setup.pathsText += printableKeyText(key)
		}
	case chooseProjectPrimary:
		switch key.String() {
		case "j", "down":
			setup.cursor = min(len(setup.paths)-1, setup.cursor+1)
		case "k", "up":
			setup.cursor = max(0, setup.cursor-1)
		case "enter":
			setup.busy, setup.status = true, "creating project…"
			return m, m.createProjectFromSetup(*setup, setup.cursor)
		}
	case enterWorktreeRepository:
		switch key.String() {
		case "enter":
			if strings.TrimSpace(setup.worktreeRepository) == "" {
				setup.status = "Repository is required."
			} else {
				repository, err := m.expandClientPath(setup.worktreeRepository)
				if err != nil {
					setup.status = err.Error()
					return m, nil
				}
				setup.worktreeRepository = repository
				setup.stage, setup.status = enterWorktreeBase, ""
			}
		case "backspace":
			setup.worktreeRepository = trimLastRune(setup.worktreeRepository)
		default:
			setup.worktreeRepository += printableKeyText(key)
		}
	case enterWorktreeBase:
		switch key.String() {
		case "enter":
			if strings.TrimSpace(setup.worktreeBase) == "" {
				setup.worktreeBase = "HEAD"
			}
			setup.stage, setup.status = enterWorktreeDestination, ""
		case "backspace":
			setup.worktreeBase = trimLastRune(setup.worktreeBase)
		default:
			setup.worktreeBase += printableKeyText(key)
		}
	case enterWorktreeDestination:
		switch key.String() {
		case "enter":
			if strings.TrimSpace(setup.worktreeDestination) == "" {
				setup.status = "Worktree destination is required."
			} else {
				destination, err := m.expandClientPath(setup.worktreeDestination)
				if err != nil {
					setup.status = err.Error()
					return m, nil
				}
				setup.worktreeDestination = destination
				setup.stage, setup.status = enterWorktreeBranch, ""
			}
		case "backspace":
			setup.worktreeDestination = trimLastRune(setup.worktreeDestination)
		default:
			setup.worktreeDestination += printableKeyText(key)
		}
	case enterWorktreeBranch:
		switch key.String() {
		case "enter":
			if strings.TrimSpace(setup.worktreeBranch) == "" {
				setup.status = "Branch name is required."
			} else if len(setup.paths) != 0 {
				setup.stage, setup.cursor, setup.status = chooseWorktreePrimary, 0, ""
			} else {
				setup.busy, setup.status = true, "reserving destination and creating worktree…"
				return m, m.createWorktreeFromSetup(*setup, 0)
			}
		case "backspace":
			setup.worktreeBranch = trimLastRune(setup.worktreeBranch)
		default:
			setup.worktreeBranch += printableKeyText(key)
		}
	case chooseWorktreePrimary:
		options := append([]string{setup.worktreeDestination}, setup.paths...)
		switch key.String() {
		case "j", "down":
			setup.cursor = min(len(options)-1, setup.cursor+1)
		case "k", "up":
			setup.cursor = max(0, setup.cursor-1)
		case "enter":
			setup.busy, setup.status = true, "reserving destination and creating worktree…"
			return m, m.createWorktreeFromSetup(*setup, setup.cursor)
		}
	case chooseProjectAgent:
		agents := setup.filteredAgents()
		switch key.String() {
		case "j", "down":
			setup.cursor = min(max(0, len(agents)-1), setup.cursor+1)
		case "k", "up":
			setup.cursor = max(0, setup.cursor-1)
		case "enter":
			if len(agents) != 0 || setup.project.ReadOnlyReplica && strings.TrimSpace(setup.query) != "" {
				if len(agents) != 0 {
					setup.agent = agents[min(setup.cursor, len(agents)-1)]
				} else {
					setup.agent = domain.NamedAgent{Name: strings.TrimSpace(setup.query), Idle: true}
				}
				setup.busy = true
				return m, func() tea.Msg {
					threads, err := m.store.ListProjectThreads(m.ctx, setup.project.ID)
					return projectThreadsMsg{threads: threads, err: err}
				}
			}
		case "backspace":
			runes := []rune(setup.query)
			if len(runes) > 0 {
				setup.query, setup.cursor = string(runes[:len(runes)-1]), 0
			}
		default:
			if key.Text != "" && !strings.ContainsAny(key.Text, "\r\n\t") {
				setup.query, setup.cursor = setup.query+key.Text, 0
			}
		}
	case enterProjectHarness:
		switch key.String() {
		case "enter":
			setup.harness = strings.TrimSpace(setup.harness)
			if setup.harness == "" {
				setup.status = "Harness provider is required."
			} else {
				setup.stage, setup.cursor, setup.status = chooseProjectThread, 0, ""
			}
		case "backspace":
			setup.harness = trimLastRune(setup.harness)
		default:
			setup.harness += printableKeyText(key)
		}
	case chooseProjectThread:
		threads := setup.compatibleThreads()
		switch key.String() {
		case "j", "down":
			setup.cursor = min(len(threads), setup.cursor+1)
		case "k", "up":
			setup.cursor = max(0, setup.cursor-1)
		case "f":
			if setup.project.Assignment != nil {
				setup.force = !setup.force
			}
		case "enter":
			if setup.project.Assignment != nil && !setup.force {
				setup.status = "This handoff is blocked; press f to acknowledge a forced takeover."
				return m, nil
			}
			if setup.cursor == 0 {
				setup.stage, setup.directory, setup.status = enterProjectDirectory, m.projectDefaultDirectory(setup.project), ""
				return m, nil
			}
			thread := threads[setup.cursor-1]
			return m.finishProjectSetup(domain.HarnessSessionResume, thread.ExternalID, thread.LaunchDir)
		}
	case enterProjectDirectory:
		switch key.String() {
		case "enter":
			directory, err := m.validRuntimeDirectory(setup.directory)
			if err != nil {
				setup.status = err.Error()
				return m, nil
			}
			return m.finishProjectSetup(domain.HarnessSessionNew, "", directory)
		case "backspace":
			runes := []rune(setup.directory)
			if len(runes) > 0 {
				setup.directory = string(runes[:len(runes)-1])
			}
		default:
			if key.Text != "" && !strings.ContainsAny(key.Text, "\r\n\t") {
				setup.directory += key.Text
			}
		}
	}
	return m, nil
}

func (s *projectComposeSetup) filteredAgents() []domain.NamedAgent {
	query := strings.ToLower(strings.TrimSpace(s.query))
	if query == "" {
		return s.agents
	}
	var result []domain.NamedAgent
	for _, agent := range s.agents {
		if strings.Contains(strings.ToLower(agent.Name), query) {
			result = append(result, agent)
		}
	}
	return result
}

func trimLastRune(value string) string {
	runes := []rune(value)
	if len(runes) == 0 {
		return value
	}
	return string(runes[:len(runes)-1])
}

func printableKeyText(key tea.KeyPressMsg) string {
	if key.Text == "" || strings.ContainsAny(key.Text, "\r\n\t") {
		return ""
	}
	return key.Text
}

func splitProjectPaths(value string) []string {
	fields := strings.FieldsFunc(value, func(r rune) bool { return r == ',' || r == ';' })
	result := make([]string, 0, len(fields))
	for _, field := range fields {
		if path := strings.TrimSpace(field); path != "" {
			result = append(result, path)
		}
	}
	return result
}

func (m app) expandClientPath(value string) (string, error) {
	value = os.ExpandEnv(strings.TrimSpace(value))
	homeRelative := strings.HasPrefix(value, "~/") ||
		(filepath.Separator != '/' && strings.HasPrefix(value, "~"+string(filepath.Separator)))
	if value == "~" || homeRelative {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", fmt.Errorf("expand home directory: %w", err)
		}
		if value == "~" {
			value = home
		} else {
			value = filepath.Join(home, value[2:])
		}
	} else if strings.HasPrefix(value, "~") {
		return "", errors.New("only the current user's ~ home path is supported")
	}
	if value == "" {
		return "", errors.New("path is empty after environment expansion")
	}
	if !filepath.IsAbs(value) {
		base := m.launchDirectory
		if base == "" {
			var err error
			base, err = os.Getwd()
			if err != nil {
				return "", fmt.Errorf("resolve client working directory: %w", err)
			}
		}
		value = filepath.Join(base, value)
	}
	return filepath.Clean(value), nil
}

func (m app) expandProjectPaths(value string) ([]string, error) {
	paths := splitProjectPaths(value)
	for index, path := range paths {
		expanded, err := m.expandClientPath(path)
		if err != nil {
			return nil, fmt.Errorf("path %q: %w", path, err)
		}
		paths[index] = expanded
	}
	return paths, nil
}

func (m app) activeProjectHomes() []domain.HumanDevice {
	var result []domain.HumanDevice
	for _, device := range m.devices {
		if device.State == "active" {
			result = append(result, device)
		}
	}
	if len(result) == 0 && m.account.LocalInstallationID != "" {
		result = append(result, domain.HumanDevice{InstallationID: m.account.LocalInstallationID, Label: "this device", State: "active"})
	}
	sort.SliceStable(result, func(i, j int) bool {
		if result[i].InstallationID == m.account.LocalInstallationID {
			return true
		}
		if result[j].InstallationID == m.account.LocalInstallationID {
			return false
		}
		return result[i].Label < result[j].Label
	})
	return result
}

func (m app) createProjectFromSetup(setup projectComposeSetup, primary int) tea.Cmd {
	return func() tea.Msg {
		request := domain.CreateProjectRequest{Name: strings.TrimSpace(setup.name), HomeInstallation: setup.home, Brief: strings.TrimSpace(setup.brief), PrimaryPath: primary}
		for _, path := range setup.paths {
			request.Paths = append(request.Paths, domain.ProjectPathInput{DisplayPath: path})
		}
		project, err := m.store.CreateProject(m.ctx, request)
		return projectCreatedMsg{project: project, err: err}
	}
}

func (m app) createWorktreeFromSetup(setup projectComposeSetup, primary int) tea.Cmd {
	return func() tea.Msg {
		provisioner, ok := m.store.(domain.ProjectWorktreeProvisioner)
		if !ok {
			return projectCreatedMsg{err: errors.New("project worktree provisioning is unavailable")}
		}
		request := domain.ProjectWorktreeRequest{RequestID: uuid.NewString(), ProjectID: uuid.NewString(), HomeInstallation: setup.home, Name: strings.TrimSpace(setup.name), Brief: strings.TrimSpace(setup.brief), Repository: strings.TrimSpace(setup.worktreeRepository), MergeBase: strings.TrimSpace(setup.worktreeBase), Destination: strings.TrimSpace(setup.worktreeDestination), Branch: strings.TrimSpace(setup.worktreeBranch), PrimaryPath: primary}
		for _, path := range setup.paths {
			request.AdditionalPaths = append(request.AdditionalPaths, domain.ProjectPathInput{DisplayPath: path})
		}
		project, err := provisioner.ProvisionProjectWorktree(m.ctx, request)
		return projectCreatedMsg{project: project, err: err}
	}
}

func (m *app) prepareProjectAgents() {
	if m.projectSetup == nil {
		return
	}
	var agents []domain.NamedAgent
	for _, agent := range m.agents {
		if !agent.Retired && agent.Idle {
			agents = append(agents, agent)
		}
	}
	sort.SliceStable(agents, func(i, j int) bool { return agents[i].Name < agents[j].Name })
	m.projectSetup.agents, m.projectSetup.stage, m.projectSetup.cursor, m.projectSetup.status = agents, chooseProjectAgent, 0, ""
}

func (s *projectComposeSetup) compatibleThreads() []domain.ProjectThread {
	var result []domain.ProjectThread
	for _, thread := range s.threads {
		if thread.AgentName == s.agent.Name && !thread.RetiredAgent && thread.Harness == s.harness {
			result = append(result, thread)
		}
	}
	return result
}

func (m app) projectDefaultDirectory(project domain.Project) string {
	for _, resource := range project.Resources {
		if resource.ID == project.PrimaryResourceID && resource.Kind == "path" {
			return resource.DisplayLocator
		}
	}
	return m.launchDirectory
}

func (m app) finishProjectSetup(action domain.HarnessSessionAction, sessionID, directory string) (tea.Model, tea.Cmd) {
	setup := m.projectSetup
	m.composeActivation = &projectActivationPlan{projectID: setup.project.ID, agentName: setup.agent.Name, harness: setup.harness, action: action, sessionID: sessionID, directory: directory, force: setup.force}
	m.projectSetup = nil
	m.answering = true
	m.answerID, m.answerGroupKey = "", ""
	m.answerQ = model.Message{}
	m.composeTo, m.composeName = setup.project.MailboxID, fmt.Sprintf("%s · %s/%s", setup.project.Name, short(setup.project.HomeInstallation, 8), short(setup.project.ID, 8))
	m.composeContext, m.composeNamed = model.RepositoryContext{}, false
	m.activeDraftKey = "draft:" + uuid.NewString()
	m.paneFocus = focusReply
	m.resizeEditor()
	m.editor.Focus()
	return m, textarea.Blink
}

func (m *app) resizeEditor() {
	layout := responsivePaneLayout(m.width, m.height, true)
	m.editor.SetWidth(max(1, layout.replyWidth-panel.GetHorizontalFrameSize()))
	m.editor.SetHeight(max(1, layout.replyHeight-4))
}

func (m app) beginComposeForSelection() (tea.Model, tea.Cmd) {
	if group, ok := m.groupAtCursor(); ok {
		if group.draft != nil {
			m.resumeDraft(*group.draft)
			return m, textarea.Blink
		}
		if canReplyGroup(group) {
			m.answering = true
			m.answerQ = replyTarget(group)
			m.answerID = m.answerQ.ID
			m.answerGroupKey = group.key
			m.activeDraftKey = group.key
			m.composeTo = ""
			m.composeName = ""
			m.composeContext = model.RepositoryContext{}
			m.composeNamed = false
			m.paneFocus = focusReply
			m.resizeEditor()
			m.editor.Focus()
			m.reconcileMessageViewport(true)
			return m, textarea.Blink
		}
	}

	m.pickingRecipient = true
	m.pickerQuery = ""
	m.pickerCursor = 0
	m.paneFocus = focusReply
	m.editor.Blur()
	return m, nil
}

func (m *app) cyclePaneFocus(direction int) {
	count := int(focusReply) + 1
	next := (int(m.paneFocus) + direction) % count
	if next < 0 {
		next += count
	}
	m.paneFocus = paneFocus(next)
}

func (m *app) stowActiveDraft() {
	if !m.answering {
		return
	}
	key := m.activeDraftKey
	if key == "" {
		key = m.answerGroupKey
	}
	body := m.editor.Value()
	if strings.TrimSpace(body) == "" {
		if key != "" {
			delete(m.drafts, key)
		}
	} else {
		if key == "" {
			key = "draft:" + uuid.NewString()
		}
		if m.drafts == nil {
			m.drafts = make(map[string]messageDraft)
		}
		m.drafts[key] = messageDraft{
			key: key, body: body, answerID: m.answerID, answerGroupKey: m.answerGroupKey,
			answerQ: m.answerQ, composeTo: m.composeTo, composeName: m.composeName,
			composeContext: m.composeContext, composeNamed: m.composeNamed, activation: m.composeActivation, updatedAt: time.Now(),
		}
	}
	m.answering = false
	m.activeDraftKey = ""
	m.answerID = ""
	m.answerGroupKey = ""
	m.answerQ = model.Message{}
	m.composeTo = ""
	m.composeName = ""
	m.composeContext = model.RepositoryContext{}
	m.composeNamed = false
	m.composeActivation = nil
	m.editor.Blur()
	m.editor.Reset()
	groups := m.visibleGroups()
	if index := groupIndex(groups, key); index >= 0 {
		m.cursor = index
	} else if m.cursor >= len(groups) {
		m.cursor = max(0, len(groups)-1)
	}
	m.reconcileMessageViewport(true)
}

func (m *app) resumeDraft(draft messageDraft) {
	m.answering = true
	m.activeDraftKey = draft.key
	m.answerID = draft.answerID
	m.answerGroupKey = draft.answerGroupKey
	m.answerQ = draft.answerQ
	m.composeTo = draft.composeTo
	m.composeName = draft.composeName
	m.composeContext = draft.composeContext
	m.composeNamed = draft.composeNamed
	m.composeActivation = draft.activation
	m.paneFocus = focusReply
	m.resizeEditor()
	m.editor.SetValue(draft.body)
	m.editor.Focus()
	m.reconcileMessageViewport(true)
}

func (m app) paneFocused(pane paneFocus) bool { return m.paneFocus == pane }

func (m *app) setMessages() {
	if m.conversationMode {
		m.groups = make([]messageGroup, 0, len(m.conversations))
		m.messages = make([]model.Message, 0, len(m.conversations))
		for _, summary := range m.conversations {
			key := conversationKeyString(summary.Key)
			messages := []model.Message{summary.Latest}
			if summary.OldestOpen != nil && summary.OldestOpen.ID != summary.Latest.ID {
				messages = append(messages, *summary.OldestOpen)
				sort.Slice(messages, func(i, j int) bool {
					if messages[i].CreatedAt.Equal(messages[j].CreatedAt) {
						return messages[i].ID < messages[j].ID
					}
					return messages[i].CreatedAt.Before(messages[j].CreatedAt)
				})
			}
			if history, loaded := m.histories[key]; loaded {
				messages = append([]model.Message(nil), history...)
			}
			m.groups = append(m.groups, messageGroup{key: key, conversationKey: summary.Key, messages: messages, activities: append([]domain.HarnessActivity(nil), m.activities[key]...)})
			m.messages = append(m.messages, summary.Latest)
		}
		return
	}
	seen := make(map[string]bool)
	var allMessages []model.Message
	add := func(batch []model.Message) {
		for _, message := range batch {
			if !seen[message.ID] {
				seen[message.ID] = true
				allMessages = append(allMessages, message)
			}
		}
	}
	add(m.inbox)
	if m.showSent {
		for _, message := range m.sent {
			if message.ArchivedAt == nil || m.showArchived {
				add([]model.Message{message})
			}
		}
	}
	if m.showArchived {
		add(m.archived)
	}
	m.groups = groupMessages(allMessages)
	m.messages = make([]model.Message, 0, len(m.groups))
	for _, group := range m.groups {
		m.messages = append(m.messages, group.latest())
	}
}

func groupMessages(messages []model.Message) []messageGroup {
	byKey := make(map[string]int)
	groups := make([]messageGroup, 0, len(messages))
	for _, message := range messages {
		key := messageGroupKey(message)
		index, found := byKey[key]
		if !found {
			index = len(groups)
			byKey[key] = index
			groups = append(groups, messageGroup{key: key})
		}
		groups[index].messages = append(groups[index].messages, message)
	}
	for i := range groups {
		sort.Slice(groups[i].messages, func(a, b int) bool {
			left, right := groups[i].messages[a], groups[i].messages[b]
			if left.CreatedAt.Equal(right.CreatedAt) {
				return left.ID < right.ID
			}
			return left.CreatedAt.Before(right.CreatedAt)
		})
	}
	sort.Slice(groups, func(i, j int) bool {
		left, right := groups[i].latest(), groups[j].latest()
		if left.CreatedAt.Equal(right.CreatedAt) {
			return left.ID > right.ID
		}
		return left.CreatedAt.After(right.CreatedAt)
	})
	return groups
}

func messageGroupKey(message model.Message) string {
	return conversationKeyString(conversationKeyForMessage(message))
}

func conversationKeyForMessage(message model.Message) model.ConversationKey {
	counterparty := message.SenderMailboxID
	if counterparty == model.HumanMailboxID {
		counterparty = message.RecipientMailboxID
	}
	correlation := correlationForMessage(message)
	key := model.ConversationKey{CounterpartyMailboxID: counterparty, HarnessProvider: correlation.Provider, HarnessSessionID: correlation.SessionID}
	if key.HarnessSessionID == "" {
		key.ThreadID = message.ThreadID
		if key.ThreadID == "" {
			key.ThreadID = message.ID
		}
	}
	return key
}

func conversationKeyString(key model.ConversationKey) string {
	if key.HarnessSessionID != "" {
		return "conversation:" + key.CounterpartyMailboxID + ":harness:" + key.HarnessProvider + ":" + key.HarnessSessionID
	}
	return "conversation:" + key.CounterpartyMailboxID + ":thread:" + key.ThreadID
}

func conversationSummaryByString(summaries []model.ConversationSummary, stableKey string) (model.ConversationSummary, bool) {
	for _, summary := range summaries {
		if conversationKeyString(summary.Key) == stableKey {
			return summary, true
		}
	}
	return model.ConversationSummary{}, false
}

func detailValue(details, prefix string) string {
	for _, line := range strings.Split(details, "\n") {
		if value, found := strings.CutPrefix(strings.TrimSpace(line), prefix); found {
			return strings.TrimSpace(value)
		}
	}
	return ""
}

func (m app) visibleGroups() []messageGroup {
	var base []messageGroup
	if len(m.groups) > 0 || len(m.messages) == 0 {
		base = m.groups
	} else {
		base = groupMessages(m.messages)
	}
	groups := make([]messageGroup, len(base))
	copy(groups, base)
	seenDrafts := make(map[string]bool)
	for i := range groups {
		if draft, ok := m.drafts[groups[i].key]; ok {
			copyDraft := draft
			groups[i].draft = &copyDraft
			seenDrafts[draft.key] = true
		}
	}
	for key, draft := range m.drafts {
		if seenDrafts[key] {
			continue
		}
		copyDraft := draft
		group := messageGroup{key: key, draft: &copyDraft}
		if draft.answerQ.ID != "" {
			group.conversationKey = conversationKeyForMessage(draft.answerQ)
			group.activities = append([]domain.HarnessActivity(nil), m.activities[key]...)
			if history, loaded := m.histories[key]; loaded {
				group.messages = append([]model.Message(nil), history...)
			} else {
				group.messages = []model.Message{draft.answerQ}
			}
		}
		groups = append(groups, group)
	}
	sort.SliceStable(groups, func(i, j int) bool {
		return groupActivity(groups[i]).After(groupActivity(groups[j]))
	})
	return groups
}

func groupActivity(group messageGroup) time.Time {
	latest := group.latest().CreatedAt
	if group.draft != nil && group.draft.updatedAt.After(latest) {
		return group.draft.updatedAt
	}
	return latest
}

func (m app) groupAtCursor() (messageGroup, bool) {
	groups := m.visibleGroups()
	if m.cursor < 0 || m.cursor >= len(groups) {
		return messageGroup{}, false
	}
	return groups[m.cursor], true
}

func (m app) withContextCommand() (tea.Model, tea.Cmd) {
	var q model.Message
	var historyCmd tea.Cmd
	if m.answering {
		if group, found := m.groupByKey(m.selectedGroupKey()); found {
			q = group.latest()
			historyCmd = m.loadConversationHistory(group.conversationKey)
		} else {
			q = m.answerQ
		}
	} else if group, found := m.groupAtCursor(); found && len(group.messages) > 0 {
		q = group.latest()
		historyCmd = m.loadConversationHistory(group.conversationKey)
	}
	if q.ID == "" || m.repo == nil {
		m.contextID, m.branch, m.remotes, m.pull = "", "", "", ""
		return m, historyCmd
	}
	if q.ID == m.contextID {
		return m, historyCmd
	}
	m.contextID = q.ID
	m.branch = "git loading…"
	m.remotes = ""
	m.pull = ""
	return m, tea.Batch(m.loadBranch(q), historyCmd)
}

func (m app) loadRemotes(q model.Message, branch string) tea.Cmd {
	return func() tea.Msg {
		remotes, err := m.repo.Remotes(m.ctx, q.Context.Directory)
		return remotesMsg{message: q, branch: branch, remotes: remotes, err: err}
	}
}

func (m app) loadBranch(q model.Message) tea.Cmd {
	return func() tea.Msg {
		branch, err := m.repo.Branch(m.ctx, q.Context.Directory)
		return branchMsg{message: q, branch: branch, err: err}
	}
}

func (m app) loadPull(q model.Message, branch string) tea.Cmd {
	return func() tea.Msg {
		pull, err := m.repo.PullRequest(m.ctx, q.Context.Directory, branch)
		return pullMsg{questionID: q.ID, pull: pull, err: err}
	}
}

func formatRemotes(remotes []repoctx.Remote) string {
	parts := make([]string, 0, len(remotes))
	for _, remote := range remotes {
		parts = append(parts, remote.Name+": "+remote.Display)
	}
	return strings.Join(parts, " · ")
}

func (m app) selectedGroupKey() string {
	if m.answering {
		if m.answerGroupKey == "" && m.answerQ.ID != "" {
			return messageGroupKey(m.answerQ)
		}
		return m.answerGroupKey
	}
	if group, found := m.groupAtCursor(); found {
		return group.key
	}
	return ""
}

func groupIndex(groups []messageGroup, key string) int {
	for i := range groups {
		if groups[i].key == key {
			return i
		}
	}
	return -1
}

func (m app) groupByKey(key string) (messageGroup, bool) {
	groups := m.visibleGroups()
	if index := groupIndex(groups, key); index >= 0 {
		return groups[index], true
	}
	return messageGroup{}, false
}

func (m app) detailGroup() (messageGroup, bool) {
	if m.answering {
		if group, found := m.groupByKey(m.selectedGroupKey()); found {
			return group, true
		}
		if m.answerQ.ID != "" {
			return messageGroup{key: messageGroupKey(m.answerQ), conversationKey: conversationKeyForMessage(m.answerQ), messages: []model.Message{m.answerQ}}, true
		}
		return messageGroup{}, false
	}
	return m.groupAtCursor()
}

func messagePaneMaxStart(rendered string, height int) int {
	lines := strings.Split(rendered, "\n")
	innerLines := max(0, len(lines)-2)
	innerHeight := max(0, height-2)
	return max(0, innerLines-innerHeight)
}

func automaticMessageStart(group messageGroup, rendered renderedMessageGroup, height int, liveAnchorID string) int {
	maximum := messagePaneMaxStart(rendered.panel, height)
	target := model.Message{}
	lastHumanMessage := -1
	for index, message := range group.messages {
		if message.SenderMailboxID == model.HumanMailboxID {
			lastHumanMessage = index
		}
	}
	for index, message := range group.messages {
		if index > lastHumanMessage && canArchive(message) {
			target = message
			break
		}
	}
	for _, message := range group.messages {
		if message.ID == liveAnchorID && canArchive(message) {
			target = message
			break
		}
	}
	if target.ID == "" {
		return maximum
	}
	for _, span := range rendered.spans {
		if span.messageID == target.ID {
			return min(maximum, max(0, span.start))
		}
	}
	return maximum
}

func (m app) resolvedMessageStart(group messageGroup, rendered renderedMessageGroup, height int) int {
	maximum := messagePaneMaxStart(rendered.panel, height)
	if !m.messageScrollManual || m.messageViewportKey != group.key {
		return automaticMessageStart(group, rendered, height, m.messageLiveAnchorID)
	}
	if m.messageAnchorID != "" {
		for _, span := range rendered.spans {
			if span.messageID == m.messageAnchorID {
				return min(maximum, max(0, span.start+m.messageAnchorOffset))
			}
		}
	}
	return min(maximum, max(0, m.messageScroll))
}

func captureMessageAnchor(rendered renderedMessageGroup, start int) (string, int) {
	var chosen *messageLineSpan
	for index := range rendered.spans {
		span := &rendered.spans[index]
		if span.start > start {
			break
		}
		chosen = span
	}
	if chosen == nil {
		return "", start
	}
	return chosen.messageID, max(0, start-chosen.start)
}

func (m *app) reconcileMessageViewport(preserveManual bool) {
	group, found := m.detailGroup()
	if !found {
		if !preserveManual {
			m.resetMessageViewport()
		}
		return
	}
	if !preserveManual || m.messageViewportKey != group.key {
		m.messageScrollManual = false
		m.messageLiveAnchorID = ""
		m.messageAnchorID = ""
		m.messageAnchorOffset = 0
	}
	layout := responsivePaneLayout(m.width, m.height, m.answering)
	rendered := m.renderGroupPanelLayout(group, layout.messageWidth)
	m.messageScroll = m.resolvedMessageStart(group, rendered, layout.messageHeight)
	m.messageViewportKey = group.key
	if m.messageScrollManual {
		m.messageAnchorID, m.messageAnchorOffset = captureMessageAnchor(rendered, m.messageScroll)
	}
}

func (m *app) resetMessageViewport() {
	m.messageScroll = 0
	m.messageScrollManual = false
	m.messageViewportKey = ""
	m.messageLiveAnchorID = ""
	m.messageAnchorID = ""
	m.messageAnchorOffset = 0
}

func (m *app) scrollMessagePane(delta int) {
	group, found := m.detailGroup()
	if !found {
		return
	}
	layout := responsivePaneLayout(m.width, m.height, m.answering)
	rendered := m.renderGroupPanelLayout(group, layout.messageWidth)
	current := m.resolvedMessageStart(group, rendered, layout.messageHeight)
	next := min(messagePaneMaxStart(rendered.panel, layout.messageHeight), max(0, current+delta))
	m.messageScroll = next
	m.messageViewportKey = group.key
	if next == current {
		return
	}
	m.messageScrollManual = true
	m.messageAnchorID, m.messageAnchorOffset = captureMessageAnchor(rendered, next)
}

func canReply(message model.Message) bool {
	return message.RecipientMailboxID == model.HumanMailboxID && message.SenderMailboxID != model.HumanMailboxID && message.ArchivedAt == nil
}

func canArchive(message model.Message) bool {
	return message.RecipientMailboxID == model.HumanMailboxID && message.ArchivedAt == nil
}

func replyTarget(group messageGroup) model.Message {
	oldest := archiveTarget(group)
	if oldest.ID == "" {
		return model.Message{}
	}
	unit := actionUnitKey(oldest)
	for i := len(group.messages) - 1; i >= 0; i-- {
		message := group.messages[i]
		if canReply(message) && actionUnitKey(message) == unit && message.Correlation.RequestID != "" {
			return message
		}
	}
	for i := len(group.messages) - 1; i >= 0; i-- {
		message := group.messages[i]
		if canReply(message) && actionUnitKey(message) == unit {
			return message
		}
	}
	return model.Message{}
}

func archiveTarget(group messageGroup) model.Message {
	for _, message := range group.messages {
		if canArchive(message) {
			return message
		}
	}
	return model.Message{}
}

func actionUnitKey(message model.Message) string {
	turn := message.Correlation.OperationID
	if turn == "" {
		turn = message.HarnessOperationID
	}
	if turn != "" {
		return "operation:" + turn
	}
	if message.ThreadID != "" {
		return "thread:" + message.ThreadID
	}
	return "message:" + message.ID
}

func canReplyGroup(group messageGroup) bool { return replyTarget(group).ID != "" }

func canArchiveGroup(group messageGroup) bool {
	for _, message := range group.messages {
		if canArchive(message) {
			return true
		}
	}
	return false
}

func displayMessageAddress(address model.MessageAddress, fallbackLabel string, context model.RepositoryContext) string {
	label := address.Label
	if label == "" {
		label = fallbackLabel
	}
	switch address.Kind {
	case model.MailboxHuman:
		return "human"
	case model.MailboxAgent:
		if address.Name != "" {
			return address.Name
		}
		if label == "" {
			label = address.Harness
		}
		if label == "" {
			return "agent"
		}
		directory := filepath.Base(filepath.Clean(context.Directory))
		if context.Directory == "" || directory == "." || directory == string(filepath.Separator) {
			return label
		}
		return label + " · " + directory
	case model.MailboxProject:
		if label == "" {
			return "project"
		}
		return label
	case model.MailboxRemote:
		if label == "" {
			return "remote"
		}
		return label
	default:
		return label
	}
}

func presentationKind(message model.Message) string {
	if message.Presentation.Valid() {
		return string(message.Presentation)
	}
	return ""
}

func presentationLabel(kind string) string {
	switch kind {
	case "final-answer":
		return "[final answer]"
	case "update":
		return "[update]"
	case "status":
		return "[status]"
	case "notice":
		return "[notice]"
	default:
		return ""
	}
}

func presentationPanelLabel(kind, sender string) string {
	switch kind {
	case "update":
		return "[an update from " + sender + "]"
	case "final-answer":
		return "[a final answer from " + sender + "]"
	case "status":
		return "[a status from " + sender + "]"
	case "notice":
		return "[a notice from " + sender + "]"
	default:
		return ""
	}
}

func (m app) presentationDetails(raw string, expanded bool) (string, bool) {
	if expanded || raw == "" {
		if !expanded {
			return raw, false
		}
		lines := strings.Split(raw, "\n")
		providerID := ""
		for _, line := range lines {
			if value, found := strings.CutPrefix(strings.TrimSpace(line), "Harness provider:"); found {
				providerID = strings.TrimSpace(value)
				break
			}
		}
		if providerID == "" {
			providerID = "codex"
		}
		for index, line := range lines {
			value, found := strings.CutPrefix(strings.TrimSpace(line), "Harness session:")
			if !found {
				continue
			}
			threadID := strings.TrimSpace(value)
			if session, ok := m.threadSessions[providerID+"\x00"+threadID]; ok && session.ThreadName != "" {
				lines[index] = "Harness session: " + threadLabel(session.ThreadName, threadID)
			}
		}
		return strings.Join(lines, "\n"), false
	}
	prefixes := []string{
		"Kind:", "Phase:", "Harness provider:", "Harness session:", "Harness operation:", "Harness item:",
		"Harness request:", "HQ message:", "HQ mailbox:",
	}
	visible := make([]string, 0, strings.Count(raw, "\n")+1)
	hidden := false
	for _, line := range strings.Split(raw, "\n") {
		technical := false
		trimmed := strings.TrimSpace(line)
		for _, prefix := range prefixes {
			if strings.HasPrefix(trimmed, prefix) {
				technical = true
				break
			}
		}
		if technical {
			hidden = true
			continue
		}
		visible = append(visible, line)
	}
	return strings.TrimSpace(strings.Join(visible, "\n")), hidden
}

func technicalIdentifiers(message model.Message) string {
	lines := []string{"hq.message.identifiers"}
	add := func(label, value string) {
		if value != "" {
			lines = append(lines, label+": "+value)
		}
	}
	add("message ID", message.ID)
	add("canonical event ID", message.EventID)
	add("thread event ID", message.ThreadID)
	add("sender installation ID", message.SenderInstallationID)
	add("recipient installation ID", message.RecipientInstallationID)
	if message.ReplyTo != nil {
		add("reply-to ID", *message.ReplyTo)
	}
	correlation := message.Correlation
	if correlation.Empty() && message.HarnessSessionID != "" {
		correlation = model.MessageCorrelation{Provider: message.HarnessProvider, SessionID: message.HarnessSessionID, OperationID: message.HarnessOperationID}
	}
	if !correlation.Empty() {
		lines = append(lines, "", "hq.message.correlation")
		add("provider", correlation.Provider)
		add("session ID", correlation.SessionID)
		add("operation ID", correlation.OperationID)
		add("item ID", correlation.ItemID)
		add("request ID", correlation.RequestID)
	}
	for _, section := range message.TechnicalSections {
		lines = append(lines, "", section.Namespace)
		for _, field := range section.Fields {
			label := field.Label
			if label == "" {
				label = field.Key
			}
			add(label, field.Value)
		}
	}
	return strings.Join(lines, "\n")
}

func hasTechnicalIdentifiers(message model.Message) bool {
	return technicalIdentifiers(message) != ""
}

func (m app) technicalContext(message model.Message) string {
	lines := make([]string, 0, 5)
	if recipient := displayMessageAddress(message.RecipientAddress, message.RecipientLabel, message.Context); recipient != "" {
		lines = append(lines, "to "+recipient)
	}
	if message.Context.Directory != "" {
		lines = append(lines, "directory "+message.Context.Directory)
	}
	if message.SourceDeviceLabel != "" || message.SenderInstallationID != "" {
		source := message.SourceDeviceLabel
		if source == "" {
			source = "installation"
		}
		lines = append(lines, "source "+source)
	}
	if m.branch != "" {
		lines = append(lines, "git "+m.branch)
	}
	if m.remotes != "" {
		remote := m.remotes
		if m.pull != "" {
			remote += " · " + m.pull
		}
		lines = append(lines, remote)
	}
	return strings.Join(lines, "\n")
}

func (m app) renderAgentManager() string {
	width := max(40, m.width)
	var body strings.Builder
	body.WriteString(titleStyle.Render("Harness agents"))
	body.WriteString("\n\n")
	switch m.agentManager.stage {
	case chooseRuntimeAgent:
		body.WriteString("Search: " + m.agentManager.query + "\n\n")
		agents := m.runtimeAgents()
		if len(agents) == 0 {
			body.WriteString(dim.Render("No non-retired named agents."))
		}
		for index, agent := range agents {
			state := "offline"
			if agent.Active {
				state = "active"
			}
			thread := threadLabel(agent.CurrentThreadName, agent.CurrentSessionID)
			if thread == "" {
				thread = "no session"
			}
			line := fmt.Sprintf("%s · %s · %s · %s", agent.Name, state, thread, agent.Context.Directory)
			if index == m.agentManager.cursor {
				line = selected.Render(line)
			}
			body.WriteString(line + "\n")
		}
	case chooseRuntimeSession:
		runtime := m.agentManager.runtime
		body.WriteString(fmt.Sprintf("%s · %s · %s\n\n", m.agentManager.agent.Name, runtime.Phase, runtime.Directory))
		yolo := "off"
		if m.agentManager.yolo {
			yolo = "ON"
		}
		body.WriteString("Codex YOLO: " + yolo + " (y to toggle)\n\n")
		newLine := "+ new harness session"
		if m.agentManager.cursor == 0 {
			newLine = selected.Render(newLine)
		}
		body.WriteString(newLine + "\n")
		for index, session := range m.agentManager.sessions {
			marker := " "
			if session.Current {
				marker = "*"
			}
			line := fmt.Sprintf("%s %s · %s · %s", marker, threadLabel(session.ThreadName, session.SessionID), session.Context.Directory, session.LastSelectedAt.Local().Format("2006-01-02 15:04"))
			if index+1 == m.agentManager.cursor {
				line = selected.Render(line)
			}
			body.WriteString(line + "\n")
		}
	case enterRuntimeHarness:
		body.WriteString("New session for " + m.agentManager.agent.Name + "\n\n")
		body.WriteString("Harness provider: " + m.agentManager.harness + "\n")
	case enterRuntimeDirectory:
		body.WriteString("New " + m.agentManager.harness + " session for " + m.agentManager.agent.Name + "\n\n")
		body.WriteString("Directory: " + m.agentManager.directory + "\n")
		if m.agentManager.yolo {
			body.WriteString("Codex YOLO: ON\n")
		}
	case confirmRuntimeSwitch:
		body.WriteString("Agent " + m.agentManager.agent.Name + " is running.\n")
		if providerYolo(m.agentManager.pending.ProviderOptions) {
			body.WriteString("Requested YOLO mode: ON\n")
		}
		body.WriteString("Stop it and switch to the requested session? y/n\n")
	case enterThreadName:
		body.WriteString("Rename " + threadLabel(m.agentManager.renameSession.ThreadName, m.agentManager.renameSession.SessionID) + "\n\n")
		body.WriteString("Session name: " + m.agentManager.threadName + "\n")
		body.WriteString(dim.Render("Leave empty to clear the name."))
	}
	if m.agentManager.busy {
		body.WriteString("\n" + dim.Render("Working…"))
	}
	if m.agentManager.status != "" {
		body.WriteString("\n" + m.agentManager.status)
	}
	help := "type to search · j/k move · enter select · esc back"
	if m.agentManager.stage == chooseRuntimeSession {
		help = "j/k move · enter resume/new · y Codex YOLO · r rename · n new · s stop · esc back"
	} else if m.agentManager.stage == enterRuntimeHarness {
		help = "type harness provider · enter continue · esc back"
	} else if m.agentManager.stage == enterRuntimeDirectory {
		help = "type directory · enter launch · esc back"
	} else if m.agentManager.stage == confirmRuntimeSwitch {
		help = "y confirm · n cancel · esc back"
	} else if m.agentManager.stage == enterThreadName {
		help = "type name · enter save · esc back"
	}
	return lipgloss.JoinVertical(lipgloss.Left, panel.Width(max(1, width-panel.GetHorizontalFrameSize())).Render(body.String()), dim.Render(help))
}

func (m app) View() tea.View {
	if m.managingAgents {
		view := tea.NewView(m.renderAgentManager())
		view.AltScreen = true
		return view
	}
	layout := responsivePaneLayout(m.width, m.height, m.answering)
	inboxPane := m.renderInboxPane(layout.width, layout.inboxHeight)
	detailGroup, hasDetail := m.detailGroup()
	messageFocused := m.paneFocused(focusMessage)
	replyFocused := m.paneFocused(focusReply)
	messagePane := renderMessagePanel("No message selected.", layout.messageWidth, "[message]", "", messageFocused)
	if hasDetail {
		rendered := m.renderGroupPanelLayout(detailGroup, layout.messageWidth)
		messagePane = fitRenderedPaneFromTop(rendered.panel, layout.messageWidth, layout.messageHeight, m.resolvedMessageStart(detailGroup, rendered, layout.messageHeight), messageFocused)
	} else {
		messagePane = fitRenderedPaneFromTop(messagePane, layout.messageWidth, layout.messageHeight, 0, messageFocused)
	}
	replyHint := "Press Tab or n to choose a recipient for a new message."
	if hasDetail && detailGroup.draft != nil {
		replyHint = "Press Tab or Enter to continue this draft, or n for a new message."
	} else if hasDetail && canReplyGroup(detailGroup) {
		replyHint = "Press Tab or Enter to reply to the selected turn, or n for a new message."
	}
	replyPane := renderMessagePanel(replyHint, layout.replyWidth, "[reply]", "", replyFocused)
	if m.pickingRecipient {
		replyPane = m.renderRecipientPicker(layout.replyWidth, layout.replyHeight)
	} else if m.projectSetup != nil {
		replyPane = m.renderProjectSetup(layout.replyWidth, layout.replyHeight)
	} else if m.answering {
		replyPane = m.renderReplyPane(layout.replyWidth)
	}
	replyPane = fitRenderedPane(replyPane, layout.replyWidth, layout.replyHeight, 0, replyFocused)
	bottom := lipgloss.JoinVertical(lipgloss.Left, messagePane, replyPane)
	help := "tab/shift+tab focus/compose · j/k navigate · pgup/pgdown or ^u/^d page · e toggle activity at viewport · enter reply · n new · g agents · d archive · u undo · i details · q quit"
	if m.pickingRecipient {
		help = "type to filter recipients · j/k or arrows move · enter select · esc cancel"
	} else if m.projectSetup != nil {
		help = "j/k move · enter select · f force blocked handoff · esc back"
	} else if m.answering {
		help = "tab/shift+tab focus · pgup/pgdown or ^u/^d page · e toggle activity at viewport · enter submit · shift+enter/ctrl+j newline · esc cancel"
	}
	if m.undoNotice != "" {
		help = m.undoNotice + " · " + help
	}
	help = dim.Render(truncateDisplay(help, layout.width))
	content := lipgloss.JoinVertical(lipgloss.Left, inboxPane, bottom, help)
	view := tea.NewView(content)
	view.AltScreen = true
	return view
}

func listWindow(total, cursor, limit int) (int, int) {
	if total == 0 {
		return 0, 0
	}
	limit = min(total, max(1, limit))
	start := max(0, cursor-limit/2)
	if start+limit > total {
		start = total - limit
	}
	return start, start + limit
}

func responsivePaneLayout(width, height int, _ bool) paneLayout {
	if width <= 0 {
		width = 80
	}
	if height <= 0 {
		height = 24
	}
	result := paneLayout{width: width, height: height}
	usableHeight := max(2, height-1) // Reserve one terminal row for responsive help.
	result.inboxHeight = max(2, (height+3)/4-1)
	result.inboxHeight = min(result.inboxHeight, usableHeight-1)
	remaining := max(1, usableHeight-result.inboxHeight)
	result.messageWidth, result.replyWidth = width, width
	result.replyHeight = max(6, (3*height+19)/20) + 1
	result.replyHeight = min(result.replyHeight, max(1, remaining-1))
	result.messageHeight = max(1, remaining-result.replyHeight)
	return result
}

func (m app) renderInboxPane(width, height int) string {
	innerWidth := max(1, width-panel.GetHorizontalFrameSize())
	innerHeight := max(0, height-2)
	var lines []string
	navigation := "Inbox"
	if m.showSent {
		navigation += "  Sent:on"
	} else {
		navigation += "  Sent:off"
	}
	if m.showArchived {
		navigation += "  Archived:on"
	} else {
		navigation += "  Archived:off"
	}
	lines = append(lines, titleStyle.Render(truncateDisplay(navigation, innerWidth)))
	appendDiagnostic := func(value string, style lipgloss.Style) {
		if value != "" && len(lines) < innerHeight {
			lines = append(lines, style.Render(truncateDisplay(singleLine(value), innerWidth)))
		}
	}
	connectionStyle := dim
	if m.connection.Blocking {
		connectionStyle = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("196"))
	}
	appendDiagnostic(m.connection.Diagnostic, connectionStyle)
	if m.connection.Blocking {
		appendDiagnostic("Restart or upgrade the indicated HQ component, then reopen the TUI. · q quit", dim)
	}
	if m.err != nil {
		appendDiagnostic(m.err.Error(), lipgloss.NewStyle().Foreground(lipgloss.Color("196")))
	}
	if m.syncErr != nil {
		appendDiagnostic("relay sync pending: "+m.syncErr.Error(), dim)
	}
	if m.showStatus {
		for _, line := range strings.Split(formatNetworkStatus(m.network), "\n") {
			appendDiagnostic(line, dim)
		}
	}
	groups := m.visibleGroups()
	listRows := max(0, innerHeight-len(lines))
	listOffset := len(lines)
	listStart, listVisible := 0, 0
	if len(groups) == 0 && listRows > 0 {
		lines = append(lines, dim.Render(truncateDisplay("No messages in this view. Press r to refresh.", innerWidth)))
	} else if listRows > 0 {
		start, end := listWindow(len(groups), m.cursor, listRows)
		listStart, listVisible = start, end-start
		for i := start; i < end; i++ {
			group := groups[i]
			message := group.latest()
			if group.draft != nil && len(group.messages) == 0 {
				message = model.Message{SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: group.draft.composeTo, RecipientLabel: group.draft.composeName, Body: group.draft.body}
			}
			direction := short(displayMessageAddress(message.SenderAddress, message.SenderLabel, message.Context), 18)
			if message.SenderMailboxID == model.HumanMailboxID {
				direction = "sent → " + short(displayMessageAddress(message.RecipientAddress, message.RecipientLabel, message.Context), 16)
			}
			kind := groupPresentationKind(group)
			badge := presentationLabel(kind)
			if group.draft != nil {
				direction = "draft → " + short(draftRecipient(*group.draft), 15)
				badge = "DRAFT"
			}
			if badge != "" {
				badge += " "
			}
			state := deliveryLabel(message)
			if message.ArchivedAt != nil {
				state += " [archived]"
			}
			line := truncateDisplay(fmt.Sprintf("%s %s%s%s", direction, badge, singleLine(message.Body), state), innerWidth-2)
			if i == m.cursor {
				lines = append(lines, selected.Render("› "+line))
			} else {
				switch kind {
				case "final-answer":
					lines = append(lines, "  "+finalStyle.Render(line))
				case "update", "status", "notice":
					lines = append(lines, "  "+dim.Render(line))
				default:
					lines = append(lines, "  "+line)
				}
			}
		}
	}
	focused := m.paneFocused(focusInbox)
	rendered := renderMessagePanel(strings.Join(lines, "\n"), width, "[HQ · Inbox]", "", focused)
	rendered = fitRenderedPane(rendered, width, height, 0, focused)
	return renderPaneScrollbar(rendered, listStart, listVisible, len(groups), listOffset, listRows, focused)
}

func groupPresentationKind(group messageGroup) string {
	kind, _ := groupPresentation(group)
	return kind
}

func groupPresentation(group messageGroup) (string, model.Message) {
	for i := len(group.messages) - 1; i >= 0; i-- {
		if presentationKind(group.messages[i]) == "final-answer" {
			return "final-answer", group.messages[i]
		}
	}
	for i := len(group.messages) - 1; i >= 0; i-- {
		if kind := presentationKind(group.messages[i]); kind != "" {
			return kind, group.messages[i]
		}
	}
	return "", group.latest()
}

func (m app) renderGroupPanel(group messageGroup, width int) string {
	return m.renderGroupPanelLayout(group, width).panel
}

func (m app) renderGroupPanelLayout(group messageGroup, width int) renderedMessageGroup {
	if cached, ok := m.cachedRenderedMessageGroup(group, width); ok {
		return cached
	}
	if len(group.messages) == 0 && group.draft != nil {
		body := titleStyle.Render("New message to "+draftRecipient(*group.draft)) + "\n\n"
		if group.draft.body == "" {
			body += dim.Render("(empty draft)")
		} else {
			body += group.draft.body
		}
		return m.cacheRenderedMessageGroup(group, width, renderedMessageGroup{panel: renderMessagePanel(body, width, "[draft]", "press enter to continue", m.paneFocused(focusMessage))})
	}
	latest := group.latest()
	kind, presentation := groupPresentation(group)
	sender := displayMessageAddress(presentation.SenderAddress, presentation.SenderLabel, presentation.Context)
	topLabel := presentationPanelLabel(kind, sender)
	var body strings.Builder
	lineCount := 0
	if topLabel == "" {
		appendRenderedText(&body, &lineCount, dim.Render("From: "+sender))
		appendRenderedText(&body, &lineCount, "\n\n")
	}
	metadataHidden := false
	var spans []messageLineSpan
	var activitySpans []activityLineSpan
	markdownWidth := max(1, width-panel.GetHorizontalFrameSize())
	for index, entry := range conversationTimeline(group) {
		if index > 0 {
			appendRenderedText(&body, &lineCount, "\n\n")
		}
		if entry.activity != nil {
			activity := *entry.activity
			activityStart := max(0, lineCount-1)
			appendRenderedText(&body, &lineCount, m.renderHarnessActivity(activity, markdownWidth, m.expandedActivities[activityExpansionKey(activity)]))
			activitySpans = append(activitySpans, activityLineSpan{key: activityExpansionKey(activity), start: activityStart, end: lineCount})
			continue
		}
		message := *entry.message
		spanStart := max(0, lineCount-1)
		header := "── " + message.CreatedAt.Local().Format("Jan 2, 3:04:05 PM")
		if direction := messageDirection(message); direction != "" {
			header += " · " + direction
		}
		appendRenderedText(&body, &lineCount, dim.Render(header+" ──"))
		appendRenderedText(&body, &lineCount, "\n")
		appendRenderedText(&body, &lineCount, m.markdown.Render(message, markdownWidth))
		visibleDetails, hidden := m.presentationDetails(message.Details, m.showTechnical)
		metadataHidden = metadataHidden || hidden
		if visibleDetails != "" {
			appendRenderedText(&body, &lineCount, "\n\n")
			appendRenderedText(&body, &lineCount, visibleDetails)
		}
		if m.showTechnical {
			if identifiers := technicalIdentifiers(message); identifiers != "" {
				appendRenderedText(&body, &lineCount, "\n\n")
				appendRenderedText(&body, &lineCount, dim.Render(identifiers))
			}
		}
		spans = append(spans, messageLineSpan{messageID: message.ID, actionUnit: actionUnitKey(message), start: spanStart, end: lineCount})
	}
	if group.draft != nil {
		appendRenderedText(&body, &lineCount, "\n\n")
		draftStart := max(0, lineCount-1)
		appendRenderedText(&body, &lineCount, titleStyle.Render("Draft reply"))
		appendRenderedText(&body, &lineCount, "\n")
		if group.draft.body == "" {
			appendRenderedText(&body, &lineCount, dim.Render("(empty draft)"))
		} else {
			appendRenderedText(&body, &lineCount, group.draft.body)
		}
		spans = append(spans, messageLineSpan{messageID: "draft:" + group.key, actionUnit: "draft:" + group.key, start: draftStart, end: lineCount})
	}
	if m.showTechnical {
		if context := m.technicalContext(latest); context != "" {
			appendRenderedText(&body, &lineCount, "\n\n")
			appendRenderedText(&body, &lineCount, dim.Render(context))
		}
	}
	bottomLabel := ""
	if !m.showTechnical && (metadataHidden || groupHasTechnicalIdentifiers(group) || m.technicalContext(latest) != "") {
		bottomLabel = "technical details hidden · press i to show"
	}
	return m.cacheRenderedMessageGroup(group, width, renderedMessageGroup{panel: renderMessagePanel(body.String(), width, topLabel, bottomLabel, m.paneFocused(focusMessage)), spans: spans, activitySpans: activitySpans})
}

func appendRenderedText(body *strings.Builder, lineCount *int, value string) {
	if value == "" {
		return
	}
	body.WriteString(value)
	if *lineCount == 0 {
		*lineCount = 1
	}
	*lineCount += strings.Count(value, "\n")
}

func (m app) cachedRenderedMessageGroup(group messageGroup, width int) (renderedMessageGroup, bool) {
	if m.markdown == nil || m.markdown.groupCache == nil {
		return renderedMessageGroup{}, false
	}
	cache := m.markdown.groupCache
	hasDraft := group.draft != nil
	var draft messageDraft
	if hasDraft {
		draft = *group.draft
	}
	if cache.groupKey != group.key || !reflect.DeepEqual(cache.messages, group.messages) || !slices.Equal(cache.activities, group.activities) || cache.activityState != m.activityExpansionState(group.activities) || cache.hasDraft != hasDraft || !reflect.DeepEqual(cache.draft, draft) ||
		cache.width != width || cache.showTechnical != m.showTechnical || cache.focused != m.paneFocused(focusMessage) ||
		cache.contextID != m.contextID || cache.branch != m.branch || cache.remotes != m.remotes || cache.pull != m.pull {
		return renderedMessageGroup{}, false
	}
	return cache.rendered, true
}

func (m app) cacheRenderedMessageGroup(group messageGroup, width int, rendered renderedMessageGroup) renderedMessageGroup {
	if m.markdown == nil {
		return rendered
	}
	hasDraft := group.draft != nil
	var draft messageDraft
	if hasDraft {
		draft = *group.draft
	}
	m.markdown.groupCache = &renderedMessageGroupCache{
		groupKey: group.key, messages: append([]model.Message(nil), group.messages...), activities: append([]domain.HarnessActivity(nil), group.activities...), activityState: m.activityExpansionState(group.activities), draft: draft, hasDraft: hasDraft,
		width: width, showTechnical: m.showTechnical, focused: m.paneFocused(focusMessage),
		contextID: m.contextID, branch: m.branch, remotes: m.remotes, pull: m.pull, rendered: rendered,
	}
	return rendered
}

func messageDirection(message model.Message) string {
	if message.SenderMailboxID == model.HumanMailboxID {
		recipient := message.RecipientLabel
		if recipient == "" {
			recipient = message.RecipientMailboxID
		}
		return "You → " + displayMessageAddress(message.RecipientAddress, recipient, message.Context)
	}
	sender := message.SenderLabel
	if sender == "" {
		sender = message.SenderMailboxID
	}
	return displayMessageAddress(message.SenderAddress, sender, message.Context) + " → You"
}

func draftRecipient(draft messageDraft) string {
	if draft.composeName != "" {
		return draft.composeName
	}
	return displayMessageAddress(draft.answerQ.SenderAddress, draft.answerQ.SenderLabel, draft.answerQ.Context)
}

func groupHasTechnicalIdentifiers(group messageGroup) bool {
	for _, message := range group.messages {
		if hasTechnicalIdentifiers(message) {
			return true
		}
	}
	return false
}

func (m app) renderReplyPane(width int) string {
	var body strings.Builder
	editor := m.editor
	if width > 0 {
		editor.SetWidth(max(1, width-panel.GetHorizontalFrameSize()))
	}
	body.WriteString(editor.View())
	body.WriteByte('\n')
	body.WriteString(dim.Render("enter submit · shift+enter/ctrl+j newline · esc cancel"))
	prefix := "Replying to"
	if m.composeTo != "" {
		prefix = "New message to"
	}
	return renderComposePanel(body.String(), width, prefix, m.composeRecipientName(), m.paneFocused(focusReply))
}

func (m app) composeRecipientName() string {
	if m.composeTo != "" && m.composeName != "" {
		return m.composeName
	}
	for _, agent := range m.agents {
		if agent.MailboxID == m.answerQ.SenderMailboxID {
			return agent.Name
		}
	}
	return displayMessageAddress(m.answerQ.SenderAddress, m.answerQ.SenderLabel, m.answerQ.Context)
}

func (m app) renderRecipientPicker(width, height int) string {
	innerWidth := max(1, width-panel.GetHorizontalFrameSize())
	var body strings.Builder
	query := m.pickerQuery
	if query == "" {
		query = "type to filter"
	}
	body.WriteString(dim.Render("Search: " + query))
	choices := m.filteredRecipients()
	rows := max(1, height-2)
	start, end := listWindow(len(choices), m.pickerCursor, rows)
	for index := start; index < end; index++ {
		choice := choices[index]
		presence := choice.status
		if presence == "" {
			presence = "offline"
			if choice.active {
				presence = "active"
			}
		}
		metadata := presence
		if !choice.active && choice.lastActiveAt != nil {
			metadata += " · last active " + choice.lastActiveAt.Local().Format("Jan 2 3:04 PM")
		}
		line := truncateDisplay(fmt.Sprintf("%-16s %s", choice.name, metadata), innerWidth-2)
		body.WriteByte('\n')
		if index == m.pickerCursor {
			body.WriteString(selected.Render("› " + line))
		} else {
			body.WriteString("  " + line)
		}
	}
	if len(choices) == 0 {
		body.WriteByte('\n')
		body.WriteString(dim.Render("No matching recipients."))
	}
	return renderMessagePanel(body.String(), width, "[project · choose project work or direct recipient]", "", m.paneFocused(focusReply))
}

func (m app) renderProjectSetup(width, height int) string {
	setup := m.projectSetup
	if setup == nil {
		return ""
	}
	innerWidth := max(1, width-panel.GetHorizontalFrameSize())
	var body strings.Builder
	if setup.project.ID == "" {
		body.WriteString(titleStyle.Render("Create project"))
	} else {
		body.WriteString(titleStyle.Render(setup.project.Name))
		body.WriteString(" · " + string(setup.project.Lifecycle))
	}
	if setup.project.Lifecycle == domain.ProjectClosed {
		body.WriteString("\nClaims checked atomically on reopen:")
		for _, resource := range setup.project.Resources {
			body.WriteString("\n  " + truncateDisplay(resource.DisplayLocator+" · "+string(resource.Health), innerWidth-4))
		}
	}
	switch setup.stage {
	case enterProjectName:
		body.WriteString("\n\nNew project name:\n" + renderProjectInput(setup.name))
	case chooseProjectHome:
		body.WriteString("\n\nChoose the immutable project home")
		devices := m.activeProjectHomes()
		for index, device := range devices {
			label := device.Label
			if label == "" {
				label = device.InstallationID
			}
			if device.InstallationID == m.account.LocalInstallationID {
				label += " · this device"
			}
			prefix := "  "
			if index == setup.cursor {
				prefix = "› "
			}
			body.WriteString("\n" + prefix + truncateDisplay(label+" · "+short(device.InstallationID, 12), innerWidth-2))
		}
	case enterProjectBrief:
		body.WriteString("\n\nOptional project brief:\n" + renderProjectInput(setup.brief))
		body.WriteString("\n" + dim.Render("Press enter to leave it empty."))
	case enterProjectPaths:
		body.WriteString("\n\nDesired path resources (comma separated):\n" + renderProjectInput(setup.pathsText))
		body.WriteString("\n" + dim.Render("Enter: ordinary paths · Tab: add a Git worktree · ~ and $VARS expand locally."))
	case chooseProjectPrimary:
		body.WriteString("\n\nChoose the primary path")
		for index, path := range setup.paths {
			prefix := "  "
			if index == setup.cursor {
				prefix = "› "
			}
			body.WriteString("\n" + prefix + truncateDisplay(path, innerWidth-2))
		}
	case enterWorktreeRepository:
		body.WriteString("\n\nExisting Git repository:\n" + renderProjectInput(setup.worktreeRepository))
	case enterWorktreeBase:
		body.WriteString("\n\nMerge base / starting ref:\n" + renderProjectInput(setup.worktreeBase))
	case enterWorktreeDestination:
		body.WriteString("\n\nNew worktree destination:\n" + renderProjectInput(setup.worktreeDestination))
		body.WriteString("\n" + dim.Render("HQ reserves this path before invoking Git."))
	case enterWorktreeBranch:
		body.WriteString("\n\nNew branch name:\n" + renderProjectInput(setup.worktreeBranch))
	case chooseWorktreePrimary:
		body.WriteString("\n\nChoose the primary path")
		options := append([]string{setup.worktreeDestination}, setup.paths...)
		for index, path := range options {
			prefix := "  "
			if index == setup.cursor {
				prefix = "› "
			}
			body.WriteString("\n" + prefix + truncateDisplay(path, innerWidth-2))
		}
	case chooseProjectAgent:
		body.WriteString("\n\nChoose an idle local agent")
		body.WriteString("\nFilter or home-agent name: " + renderProjectInput(setup.query))
		agents := setup.filteredAgents()
		rows := max(1, height-8)
		start, end := listWindow(len(agents), setup.cursor, rows)
		for index := start; index < end; index++ {
			label := agents[index].Name
			if label == setup.project.SuggestedAgentName {
				label += " · recent"
			}
			prefix := "  "
			if index == setup.cursor {
				prefix = "› "
			}
			body.WriteString("\n" + prefix + truncateDisplay(label, innerWidth-2))
		}
		if len(agents) == 0 {
			message := "No idle local agents."
			if setup.project.ReadOnlyReplica {
				message = "Type an agent name on the project home; the home validates availability."
			}
			body.WriteString("\n" + dim.Render(message))
		}
	case enterProjectHarness:
		body.WriteString("\n\nAgent: " + setup.agent.Name + "\nHarness provider:\n" + renderProjectInput(setup.harness))
	case chooseProjectThread:
		body.WriteString("\n\nAgent: " + setup.agent.Name + " · " + setup.harness + "\nChoose an execution session")
		options := append([]domain.ProjectThread{{}}, setup.compatibleThreads()...)
		for index, thread := range options {
			label := "new " + setup.harness + " session"
			if index > 0 {
				label = "resume " + shortThreadID(thread.ExternalID) + " · " + thread.LaunchDir
			}
			prefix := "  "
			if index == setup.cursor {
				prefix = "› "
			}
			body.WriteString("\n" + prefix + truncateDisplay(label, innerWidth-2))
		}
		if setup.project.Assignment != nil {
			warning := "blocked handoff · press f to authorize force takeover"
			if setup.force {
				warning = "FORCE TAKEOVER authorized · old runtime may still access resources"
			}
			body.WriteString("\n\n" + warning)
		}
	case enterProjectDirectory:
		body.WriteString("\n\nNew session launch directory:\n" + renderProjectInput(setup.directory))
		body.WriteString("\n" + dim.Render("Paths outside claims are allowed with a warning · ~ and $VARS expand locally."))
	}
	if setup.busy {
		body.WriteString("\n" + dim.Render("Loading…"))
	}
	if setup.status != "" {
		body.WriteString("\n" + setup.status)
	}
	return renderMessagePanel(body.String(), width, "[project activation]", "", true)
}

func renderProjectInput(value string) string {
	return value + inputCursor.Render("▏")
}

func short(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n-1] + "…"
}

func singleLine(s string) string { return strings.Join(strings.Fields(s), " ") }

func renderMessagePanel(content string, terminalWidth int, topLabel, bottomLabel string, focused bool) string {
	paneStyle, edgeStyle := dimPanel, dimPanelEdge
	if focused {
		paneStyle, edgeStyle = panel, panelEdge
	}
	rendered := paneStyle.Render(content)
	if terminalWidth > paneStyle.GetHorizontalFrameSize() {
		rendered = paneStyle.Width(terminalWidth).Render(content)
	}
	width := lipgloss.Width(rendered)
	minimumWidth := max(lipgloss.Width(topLabel), lipgloss.Width(bottomLabel)) + 6
	if (topLabel != "" || bottomLabel != "") && width < minimumWidth && (terminalWidth <= 0 || minimumWidth <= terminalWidth) {
		width = minimumWidth
		rendered = paneStyle.Width(width).Render(content)
	}
	if topLabel == "" && bottomLabel == "" {
		return rendered
	}
	lines := strings.Split(rendered, "\n")
	bottomWidth := lipgloss.Width(lines[len(lines)-1])
	if bottomWidth < 6 {
		return rendered
	}
	if topLabel != "" {
		label := truncateDisplay(topLabel, bottomWidth-6)
		right := bottomWidth - lipgloss.Width(label) - 5
		lines[0] = edgeStyle.Render("╭─" + " " + label + " " + strings.Repeat("─", right) + "╮")
	}
	if bottomLabel != "" {
		label := truncateDisplay(bottomLabel, bottomWidth-6)
		left := bottomWidth - lipgloss.Width(label) - 5
		lines[len(lines)-1] = edgeStyle.Render("╰"+strings.Repeat("─", left)) + edgeStyle.Render(" "+label+" ") + edgeStyle.Render("─╯")
	}
	return strings.Join(lines, "\n")
}

func renderComposePanel(content string, terminalWidth int, prefix, name string, focused bool) string {
	plainPrefix, suffix := "["+prefix+" ", "]"
	plainLabel := plainPrefix + name + suffix
	rendered := renderMessagePanel(content, terminalWidth, plainLabel, "", focused)
	lines := strings.Split(rendered, "\n")
	if len(lines) == 0 {
		return rendered
	}
	borderWidth := lipgloss.Width(lines[0])
	availableNameWidth := borderWidth - 6 - lipgloss.Width(plainPrefix) - lipgloss.Width(suffix)
	if availableNameWidth < 1 {
		return rendered
	}
	displayName := truncateDisplay(name, availableNameWidth)
	labelWidth := lipgloss.Width(plainPrefix) + lipgloss.Width(displayName) + lipgloss.Width(suffix)
	right := max(0, borderWidth-labelWidth-5)
	edgeStyle := dimPanelEdge
	if focused {
		edgeStyle = panelEdge
	}
	lines[0] = edgeStyle.Render("╭─ "+plainPrefix) + titleStyle.Render(displayName) + edgeStyle.Render(suffix+" "+strings.Repeat("─", right)+"╮")
	return strings.Join(lines, "\n")
}

func fitRenderedPane(rendered string, width, height, scrollBack int, focused bool) string {
	if height <= 0 {
		return ""
	}
	lines := strings.Split(rendered, "\n")
	if len(lines) == 0 {
		return ""
	}
	if height == 1 {
		return lines[0]
	}
	top, bottom := lines[0], lines[len(lines)-1]
	inner := lines[1 : len(lines)-1]
	innerHeight := max(0, height-2)
	start := 0
	if len(inner) > innerHeight {
		maxStart := len(inner) - innerHeight
		start = maxStart - min(maxStart, max(0, scrollBack))
		inner = inner[start : start+innerHeight]
	}
	paneStyle := dimPanel
	if focused {
		paneStyle = panel
	}
	blankRendered := paneStyle.Width(max(width, paneStyle.GetHorizontalFrameSize()+1)).Render("")
	blankLines := strings.Split(blankRendered, "\n")
	blank := ""
	if len(blankLines) >= 3 {
		blank = blankLines[1]
	}
	for len(inner) < innerHeight {
		inner = append(inner, blank)
	}
	result := make([]string, 0, height)
	result = append(result, top)
	result = append(result, inner...)
	result = append(result, bottom)
	return strings.Join(result, "\n")
}

func fitRenderedPaneFromTop(rendered string, width, height, start int, focused bool) string {
	if height <= 0 {
		return ""
	}
	lines := strings.Split(rendered, "\n")
	if len(lines) == 0 {
		return ""
	}
	if height == 1 {
		return lines[0]
	}
	top, bottom := lines[0], lines[len(lines)-1]
	inner := lines[1 : len(lines)-1]
	innerHeight := max(0, height-2)
	totalInner := len(inner)
	maximum := max(0, totalInner-innerHeight)
	start = min(maximum, max(0, start))
	if len(inner) > innerHeight {
		end := min(len(inner), start+innerHeight)
		inner = inner[start:end]
	}
	paneStyle := dimPanel
	if focused {
		paneStyle = panel
	}
	blankRendered := paneStyle.Width(max(width, paneStyle.GetHorizontalFrameSize()+1)).Render("")
	blankLines := strings.Split(blankRendered, "\n")
	blank := ""
	if len(blankLines) >= 3 {
		blank = blankLines[1]
	}
	for len(inner) < innerHeight {
		inner = append(inner, blank)
	}
	result := make([]string, 0, height)
	result = append(result, top)
	result = append(result, inner...)
	result = append(result, bottom)
	return renderPaneScrollbar(strings.Join(result, "\n"), start, min(totalInner, innerHeight), totalInner, 0, innerHeight, focused)
}

func renderPaneScrollbar(rendered string, start, visible, total, trackOffset, trackHeight int, focused bool) string {
	if total <= visible || visible <= 0 || trackHeight <= 0 {
		return rendered
	}
	lines := strings.Split(rendered, "\n")
	available := max(0, len(lines)-2-trackOffset)
	trackHeight = min(trackHeight, available)
	if trackHeight <= 0 {
		return rendered
	}
	thumbHeight := max(1, visible*trackHeight/total)
	thumbHeight = min(trackHeight, thumbHeight)
	maximumStart := max(1, total-visible)
	thumbRange := trackHeight - thumbHeight
	thumbStart := min(thumbRange, max(0, start)*thumbRange/maximumStart)
	trackStyle, thumbStyle := dimPanelEdge, panelEdge
	if !focused {
		thumbStyle = dimPanelEdge.Copy().Foreground(lipgloss.Color("103"))
	}
	for offset := range trackHeight {
		lineIndex := 1 + trackOffset + offset
		border := strings.LastIndex(lines[lineIndex], "│")
		if border < 0 {
			continue
		}
		glyph := trackStyle.Render("░")
		if offset >= thumbStart && offset < thumbStart+thumbHeight {
			glyph = thumbStyle.Render("█")
		}
		lines[lineIndex] = lines[lineIndex][:border] + glyph + lines[lineIndex][border+len("│"):]
	}
	return strings.Join(lines, "\n")
}

func truncateDisplay(value string, width int) string {
	if lipgloss.Width(value) <= width {
		return value
	}
	if width <= 0 {
		return ""
	}
	if width == 1 {
		return "…"
	}
	var b strings.Builder
	for _, r := range value {
		if lipgloss.Width(b.String()+string(r))+1 > width {
			break
		}
		b.WriteRune(r)
	}
	return b.String() + "…"
}

func deliveryLabel(message model.Message) string {
	if message.SenderMailboxID != model.HumanMailboxID {
		return ""
	}
	switch message.DeliveryState {
	case "queued":
		return " [sending]"
	case "relay-accepted":
		return " [sent]"
	case "peer-received":
		return " [peer received]"
	case "rejected":
		return " [rejected]"
	default:
		return ""
	}
}

func formatNetworkStatus(status domain.NetworkStatus) string {
	var value strings.Builder
	value.WriteString(titleStyle.Render("Relay status"))
	fmt.Fprintf(&value, "\nqueued %d · relay accepted %d · rejected %d · unresolved %d · unsupported %d · staged %d · quarantined %d", status.Queued, status.RelayAccepted, status.Rejected, status.Unresolved, status.Unsupported, status.Staged, status.Quarantined)
	fmt.Fprintf(&value, "\naccount members %d · pending fanout %d · invalid account %d · revoked device %d", status.AccountMembers, status.PendingAccountFanout, status.InvalidAccountTraffic, status.RevokedDeviceTraffic)
	for _, relay := range status.Relays {
		fmt.Fprintf(&value, "\n%s · connected %t · auth %t", relay.URL, relay.Connected, relay.Authenticated)
		if relay.LastEvent != nil {
			fmt.Fprintf(&value, " · last receive %s", relay.LastEvent.Format(time.RFC3339))
		}
		if relay.LastError != "" {
			value.WriteString(" · " + relay.LastError)
		}
	}
	if len(status.Relays) == 0 {
		value.WriteString("\nno relay sync state")
	}
	return value.String()
}
