package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/charmbracelet/x/term"
	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/agenthelp"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/store"
	"github.com/wbbradley/hq/internal/tui"
)

const usage = `hq delivers messages between agent sessions and a human mailbox.

Usage:
  hq [--db PATH] <command> [options]
  hq                          Open the TUI in a terminal; list the open human inbox otherwise

Agent commands:
  agents  Print instructions for agents
  ask     Send a message to the human inbox; print its ID
  wait    Wait for a reply to one message
  poll    Read and complete messages in an agent mailbox
  get     Read one message without changing it

Human commands:
  tui     Read inbox, sent, and archived messages
  list    List messages
  answer  Reply to one inbox message
  cancel  Archive one inbox message

Other commands:
  help     Print command help
  version  Print the version

HQ_SESSION sets the agent session. HQ can also detect CODEX_THREAD_ID.
The default database is $XDG_STATE_HOME/hq/hq.db or ~/.local/state/hq/hq.db.
`

var ErrNoMessages = errors.New("no messages ready")

type App struct {
	In     io.Reader
	Out    io.Writer
	ErrOut io.Writer
	Getwd  func() (string, error)
	Getenv func(string) string
	IsTTY  func() bool
	Open   func(string) (store.Store, error)
	RunTUI func(context.Context, store.Store, io.Reader, io.Writer) error
}

func New() *App {
	return &App{
		In: os.Stdin, Out: os.Stdout, ErrOut: os.Stderr, Getwd: os.Getwd, Getenv: os.Getenv,
		IsTTY: func() bool {
			return term.IsTerminal(os.Stdin.Fd()) && term.IsTerminal(os.Stdout.Fd())
		},
		Open:   func(path string) (store.Store, error) { return store.Open(path) },
		RunTUI: tui.Run,
	}
}

func (a *App) Run(ctx context.Context, args []string) error {
	dbPath, args, err := globalArgs(args, a.getenv("HQ_DB"))
	if err != nil {
		return err
	}
	command := ""
	if len(args) == 0 {
		if a.isTTY() {
			command = "tui"
		} else {
			command = "list"
			args, err = a.bareListArgs()
			if err != nil {
				return err
			}
		}
	} else {
		command, args = args[0], args[1:]
	}
	if command == "help" || command == "-h" || command == "--help" {
		_, err := io.WriteString(a.Out, usage)
		return err
	}
	if command == "agents" {
		if len(args) != 0 {
			return errors.New("agents takes no arguments")
		}
		return writeOnce(a.Out, []byte(agenthelp.Text))
	}
	s, err := a.Open(dbPath)
	if err != nil {
		return err
	}
	defer s.Close()
	switch command {
	case "ask":
		return a.ask(ctx, s, args)
	case "wait":
		return a.wait(ctx, s, args)
	case "poll":
		return a.poll(ctx, s, args)
	case "get":
		return a.get(ctx, s, args)
	case "list":
		return a.list(ctx, s, args)
	case "answer":
		return a.answer(ctx, s, args)
	case "cancel":
		return a.cancel(ctx, s, args)
	case "tui":
		if len(args) != 0 {
			return errors.New("tui takes no arguments")
		}
		if a.RunTUI != nil {
			return a.RunTUI(ctx, s, a.In, a.Out)
		}
		return tui.Run(ctx, s, a.In, a.Out)
	default:
		return fmt.Errorf("unknown command %q\n\n%s", command, usage)
	}
}

func globalArgs(args []string, path string) (string, []string, error) {
	if len(args) >= 2 && args[0] == "--db" {
		path, args = args[1], args[2:]
	} else if len(args) > 0 && strings.HasPrefix(args[0], "--db=") {
		path, args = strings.TrimPrefix(args[0], "--db="), args[1:]
	} else if len(args) > 0 && args[0] == "--db" {
		return "", nil, errors.New("--db needs a path")
	}
	return path, args, nil
}

func (a *App) getenv(name string) string {
	if a.Getenv != nil {
		return a.Getenv(name)
	}
	return os.Getenv(name)
}

func (a *App) isTTY() bool {
	return a.IsTTY != nil && a.IsTTY()
}

func (a *App) inferredSession() string {
	for _, name := range []string{"HQ_SESSION", "CODEX_THREAD_ID"} {
		if value := strings.TrimSpace(a.getenv(name)); value != "" {
			return value
		}
	}
	return ""
}

func (a *App) workDirectory() (string, error) {
	directory, err := a.Getwd()
	if err != nil {
		return "", fmt.Errorf("get work directory: %w", err)
	}
	absolute, err := filepath.Abs(directory)
	if err != nil {
		return "", fmt.Errorf("resolve work directory: %w", err)
	}
	return filepath.Clean(absolute), nil
}

func (a *App) bareListArgs() ([]string, error) {
	directory, err := a.workDirectory()
	if err != nil {
		return nil, err
	}
	return []string{"--recipient", model.HumanSession, "--open", "--dir", directory}, nil
}

