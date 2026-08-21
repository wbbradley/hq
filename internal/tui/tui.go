package tui

import (
	"context"
	"errors"
	"fmt"
	"io"
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
	selected   = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("230")).Background(lipgloss.Color("62"))
	dim        = lipgloss.NewStyle().Foreground(lipgloss.Color("241"))
	panel      = lipgloss.NewStyle().Border(lipgloss.RoundedBorder()).BorderForeground(lipgloss.Color("62")).Padding(0, 1)
)

type app struct {
	ctx          context.Context
	store        domain.Store
	repo         repoctx.Provider
	messages     []model.Message
	inbox        []model.Message
	sent         []model.Message
	archived     []model.Message
	showSent     bool
	showArchived bool
	showStatus   bool
	cursor       int
	width        int
	height       int
	answering    bool
	answerID     string
	answerQ      model.Message
	composeTo    string
	editor       textarea.Model
	err          error
	contextID    string
	branch       string
	remotes      string
	pull         string
	sync         func(context.Context) error
	syncErr      error
	network      domain.NetworkStatus
	changes      <-chan domain.Invalidation
	states       <-chan domain.ConnectionUpdate
	connection   domain.ConnectionUpdate
}

type loadedMsg struct {
	inbox    []model.Message
	sent     []model.Message
	archived []model.Message
	network  domain.NetworkStatus
	err      error
}

type answeredMsg struct{ err error }

type archivedMsg struct{ err error }

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
		err = m.store.Reply(m.ctx, m.answerID, message)
	}
	return answeredMsg{err: err}
}

func (m app) archive(id string) tea.Cmd {
	return func() tea.Msg {
		return archivedMsg{err: m.store.Archive(m.ctx, id)}
	}
}

