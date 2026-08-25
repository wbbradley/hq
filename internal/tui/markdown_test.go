package tui

import (
	"errors"
	"strings"
	"testing"

	"charm.land/lipgloss/v2"
	"github.com/charmbracelet/x/ansi"
	"github.com/wbbradley/hq/internal/model"
)

func TestRenderMessageMarkdownSupportsBoldAndTables(t *testing.T) {
	body := "A **bold value**.\n\n| Name | Description |\n| --- | --- |\n| alpha | a deliberately long table cell that must wrap |"
	rendered, err := renderMessageMarkdown(body, "update", 38)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(rendered, "**bold value**") || !strings.Contains(rendered, "\x1b[1m") {
		t.Fatalf("bold Markdown was not rendered: %q", rendered)
	}
	for _, value := range []string{"Name", "Description", "alpha", "deliberately"} {
		if !strings.Contains(rendered, value) {
			t.Fatalf("rendered table omitted %q: %q", value, rendered)
		}
	}
	for lineNumber, line := range strings.Split(rendered, "\n") {
		if got := lipgloss.Width(line); got > 38 {
			t.Fatalf("line %d width = %d; want at most 38: %q", lineNumber+1, got, line)
		}
	}
}

func TestRenderMessageMarkdownPreservesPlainTextNewlines(t *testing.T) {
	rendered, err := renderMessageMarkdown("first line\nsecond line", "update", 40)
	if err != nil {
		t.Fatal(err)
	}
	lines := strings.Split(ansi.Strip(rendered), "\n")
	if len(lines) != 2 || strings.TrimSpace(lines[0]) != "first line" || strings.TrimSpace(lines[1]) != "second line" {
		t.Fatalf("plain-text newline was not preserved: %q", rendered)
	}
}

func TestRenderMessageMarkdownPreservesFinalAnswerColor(t *testing.T) {
	rendered, err := renderMessageMarkdown("Final answer with **specific emphasis**", "final-answer", 50)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(rendered, "38;5;42") || !strings.Contains(rendered, ";1m") {
		t.Fatalf("final-answer styling = %q", rendered)
	}
}

func TestMessageMarkdownRendererCachesAndInvalidates(t *testing.T) {
	calls := 0
	renderer := newMessageMarkdownRenderer(func(body, kind string, width int) (string, error) {
		calls++
		return body, nil
	})
	message := model.Message{ID: "message", Body: "body", Presentation: model.PresentationUpdate}

	if got := renderer.Render(message, 40); got != message.Body {
		t.Fatalf("rendered body = %q", got)
	}
	renderer.Render(message, 40)
	if calls != 1 {
		t.Fatalf("same cache key rendered %d times; want 1", calls)
	}
	renderer.Render(message, 30)
	if calls != 2 {
		t.Fatalf("width change rendered %d times; want 2", calls)
	}
	message.Body = "changed"
	renderer.Render(message, 30)
	if calls != 3 {
		t.Fatalf("body change rendered %d times; want 3", calls)
	}
	renderer.Reset()
	renderer.Render(message, 30)
	if calls != 4 {
		t.Fatalf("reset rendered %d times; want 4", calls)
	}
}

func TestMessageMarkdownRendererFallsBackToOriginalBody(t *testing.T) {
	renderer := newMessageMarkdownRenderer(func(string, string, int) (string, error) {
		return "", errors.New("render failed")
	})
	message := model.Message{ID: "message", Body: "**unrendered but visible**", Presentation: model.PresentationFinalAnswer}
	if got := renderer.Render(message, 40); got != message.Body {
		t.Fatalf("fallback = %q; want %q", got, message.Body)
	}
}
