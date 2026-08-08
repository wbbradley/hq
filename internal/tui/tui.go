package tui

import (
	"context"
	"errors"
	"fmt"
	"io"
	"strings"
	"time"

	"charm.land/bubbles/v2/textarea"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/wbbradley/hq/internal/model"
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
	questions []model.Question
	cursor    int
	width     int
	height    int
	answering bool
	answerID  string
	answerQ   model.Question
	editor    textarea.Model
	err       error
}

type loadedMsg struct {
	questions []model.Question
	err       error
}

type answeredMsg struct{ err error }

type refreshMsg struct{}

func Run(ctx context.Context, s store.Store, in io.Reader, out io.Writer) error {
	editor := textarea.New()
	editor.Placeholder = "Type the answer"
	editor.SetWidth(72)
	editor.SetHeight(6)
	m := app{ctx: ctx, store: s, editor: editor}
	_, err := tea.NewProgram(m, tea.WithInput(in), tea.WithOutput(out), tea.WithContext(ctx)).Run()
	return err
}

func (m app) Init() tea.Cmd { return tea.Batch(m.load, scheduleRefresh()) }

func scheduleRefresh() tea.Cmd {
	return tea.Tick(refreshInterval, func(time.Time) tea.Msg { return refreshMsg{} })
}

func (m app) load() tea.Msg {
	questions, err := m.store.List(m.ctx, model.Filter{Status: model.StatusPending, Limit: 1000})
	return loadedMsg{questions: questions, err: err}
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
		m.questions, m.err = msg.questions, msg.err
		if index := questionIndex(m.questions, selectedID); index >= 0 {
			m.cursor = index
		} else if m.cursor >= len(m.questions) {
			m.cursor = max(0, len(m.questions)-1)
		}
	case refreshMsg:
		return m, tea.Batch(m.load, scheduleRefresh())
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
			case "shift+enter", "ctrl+s":
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
			}
		case "k", "up":
			if m.cursor > 0 {
				m.cursor--
			}
		case "enter", "a":
			if len(m.questions) > 0 {
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
	b.WriteString("\n\n")
	if m.err != nil {
		b.WriteString(lipgloss.NewStyle().Foreground(lipgloss.Color("196")).Render(m.err.Error()))
		b.WriteString("\n\n")
	}
	if len(m.questions) == 0 {
		b.WriteString(dim.Render("No pending questions. Press r to refresh."))
	} else {
		for i, q := range m.questions {
			line := fmt.Sprintf("%-8s  %s", short(q.SessionID, 8), singleLine(q.Prompt))
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
		b.WriteString(panel.Render(fmt.Sprintf("%s\n%s\n\n%s", titleStyle.Render(detail.Prompt), dim.Render(detail.Directory+" · "+detail.SessionID), detail.Details)))
	}
	if m.answering {
		b.WriteString("\n\n")
		b.WriteString(titleStyle.Render("Answer"))
		b.WriteString("\n")
		b.WriteString(m.editor.View())
		b.WriteString("\n")
		b.WriteString(dim.Render("shift+enter submit · ctrl+s fallback · esc cancel"))
	} else {
		b.WriteString("\n\n")
		b.WriteString(dim.Render("j/k move · enter answer · r refresh · q quit · auto-refresh 1m"))
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