func (m app) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width, m.height = msg.Width, msg.Height
		m.editor.SetWidth(max(20, min(72, msg.Width-6)))
	case loadedMsg:
		selectedID := m.selectedID()
		m.inbox, m.sent, m.archived, m.network, m.err = msg.inbox, msg.sent, msg.archived, msg.network, msg.err
		m.setMessages()
		if index := messageIndex(m.messages, selectedID); index >= 0 {
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
			m.answerQ = model.Message{}
			m.composeTo = ""
			m.editor.Reset()
			return m, tea.Batch(m.load, m.syncNow())
		}
	case archivedMsg:
		m.err = msg.err
		if msg.err == nil {
			return m, tea.Batch(m.load, m.syncNow())
		}
	case tea.PasteMsg:
		if m.answering {
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
		if m.answering {
			switch msg.String() {
			case "ctrl+c", "esc":
				m.answering = false
				m.answerID = ""
				m.answerQ = model.Message{}
				m.composeTo = ""
				m.editor.Blur()
				m.editor.Reset()
				return m, nil
			case "enter":
				if strings.TrimSpace(m.editor.Value()) != "" {
					return m, m.answer
				}
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
			if m.cursor+1 < len(m.messages) {
				m.cursor++
				return m.withContextCommand()
			}
		case "k", "up":
			if m.cursor > 0 {
				m.cursor--
				return m.withContextCommand()
			}
		case "s":
			m.showSent = !m.showSent
			m.cursor = 0
			m.setMessages()
			return m.withContextCommand()
		case "x":
			m.showArchived = !m.showArchived
			m.cursor = 0
			m.setMessages()
			return m.withContextCommand()
		case "v":
			m.showStatus = !m.showStatus
			return m, nil
		case "enter", "a":
			if len(m.messages) > 0 && canReply(m.messages[m.cursor]) {
				m.answering = true
				m.answerQ = m.messages[m.cursor]
				m.answerID = m.answerQ.ID
				m.composeTo = ""
				m.editor.Focus()
				return m, textarea.Blink
			}
		case "d":
			if len(m.messages) > 0 && canArchive(m.messages[m.cursor]) {
				return m, m.archive(m.messages[m.cursor].ID)
			}
		case "n":
			if len(m.messages) > 0 {
				target := agentMailbox(m.messages[m.cursor])
				if target != "" {
					m.answering = true
					m.answerQ = m.messages[m.cursor]
					m.answerID = ""
					m.composeTo = target
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

func (m *app) setMessages() {
	seen := make(map[string]bool)
	m.messages = nil
	add := func(messages []model.Message) {
		for _, message := range messages {
			if !seen[message.ID] {
				seen[message.ID] = true
				m.messages = append(m.messages, message)
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
	sort.Slice(m.messages, func(i, j int) bool {
		if m.messages[i].CreatedAt.Equal(m.messages[j].CreatedAt) {
			return m.messages[i].ID > m.messages[j].ID
		}
		return m.messages[i].CreatedAt.After(m.messages[j].CreatedAt)
	})
}

func (m app) withContextCommand() (tea.Model, tea.Cmd) {
	var q model.Message
	if m.answering {
		q = m.answerQ
	} else if len(m.messages) > 0 {
		q = m.messages[m.cursor]
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

func (m app) selectedID() string {
	if m.answering {
		return m.answerQ.ID
	}
	if m.cursor >= 0 && m.cursor < len(m.messages) {
		return m.messages[m.cursor].ID
	}
	return ""
}

func messageIndex(messages []model.Message, id string) int {
	for i := range messages {
		if messages[i].ID == id {
			return i
		}
	}
	return -1
}

func canReply(message model.Message) bool {
	return message.RecipientMailboxID == model.HumanMailboxID && message.SenderMailboxID != model.HumanMailboxID && message.ArchivedAt == nil
}

func canArchive(message model.Message) bool {
	return message.RecipientMailboxID == model.HumanMailboxID && message.ArchivedAt == nil
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

func (m app) View() tea.View {
	var b strings.Builder
	b.WriteString(titleStyle.Render("HQ · Mailbox"))
	b.WriteString("  ")
	b.WriteString(selected.Render("Inbox"))
	b.WriteString("  ")
	if m.showSent {
		b.WriteString(selected.Render("Sent:on"))
	} else {
		b.WriteString(dim.Render("Sent:off"))
	}
	b.WriteString("  ")
	if m.showArchived {
		b.WriteString(selected.Render("Archived:on"))
	} else {
		b.WriteString(dim.Render("Archived:off"))
	}
	b.WriteString("\n\n")
	if m.connection.Diagnostic != "" {
		style := dim
		if m.connection.Blocking {
			style = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("196"))
		}
		b.WriteString(style.Render(m.connection.Diagnostic))
		b.WriteString("\n\n")
		if m.connection.Blocking {
			b.WriteString(dim.Render("Restart or upgrade the indicated HQ component, then reopen the TUI. · q quit"))
			view := tea.NewView(b.String())
			view.AltScreen = true
			return view
		}
	}
	if m.err != nil {
		b.WriteString(lipgloss.NewStyle().Foreground(lipgloss.Color("196")).Render(m.err.Error()))
		b.WriteString("\n\n")
	}
	if m.syncErr != nil {
		b.WriteString(dim.Render("relay sync pending: " + m.syncErr.Error()))
		b.WriteString("\n\n")
	}
	if m.showStatus {
		b.WriteString(panel.Render(formatNetworkStatus(m.network)))
		b.WriteString("\n\n")
	}
	if len(m.messages) == 0 {
		b.WriteString(dim.Render("No messages in this view. Press r to refresh."))
	} else {
		for i, message := range m.messages {
			direction := "inbox ← " + short(message.SenderLabel, 16)
			if message.SenderMailboxID == model.HumanMailboxID {
				direction = "sent → " + short(message.RecipientLabel, 16)
			}
			state := deliveryLabel(message)
			if message.ArchivedAt != nil {
				state += " [archived]"
			}
			line := fmt.Sprintf("%-18s %s%s", direction, singleLine(message.Body), state)
			if i == m.cursor && (!m.answering || message.ID == m.answerQ.ID) {
				b.WriteString(selected.Render("› " + line))
			} else {
				b.WriteString("  " + line)
			}
			b.WriteByte('\n')
		}
	}
	var detail model.Message
	if m.answering {
		detail = m.answerQ
	} else if len(m.messages) > 0 {
		detail = m.messages[m.cursor]
	}
	if detail.ID != "" {
		b.WriteString("\n")
		var body strings.Builder
		body.WriteString(titleStyle.Render(detail.Body))
		body.WriteByte('\n')
		body.WriteString(dim.Render(detail.SenderLabel + " → " + detail.RecipientLabel + " · " + detail.Context.Directory))
		if detail.SourceDeviceLabel != "" || detail.SenderInstallationID != "" {
			body.WriteByte('\n')
			source := detail.SourceDeviceLabel
			if source == "" {
				source = "installation"
			}
			body.WriteString(dim.Render("source " + source + " · " + short(detail.SenderInstallationID, 13)))
		}
		if detail.ReplyTo != nil {
			body.WriteByte('\n')
			body.WriteString(dim.Render("reply to " + *detail.ReplyTo))
		}
		if m.branch != "" {
			body.WriteByte('\n')
			body.WriteString(dim.Render("git " + m.branch))
		}
		if m.remotes != "" {
			body.WriteByte('\n')
			body.WriteString(dim.Render(m.remotes))
		}
		if m.pull != "" {
			body.WriteString(dim.Render(" · " + m.pull))
		}
		if detail.Details != "" {
			body.WriteString("\n\n")
			body.WriteString(detail.Details)
		}
		b.WriteString(renderMessagePanel(body.String(), m.width))
	}
	if m.answering {
		b.WriteString("\n\n")
		if m.composeTo != "" {
			b.WriteString(titleStyle.Render("New message to " + agentLabel(m.answerQ)))
		} else {
			b.WriteString(titleStyle.Render("Reply to " + m.answerQ.SenderLabel))
		}
		b.WriteString("\n")
		b.WriteString(m.editor.View())
		b.WriteString("\n")
		b.WriteString(dim.Render("enter submit · shift+enter/ctrl+j newline · esc cancel"))
	} else {
		b.WriteString("\n\n")
		b.WriteString(dim.Render("j/k move · enter reply · d archive · n new message · s sent · x archived · v status · r refresh · q quit · live updates · repair 5m"))
	}
	view := tea.NewView(b.String())
	view.AltScreen = true
	return view
}

func short(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n-1] + "…"
}

func singleLine(s string) string { return strings.Join(strings.Fields(s), " ") }

func renderMessagePanel(content string, terminalWidth int) string {
	rendered := panel.Render(content)
	if terminalWidth <= panel.GetHorizontalFrameSize() || lipgloss.Width(rendered) <= terminalWidth {
		return rendered
	}
	return panel.Width(terminalWidth).Render(content)
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