func flags(name string) *flag.FlagSet {
	f := flag.NewFlagSet(name, flag.ContinueOnError)
	f.SetOutput(io.Discard)
	return f
}

func (a *App) ask(ctx context.Context, s store.Store, args []string) error {
	f := flags("ask")
	session := f.String("session", a.inferredSession(), "caller session ID")
	directory := f.String("dir", "", "work directory scope")
	details := f.String("details", "", "extra context")
	jsonOutput := f.Bool("json", false, "write JSON")
	if err := f.Parse(args); err != nil {
		return err
	}
	if strings.TrimSpace(*session) == "" {
		return errors.New("ask needs --session, HQ_SESSION, or a detected agent session")
	}
	if *session == model.HumanSession {
		return fmt.Errorf("%q is reserved for the human mailbox", model.HumanSession)
	}
	if *directory == "" {
		var err error
		*directory, err = a.workDirectory()
		if err != nil {
			return err
		}
	}
	absolute, err := filepath.Abs(*directory)
	if err != nil {
		return fmt.Errorf("resolve work directory: %w", err)
	}
	prompt := strings.TrimSpace(strings.Join(f.Args(), " "))
	if prompt == "" {
		data, err := io.ReadAll(a.In)
		if err != nil {
			return fmt.Errorf("read message: %w", err)
		}
		prompt = strings.TrimSpace(string(data))
	}
	if prompt == "" {
		return errors.New("message text is required")
	}
	id, err := uuid.NewV7()
	if err != nil {
		return fmt.Errorf("make message ID: %w", err)
	}
	m := model.Message{ID: id.String(), Directory: filepath.Clean(absolute), SenderSession: *session, RecipientSession: model.HumanSession, Body: prompt, Details: *details, CreatedAt: time.Now().UTC()}
	if err := s.Create(ctx, m); err != nil {
		return err
	}
	if *jsonOutput {
		return writeJSON(a.Out, m)
	}
	return writeOnce(a.Out, []byte(m.ID+"\n"))
}

func (a *App) wait(ctx context.Context, s store.Store, args []string) error {
	f := flags("wait")
	timeout := f.Duration("timeout", 0, "maximum wait time")
	jsonOutput := f.Bool("json", false, "write JSON")
	interval := f.Duration("interval", 250*time.Millisecond, "poll interval")
	if err := f.Parse(args); err != nil {
		return err
	}
	if len(f.Args()) != 1 {
		return errors.New("wait needs one message ID")
	}
	if *interval <= 0 {
		return errors.New("--interval must be positive")
	}
	if *timeout > 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, *timeout)
		defer cancel()
	}
	id := f.Args()[0]
	original, err := s.Get(ctx, id)
	if err != nil {
		return err
	}
	if original.SenderSession == model.HumanSession || original.RecipientSession != model.HumanSession {
		return errors.New("wait needs an agent-to-human message ID")
	}
	for {
		token := uuid.NewString()
		m, err := s.Claim(ctx, store.Claim{ReplyTo: id, Directory: original.Directory, RecipientSession: original.SenderSession}, token)
		if err == nil {
			if err := deliver(a.Out, m, *jsonOutput); err != nil {
				_ = s.Release(context.Background(), m.ID, token)
				return err
			}
			return s.Complete(context.Background(), m.ID, token)
		}
		if !errors.Is(err, store.ErrNotReady) && !errors.Is(err, store.ErrClaimed) {
			return err
		}
		replies, listErr := s.List(ctx, model.Filter{ReplyTo: id, Directory: original.Directory, RecipientSession: original.SenderSession, Limit: 1})
		if listErr != nil {
			return listErr
		}
		if len(replies) == 1 && replies[0].CompletedAt != nil {
			return errors.New("reply was already delivered")
		}
		current, getErr := s.Get(ctx, id)
		if getErr != nil {
			return getErr
		}
		if current.ArchivedAt != nil && len(replies) == 0 {
			return errors.New("message was archived without a reply")
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(*interval):
		}
	}
}

