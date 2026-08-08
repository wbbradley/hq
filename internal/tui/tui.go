package tui

import (
	"context"
	"errors"
	"fmt"
	"io"
	"strings"
	"time"

	"charm.land/bubbles/v2/key"
	"charm.land/bubbles/v2/textarea"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/repoctx"
	"github.com/wbbradley/hq/internal/store"
)

const refreshInterval = time.Minute

var (
	titleStyle = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("212"))
	selected   = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("230")).Background(lipgloss.Color("62"))
	dim        = lipgloss.NewStyle().Foreground(lipgloss.Color("241"))
	panel      = lipgloss.NewStyle().Border(lipgloss.RoundedBorder()).BorderForeground(lipgloss.Color("62")).Padding(0, 1)
)

type app struct {
	ctx       context.Context
	store     store.Store
	repo      repoctx.Provider
	questions []model.Question
	pending   []model.Question
	history   []model.Question
	historyOn bool
	cursor    int
	width     int
	height    int
	answering bool
	answerID  string
	answerQ   model.Question
	editor    textarea.Model
	err       error
	contextID string
	branch    string
	remotes   string
	pull      string
}

type loadedMsg struct {
	pending []model.Question
	history []model.Question
	err     error
}

type answeredMsg struct{ err error }

type refreshMsg struct{}

type branchMsg struct {
	question model.Question
	branch   string
	err      error
}

type pullMsg struct {
	questionID string
	pull       *repoctx.PullRequest
	err        error
}

type remotesMsg struct {
	question model.Question
	branch   string
	remotes  []repoctx.Remote
	err      error
}

func Run(ctx context.Context, s store.Store, in io.Reader, out io.Writer) error {
	editor := textarea.New()
	editor.Placeholder = "Type the answer"
	editor.KeyMap.InsertNewline = key.NewBinding(
		key.WithKeys("shift+enter", "ctrl+j"),
		key.WithHelp("shift+enter/ctrl+j", "insert newline"),
	)
	editor.SetWidth(72)
	editor.SetHeight(6)
	m := app{ctx: ctx, store: s, repo: repoctx.GitHub{}, editor: editor}
	_, err := tea.NewProgram(m, tea.WithInput(in), tea.WithOutput(out), tea.WithContext(ctx)).Run()
	return err
}

func (m app) Init() tea.Cmd { return tea.Batch(m.load, scheduleRefresh()) }

func scheduleRefresh() tea.Cmd {
	return tea.Tick(refreshInterval, func(time.Time) tea.Msg { return refreshMsg{} })
}

func (m app) load() tea.Msg {
	pending, err := m.store.List(m.ctx, model.Filter{Status: model.StatusPending, Limit: 1000})
	if err != nil {
		return loadedMsg{err: err}
	}
	history, err := m.store.List(m.ctx, model.Filter{ExcludeStatus: model.StatusPending, Limit: 1000, NewestFirst: true})
	return loadedMsg{pending: pending, history: history, err: err}
}

func (m app) answer() tea.Msg {
	if m.answerID == "" {
		return answeredMsg{err: errors.New("answer has no question")}
	}
	err := m.store.Answer(m.ctx, m.answerID, strings.TrimSpace(m.editor.Value()))
	return answeredMsg{err: err}
}

func (m app) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width, m.height = msg.Width, msg.Height
		m.editor.SetWidth(max(20, min(72, msg.Width-6)))
	case loadedMsg:
		selectedID := m.selectedID()
		m.pending, m.history, m.err = msg.pending, msg.history, msg.err
		m.setQuestions()
		if index := questionIndex(m.questions, selectedID); index >= 0 {
			m.cursor = index
		} else if m.cursor >= len(m.questions) {
			m.cursor = max(0, len(m.questions)-1)
		}
		return m.withContextCommand()
	case refreshMsg:
		return m, tea.Batch(m.load, scheduleRefresh())
	case branchMsg:
		if msg.question.ID != m.contextID {
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
		return m, m.loadRemotes(msg.question, msg.branch)
	case remotesMsg:
		if msg.question.ID != m.contextID {
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
		return m, m.loadPull(msg.question, msg.branch)
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
			m.answerQ = model.Question{}
			m.editor.Reset()
			return m, m.load
		}
	case tea.KeyPressMsg:
		if m.answering {
			switch msg.String() {
			case "ctrl+c", "esc":
				m.answering = false
				m.answerID = ""
				m.answerQ = model.Question{}
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
			if m.cursor+1 < len(m.questions) {
				m.cursor++
				return m.withContextCommand()
			}
		case "k", "up":
			if m.cursor > 0 {
				m.cursor--
				return m.withContextCommand()
			}
		case "tab", "h":
			m.historyOn = !m.historyOn
			m.cursor = 0
			m.setQuestions()
			return m.withContextCommand()
		case "enter", "a":
			if !m.historyOn && len(m.questions) > 0 {
				m.answering = true
				m.answerQ = m.questions[m.cursor]
				m.answerID = m.answerQ.ID
				m.editor.Focus()
				return m, textarea.Blink
			}
		case "r":
			return m, m.load
		}
	}
	return m, nil
}

