package tui

import (
	"context"
	"errors"
	"fmt"
	"io"
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
	titleStyle = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("212"))
	finalStyle = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("42"))
	selected   = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("230")).Background(lipgloss.Color("62"))
	dim        = lipgloss.NewStyle().Foreground(lipgloss.Color("241"))
	panelEdge  = lipgloss.NewStyle().Foreground(lipgloss.Color("62"))
	panel      = lipgloss.NewStyle().Border(lipgloss.RoundedBorder()).BorderForeground(lipgloss.Color("62")).Padding(0, 1)
)

type app struct {
	ctx            context.Context
	store          domain.Store
	repo           repoctx.Provider
	messages       []model.Message
	groups         []messageGroup
	inbox          []model.Message
	sent           []model.Message
	archived       []model.Message
	showSent       bool
	showArchived   bool
	showStatus     bool
	showTechnical  bool
	cursor         int
	width          int
	height         int
	answering      bool
	answerID       string
	answerGroupKey string
	answerQ        model.Message
	composeTo      string
	editor         textarea.Model
	err            error
	contextID      string
	branch         string
	remotes        string
	pull           string
	sync           func(context.Context) error
	syncErr        error
	network        domain.NetworkStatus
	changes        <-chan domain.Invalidation
	states         <-chan domain.ConnectionUpdate
	connection     domain.ConnectionUpdate
	undoStack      []undoAction
	nextUndoID     uint64
	undoing        bool
	undoNotice     string
	messageScroll  int
	paneFocus      paneFocus
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
}

type paneLayout struct {
	width         int
	height        int
	inboxHeight   int
	messageWidth  int
	messageHeight int
	replyWidth    int
	replyHeight   int
	horizontal    bool
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
	err      error
}