func (a *App) poll(ctx context.Context, s store.Store, args []string) error {
	f := flags("poll")
	session := f.String("session", a.inferredSession(), "caller session ID")
	directory := f.String("dir", "", "work directory scope")
	jsonOutput := f.Bool("json", false, "write JSON")
	if err := f.Parse(args); err != nil {
		return err
	}
	if len(f.Args()) != 0 || strings.TrimSpace(*session) == "" {
		return errors.New("poll needs --session, HQ_SESSION, or a detected agent session")
	}
	if *session == model.HumanSession {
		return fmt.Errorf("%q is reserved for the human mailbox", model.HumanSession)
	}
	if *directory == "" {
		var err error
		*directory, err = a.workDirectory()
		if err != nil {
			return err
		}
	}
	abs, err := filepath.Abs(*directory)
	if err != nil {
		return err
	}
	completed := false
	messages, err := s.List(ctx, model.Filter{Directory: filepath.Clean(abs), RecipientSession: *session, Completed: &completed, Limit: 1000})
	if err != nil {
		return err
	}
	type claim struct {
		m     model.Message
		token string
	}
	var claims []claim
	for _, candidate := range messages {
		token := uuid.NewString()
		m, err := s.Claim(ctx, store.Claim{MessageID: candidate.ID}, token)
		if err == nil {
			claims = append(claims, claim{m: m, token: token})
		}
	}
	if len(claims) == 0 {
		return ErrNoMessages
	}
	var data []byte
	if *jsonOutput {
		items := make([]model.Message, len(claims))
		for i := range claims {
			items[i] = claims[i].m
		}
		data, err = json.Marshal(items)
		data = append(data, '\n')
	} else {
		var b bytes.Buffer
		for _, c := range claims {
			fmt.Fprintf(&b, "%s\t%s\n", c.m.ID, c.m.Body)
		}
		data = b.Bytes()
	}
	writeErr := writeOnce(a.Out, data)
	if err != nil || writeErr != nil {
		for _, c := range claims {
			_ = s.Release(context.Background(), c.m.ID, c.token)
		}
		if err != nil {
			return err
		}
		return writeErr
	}
	for _, c := range claims {
		if err := s.Complete(context.Background(), c.m.ID, c.token); err != nil {
			return err
		}
	}
	return nil
}

func (a *App) get(ctx context.Context, s store.Store, args []string) error {
	if len(args) != 1 {
		return errors.New("get needs one message ID")
	}
	m, err := s.Get(ctx, args[0])
	if err != nil {
		return err
	}
	return writeJSON(a.Out, m)
}

func (a *App) list(ctx context.Context, s store.Store, args []string) error {
	f := flags("list")
	session := f.String("session", "", "recipient session ID")
	sender := f.String("sender", "", "sender session ID")
	recipient := f.String("recipient", "", "recipient session ID")
	directory := f.String("dir", "", "work directory scope")
	open := f.Bool("open", false, "only unarchived messages")
	archived := f.Bool("archived", false, "only archived messages")
	limit := f.Int("limit", 100, "maximum rows")
	jsonOutput := f.Bool("json", false, "write JSON")
	if err := f.Parse(args); err != nil {
		return err
	}
	if len(f.Args()) != 0 {
		return errors.New("list takes flags only")
	}
	if *open && *archived {
		return errors.New("list cannot combine --open and --archived")
	}
	if *recipient == "" {
		*recipient = *session
	}
	filter := model.Filter{Directory: *directory, SenderSession: *sender, RecipientSession: *recipient, Limit: *limit}
	if *open || *archived {
		filter.Archived = new(bool)
		*filter.Archived = *archived
	}
	messages, err := s.List(ctx, filter)
	if err != nil {
		return err
	}
	if *jsonOutput {
		return writeJSON(a.Out, messages)
	}
	var b bytes.Buffer
	for _, m := range messages {
		fmt.Fprintf(&b, "%s\t%s→%s\t%s\n", m.ID, m.SenderSession, m.RecipientSession, strings.Join(strings.Fields(m.Body), " "))
	}
	return writeOnce(a.Out, b.Bytes())
}

func (a *App) answer(ctx context.Context, s store.Store, args []string) error {
	f := flags("answer")
	if err := f.Parse(args); err != nil {
		return err
	}
	if len(f.Args()) < 1 {
		return errors.New("answer needs a message ID")
	}
	response := strings.TrimSpace(strings.Join(f.Args()[1:], " "))
	if response == "" {
		data, err := io.ReadAll(a.In)
		if err != nil {
			return err
		}
		response = strings.TrimSpace(string(data))
	}
	if response == "" {
		return errors.New("reply text is required")
	}
	original, err := s.Get(ctx, f.Args()[0])
	if err != nil {
		return err
	}
	id, err := uuid.NewV7()
	if err != nil {
		return fmt.Errorf("make message ID: %w", err)
	}
	replyTo := original.ID
	reply := model.Message{ID: id.String(), Directory: original.Directory, SenderSession: model.HumanSession,
		RecipientSession: original.SenderSession, Body: response, ReplyTo: &replyTo, CreatedAt: time.Now().UTC()}
	return s.Reply(ctx, original.ID, reply)
}

func (a *App) cancel(ctx context.Context, s store.Store, args []string) error {
	if len(args) != 1 {
		return errors.New("cancel needs one message ID")
	}
	return s.Archive(ctx, args[0])
}

func deliver(out io.Writer, m model.Message, asJSON bool) error {
	if asJSON {
		return writeJSON(out, m)
	}
	return writeOnce(out, []byte(m.Body+"\n"))
}

func writeJSON(out io.Writer, value any) error {
	data, err := json.Marshal(value)
	if err != nil {
		return err
	}
	return writeOnce(out, append(data, '\n'))
}

func writeOnce(out io.Writer, data []byte) error {
	n, err := out.Write(data)
	if err != nil {
		return err
	}
	if n != len(data) {
		return io.ErrShortWrite
	}
	return nil
}
