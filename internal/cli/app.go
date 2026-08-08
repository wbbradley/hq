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

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/store"
	"github.com/wbbradley/hq/internal/tui"
)

const usage = `hq queues questions from agents for people to answer.

Usage:
  hq [--db PATH] <command> [options]
  hq                         Open the human TUI
	  hq version                 Print the version

Agent commands:
  ask     Create a question; print its ID
  wait    Wait for one answer; print and complete it
  poll    Print and complete all ready answers in a session
  get     Read one question without changing it

Human commands:
  tui     Browse and answer pending questions
  list    List questions
  answer  Answer one question
  cancel  Cancel one pending question

Set HQ_SESSION to avoid passing --session to ask and poll.
The default database is $XDG_STATE_HOME/hq/hq.db or ~/.local/state/hq/hq.db.
`

var ErrNoAnswers = errors.New("no answers ready")

type App struct {
	In     io.Reader
	Out    io.Writer
	ErrOut io.Writer
	Getwd  func() (string, error)
	Open   func(string) (store.Store, error)
}

func New() *App {
	return &App{
		In: os.Stdin, Out: os.Stdout, ErrOut: os.Stderr, Getwd: os.Getwd,
		Open: func(path string) (store.Store, error) { return store.Open(path) },
	}
}

func (a *App) Run(ctx context.Context, args []string) error {
	dbPath, args, err := globalArgs(args)
	if err != nil {
		return err
	}
	command := "tui"
	if len(args) > 0 {
		command, args = args[0], args[1:]
	}
	if command == "help" || command == "-h" || command == "--help" {
		_, err := io.WriteString(a.Out, usage)
		return err
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
		return tui.Run(ctx, s, a.In, a.Out)
	default:
		return fmt.Errorf("unknown command %q\n\n%s", command, usage)
	}
}

func globalArgs(args []string) (string, []string, error) {
	path := os.Getenv("HQ_DB")
	if len(args) >= 2 && args[0] == "--db" {
		path, args = args[1], args[2:]
	} else if len(args) > 0 && strings.HasPrefix(args[0], "--db=") {
		path, args = strings.TrimPrefix(args[0], "--db="), args[1:]
	} else if len(args) > 0 && args[0] == "--db" {
		return "", nil, errors.New("--db needs a path")
	}
	return path, args, nil
}

func flags(name string) *flag.FlagSet {
	f := flag.NewFlagSet(name, flag.ContinueOnError)
	f.SetOutput(io.Discard)
	return f
}

