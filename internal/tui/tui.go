package tui

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"charm.land/bubbles/v2/key"
	"charm.land/bubbles/v2/textarea"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/repoctx"
)

const repairInterval = 5 * time.Minute

var (
	titleStyle   = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("212"))
	finalStyle   = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("42"))
	selected     = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("230")).Background(lipgloss.Color("62"))
	dim          = lipgloss.NewStyle().Foreground(lipgloss.Color("241"))
	panelEdge    = lipgloss.NewStyle().Foreground(lipgloss.Color("63"))
	dimPanelEdge = lipgloss.NewStyle().Foreground(lipgloss.Color("59"))
	panel        = lipgloss.NewStyle().Border(lipgloss.RoundedBorder()).BorderForeground(lipgloss.Color("63")).Padding(0, 1)
	dimPanel     = lipgloss.NewStyle().Border(lipgloss.RoundedBorder()).BorderForeground(lipgloss.Color("59")).Padding(0, 1)
)

type app struct {
	ctx               context.Context
	store             domain.Store
	repo              repoctx.Provider
	messages          []model.Message
	groups            []messageGroup
	inbox             []model.Message
	sent              []model.Message
	archived          []model.Message
	showSent          bool
	showArchived      bool
	showStatus        bool
	showTechnical     bool
	cursor            int
	width             int
	height            int
	answering         bool
	answerID          string
	answerGroupKey    string
	answerQ           model.Message
	drafts            map[string]messageDraft
	activeDraftKey    string
	composeTo         string
	composeName       string
	composeContext    model.RepositoryContext
	composeNamed      bool
	agents            []domain.NamedAgent
	threadSessions    map[string]domain.AgentSession
	pickingRecipient  bool
	pickerQuery       string
	pickerCursor      int
	editor            textarea.Model
	err               error
	contextID         string
	branch            string
	remotes           string
	pull              string
	sync              func(context.Context) error
	syncErr           error
	network           domain.NetworkStatus
	changes           <-chan domain.Invalidation
	states            <-chan domain.ConnectionUpdate
	connection        domain.ConnectionUpdate
	undoStack         []undoAction
	nextUndoID        uint64
	undoing           bool
	undoNotice        string
	messageScroll     int
	paneFocus         paneFocus
	markdown          *messageMarkdownRenderer
	launchDirectory   string
	launchEnvironment []string
	managingAgents    bool
	agentManager      agentManager
}

type agentManagerStage int

const (
	chooseRuntimeAgent agentManagerStage = iota
	chooseRuntimeSession
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
	runtime       domain.CodexRuntime
	directory     string
	threadName    string
	renameSession domain.AgentSession
	pending       domain.CodexLaunchRequest
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
	key      string
	messages []model.Message
	draft    *messageDraft
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
	updatedAt      time.Time
}

