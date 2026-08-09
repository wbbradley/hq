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
	"github.com/wbbradley/hq/internal/repoctx"
	"github.com/wbbradley/hq/internal/session"
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
  mailboxes  Find agent mailboxes seen in this repository
  help       Print command help
  version    Print the version

HQ detects Codex, Claude Code, and Pi sessions. HQ_SESSION is an advanced override.
The default database is $XDG_STATE_HOME/hq/hq.db or ~/.local/state/hq/hq.db.
`

var ErrNoMessages = errors.New("no messages ready")

type App struct {
	In          io.Reader
	Out         io.Writer
	ErrOut      io.Writer
	Getwd       func() (string, error)
	Getenv      func(string) string
	IsTTY       func() bool
	Open        func(string) (store.Store, error)
	RunTUI      func(context.Context, store.Store, io.Reader, io.Writer) error
	RepoContext func(context.Context, string) model.RepositoryContext
	Sessions    session.IdentityResolver
}

func New() *App {
	return &App{
		In: os.Stdin, Out: os.Stdout, ErrOut: os.Stderr, Getwd: os.Getwd, Getenv: os.Getenv,
		IsTTY: func() bool {
			return term.IsTerminal(os.Stdin.Fd()) && term.IsTerminal(os.Stdout.Fd())
		},
		Open:        func(path string) (store.Store, error) { return store.Open(path) },
		RunTUI:      tui.Run,
		RepoContext: repoctx.GitHub{}.Snapshot,
		Sessions:    session.Resolver{Getenv: os.Getenv},
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
	case "mailboxes":
		return a.mailboxes(ctx, s, args)
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
	return []string{"--recipient", "human", "--dir", directory}, nil
}

func (a *App) repositoryContext(ctx context.Context, directory string) model.RepositoryContext {
	if a.RepoContext != nil {
		return a.RepoContext(ctx, directory)
	}
	return repoctx.GitHub{}.Snapshot(ctx, directory)
}

func (a *App) resolveMailbox(ctx context.Context, s store.Store, explicit, directory string) (model.Mailbox, model.RepositoryContext, error) {
	resolver := a.Sessions
	if resolver == nil {
		resolver = session.Resolver{Getenv: a.getenv}
	}
	identity, err := resolver.Resolve(explicit)
	if err != nil {
		return model.Mailbox{}, model.RepositoryContext{}, fmt.Errorf("resolve agent mailbox: %w", err)
	}
	if directory == "" {
		directory, err = a.workDirectory()
		if err != nil {
			return model.Mailbox{}, model.RepositoryContext{}, err
		}
	}
	abs, err := filepath.Abs(directory)
	if err != nil {
		return model.Mailbox{}, model.RepositoryContext{}, fmt.Errorf("resolve work directory: %w", err)
	}
	repo := a.repositoryContext(ctx, filepath.Clean(abs))
	mailbox, err := s.ResolveMailbox(ctx, identity, repo)
	return mailbox, repo, err
}

func flags(name string) *flag.FlagSet {
	f := flag.NewFlagSet(name, flag.ContinueOnError)
	f.SetOutput(io.Discard)
	return f
}

func (a *App) ask(ctx context.Context, s store.Store, args []string) error {
	f := flags("ask")
	sessionID := f.String("session", "", "advanced session override")
	directory := f.String("dir", "", "work directory scope")
	details := f.String("details", "", "extra context")
	jsonOutput := f.Bool("json", false, "write JSON")
	if err := f.Parse(args); err != nil {
		return err
	}
	mailbox, repo, err := a.resolveMailbox(ctx, s, *sessionID, *directory)
	if err != nil {
		return err
	}
	human, err := s.HumanMailbox(ctx)
	if err != nil {
		return err
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
	m := model.Message{ID: id.String(), Context: repo, SenderMailboxID: mailbox.ID, RecipientMailboxID: human.ID, SenderLabel: mailbox.Label, RecipientLabel: human.Label, Body: prompt, Details: *details, CreatedAt: time.Now().UTC()}
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
	sessionID := f.String("session", "", "advanced session override")
	directory := f.String("dir", "", "work directory context")
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
	mailbox, _, err := a.resolveMailbox(ctx, s, *sessionID, *directory)
	if err != nil {
		return err
	}
	original, err := s.Get(ctx, id)
	if err != nil {
		return err
	}
	if original.SenderMailboxID != mailbox.ID {
		return errors.New("message was sent by another agent mailbox")
	}
	if original.RecipientMailboxID != model.HumanMailboxID {
		return errors.New("wait needs an agent-to-human message ID")
	}
	for {
		token := uuid.NewString()
		m, err := s.Claim(ctx, store.Claim{ReplyTo: id, RecipientMailboxID: mailbox.ID}, token)
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
		replies, listErr := s.List(ctx, model.Filter{ReplyTo: id, RecipientMailboxID: mailbox.ID, Limit: 1})
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
	sessionID := f.String("session", "", "advanced session override")
	directory := f.String("dir", "", "work directory scope")
	jsonOutput := f.Bool("json", false, "write JSON")
	if err := f.Parse(args); err != nil {
		return err
	}
	if len(f.Args()) != 0 {
		return errors.New("poll takes flags only")
	}
	mailbox, _, err := a.resolveMailbox(ctx, s, *sessionID, *directory)
	if err != nil {
		return err
	}
	completed := false
	messages, err := s.List(ctx, model.Filter{RecipientMailboxID: mailbox.ID, Completed: &completed, Limit: 1000})
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
	sessionID := f.String("session", "", "recipient mailbox ID")
	sender := f.String("sender", "", "sender mailbox ID or human")
	recipient := f.String("recipient", "", "recipient mailbox ID or human")
	directory := f.String("dir", "", "work directory scope")
	open := f.Bool("open", false, "only unarchived messages")
	archived := f.Bool("archived", false, "only archived messages")
	all := f.Bool("all", false, "include open and archived messages")
	limit := f.Int("limit", 100, "maximum rows")
	jsonOutput := f.Bool("json", false, "write JSON")
	if err := f.Parse(args); err != nil {
		return err
	}
	if len(f.Args()) != 0 {
		return errors.New("list takes flags only")
	}
	if (*open && *archived) || (*all && (*open || *archived)) {
		return errors.New("list cannot combine --open, --archived, and --all")
	}
	if *recipient == "" {
		*recipient = *sessionID
	}
	mailboxID := func(value string) string {
		if value == "human" {
			return model.HumanMailboxID
		}
		return value
	}
	filter := model.Filter{Directory: *directory, SenderMailboxID: mailboxID(*sender), RecipientMailboxID: mailboxID(*recipient), Limit: *limit}
	if !*all {
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
		fmt.Fprintf(&b, "%s\t%s→%s\t%s\n", m.ID, m.SenderLabel, m.RecipientLabel, strings.Join(strings.Fields(m.Body), " "))
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
	reply := model.Message{ID: id.String(), Context: original.Context, SenderMailboxID: model.HumanMailboxID,
		RecipientMailboxID: original.SenderMailboxID, SenderLabel: "human", RecipientLabel: original.SenderLabel, Body: response, ReplyTo: &replyTo, CreatedAt: time.Now().UTC()}
	return s.Reply(ctx, original.ID, reply)
}

func (a *App) mailboxes(ctx context.Context, s store.Store, args []string) error {
	f := flags("mailboxes")
	directory := f.String("dir", "", "work directory context")
	jsonOutput := f.Bool("json", false, "write JSON")
	if err := f.Parse(args); err != nil {
		return err
	}
	if len(f.Args()) != 0 {
		return errors.New("mailboxes takes flags only")
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
	repo := a.repositoryContext(ctx, filepath.Clean(abs))
	mailboxes, err := s.FindMailboxes(ctx, repo)
	if err != nil {
		return err
	}
	if *jsonOutput {
		return writeJSON(a.Out, mailboxes)
	}
	var b bytes.Buffer
	for _, mailbox := range mailboxes {
		fmt.Fprintf(&b, "%s\t%s\t%s\t%s\n", mailbox.ID, mailbox.Label, mailbox.LastSeen.Format(time.RFC3339), mailbox.Context.Directory)
	}
	return writeOnce(a.Out, b.Bytes())
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