func (m *app) setQuestions() {
	if m.historyOn {
		m.questions = m.history
	} else {
		m.questions = m.pending
	}
}

func (m app) withContextCommand() (tea.Model, tea.Cmd) {
	var q model.Question
	if m.answering {
		q = m.answerQ
	} else if len(m.questions) > 0 {
		q = m.questions[m.cursor]
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

func (m app) loadRemotes(q model.Question, branch string) tea.Cmd {
	return func() tea.Msg {
		remotes, err := m.repo.Remotes(m.ctx, q.Directory)
		return remotesMsg{question: q, branch: branch, remotes: remotes, err: err}
	}
}

func (m app) loadBranch(q model.Question) tea.Cmd {
	return func() tea.Msg {
		branch, err := m.repo.Branch(m.ctx, q.Directory)
		return branchMsg{question: q, branch: branch, err: err}
	}
}

func (m app) loadPull(q model.Question, branch string) tea.Cmd {
	return func() tea.Msg {
		pull, err := m.repo.PullRequest(m.ctx, q.Directory, branch)
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
		return m.answerID
	}
	if m.cursor >= 0 && m.cursor < len(m.questions) {
		return m.questions[m.cursor].ID
	}
	return ""
}

func questionIndex(questions []model.Question, id string) int {
	for i := range questions {
		if questions[i].ID == id {
			return i
		}
	}
	return -1
}

func (m app) View() tea.View {
	var b strings.Builder
	b.WriteString(titleStyle.Render("HQ · Questions"))
	b.WriteString("  ")
	if m.historyOn {
		b.WriteString(dim.Render("Pending"))
		b.WriteString("  ")
		b.WriteString(selected.Render("History"))
	} else {
		b.WriteString(selected.Render("Pending"))
		b.WriteString("  ")
		b.WriteString(dim.Render("History"))
	}
	b.WriteString("\n\n")
	if m.err != nil {
		b.WriteString(lipgloss.NewStyle().Foreground(lipgloss.Color("196")).Render(m.err.Error()))
		b.WriteString("\n\n")
	}
	if len(m.questions) == 0 {
		if m.historyOn {
			b.WriteString(dim.Render("No question history. Press r to refresh."))
		} else {
			b.WriteString(dim.Render("No pending questions. Press r to refresh."))
		}
	} else {
		for i, q := range m.questions {
			line := fmt.Sprintf("%-9s %-8s  %s", q.Status, short(q.SessionID, 8), singleLine(q.Prompt))
			if i == m.cursor && (!m.answering || q.ID == m.answerID) {
				b.WriteString(selected.Render("› " + line))
			} else {
				b.WriteString("  " + line)
			}
			b.WriteByte('\n')
		}
	}
	var detail model.Question
	if m.answering {
		detail = m.answerQ
	} else if len(m.questions) > 0 {
		detail = m.questions[m.cursor]
	}
	if detail.ID != "" {
		b.WriteString("\n")
		var body strings.Builder
		body.WriteString(titleStyle.Render(detail.Prompt))
		body.WriteByte('\n')
		body.WriteString(dim.Render(detail.Directory + " · " + detail.SessionID))
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
		if m.historyOn && detail.Response != nil {
			body.WriteString("\n\n")
			body.WriteString(titleStyle.Render("Answer"))
			body.WriteByte('\n')
			body.WriteString(*detail.Response)
		}
		b.WriteString(panel.Render(body.String()))
	}
	if m.answering {
		b.WriteString("\n\n")
		b.WriteString(titleStyle.Render("Answer"))
		b.WriteString("\n")
		b.WriteString(m.editor.View())
		b.WriteString("\n")
		b.WriteString(dim.Render("enter submit · shift+enter/ctrl+j newline · esc cancel"))
	} else {
		b.WriteString("\n\n")
		if m.historyOn {
			b.WriteString(dim.Render("j/k move · tab/h pending · r refresh · q quit · auto-refresh 1m"))
		} else {
			b.WriteString(dim.Render("j/k move · enter answer · tab/h history · r refresh · q quit · auto-refresh 1m"))
		}
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