type recipientChoice struct {
	name         string
	mailboxID    string
	active       bool
	lastActiveAt *time.Time
	context      model.RepositoryContext
	named        bool
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

func (g messageGroup) latest() model.Message {
	if len(g.messages) == 0 {
		return model.Message{}
	}
	return g.messages[len(g.messages)-1]
}

type loadedMsg struct {
	inbox    []model.Message
	sent     []model.Message
	archived []model.Message
	network  domain.NetworkStatus
	agents   []domain.NamedAgent
	sessions map[string]domain.AgentSession
	err      error
}

type answeredMsg struct {
	err  error
	sent bool
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
	runtime  domain.CodexRuntime
	err      error
}

type codexRuntimeMsg struct {
	runtime domain.CodexRuntime
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
	var subscription domain.ChangeSubscription
	var err error
	if updates.Subscribe != nil {
		subscription, err = updates.Subscribe(ctx, domain.TopicMessages, domain.TopicMailboxes, domain.TopicNetwork, domain.TopicPeers, domain.TopicHuman, domain.TopicRelays, domain.TopicAgents)
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
	m := app{ctx: ctx, store: s, repo: repoctx.GitHub{}, editor: editor, sync: sync, states: updates.States, connection: updates.Initial, markdown: newMessageMarkdownRenderer(nil), launchDirectory: launchDirectory, launchEnvironment: os.Environ()}
	if subscription != nil {
		m.changes = subscription.Changes()
	}
	_, err = tea.NewProgram(m, tea.WithInput(in), tea.WithOutput(out), tea.WithContext(ctx)).Run()
	return err
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
	open := false
	inbox, err := m.store.List(m.ctx, model.Filter{RecipientMailboxID: model.HumanMailboxID, Archived: &open, Limit: 1000, NewestFirst: true})
	if err != nil {
		return loadedMsg{err: err}
	}
	sent, err := m.store.List(m.ctx, model.Filter{SenderMailboxID: model.HumanMailboxID, Limit: 1000, NewestFirst: true})
	if err != nil {
		return loadedMsg{err: err}
	}
	closed := true
	archived, err := m.store.List(m.ctx, model.Filter{RecipientMailboxID: model.HumanMailboxID, Archived: &closed, Limit: 1000, NewestFirst: true})
	if err != nil {
		return loadedMsg{err: err}
	}
	agents, err := m.store.ListNamedAgents(m.ctx)
	if err != nil {
		return loadedMsg{err: err}
	}
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
	network, err := m.store.NetworkStatus(m.ctx)
	return loadedMsg{inbox: inbox, sent: sent, archived: archived, agents: agents, sessions: sessions, network: network, err: err}
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
		}
		message.RecipientMailboxID = m.composeTo
		message.Context = m.composeContext
		message.RecipientLabel = m.composeName
		err = m.store.Create(m.ctx, message)
	} else {
		replyTo := m.answerID
		message.ReplyTo = &replyTo
		message.Details = turnCorrelationDetails(m.answerQ)
		err = m.store.Reply(m.ctx, m.answerID, message)
		if err == nil {
			err = m.archiveAnsweredGroup()
			return answeredMsg{err: err, sent: true}
		}
	}
	return answeredMsg{err: err, sent: err == nil}
}

func (m app) archiveAnsweredGroup() error {
	group, ok := m.groupByKey(m.answerGroupKey)
	if !ok {
		return nil
	}
	for _, message := range group.messages {
		if message.ID == m.answerID || !canArchive(message) {
			continue
		}
		if err := m.store.Archive(m.ctx, message.ID); err != nil && !errors.Is(err, domain.ErrAlreadyHandled) {
			return fmt.Errorf("reply sent, but archive turn message %s: %w", message.ID, err)
		}
	}
	return nil
}

func turnCorrelationDetails(message model.Message) string {
	thread := detailValue(message.Details, "Codex thread:")
	turn := detailValue(message.Details, "Codex turn:")
	var lines []string
	if thread != "" {
		lines = append(lines, "Codex thread: "+thread)
	}
	if turn != "" {
		lines = append(lines, "Codex turn: "+turn)
	}
	return strings.Join(lines, "\n")
}