func (a *App) ask(ctx context.Context, s store.Store, args []string) error {
	f := flags("ask")
	session := f.String("session", os.Getenv("HQ_SESSION"), "caller session ID")
	directory := f.String("dir", "", "work directory scope")
	details := f.String("details", "", "extra context")
	jsonOutput := f.Bool("json", false, "write JSON")
	if err := f.Parse(args); err != nil {
		return err
	}
	if strings.TrimSpace(*session) == "" {
		return errors.New("ask needs --session or HQ_SESSION")
	}
	if *directory == "" {
		var err error
		*directory, err = a.Getwd()
		if err != nil {
			return fmt.Errorf("get work directory: %w", err)
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
			return fmt.Errorf("read question: %w", err)
		}
		prompt = strings.TrimSpace(string(data))
	}
	if prompt == "" {
		return errors.New("question text is required")
	}
	id, err := uuid.NewV7()
	if err != nil {
		return fmt.Errorf("make question ID: %w", err)
	}
	q := model.Question{ID: id.String(), Directory: filepath.Clean(absolute), SessionID: *session, Prompt: prompt, Details: *details, Status: model.StatusPending, CreatedAt: time.Now().UTC()}
	if err := s.Create(ctx, q); err != nil {
		return err
	}
	if *jsonOutput {
		return writeJSON(a.Out, q)
	}
	return writeOnce(a.Out, []byte(q.ID+"\n"))
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
		return errors.New("wait needs one question ID")
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
	for {
		token := uuid.NewString()
		q, err := s.ClaimAnswer(ctx, id, token)
		if err == nil {
			if err := deliver(a.Out, q, *jsonOutput); err != nil {
				_ = s.ReleaseAnswer(context.Background(), id, token)
				return err
			}
			return s.CompleteAnswer(context.Background(), id, token)
		}
		if !errors.Is(err, store.ErrNotReady) && !errors.Is(err, store.ErrClaimed) {
			return err
		}
		q, getErr := s.Get(ctx, id)
		if getErr != nil {
			return getErr
		}
		if q.Status == model.StatusCancelled {
			return errors.New("question was cancelled")
		}
		if q.CompletedAt != nil {
			return errors.New("answer was already completed")
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
	session := f.String("session", os.Getenv("HQ_SESSION"), "caller session ID")
	directory := f.String("dir", "", "work directory scope")
	jsonOutput := f.Bool("json", false, "write JSON")
	if err := f.Parse(args); err != nil {
		return err
	}
	if len(f.Args()) != 0 || strings.TrimSpace(*session) == "" {
		return errors.New("poll needs --session or HQ_SESSION")
	}
	if *directory == "" {
		var err error
		*directory, err = a.Getwd()
		if err != nil {
			return err
		}
	}
	abs, err := filepath.Abs(*directory)
	if err != nil {
		return err
	}
	questions, err := s.List(ctx, model.Filter{Directory: filepath.Clean(abs), SessionID: *session, Status: model.StatusAnswered, Limit: 1000})
	if err != nil {
		return err
	}
	type claim struct {
		q     model.Question
		token string
	}
	var claims []claim
	for _, candidate := range questions {
		if candidate.CompletedAt != nil {
			continue
		}
		token := uuid.NewString()
		q, err := s.ClaimAnswer(ctx, candidate.ID, token)
		if err == nil {
			claims = append(claims, claim{q: q, token: token})
		}
	}
	if len(claims) == 0 {
		return ErrNoAnswers
	}
	var data []byte
	if *jsonOutput {
		items := make([]model.Question, len(claims))
		for i := range claims {
			items[i] = claims[i].q
		}
		data, err = json.Marshal(items)
		data = append(data, '\n')
	} else {
		var b bytes.Buffer
		for _, c := range claims {
			fmt.Fprintf(&b, "%s\t%s\n", c.q.ID, *c.q.Response)
		}
		data = b.Bytes()
	}
	writeErr := writeOnce(a.Out, data)
	if err != nil || writeErr != nil {
		for _, c := range claims {
			_ = s.ReleaseAnswer(context.Background(), c.q.ID, c.token)
		}
		if err != nil {
			return err
		}
		return writeErr
	}
	for _, c := range claims {
		if err := s.CompleteAnswer(context.Background(), c.q.ID, c.token); err != nil {
			return err
		}
	}
	return nil
}

func (a *App) get(ctx context.Context, s store.Store, args []string) error {
	if len(args) != 1 {
		return errors.New("get needs one question ID")
	}
	q, err := s.Get(ctx, args[0])
	if err != nil {
		return err
	}
	return writeJSON(a.Out, q)
}

func (a *App) list(ctx context.Context, s store.Store, args []string) error {
	f := flags("list")
	session := f.String("session", "", "session ID")
	directory := f.String("dir", "", "work directory scope")
	status := f.String("status", "", "pending, answered, or cancelled")
	limit := f.Int("limit", 100, "maximum rows")
	jsonOutput := f.Bool("json", false, "write JSON")
	if err := f.Parse(args); err != nil {
		return err
	}
	if len(f.Args()) != 0 {
		return errors.New("list takes flags only")
	}
	if *status != "" && *status != string(model.StatusPending) && *status != string(model.StatusAnswered) && *status != string(model.StatusCancelled) {
		return fmt.Errorf("invalid status %q", *status)
	}
	filter := model.Filter{Directory: *directory, SessionID: *session, Status: model.Status(*status), Limit: *limit}
	questions, err := s.List(ctx, filter)
	if err != nil {
		return err
	}
	if *jsonOutput {
		return writeJSON(a.Out, questions)
	}
	var b bytes.Buffer
	for _, q := range questions {
		fmt.Fprintf(&b, "%s\t%s\t%s\t%s\n", q.ID, q.Status, q.SessionID, strings.Join(strings.Fields(q.Prompt), " "))
	}
	return writeOnce(a.Out, b.Bytes())
}

func (a *App) answer(ctx context.Context, s store.Store, args []string) error {
	f := flags("answer")
	if err := f.Parse(args); err != nil {
		return err
	}
	if len(f.Args()) < 1 {
		return errors.New("answer needs a question ID")
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
		return errors.New("answer text is required")
	}
	return s.Answer(ctx, f.Args()[0], response)
}

func (a *App) cancel(ctx context.Context, s store.Store, args []string) error {
	if len(args) != 1 {
		return errors.New("cancel needs one question ID")
	}
	return s.Cancel(ctx, args[0])
}

func deliver(out io.Writer, q model.Question, asJSON bool) error {
	if asJSON {
		return writeJSON(out, q)
	}
	return writeOnce(out, []byte(*q.Response+"\n"))
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
