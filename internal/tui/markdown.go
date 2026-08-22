package tui

import (
	"strings"

	"charm.land/glamour/v2"
	"charm.land/glamour/v2/ansi"
	"charm.land/glamour/v2/styles"
	"github.com/wbbradley/hq/internal/model"
)

const messageMarkdownCacheLimit = 512

type markdownRenderFunc func(body, kind string, width int) (string, error)

type messageMarkdownCacheKey struct {
	messageID string
	body      string
	kind      string
	width     int
}

type messageMarkdownRenderer struct {
	render markdownRenderFunc
	cache  map[messageMarkdownCacheKey]string
}

func newMessageMarkdownRenderer(render markdownRenderFunc) *messageMarkdownRenderer {
	if render == nil {
		render = renderMessageMarkdown
	}
	return &messageMarkdownRenderer{render: render, cache: make(map[messageMarkdownCacheKey]string)}
}

func (r *messageMarkdownRenderer) Render(message model.Message, width int) string {
	if r == nil {
		rendered, err := renderMessageMarkdown(message.Body, presentationKind(message), width)
		if err != nil {
			return message.Body
		}
		return rendered
	}
	if r.render == nil {
		r.render = renderMessageMarkdown
	}
	if r.cache == nil {
		r.cache = make(map[messageMarkdownCacheKey]string)
	}
	key := messageMarkdownCacheKey{
		messageID: message.ID,
		body:      message.Body,
		kind:      presentationKind(message),
		width:     max(1, width),
	}
	if rendered, ok := r.cache[key]; ok {
		return rendered
	}
	rendered, err := r.render(message.Body, key.kind, key.width)
	if err != nil {
		rendered = message.Body
	}
	if len(r.cache) >= messageMarkdownCacheLimit {
		r.Reset()
	}
	r.cache[key] = rendered
	return rendered
}

func (r *messageMarkdownRenderer) Reset() {
	if r != nil {
		r.cache = make(map[messageMarkdownCacheKey]string)
	}
}

func renderMessageMarkdown(body, kind string, width int) (string, error) {
	renderer, err := glamour.NewTermRenderer(
		glamour.WithStyles(messageMarkdownStyle(kind)),
		glamour.WithWordWrap(max(1, width)),
		glamour.WithTableWrap(true),
		glamour.WithInlineTableLinks(true),
		glamour.WithPreservedNewLines(),
	)
	if err != nil {
		return "", err
	}
	rendered, err := renderer.Render(body)
	if err != nil {
		return "", err
	}
	return strings.Trim(rendered, "\n"), nil
}

func messageMarkdownStyle(kind string) ansi.StyleConfig {
	style := styles.DarkStyleConfig
	zero := uint(0)
	style.Document.Margin = &zero
	style.Document.BlockPrefix = ""
	style.Document.BlockSuffix = ""
	style.Document.Color = nil
	style.CodeBlock.Margin = &zero
	style.Heading.Color = markdownString("212")
	style.H1.BackgroundColor = markdownString("62")
	if kind == "final-answer" {
		style.Document.Color = markdownString("42")
	}
	return style
}

func markdownString(value string) *string { return &value }