func (m app) archiveGroup(group messageGroup) tea.Cmd {
	return func() tea.Msg {
		var archived []string
		for _, message := range group.messages {
			if canArchive(message) {
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
		oldLayout := responsivePaneLayout(m.width, m.height, m.answering)
		m.width, m.height = msg.Width, msg.Height
		layout := responsivePaneLayout(msg.Width, msg.Height, m.answering)
		if oldLayout.messageWidth != layout.messageWidth && m.markdown != nil {
			m.markdown.Reset()
		}
		m.editor.SetWidth(max(1, layout.replyWidth-panel.GetHorizontalFrameSize()))
		m.editor.SetHeight(max(1, layout.replyHeight-4))
	case loadedMsg:
		selectedKey := m.selectedGroupKey()
		m.inbox, m.sent, m.archived, m.agents, m.threadSessions, m.network, m.err = msg.inbox, msg.sent, msg.archived, msg.agents, msg.sessions, msg.network, msg.err
		if choices := m.filteredRecipients(); m.pickerCursor >= len(choices) {
			m.pickerCursor = max(0, len(choices)-1)
		}
		m.setMessages()
		if m.markdown != nil {
			m.markdown.Reset()
		}
		visibleGroups := m.visibleGroups()
		if index := groupIndex(visibleGroups, selectedKey); index >= 0 {
			m.cursor = index
		} else if m.cursor >= len(visibleGroups) {
			m.cursor = max(0, len(visibleGroups)-1)
		}
		return m.withContextCommand()
	case repairMsg:
		return m, tea.Batch(m.load, m.syncNow(), scheduleRepair())
	case invalidatedMsg:
		return m, tea.Batch(m.load, m.waitInvalidation())
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
	case codexRuntimeMsg:
		m.agentManager.busy = false
		m.agentManager.runtime = msg.runtime
		if msg.err != nil {
			m.agentManager.status = msg.err.Error()
		} else {
			m.agentManager.status = fmt.Sprintf("%s · %s · %s", msg.runtime.Phase, threadLabel(m.managedThreadName(msg.runtime.ThreadID), msg.runtime.ThreadID), msg.runtime.Directory)
		}
		return m, m.load
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
		m.agentManager.status = "thread renamed"
		return m, m.load
	case syncMsg:
		m.syncErr = msg.err
		if msg.err == nil {
			return m, m.load
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
			m.paneFocus = focusInbox
			m.editor.Reset()
			return m, tea.Batch(m.load, m.syncNow())
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
			m.paneFocus = focusReply
			m.editor.Blur()
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
			return m, tea.Batch(m.load, m.syncNow())
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
			return m, tea.Batch(m.load, m.syncNow())
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
		if m.managingAgents {
			return m.updateAgentManager(msg)
		}
		switch msg.String() {
		case "tab":
			wasReply := m.answering && m.paneFocus == focusReply
			m.cyclePaneFocus(1)
			if wasReply {
				m.stowActiveDraft()
				return m.withContextCommand()
			}
			if m.answering && m.paneFocus == focusReply {
				m.editor.Focus()
				return m, textarea.Blink
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
			if m.answering && m.paneFocus == focusReply {
				m.editor.Focus()
				return m, textarea.Blink
			}
			m.editor.Blur()
			return m, nil
		}
		switch msg.String() {
		case "pgup":
			layout := responsivePaneLayout(m.width, m.height, m.answering)
			switch m.paneFocus {
			case focusInbox:
				m.cursor = max(0, m.cursor-max(1, layout.inboxHeight-3))
				m.messageScroll = 0
				return m.withContextCommand()
			case focusMessage:
				m.messageScroll += max(1, layout.messageHeight-3)
				return m, nil
			case focusReply:
				if m.answering {
					var cmd tea.Cmd
					m.editor, cmd = m.editor.Update(msg)
					return m, cmd
				}
			}
			return m, nil
		case "pgdown":
			layout := responsivePaneLayout(m.width, m.height, m.answering)
			switch m.paneFocus {
			case focusInbox:
				m.cursor = min(max(0, len(m.messages)-1), m.cursor+max(1, layout.inboxHeight-3))
				m.messageScroll = 0
				return m.withContextCommand()
			case focusMessage:
				m.messageScroll = max(0, m.messageScroll-max(1, layout.messageHeight-3))
				return m, nil
			case focusReply:
				if m.answering {
					var cmd tea.Cmd
					m.editor, cmd = m.editor.Update(msg)
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
				m.paneFocus = focusInbox
				m.editor.Blur()
				m.editor.Reset()
				return m, nil
			case "j", "down":
				if m.paneFocus == focusMessage {
					m.messageScroll = max(0, m.messageScroll-1)
					return m, nil
				}
				if m.paneFocus == focusInbox && m.cursor+1 < len(m.messages) {
					m.cursor++
					return m, nil
				}
			case "k", "up":
				if m.paneFocus == focusMessage {
					m.messageScroll++
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
				m.messageScroll = max(0, m.messageScroll-1)
				return m, nil
			}
			if m.paneFocus != focusInbox {
				return m, nil
			}
			if m.cursor+1 < len(m.messages) {
				m.cursor++
				m.messageScroll = 0
				return m.withContextCommand()
			}
		case "k", "up":
			if m.paneFocus == focusMessage {
				m.messageScroll++
				return m, nil
			}
			if m.paneFocus != focusInbox {
				return m, nil
			}
			if m.cursor > 0 {
				m.cursor--
				m.messageScroll = 0
				return m.withContextCommand()
			}
		case "s":
			m.showSent = !m.showSent
			m.cursor = 0
			m.messageScroll = 0
			m.setMessages()
			return m.withContextCommand()
		case "x":
			m.showArchived = !m.showArchived
			m.cursor = 0
			m.messageScroll = 0
			m.setMessages()
			return m.withContextCommand()
		case "v":
			m.showStatus = !m.showStatus
			return m, nil
		case "i":
			m.showTechnical = !m.showTechnical
			return m, nil
		case "enter", "a":
			if group, ok := m.groupAtCursor(); ok && group.draft != nil {
				m.resumeDraft(*group.draft)
				return m, textarea.Blink
			} else if ok && canReplyGroup(group) {
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
				return m, textarea.Blink
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
			m.agentManager = agentManager{stage: chooseRuntimeAgent}
			m.editor.Blur()
			return m, nil
		case "r":
			return m, m.load
		}
	}
	return m, nil
}

func (m app) recipients() []recipientChoice {
	choices := []recipientChoice{{name: "self", mailboxID: model.HumanMailboxID, active: true}}
	for _, agent := range m.agents {
		if agent.Retired {
			continue
		}
		choices = append(choices, recipientChoice{
			name: agent.Name, mailboxID: agent.MailboxID, active: agent.Active,
			lastActiveAt: agent.LastActiveAt, context: agent.Context, named: true,
		})
	}
	sort.SliceStable(choices, func(left, right int) bool {
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
		} else if m.agentManager.stage == enterRuntimeDirectory || m.agentManager.stage == confirmRuntimeSwitch || m.agentManager.stage == enterThreadName {
			m.agentManager.stage = chooseRuntimeSession
			m.agentManager.pending = domain.CodexLaunchRequest{}
			m.agentManager.renameSession = domain.AgentSession{}
			m.agentManager.status = ""
		} else {
			m.agentManager = agentManager{stage: chooseRuntimeAgent}
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
			m.beginNewRuntimeDirectory()
		case "s":
			m.agentManager.busy = true
			return m, m.stopManagedAgent()
		case "r":
			if m.agentManager.cursor > 0 && m.agentManager.cursor <= len(m.agentManager.sessions) {
				m.agentManager.renameSession = m.agentManager.sessions[m.agentManager.cursor-1]
				m.agentManager.threadName = m.agentManager.renameSession.ThreadName
				m.agentManager.stage = enterThreadName
				m.agentManager.status = ""
			}
		case "enter":
			if m.agentManager.cursor == 0 {
				m.beginNewRuntimeDirectory()
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
			request := m.runtimeRequest(domain.CodexSessionResume, session.SessionID, directory)
			return m.confirmOrLaunch(request)
		}
	case enterRuntimeDirectory:
		switch key.String() {
		case "enter":
			directory, err := m.validRuntimeDirectory(m.agentManager.directory)
			if err != nil {
				m.agentManager.status = err.Error()
				return m, nil
			}
			request := m.runtimeRequest(domain.CodexSessionNew, "", directory)
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
			m.agentManager.status = "switching Codex runtime…"
			return m, m.launchManagedAgent(request)
		case "n":
			for index := range m.agentManager.pending.Environment {
				m.agentManager.pending.Environment[index] = ""
			}
			m.agentManager.stage = chooseRuntimeSession
			m.agentManager.pending = domain.CodexLaunchRequest{}
		}
	case enterThreadName:
		switch key.String() {
		case "enter":
			m.agentManager.busy = true
			m.agentManager.status = "renaming thread…"
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

func (m *app) beginNewRuntimeDirectory() {
	m.agentManager.stage = enterRuntimeDirectory
	m.agentManager.directory = m.defaultRuntimeDirectory()
	m.agentManager.status = ""
}

func (m app) defaultRuntimeDirectory() string {
	if m.agentManager.agent.Context.Directory != "" {
		return m.agentManager.agent.Context.Directory
	}
	return m.launchDirectory
}

func (m app) validRuntimeDirectory(raw string) (string, error) {
	directory := strings.TrimSpace(raw)
	if !filepath.IsAbs(directory) {
		directory = filepath.Join(m.launchDirectory, directory)
	}
	directory = filepath.Clean(directory)
	info, err := os.Stat(directory)
	if err != nil {
		return "", errors.New("directory does not exist")
	}
	if !info.IsDir() {
		return "", errors.New("path is not a directory")
	}
	return directory, nil
}

func (m app) runtimeRequest(action domain.CodexSessionAction, sessionID, directory string) domain.CodexLaunchRequest {
	repository := model.RepositoryContext{Directory: directory}
	if snapshotter, ok := m.repo.(interface {
		Snapshot(context.Context, string) model.RepositoryContext
	}); ok {
		repository = snapshotter.Snapshot(m.ctx, directory)
	}
	return domain.CodexLaunchRequest{
		RequestID: uuid.NewString(), AgentName: m.agentManager.agent.Name, Action: action, SessionID: sessionID,
		Directory: directory, Repository: repository, Environment: append([]string(nil), m.launchEnvironment...),
	}
}

func (m app) confirmOrLaunch(request domain.CodexLaunchRequest) (tea.Model, tea.Cmd) {
	if m.agentManager.runtime.Phase == domain.CodexRuntimeRunning && (request.Action == domain.CodexSessionNew || request.SessionID != m.agentManager.runtime.ThreadID) {
		m.agentManager.stage = confirmRuntimeSwitch
		m.agentManager.pending = request
		m.agentManager.status = "replace the running Codex worker? y/n"
		return m, nil
	}
	m.agentManager.busy = true
	m.agentManager.status = "starting Codex runtime…"
	return m, m.launchManagedAgent(request)
}

func (m app) loadAgentSessions(agent domain.NamedAgent) tea.Cmd {
	return func() tea.Msg {
		sessions, err := m.store.ListNamedAgentSessions(m.ctx, agent.Name)
		controller, ok := m.store.(domain.CodexRuntimeController)
		if err == nil && !ok {
			err = errors.New("Codex runtime control is unavailable")
		}
		var runtime domain.CodexRuntime
		if err == nil {
			runtime, err = controller.CodexAgentRuntime(m.ctx, agent.Name)
		}
		return agentSessionsMsg{agent: agent, sessions: sessions, runtime: runtime, err: err}
	}
}

func (m app) launchManagedAgent(request domain.CodexLaunchRequest) tea.Cmd {
	return func() tea.Msg {
		controller, ok := m.store.(domain.CodexRuntimeController)
		if !ok {
			return codexRuntimeMsg{err: errors.New("Codex runtime control is unavailable")}
		}
		runtime, err := controller.LaunchCodexAgent(m.ctx, request)
		for index := range request.Environment {
			request.Environment[index] = ""
		}
		return codexRuntimeMsg{runtime: runtime, err: err}
	}
}

func (m app) stopManagedAgent() tea.Cmd {
	return func() tea.Msg {
		controller, ok := m.store.(domain.CodexRuntimeController)
		if !ok {
			return codexRuntimeMsg{err: errors.New("Codex runtime control is unavailable")}
		}
		runtime, err := controller.StopCodexAgent(m.ctx, m.agentManager.agent.Name)
		return codexRuntimeMsg{runtime: runtime, err: err}
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

func (m app) managedThreadName(id string) string {
	for _, session := range m.agentManager.sessions {
		if session.Harness == "codex" && session.SessionID == id {
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
		if strings.Contains(strings.ToLower(choice.name), query) {
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
		m.pickingRecipient = false
		m.answering = true
		m.answerID = ""
		m.answerGroupKey = ""
		m.answerQ = model.Message{}
		m.composeTo, m.composeName = choice.mailboxID, choice.name
		m.composeContext, m.composeNamed = choice.context, choice.named
		m.activeDraftKey = "draft:" + uuid.NewString()
		m.paneFocus = focusReply
		m.resizeEditor()
		m.editor.Focus()
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

func (m *app) resizeEditor() {
	layout := responsivePaneLayout(m.width, m.height, true)
	m.editor.SetWidth(max(1, layout.replyWidth-panel.GetHorizontalFrameSize()))
	m.editor.SetHeight(max(1, layout.replyHeight-4))
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
	if key == "" {
		key = "draft:" + uuid.NewString()
	}
	if m.drafts == nil {
		m.drafts = make(map[string]messageDraft)
	}
	m.drafts[key] = messageDraft{
		key: key, body: m.editor.Value(), answerID: m.answerID, answerGroupKey: m.answerGroupKey,
		answerQ: m.answerQ, composeTo: m.composeTo, composeName: m.composeName,
		composeContext: m.composeContext, composeNamed: m.composeNamed, updatedAt: time.Now(),
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
	m.editor.Blur()
	m.editor.Reset()
	if index := groupIndex(m.visibleGroups(), key); index >= 0 {
		m.cursor = index
	}
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
	m.paneFocus = focusReply
	m.resizeEditor()
	m.editor.SetValue(draft.body)
	m.editor.Focus()
}

func (m app) paneFocused(pane paneFocus) bool { return m.paneFocus == pane }

func (m *app) setMessages() {
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
	turn := detailValue(message.Details, "Codex turn:")
	if turn == "" || turn == "(none)" {
		return "message:" + message.ID
	}
	thread := detailValue(message.Details, "Codex thread:")
	return "turn:" + message.SenderMailboxID + ":" + thread + ":" + turn
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
			group.messages = []model.Message{draft.answerQ}
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
	if m.answering {
		if group, found := m.groupByKey(m.selectedGroupKey()); found {
			q = group.latest()
		} else {
			q = m.answerQ
		}
	} else if group, found := m.groupAtCursor(); found && len(group.messages) > 0 {
		q = group.latest()
	}
	if q.ID == "" || m.repo == nil {
		m.contextID, m.branch, m.remotes, m.pull = "", "", "", ""
		return m, nil
	}
	if q.ID == m.contextID {
		return m, nil
	}
	m.contextID = q.ID
	m.branch = "git loading…"
	m.remotes = ""
	m.pull = ""
	return m, m.loadBranch(q)
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

func canReply(message model.Message) bool {
	return message.RecipientMailboxID == model.HumanMailboxID && message.SenderMailboxID != model.HumanMailboxID && message.ArchivedAt == nil
}

func canArchive(message model.Message) bool {
	return message.RecipientMailboxID == model.HumanMailboxID && message.ArchivedAt == nil
}

func replyTarget(group messageGroup) model.Message {
	for i := len(group.messages) - 1; i >= 0; i-- {
		message := group.messages[i]
		if canReply(message) && detailValue(message.Details, "Codex request:") != "" {
			return message
		}
	}
	for i := len(group.messages) - 1; i >= 0; i-- {
		if canReply(group.messages[i]) {
			return group.messages[i]
		}
	}
	return model.Message{}
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

func displayMailboxLabel(label string, context model.RepositoryContext) string {
	if label == "human" {
		return label
	}
	harness, _, found := strings.Cut(label, ":")
	if !found || harness == "" {
		return label
	}
	directory := filepath.Base(filepath.Clean(context.Directory))
	if context.Directory == "" || directory == "." || directory == string(filepath.Separator) {
		return harness
	}
	return harness + " · " + directory
}

func presentationKind(message model.Message) string {
	for _, line := range strings.Split(message.Details, "\n") {
		value, found := strings.CutPrefix(strings.TrimSpace(line), "Kind:")
		if found {
			switch kind := strings.TrimSpace(value); kind {
			case "final-answer", "update", "status", "notice":
				return kind
			}
		}
	}
	for _, line := range strings.Split(message.Details, "\n") {
		if strings.TrimSpace(line) == "Phase: final_answer" {
			return "final-answer"
		}
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
		for index, line := range lines {
			value, found := strings.CutPrefix(strings.TrimSpace(line), "Codex thread:")
			if !found {
				continue
			}
			threadID := strings.TrimSpace(value)
			if session, ok := m.threadSessions["codex\x00"+threadID]; ok && session.ThreadName != "" {
				lines[index] = "Codex thread: " + threadLabel(session.ThreadName, threadID)
			}
		}
		return strings.Join(lines, "\n"), false
	}
	prefixes := []string{
		"Kind:", "Phase:", "Codex thread:", "Codex turn:", "Codex item:",
		"Codex request:", "HQ message:", "HQ mailbox:",
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
	lines := make([]string, 0, 6)
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
	return strings.Join(lines, "\n")
}

func hasTechnicalIdentifiers(message model.Message) bool {
	return technicalIdentifiers(message) != ""
}

func (m app) technicalContext(message model.Message) string {
	lines := make([]string, 0, 5)
	if recipient := displayMailboxLabel(message.RecipientLabel, message.Context); recipient != "" {
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
	body.WriteString(titleStyle.Render("Codex agents"))
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
		newLine := "+ new Codex thread"
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
	case enterRuntimeDirectory:
		body.WriteString("New Codex thread for " + m.agentManager.agent.Name + "\n\n")
		body.WriteString("Directory: " + m.agentManager.directory + "\n")
	case confirmRuntimeSwitch:
		body.WriteString("Agent " + m.agentManager.agent.Name + " is running.\n")
		body.WriteString("Stop it and switch to the requested session? y/n\n")
	case enterThreadName:
		body.WriteString("Rename " + threadLabel(m.agentManager.renameSession.ThreadName, m.agentManager.renameSession.SessionID) + "\n\n")
		body.WriteString("Thread name: " + m.agentManager.threadName + "\n")
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
		help = "j/k move · enter resume/new · r rename · n new · s stop · esc back"
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
	var detailGroup messageGroup
	hasDetail := false
	if m.answering {
		detailGroup, hasDetail = m.groupByKey(m.selectedGroupKey())
		if !hasDetail && m.answerQ.ID != "" {
			detailGroup, hasDetail = messageGroup{key: messageGroupKey(m.answerQ), messages: []model.Message{m.answerQ}}, true
		}
	} else {
		detailGroup, hasDetail = m.groupAtCursor()
	}
	messageFocused := m.paneFocused(focusMessage)
	replyFocused := m.paneFocused(focusReply)
	messagePane := renderMessagePanel("No message selected.", layout.messageWidth, "[message]", "", messageFocused)
	if hasDetail {
		messagePane = m.renderGroupPanel(detailGroup, layout.messageWidth)
	}
	messagePane = fitRenderedPane(messagePane, layout.messageWidth, layout.messageHeight, m.messageScroll, messageFocused)
	replyHint := "Press Enter to reply to the selected turn."
	if hasDetail && detailGroup.draft != nil {
		replyHint = "Draft saved. Press Enter to continue editing."
	}
	replyPane := renderMessagePanel(replyHint, layout.replyWidth, "[reply]", "", replyFocused)
	if m.pickingRecipient {
		replyPane = m.renderRecipientPicker(layout.replyWidth, layout.replyHeight)
	} else if m.answering {
		replyPane = m.renderReplyPane(layout.replyWidth)
	}
	replyPane = fitRenderedPane(replyPane, layout.replyWidth, layout.replyHeight, 0, replyFocused)
	bottom := lipgloss.JoinVertical(lipgloss.Left, messagePane, replyPane)
	help := "tab/shift+tab focus · j/k navigate · pgup/pgdown message · enter reply · n new · g agents · d archive · u undo · i details · q quit"
	if m.pickingRecipient {
		help = "type to filter recipients · j/k or arrows move · enter select · esc cancel"
	} else if m.answering {
		help = "tab/shift+tab focus · pgup/pgdown message · enter submit · shift+enter/ctrl+j newline · esc cancel"
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
	result.inboxHeight = max(2, (height+3)/4)
	result.inboxHeight = min(result.inboxHeight, usableHeight-1)
	remaining := max(1, usableHeight-result.inboxHeight)
	result.messageWidth, result.replyWidth = width, width
	result.replyHeight = max(6, (3*height+19)/20)
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
	if len(groups) == 0 && listRows > 0 {
		lines = append(lines, dim.Render(truncateDisplay("No messages in this view. Press r to refresh.", innerWidth)))
	} else if listRows > 0 {
		start, end := listWindow(len(groups), m.cursor, listRows)
		for i := start; i < end; i++ {
			group := groups[i]
			message := group.latest()
			if group.draft != nil && len(group.messages) == 0 {
				message = model.Message{SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: group.draft.composeTo, RecipientLabel: group.draft.composeName, Body: group.draft.body}
			}
			direction := short(displayMailboxLabel(message.SenderLabel, message.Context), 18)
			if message.SenderMailboxID == model.HumanMailboxID {
				direction = "sent → " + short(displayMailboxLabel(message.RecipientLabel, message.Context), 16)
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
			line := truncateDisplay(fmt.Sprintf("%-18s %s%s%s", direction, badge, singleLine(message.Body), state), innerWidth-2)
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
	return fitRenderedPane(rendered, width, height, 0, focused)
}

func groupPresentationKind(group messageGroup) string {
	for i := len(group.messages) - 1; i >= 0; i-- {
		if presentationKind(group.messages[i]) == "final-answer" {
			return "final-answer"
		}
	}
	for i := len(group.messages) - 1; i >= 0; i-- {
		if kind := presentationKind(group.messages[i]); kind != "" {
			return kind
		}
	}
	return ""
}

func (m app) renderGroupPanel(group messageGroup, width int) string {
	if len(group.messages) == 0 && group.draft != nil {
		body := titleStyle.Render("New message to "+draftRecipient(*group.draft)) + "\n\n"
		if group.draft.body == "" {
			body += dim.Render("(empty draft)")
		} else {
			body += group.draft.body
		}
		return renderMessagePanel(body, width, "[draft]", "press enter to continue", m.paneFocused(focusMessage))
	}
	latest := group.latest()
	kind := groupPresentationKind(group)
	sender := displayMailboxLabel(latest.SenderLabel, latest.Context)
	topLabel := presentationPanelLabel(kind, sender)
	var body strings.Builder
	if topLabel == "" {
		body.WriteString(dim.Render("From: " + sender))
		body.WriteString("\n\n")
	}
	metadataHidden := false
	markdownWidth := max(1, width-panel.GetHorizontalFrameSize())
	for i, message := range group.messages {
		if i > 0 {
			body.WriteString("\n\n")
		}
		body.WriteString(dim.Render("── " + message.CreatedAt.Local().Format("Jan 2, 3:04:05 PM") + " ──"))
		body.WriteByte('\n')
		body.WriteString(m.markdown.Render(message, markdownWidth))
		visibleDetails, hidden := m.presentationDetails(message.Details, m.showTechnical)
		metadataHidden = metadataHidden || hidden
		if visibleDetails != "" {
			body.WriteString("\n\n")
			body.WriteString(visibleDetails)
		}
		if m.showTechnical {
			if identifiers := technicalIdentifiers(message); identifiers != "" {
				body.WriteString("\n\n")
				body.WriteString(dim.Render(identifiers))
			}
		}
	}
	if group.draft != nil {
		body.WriteString("\n\n")
		body.WriteString(titleStyle.Render("Draft reply"))
		body.WriteByte('\n')
		if group.draft.body == "" {
			body.WriteString(dim.Render("(empty draft)"))
		} else {
			body.WriteString(group.draft.body)
		}
	}
	if m.showTechnical {
		if context := m.technicalContext(latest); context != "" {
			body.WriteString("\n\n")
			body.WriteString(dim.Render(context))
		}
	}
	bottomLabel := ""
	if !m.showTechnical && (metadataHidden || groupHasTechnicalIdentifiers(group) || m.technicalContext(latest) != "") {
		bottomLabel = "technical details hidden · press i to show"
	}
	return renderMessagePanel(body.String(), width, topLabel, bottomLabel, m.paneFocused(focusMessage))
}

func draftRecipient(draft messageDraft) string {
	if draft.composeName != "" {
		return draft.composeName
	}
	return displayMailboxLabel(draft.answerQ.SenderLabel, draft.answerQ.Context)
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
	if m.composeTo != "" {
		body.WriteString(titleStyle.Render("New message to " + m.composeName))
	} else {
		body.WriteString(titleStyle.Render("Reply to this turn"))
	}
	body.WriteByte('\n')
	editor := m.editor
	if width > 0 {
		editor.SetWidth(max(1, width-panel.GetHorizontalFrameSize()))
	}
	body.WriteString(editor.View())
	body.WriteByte('\n')
	body.WriteString(dim.Render("enter submit · shift+enter/ctrl+j newline · esc cancel"))
	return renderMessagePanel(body.String(), width, "[reply]", "", m.paneFocused(focusReply))
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
	rows := max(1, height-3)
	start, end := listWindow(len(choices), m.pickerCursor, rows)
	for index := start; index < end; index++ {
		choice := choices[index]
		presence := "offline"
		if choice.active {
			presence = "active"
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
	return renderMessagePanel(body.String(), width, "[recipient · choose a local recipient]", "", m.paneFocused(focusReply))
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