type answeredMsg struct{ err error }

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
		subscription, err = updates.Subscribe(ctx, domain.TopicMessages, domain.TopicMailboxes, domain.TopicNetwork, domain.TopicPeers, domain.TopicHuman, domain.TopicRelays)
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
	m := app{ctx: ctx, store: s, repo: repoctx.GitHub{}, editor: editor, sync: sync, states: updates.States, connection: updates.Initial}
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
	network, err := m.store.NetworkStatus(m.ctx)
	return loadedMsg{inbox: inbox, sent: sent, archived: archived, network: network, err: err}
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
		message.RecipientMailboxID = m.composeTo
		message.RecipientInstallationID = agentInstallation(m.answerQ)
		message.RecipientLabel = agentLabel(m.answerQ)
		err = m.store.Create(m.ctx, message)
	} else {
		replyTo := m.answerID
		message.ReplyTo = &replyTo
		message.Details = turnCorrelationDetails(m.answerQ)
		err = m.store.Reply(m.ctx, m.answerID, message)
	}
	return answeredMsg{err: err}
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
		m.width, m.height = msg.Width, msg.Height
		layout := responsivePaneLayout(msg.Width, msg.Height, m.answering)
		m.editor.SetWidth(max(1, layout.replyWidth-panel.GetHorizontalFrameSize()))
		m.editor.SetHeight(max(1, layout.replyHeight-4))
	case loadedMsg:
		selectedKey := m.selectedGroupKey()
		m.inbox, m.sent, m.archived, m.network, m.err = msg.inbox, msg.sent, msg.archived, msg.network, msg.err
		m.setMessages()
		if index := groupIndex(m.groups, selectedKey); index >= 0 {
			m.cursor = index
		} else if m.cursor >= len(m.messages) {
			m.cursor = max(0, len(m.messages)-1)
		}
		return m.withContextCommand()
	case repairMsg:
		return m, tea.Batch(m.load, m.syncNow(), scheduleRepair())
	case invalidatedMsg:
		return m, tea.Batch(m.load, m.waitInvalidation())
	case connectionMsg:
		m.connection = msg.state
		return m, m.waitConnection()
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
		if msg.err == nil {
			m.answering = false
			m.answerID = ""
			m.answerGroupKey = ""
			m.answerQ = model.Message{}
			m.composeTo = ""
			m.paneFocus = focusInbox
			m.editor.Reset()
			return m, tea.Batch(m.load, m.syncNow())
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
		switch msg.String() {
		case "tab":
			m.cyclePaneFocus(1)
			if m.answering && m.paneFocus == focusReply {
				m.editor.Focus()
				return m, textarea.Blink
			}
			m.editor.Blur()
			return m, nil
		case "shift+tab":
			m.cyclePaneFocus(-1)
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
				m.answering = false
				m.answerID = ""
				m.answerGroupKey = ""
				m.answerQ = model.Message{}
				m.composeTo = ""
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
			if group, ok := m.groupAtCursor(); ok && canReplyGroup(group) {
				m.answering = true
				m.answerQ = replyTarget(group)
				m.answerID = m.answerQ.ID
				m.answerGroupKey = group.key
				m.composeTo = ""
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
			if group, ok := m.groupAtCursor(); ok {
				representative := group.latest()
				target := agentMailbox(representative)
				if target != "" {
					m.answering = true
					m.answerQ = representative
					m.answerID = ""
					m.answerGroupKey = group.key
					m.composeTo = target
					m.paneFocus = focusReply
					m.resizeEditor()
					m.editor.Focus()
					return m, textarea.Blink
				}
			}
		case "r":
			return m, m.load
		}
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

func (m app) paneFocused(pane paneFocus) bool { return m.paneFocus == pane }

func focusPanelLabel(label string, focused bool) string {
	if !focused || label == "" {
		return label
	}
	if strings.HasSuffix(label, "]") {
		return strings.TrimSuffix(label, "]") + " · focused]"
	}
	return label + " · focused"
}

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
	if len(m.groups) > 0 || len(m.messages) == 0 {
		return m.groups
	}
	return groupMessages(m.messages)
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
	} else if group, found := m.groupAtCursor(); found {
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

func agentMailbox(message model.Message) string {
	if message.SenderMailboxID != model.HumanMailboxID {
		return message.SenderMailboxID
	}
	if message.RecipientMailboxID != model.HumanMailboxID {
		return message.RecipientMailboxID
	}
	return ""
}

func agentLabel(message model.Message) string {
	if message.SenderMailboxID != model.HumanMailboxID {
		return message.SenderLabel
	}
	if message.RecipientMailboxID != model.HumanMailboxID {
		return message.RecipientLabel
	}
	return ""
}

func agentInstallation(message model.Message) string {
	if message.SenderMailboxID != model.HumanMailboxID {
		return message.SenderInstallationID
	}
	if message.RecipientMailboxID != model.HumanMailboxID {
		return message.RecipientInstallationID
	}
	return ""
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

func presentationDetails(raw string, expanded bool) (string, bool) {
	if expanded || raw == "" {
		return raw, false
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

func (m app) View() tea.View {
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
	messagePane := renderMessagePanel("No message selected.", layout.messageWidth, focusPanelLabel("[message]", m.paneFocused(focusMessage)), "")
	if hasDetail {
		messagePane = m.renderGroupPanel(detailGroup, layout.messageWidth)
	}
	messagePane = fitRenderedPane(messagePane, layout.messageWidth, layout.messageHeight, m.messageScroll)
	replyPane := renderMessagePanel("Press Enter to reply to the selected turn.", layout.replyWidth, focusPanelLabel("[reply]", m.paneFocused(focusReply)), "")
	if m.answering {
		replyPane = m.renderReplyPane(layout.replyWidth)
	}
	replyPane = fitRenderedPane(replyPane, layout.replyWidth, layout.replyHeight, 0)
	bottom := ""
	if layout.horizontal {
		bottom = lipgloss.JoinHorizontal(lipgloss.Top, messagePane, replyPane)
	} else {
		bottom = lipgloss.JoinVertical(lipgloss.Left, messagePane, replyPane)
	}
	help := "tab/shift+tab focus · j/k navigate · pgup/pgdown message · enter reply · d archive · u undo · i details · q quit"
	if m.answering {
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
	result.messageWidth, result.messageHeight = width, remaining
	if width >= 120 {
		result.horizontal = true
		result.messageWidth = width / 2
		result.replyWidth = width - result.messageWidth
		result.replyHeight = remaining
		return result
	}
	result.replyWidth = width
	result.messageHeight = max(1, (remaining+1)/2)
	result.replyHeight = max(1, remaining-result.messageHeight)
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
			message := groups[i].latest()
			direction := short(displayMailboxLabel(message.SenderLabel, message.Context), 18)
			if message.SenderMailboxID == model.HumanMailboxID {
				direction = "sent → " + short(displayMailboxLabel(message.RecipientLabel, message.Context), 16)
			}
			kind := groupPresentationKind(groups[i])
			badge := presentationLabel(kind)
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
	rendered := renderMessagePanel(strings.Join(lines, "\n"), width, focusPanelLabel("[HQ · Inbox]", m.paneFocused(focusInbox)), "")
	return fitRenderedPane(rendered, width, height, 0)
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
	latest := group.latest()
	kind := groupPresentationKind(group)
	sender := displayMailboxLabel(latest.SenderLabel, latest.Context)
	topLabel := presentationPanelLabel(kind, sender)
	topLabel = focusPanelLabel(topLabel, m.paneFocused(focusMessage))
	if topLabel == "" && m.paneFocused(focusMessage) {
		topLabel = "[message · focused]"
	}
	var body strings.Builder
	if topLabel == "" {
		body.WriteString(dim.Render("From: " + sender))
		body.WriteString("\n\n")
	}
	metadataHidden := false
	for i, message := range group.messages {
		if i > 0 {
			body.WriteString("\n\n")
		}
		body.WriteString(dim.Render("── " + message.CreatedAt.Local().Format("Jan 2, 3:04:05 PM") + " ──"))
		body.WriteByte('\n')
		switch presentationKind(message) {
		case "final-answer":
			body.WriteString(finalStyle.Render(message.Body))
		case "update", "status", "notice":
			body.WriteString(message.Body)
		default:
			body.WriteString(titleStyle.Render(message.Body))
		}
		visibleDetails, hidden := presentationDetails(message.Details, m.showTechnical)
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
	return renderMessagePanel(body.String(), width, topLabel, bottomLabel)
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
		body.WriteString(titleStyle.Render("New message to " + displayMailboxLabel(agentLabel(m.answerQ), m.answerQ.Context)))
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
	return renderMessagePanel(body.String(), width, focusPanelLabel("[reply]", m.paneFocused(focusReply)), "")
}

func short(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n-1] + "…"
}

func singleLine(s string) string { return strings.Join(strings.Fields(s), " ") }

func renderMessagePanel(content string, terminalWidth int, topLabel, bottomLabel string) string {
	rendered := panel.Render(content)
	if terminalWidth > panel.GetHorizontalFrameSize() {
		rendered = panel.Width(terminalWidth).Render(content)
	}
	width := lipgloss.Width(rendered)
	minimumWidth := max(lipgloss.Width(topLabel), lipgloss.Width(bottomLabel)) + 6
	if (topLabel != "" || bottomLabel != "") && width < minimumWidth && (terminalWidth <= 0 || minimumWidth <= terminalWidth) {
		width = minimumWidth
		rendered = panel.Width(width).Render(content)
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
		lines[0] = panelEdge.Render("╭─" + " " + label + " " + strings.Repeat("─", right) + "╮")
	}
	if bottomLabel != "" {
		label := truncateDisplay(bottomLabel, bottomWidth-6)
		left := bottomWidth - lipgloss.Width(label) - 5
		lines[len(lines)-1] = panelEdge.Render("╰"+strings.Repeat("─", left)) + panelEdge.Render(" "+label+" ") + panelEdge.Render("─╯")
	}
	return strings.Join(lines, "\n")
}

func fitRenderedPane(rendered string, width, height, scrollBack int) string {
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
	blankRendered := panel.Width(max(width, panel.GetHorizontalFrameSize()+1)).Render("")
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
