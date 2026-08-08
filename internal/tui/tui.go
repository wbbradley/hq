package tui

import (
	"context"
	"fmt"
	"io"
	"strings"

	"github.com/charmbracelet/bubbles/textarea"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/store"
)

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
	editor    textarea.Model
	err       error
}

type loadedMsg struct {
	questions []model.Question
	err       error
}

type answeredMsg struct{ err error }

func Run(ctx context.Context, s store.Store, in io.Reader, out io.Writer) error {
	editor := textarea.New()
	editor.Placeholder = "Type the answer"
	editor.SetWidth(72)
	editor.SetHeight(6)
	m := app{ctx: ctx, store: s, editor: editor}
	_, err := tea.NewProgram(m, tea.WithInput(in), tea.WithOutput(out), tea.WithContext(ctx), tea.WithAltScreen()).Run()
	return err
}

func (m app) Init() tea.Cmd { return m.load }

func (m app) load() tea.Msg {
	questions, err := m.store.List(m.ctx, model.Filter{Status: model.StatusPending, Limit: 1000})
	return loadedMsg{questions: questions, err: err}
}

func (m app) answer() tea.Msg {
	if len(m.questions) == 0 {
		return answeredMsg{}
	}
	err := m.store.Answer(m.ctx, m.questions[m.cursor].ID, strings.TrimSpace(m.editor.Value()))
	return answeredMsg{err: err}
}

func (m app) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width, m.height = msg.Width, msg.Height
		m.editor.SetWidth(max(20, min(72, msg.Width-6)))
	case loadedMsg:
		m.questions, m.err = msg.questions, msg.err
		if m.cursor >= len(m.questions) {
			m.cursor = max(0, len(m.questions)-1)
		}
	case answeredMsg:
		m.err = msg.err
		if msg.err == nil {
			m.answering = false
			m.editor.Reset()
			return m, m.load
		}
	case tea.KeyMsg:
		if m.answering {
			switch msg.String() {
			case "ctrl+c", "esc":
				m.answering = false
				m.editor.Blur()
				return m, nil
			case "ctrl+s":
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
				m.editor.Focus()
				return m, textarea.Blink
			}
		case "r":
			return m, m.load
		}
	}
	return m, nil
}

func (m app) View() string {
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
			if i == m.cursor {
				b.WriteString(selected.Render("› " + line))
			} else {
				b.WriteString("  " + line)
			}
			b.WriteByte('\n')
		}
		q := m.questions[m.cursor]
		b.WriteString("\n")
		b.WriteString(panel.Render(fmt.Sprintf("%s\n%s\n\n%s", titleStyle.Render(q.Prompt), dim.Render(q.Directory+" · "+q.SessionID), q.Details)))
	}
	if m.answering {
		b.WriteString("\n\n")
		b.WriteString(titleStyle.Render("Answer"))
		b.WriteString("\n")
		b.WriteString(m.editor.View())
		b.WriteString("\n")
		b.WriteString(dim.Render("ctrl+s submit · esc cancel"))
	} else {
		b.WriteString("\n\n")
		b.WriteString(dim.Render("j/k move · enter answer · r refresh · q quit"))
	}
	return b.String()
}

func short(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n-1] + "…"
}

func singleLine(s string) string { return strings.Join(strings.Fields(s), " ") }
