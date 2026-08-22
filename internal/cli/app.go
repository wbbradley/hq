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

	charmterm "github.com/charmbracelet/x/term"
	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/agenthelp"
	"github.com/wbbradley/hq/internal/codexbridge"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/hqclient"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/node"
	"github.com/wbbradley/hq/internal/repoctx"
	"github.com/wbbradley/hq/internal/session"
	"github.com/wbbradley/hq/internal/syncer"
	"github.com/wbbradley/hq/internal/tui"
	systerm "golang.org/x/term"
)

const usage = `hq delivers messages between agent sessions and a human mailbox.

Usage:
  hq [--db PATH] [--no-sync] <command> [options]
  hq                          Open the TUI in a terminal; list the open human inbox otherwise

Agent commands:
  agents  Print instructions for agents, with optional focused topics
  ask     Send a question and wait for the human's reply
  send    Send a message without waiting; print its ID
  wait    Wait for a reply to one message
  poll    Read and complete messages in an agent mailbox
  get     Read one message without changing it

Human commands:
  tui     Read inbox, sent, and archived messages
  list    List messages
  answer  Reply to one inbox message
  cancel  Archive one inbox message

Other commands:
  codex [--cwd PATH] [--agent NAME [--new-thread] | --resume THREAD_ID] [--yolo] [INITIAL PROMPT...]
             Bridge a Codex app-server thread through HQ
  agent      Create, list, or retire durable named agents
  mailboxes  Find agent mailboxes seen in this repository
  identity   Create, inspect, back up, import, or reset the installation identity
  human      Show, pair, list, or revoke human account devices
  peer       Add, list, or distrust peer installations
  mailbox    Share or revoke a mailbox for one peer
  relay      Configure installation inbox relays
  status     Show relay and event processing status
  sync       Run one full foreground relay sync pass
  daemon     Run or control the local HQ node (run|status|stop|restart)
  help       Print command help
  version    Print the version

HQ detects Codex, Claude Code, and Pi sessions. HQ_SESSION is an advanced override.
The default database is $XDG_STATE_HOME/hq/hq.db or ~/.local/state/hq/hq.db.
Normal commands connect to and, when needed, auto-start the single node that owns that database.
Local client transport currently requires Unix; Windows named-pipe support is not available yet.
--no-sync skips client-requested immediate relay synchronization; the node may still publish
already-durable outbox work through its configured network engine.
`

const codexUsage = `hq codex bridges one Codex app-server thread through an HQ mailbox.

Usage:
  hq [--db PATH] [--no-sync] codex [--cwd PATH] [--agent NAME [--new-thread] | --resume THREAD_ID] [--yolo] [INITIAL PROMPT...]

Requirements:
  Install and authenticate Codex CLI v0.148.0, and run hq identity init once.

Options:
  --cwd PATH          Thread working directory. Defaults to the current directory;
                      relative paths are resolved from the current directory.
  --resume THREAD_ID  Resume this exact Codex thread instead of starting a new one.
  --agent NAME        Attach to a durable installation-local named agent. Creates it
                      when absent and automatically resumes its selected Codex thread.
  --new-thread        Start and select a replacement thread for --agent while retaining
                      its mailbox, queued root messages, and historical thread bindings.
  --yolo              Pass Codex's --yolo mode to app-server; disables approvals and sandboxing.
                      Use only inside an externally secured environment.

Remaining arguments are joined as the optional initial prompt. Without a prompt, the
bridge waits for HQ input. A new thread receives the structured-human-input instruction;
a resumed thread keeps its existing instructions. The Codex thread ID is bound to one
HQ mailbox, and restart/deduplication state is stored beside the HQ database in
<database>.codexbridge.json.

Questions, approvals, final output, and lifecycle status appear in the human HQ inbox.
Approval replies must exactly match the choices shown by HQ. Secret-marked requests are
rejected because HQ persists messages. Run only one bridge process for a thread. With
--no-sync, the bridge does not request immediate relay synchronization; node-owned networking
may still publish durable outbox work.
`

var ErrNoMessages = errors.New("no messages ready")

const replyRepairInterval = 5 * time.Minute

type App struct {
	In               io.Reader
	Out              io.Writer
	ErrOut           io.Writer
	Getwd            func() (string, error)
	Getenv           func(string) string
	Hostname         func() (string, error)
	IsTTY            func() bool
	Open             func(context.Context, string) (domain.Store, error)
	RunTUI           func(context.Context, domain.Store, io.Reader, io.Writer) error
	RunTUIWithSync   func(context.Context, domain.Store, io.Reader, io.Writer, func(context.Context) error) error
	RunTUIWithClient func(context.Context, domain.Store, io.Reader, io.Writer, domain.ClientUpdates, func(context.Context) error) error
	RepoContext      func(context.Context, string) model.RepositoryContext
	Sessions         session.IdentityResolver
	ReadPassword     func(string) ([]byte, error)
	Synchronize      func(context.Context, domain.Store) error
	RunDaemon        func(context.Context, string) error
	DaemonStatus     func(string) (string, error)
	StopDaemon       func(string) error
	RestartDaemon    func(string) error
	RunCodexBridge   func(context.Context, codexbridge.Options) error
}

func New() *App {
	return &App{
		In: os.Stdin, Out: os.Stdout, ErrOut: os.Stderr, Getwd: os.Getwd, Getenv: os.Getenv,
		Hostname: os.Hostname,
		IsTTY: func() bool {
			return charmterm.IsTerminal(os.Stdin.Fd()) && charmterm.IsTerminal(os.Stdout.Fd())
		},
		Open:             func(ctx context.Context, path string) (domain.Store, error) { return hqclient.Open(ctx, path) },
		RunTUIWithSync:   tui.RunWithSync,
		RunTUIWithClient: tui.RunWithClient,
		RepoContext:      repoctx.GitHub{}.Snapshot,
		Sessions:         session.Resolver{Getenv: os.Getenv},
		Synchronize:      func(ctx context.Context, s domain.Store) error { return s.Synchronize(ctx) },
		RunDaemon:        node.Run,
		DaemonStatus:     syncer.DaemonStatus,
		StopDaemon:       syncer.StopDaemon,
		RestartDaemon:    syncer.RestartDaemon,
		RunCodexBridge:   codexbridge.Run,
		ReadPassword: func(prompt string) ([]byte, error) {
			if _, err := io.WriteString(os.Stderr, prompt); err != nil {
				return nil, err
			}
			password, err := systerm.ReadPassword(int(os.Stdin.Fd()))
			_, _ = io.WriteString(os.Stderr, "\n")
			return password, err
		},
	}
}

func (a *App) Run(ctx context.Context, args []string) error {
	dbPath, noSync, args, err := globalArgs(args, a.getenv("HQ_DB"))
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
		if len(args) == 1 && args[0] == "codex" {
			_, err := io.WriteString(a.Out, codexUsage)
			return err
		}
		if len(args) != 0 {
			return fmt.Errorf("help takes no arguments or the topic codex")
		}
		_, err := io.WriteString(a.Out, usage)
		return err
	}
	if command == "codex" && hasHelpFlag(args) {
		_, err := io.WriteString(a.Out, codexUsage)
		return err
	}
	if command == "agents" {
		if len(args) == 0 {
			return writeOnce(a.Out, []byte(agenthelp.Text))
		}
		if len(args) > 1 {
			return errors.New("agents takes at most one topic")
		}
		text, ok := agenthelp.Topic(args[0])
		if !ok {
			return fmt.Errorf("unknown agents topic %q; run hq agents for available topics", args[0])
		}
		return writeOnce(a.Out, []byte(text))
	}
	if command == "identity" {
		return a.identity(dbPath, args)
	}
	if command == "daemon" {
		return a.daemon(ctx, dbPath, args)
	}
	s, err := a.Open(ctx, dbPath)
	if err != nil {
		return err
	}
	defer s.Close()
	updates := clientUpdates(s)
	if command != "tui" && command != "codex" {
		a.writeConnectionDiagnostic(updates.Initial)
	}
	var commandErr error
	switch command {
	case "ask":
		return a.ask(ctx, s, args, noSync)
	case "send":
		commandErr = a.send(ctx, s, args)
	case "wait":
		return a.wait(ctx, s, args, noSync)
	case "poll":
		if !noSync {
			a.trySync(ctx, s, false, "")
		}
		return a.poll(ctx, s, args)
	case "get":
		return a.get(ctx, s, args)
	case "list":
		return a.list(ctx, s, args)
	case "mailboxes":
		return a.mailboxes(ctx, s, args)
	case "agent":
		commandErr = a.agent(ctx, s, args)
	case "answer":
		commandErr = a.answer(ctx, s, args)
	case "cancel":
		commandErr = a.cancel(ctx, s, args)
	case "codex":
		return a.codex(ctx, s, args, dbPath, noSync)
	case "peer":
		commandErr = a.peer(ctx, s, args)
	case "human":
		commandErr = a.human(ctx, s, args)
	case "mailbox":
		commandErr = a.mailbox(ctx, s, args)
	case "relay":
		commandErr = a.relay(ctx, s, args)
	case "status":
		return a.status(ctx, s, args)
	case "sync":
		if len(args) != 0 {
			return errors.New("sync takes no arguments")
		}
		if noSync {
			return errors.New("sync cannot be combined with --no-sync")
		}
		return a.trySync(ctx, s, true, "")
	case "tui":
		if len(args) != 0 {
			return errors.New("tui takes no arguments")
		}
		if a.RunTUI != nil {
			return a.RunTUI(ctx, s, a.In, a.Out)
		}
		if a.RunTUIWithClient != nil {
			var synchronize func(context.Context) error
			if !noSync {
				synchronize = func(syncCtx context.Context) error { return a.trySync(syncCtx, s, true, "") }
			}
			return a.RunTUIWithClient(ctx, s, a.In, a.Out, updates, synchronize)
		}
		if noSync {
			return tui.Run(ctx, s, a.In, a.Out)
		}
		if a.RunTUIWithSync != nil {
			return a.RunTUIWithSync(ctx, s, a.In, a.Out, func(syncCtx context.Context) error {
				return a.trySync(syncCtx, s, true, "")
			})
		}
		return tui.Run(ctx, s, a.In, a.Out)
	default:
		return fmt.Errorf("unknown command %q\n\n%s", command, usage)
	}
	if commandErr != nil || noSync || !mutatesState(command, args) {
		return commandErr
	}
	note := "local change saved"
	if command == "send" {
		note = "message saved"
	}
	return a.trySync(ctx, s, false, note)
}

func hasHelpFlag(args []string) bool {
	for _, arg := range args {
		if arg == "--" {
			return false
		}
		if arg == "-h" || arg == "--help" {
			return true
		}
	}
	return false
}

func (a *App) codex(ctx context.Context, s domain.Store, args []string, databasePath string, noSync bool) error {
	f := flags("codex")
	workingDirectory := f.String("cwd", "", "Codex thread working directory")
	resumeThreadID := f.String("resume", "", "existing Codex thread ID")
	agentName := f.String("agent", "", "durable named agent")
	newThread := f.Bool("new-thread", false, "start and select a replacement named-agent thread")
	yolo := f.Bool("yolo", false, "disable Codex approvals and sandboxing")
	if err := f.Parse(args); err != nil {
		return err
	}
	if strings.TrimSpace(*agentName) != "" && strings.TrimSpace(*resumeThreadID) != "" {
		return errors.New("codex cannot combine --agent and --resume")
	}
	if *newThread && strings.TrimSpace(*agentName) == "" {
		return errors.New("codex --new-thread requires --agent NAME")
	}
	baseDirectory, err := a.workDirectory()
	if err != nil {
		return err
	}
	directory := strings.TrimSpace(*workingDirectory)
	if directory == "" {
		directory = baseDirectory
	} else if !filepath.IsAbs(directory) {
		directory = filepath.Join(baseDirectory, directory)
	}
	directory = filepath.Clean(directory)
	if a.RunCodexBridge == nil {
		return errors.New("Codex bridge runner is unavailable")
	}
	options := codexbridge.Options{
		Directory: directory, ResumeThreadID: strings.TrimSpace(*resumeThreadID), AgentName: strings.TrimSpace(*agentName), NewThread: *newThread,
		InitialPrompt: strings.Join(f.Args(), " "), Repository: a.repositoryContext(ctx, directory),
		Store: s, Stderr: a.ErrOut, Updates: clientUpdates(s), Yolo: *yolo,
	}
	resolvedDatabasePath, err := identity.ResolveDatabasePath(databasePath)
	if err != nil {
		return err
	}
	options.LedgerPath = resolvedDatabasePath + ".codexbridge.json"
	if !noSync {
		options.Sync = func(syncContext context.Context) error {
			return a.trySync(syncContext, s, false, "Codex bridge status saved")
		}
	}
	return a.RunCodexBridge(ctx, options)
}

func (a *App) daemon(ctx context.Context, databasePath string, args []string) error {
	if len(args) == 1 && args[0] != "run" {
		return a.daemonControl(databasePath, args)
	}
	if len(args) != 1 {
		return errors.New("daemon needs run, status, stop, or restart")
	}
	if a.RunDaemon == nil {
		return errors.New("daemon runner is unavailable")
	}
	resolved, err := identity.ResolveDatabasePath(databasePath)
	if err != nil {
		return err
	}
	return a.RunDaemon(ctx, resolved)
}

func (a *App) daemonControl(databasePath string, args []string) error {
	if len(args) != 1 {
		return errors.New("daemon needs run, status, stop, or restart")
	}
	resolved, err := identity.ResolveDatabasePath(databasePath)
	if err != nil {
		return err
	}
	switch args[0] {
	case "status":
		if a.DaemonStatus == nil {
			return errors.New("daemon status is unavailable")
		}
		status, err := a.DaemonStatus(resolved)
		if err != nil {
			return err
		}
		return writeOnce(a.Out, []byte(status+"\n"))
	case "stop":
		if a.StopDaemon == nil {
			return errors.New("daemon stop is unavailable")
		}
		return a.StopDaemon(resolved)
	case "restart":
		if a.RestartDaemon == nil {
			return errors.New("daemon restart is unavailable")
		}
		return a.RestartDaemon(resolved)
	default:
		return fmt.Errorf("unknown daemon command %q", args[0])
	}
}

func (a *App) trySync(ctx context.Context, s domain.Store, strict bool, savedNote string) error {
	if a.Synchronize == nil {
		return nil
	}
	syncCtx, cancel := context.WithTimeout(ctx, 3*time.Second)
	defer cancel()
	err := a.Synchronize(syncCtx, s)
	if err == nil {
		return nil
	}
	if strict {
		return err
	}
	if a.ErrOut == nil {
		return nil
	}
	prefix := ""
	if savedNote != "" {
		prefix = savedNote + "; "
	}
	_, _ = fmt.Fprintf(a.ErrOut, "hq: %srelay sync pending: %v\n", prefix, err)
	return nil
}

type clientUpdateProvider interface {
	Updates() domain.ClientUpdates
}

func clientUpdates(store domain.Store) domain.ClientUpdates {
	if provider, ok := store.(clientUpdateProvider); ok {
		return provider.Updates()
	}
	return domain.ClientUpdates{}
}

func (a *App) writeConnectionDiagnostic(update domain.ConnectionUpdate) {
	if update.Diagnostic != "" && a.ErrOut != nil {
		_, _ = fmt.Fprintf(a.ErrOut, "hq: %s\n", update.Diagnostic)
	}
}

func mutatesState(command string, args []string) bool {
	switch command {
	case "send", "answer", "cancel", "mailbox":
		return true
	case "peer":
		return len(args) > 0 && args[0] != "list"
	case "human":
		return len(args) > 0 && args[0] != "show" && args[0] != "devices"
	case "relay":
		return len(args) > 0 && args[0] != "list"
	default:
		return false
	}
}

func (a *App) password(prompt string) ([]byte, error) {
	if a.ReadPassword == nil {
		return nil, errors.New("password input is unavailable")
	}
	password, err := a.ReadPassword(prompt)
	if err != nil {
		return nil, err
	}
	if len(password) == 0 {
		return nil, errors.New("password is required")
	}
	return password, nil
}

func (a *App) identity(databasePath string, args []string) error {
	if len(args) == 0 {
		return errors.New("identity needs init, show, export, import, or reset")
	}
	resolved, err := identity.ResolveDatabasePath(databasePath)
	if err != nil {
		return err
	}
	keyPath, err := identity.KeyPath(resolved)
	if err != nil {
		return err
	}
	switch args[0] {
	case "init":
		if len(args) != 1 {
			return errors.New("identity init takes no arguments")
		}
		material, err := identity.Initialize(keyPath, nil)
		if err != nil {
			return err
		}
		return a.writeIdentity(material, false)
	case "show":
		f := flags("identity show")
		asJSON := f.Bool("json", false, "write JSON")
		if err := f.Parse(args[1:]); err != nil {
			return err
		}
		if len(f.Args()) != 0 {
			return errors.New("identity show takes flags only")
		}
		material, err := identity.Load(keyPath)
		if err != nil {
			return err
		}
		return a.writeIdentity(material, *asJSON)
	case "export":
		if len(args) != 2 {
			return errors.New("identity export needs one backup path")
		}
		material, err := identity.Load(keyPath)
		if err != nil {
			return err
		}
		password, err := a.password("Backup password: ")
		if err != nil {
			return err
		}
		defer clear(password)
		confirm, err := a.password("Confirm backup password: ")
		if err != nil {
			return err
		}
		defer clear(confirm)
		if !bytes.Equal(password, confirm) {
			return errors.New("backup passwords do not match")
		}
		return identity.WriteBackup(args[1], material, password, nil)
	case "import":
		if len(args) != 2 {
			return errors.New("identity import needs one backup path")
		}
		if _, err := identity.Load(keyPath); err == nil {
			return identity.ErrAlreadyExists
		} else if !errors.Is(err, identity.ErrNotInitialized) {
			return err
		}
		password, err := a.password("Backup password: ")
		if err != nil {
			return err
		}
		defer clear(password)
		material, err := identity.ReadBackup(args[1], password)
		if err != nil {
			return err
		}
		if err := identity.WriteNew(keyPath, material); err != nil {
			return err
		}
		return a.writeIdentity(material, false)
	case "reset":
		f := flags("identity reset")
		yes := f.Bool("yes", false, "confirm destructive reset")
		if err := f.Parse(args[1:]); err != nil {
			return err
		}
		if len(f.Args()) != 0 || !*yes {
			return errors.New("identity reset deletes the identity and database; pass --yes to confirm")
		}
		return identity.Reset(resolved, keyPath)
	default:
		return fmt.Errorf("unknown identity command %q", args[0])
	}
}

func (a *App) writeIdentity(material identity.Material, asJSON bool) error {
	npub, err := material.NPub()
	if err != nil {
		return err
	}
	value := struct {
		InstallationID string `json:"installation_id"`
		PublicKey      string `json:"public_key"`
		NPub           string `json:"npub"`
	}{material.InstallationID, material.PublicKey(), npub}
	if asJSON {
		return writeJSON(a.Out, value)
	}
	_, err = fmt.Fprintf(a.Out, "installation: %s\npublic key: %s\nnpub: %s\n", value.InstallationID, value.PublicKey, value.NPub)
	return err
}

type stringList []string

func (s *stringList) String() string         { return strings.Join(*s, ",") }
func (s *stringList) Set(value string) error { *s = append(*s, value); return nil }

func (a *App) peer(ctx context.Context, s domain.Store, args []string) error {
	if len(args) == 0 {
		return errors.New("peer needs add, list, or distrust")
	}
	switch args[0] {
	case "add":
		f := flags("peer add")
		name := f.String("name", "", "local peer name")
		var relays stringList
		f.Var(&relays, "relay", "relay hint; may be repeated")
		if err := f.Parse(args[1:]); err != nil {
			return err
		}
		if len(f.Args()) != 2 {
			return errors.New("peer add needs INSTALLATION_ID NPUB")
		}
		public, err := identity.DecodePublicKey(f.Args()[1])
		if err != nil {
			return err
		}
		return s.TrustPeer(ctx, domain.Peer{InstallationID: f.Args()[0], SignerKeyID: public, Name: *name, Relays: relays, Trusted: true})
	case "list":
		f := flags("peer list")
		asJSON := f.Bool("json", false, "write JSON")
		if err := f.Parse(args[1:]); err != nil {
			return err
		}
		if len(f.Args()) != 0 {
			return errors.New("peer list takes flags only")
		}
		peers, err := s.ListPeers(ctx)
		if err != nil {
			return err
		}
		if *asJSON {
			return writeJSON(a.Out, peers)
		}
		var b bytes.Buffer
		for _, peer := range peers {
			state := "distrusted"
			if peer.Trusted {
				state = "trusted"
			}
			fmt.Fprintf(&b, "%s\t%s\t%s\n", peer.InstallationID, state, peer.Name)
		}
		return writeOnce(a.Out, b.Bytes())
	case "distrust":
		if len(args) != 2 {
			return errors.New("peer distrust needs INSTALLATION_ID")
		}
		return s.DistrustPeer(ctx, args[1])
	default:
		return fmt.Errorf("unknown peer command %q", args[0])
	}
}

func (a *App) human(ctx context.Context, s domain.Store, args []string) error {
	if len(args) == 0 {
		return errors.New("human needs show, invite, join, devices, or revoke")
	}
	switch args[0] {
	case "show":
		f := flags("human show")
		asJSON := f.Bool("json", false, "write JSON")
		if err := f.Parse(args[1:]); err != nil {
			return err
		}
		if len(f.Args()) != 0 {
			return errors.New("human show takes flags only")
		}
		account, err := s.HumanAccount(ctx)
		if err != nil {
			return err
		}
		if *asJSON {
			return writeJSON(a.Out, account)
		}
		_, err = fmt.Fprintf(a.Out, "account: %s\nlabel: %s\ncreator: %s\nlocal installation: %s\n", account.ID, account.Label, account.CreatorInstallationID, account.LocalInstallationID)
		return err
	case "devices":
		f := flags("human devices")
		asJSON := f.Bool("json", false, "write JSON")
		if err := f.Parse(args[1:]); err != nil {
			return err
		}
		if len(f.Args()) != 0 {
			return errors.New("human devices takes flags only")
		}
		devices, err := s.HumanDevices(ctx)
		if err != nil {
			return err
		}
		if *asJSON {
			return writeJSON(a.Out, devices)
		}
		var output bytes.Buffer
		for _, device := range devices {
			fmt.Fprintf(&output, "%s\t%s\t%s\n", device.InstallationID, device.State, device.Label)
		}
		return writeOnce(a.Out, output.Bytes())
	case "invite":
		f := flags("human invite")
		name := f.String("name", "", "signed device display name")
		var relays stringList
		f.Var(&relays, "relay", "target relay hint; may be repeated")
		if err := f.Parse(args[1:]); err != nil {
			return err
		}
		if len(f.Args()) != 2 {
			return errors.New("human invite needs INSTALLATION_ID NPUB")
		}
		public, err := identity.DecodePublicKey(f.Args()[1])
		if err != nil {
			return err
		}
		if strings.TrimSpace(*name) == "" {
			hostname := a.Hostname
			if hostname == nil {
				hostname = os.Hostname
			}
			*name, err = hostname()
			if err != nil || strings.TrimSpace(*name) == "" {
				return errors.New("--name is required when the hostname is unavailable")
			}
		}
		bundle, err := s.CreateHumanInvite(ctx, domain.HumanInviteRequest{InstallationID: f.Args()[0], SignerKeyID: public, Name: *name, Relays: relays})
		if err != nil {
			return err
		}
		return writeJSON(a.Out, bundle)
	case "join":
		if len(args) != 2 {
			return errors.New("human join needs FILE")
		}
		var raw []byte
		var err error
		if args[1] == "-" {
			raw, err = io.ReadAll(a.In)
		} else {
			raw, err = os.ReadFile(args[1])
		}
		if err != nil {
			return fmt.Errorf("read pairing invite: %w", err)
		}
		return s.JoinHumanInvite(ctx, raw)
	case "revoke":
		if len(args) != 2 {
			return errors.New("human revoke needs INSTALLATION_ID")
		}
		return s.RevokeHumanDevice(ctx, args[1])
	default:
		return fmt.Errorf("unknown human command %q", args[0])
	}
}

func (a *App) mailbox(ctx context.Context, s domain.Store, args []string) error {
	if len(args) != 3 || (args[0] != "share" && args[0] != "revoke") {
		return errors.New("mailbox needs share or revoke, MAILBOX_ID, and PEER_INSTALLATION_ID")
	}
	return s.SetMailboxShare(ctx, args[1], args[2], args[0] == "share")
}

func (a *App) agent(ctx context.Context, s domain.Store, args []string) error {
	if len(args) == 0 {
		return errors.New("agent needs create, list, or retire")
	}
	switch args[0] {
	case "create":
		f := flags("agent create")
		mailboxID := f.String("mailbox", "", "adopt an existing local unnamed agent mailbox")
		arguments := flagsAfterName(args[1:])
		if err := f.Parse(arguments); err != nil {
			return err
		}
		if len(f.Args()) != 1 {
			return errors.New("agent create needs NAME")
		}
		agent, err := s.CreateNamedAgent(ctx, f.Args()[0], *mailboxID)
		if err != nil {
			return err
		}
		_, err = fmt.Fprintf(a.Out, "%s\t%s\n", agent.Name, agent.MailboxID)
		return err
	case "list":
		f := flags("agent list")
		jsonOutput := f.Bool("json", false, "write JSON")
		if err := f.Parse(args[1:]); err != nil {
			return err
		}
		if len(f.Args()) != 0 {
			return errors.New("agent list takes flags only")
		}
		agents, err := s.ListNamedAgents(ctx)
		if err != nil {
			return err
		}
		if *jsonOutput {
			return writeJSON(a.Out, agents)
		}
		for _, agent := range agents {
			presence := "offline"
			if agent.Active {
				presence = "active"
			}
			if agent.Retired {
				presence = "retired"
			}
			session := "-"
			if agent.CurrentSessionID != "" {
				session = agent.Harness + ":" + agent.CurrentSessionID
			}
			lastActive := "-"
			if agent.LastActiveAt != nil {
				lastActive = agent.LastActiveAt.Format(time.RFC3339)
			}
			if _, err := fmt.Fprintf(a.Out, "%s\t%s\t%s\t%s\t%s\t%s\n", agent.Name, presence, agent.MailboxID, session, lastActive, agent.Context.Directory); err != nil {
				return err
			}
		}
		return nil
	case "retire":
		f := flags("agent retire")
		yes := f.Bool("yes", false, "confirm permanent retirement")
		if err := f.Parse(flagsAfterName(args[1:])); err != nil {
			return err
		}
		if len(f.Args()) != 1 {
			return errors.New("agent retire needs NAME")
		}
		if !*yes {
			return errors.New("agent retirement is permanent; pass --yes to confirm")
		}
		return s.RetireNamedAgent(ctx, f.Args()[0])
	default:
		return fmt.Errorf("unknown agent command %q", args[0])
	}
}

func flagsAfterName(arguments []string) []string {
	if len(arguments) > 1 && !strings.HasPrefix(arguments[0], "-") {
		result := append([]string(nil), arguments[1:]...)
		return append(result, arguments[0])
	}
	return arguments
}

func (a *App) relay(ctx context.Context, s domain.Store, args []string) error {
	if len(args) == 0 {
		return errors.New("relay needs add, list, or remove")
	}
	switch args[0] {
	case "add":
		f := flags("relay add")
		read := f.Bool("read", true, "read the installation inbox")
		write := f.Bool("write", true, "allow writes")
		unsafeNoAuth := f.Bool("unsafe-no-auth", false, "allow private reads without NIP-42")
		if err := f.Parse(args[1:]); err != nil {
			return err
		}
		if len(f.Args()) != 1 {
			return errors.New("relay add needs one WebSocket URL")
		}
		return s.AddRelay(ctx, domain.RelayConfig{URL: f.Args()[0], Read: *read, Write: *write, RequireAuth: *read && !*unsafeNoAuth, UnsafeNoAuth: *unsafeNoAuth})
	case "list":
		f := flags("relay list")
		asJSON := f.Bool("json", false, "write JSON")
		if err := f.Parse(args[1:]); err != nil {
			return err
		}
		if len(f.Args()) != 0 {
			return errors.New("relay list takes flags only")
		}
		relays, err := s.ListRelays(ctx)
		if err != nil {
			return err
		}
		if *asJSON {
			return writeJSON(a.Out, relays)
		}
		var output bytes.Buffer
		for _, relay := range relays {
			fmt.Fprintf(&output, "%s\tread=%t\twrite=%t\tauth=%t\n", relay.URL, relay.Read, relay.Write, relay.RequireAuth)
		}
		return writeOnce(a.Out, output.Bytes())
	case "remove":
		if len(args) != 2 {
			return errors.New("relay remove needs one WebSocket URL")
		}
		return s.RemoveRelay(ctx, args[1])
	default:
		return fmt.Errorf("unknown relay command %q", args[0])
	}
}

func (a *App) status(ctx context.Context, s domain.Store, args []string) error {
	f := flags("status")
	asJSON := f.Bool("json", false, "write JSON")
	if err := f.Parse(args); err != nil {
		return err
	}
	if len(f.Args()) != 0 {
		return errors.New("status takes flags only")
	}
	status, err := s.NetworkStatus(ctx)
	if err != nil {
		return err
	}
	if *asJSON {
		return writeJSON(a.Out, status)
	}
	var output bytes.Buffer
	fmt.Fprintf(&output, "queued=%d relay_accepted=%d rejected=%d unresolved=%d unsupported=%d staged=%d quarantined=%d account_members=%d pending_account_fanout=%d invalid_account_traffic=%d revoked_device_traffic=%d\n", status.Queued, status.RelayAccepted, status.Rejected, status.Unresolved, status.Unsupported, status.Staged, status.Quarantined, status.AccountMembers, status.PendingAccountFanout, status.InvalidAccountTraffic, status.RevokedDeviceTraffic)
	for _, relay := range status.Relays {
		fmt.Fprintf(&output, "%s\tconnected=%t\tauth=%t", relay.URL, relay.Connected, relay.Authenticated)
		if relay.LastEvent != nil {
			fmt.Fprintf(&output, "\tlast_receive=%s", relay.LastEvent.Format(time.RFC3339))
		}
		if relay.LastError != "" {
			fmt.Fprintf(&output, "\terror=%s", relay.LastError)
		}
		output.WriteByte('\n')
	}
	return writeOnce(a.Out, output.Bytes())
}

func globalArgs(args []string, path string) (string, bool, []string, error) {
	noSync := false
	for len(args) > 0 {
		switch {
		case args[0] == "--no-sync":
			noSync, args = true, args[1:]
		case args[0] == "--db" && len(args) >= 2:
			path, args = args[1], args[2:]
		case strings.HasPrefix(args[0], "--db="):
			path, args = strings.TrimPrefix(args[0], "--db="), args[1:]
		case args[0] == "--db":
			return "", false, nil, errors.New("--db needs a path")
		default:
			return path, noSync, args, nil
		}
	}
	return path, noSync, args, nil
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

func (a *App) resolveMailbox(ctx context.Context, s domain.Store, explicit, directory string) (model.Mailbox, model.RepositoryContext, error) {
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

type questionOptions struct {
	sessionID  string
	directory  string
	details    string
	message    string
	jsonOutput bool
	timeout    time.Duration
	interval   time.Duration
}

func (a *App) questionOptions(command string, args []string, waits bool) (questionOptions, error) {
	f := flags(command)
	var options questionOptions
	f.StringVar(&options.sessionID, "session", "", "advanced session override")
	f.StringVar(&options.directory, "dir", "", "work directory scope")
	f.StringVar(&options.details, "details", "", "extra context")
	f.BoolVar(&options.jsonOutput, "json", false, "write JSON")
	if waits {
		f.DurationVar(&options.timeout, "timeout", 0, "maximum wait time")
		f.DurationVar(&options.interval, "interval", replyRepairInterval, "repair interval")
	}
	if err := f.Parse(args); err != nil {
		return questionOptions{}, err
	}
	options.message = strings.TrimSpace(strings.Join(f.Args(), " "))
	if options.message == "" {
		data, err := io.ReadAll(a.In)
		if err != nil {
			return questionOptions{}, fmt.Errorf("read message: %w", err)
		}
		options.message = strings.TrimSpace(string(data))
	}
	if options.message == "" {
		return questionOptions{}, errors.New("message text is required")
	}
	if waits && options.interval <= 0 {
		return questionOptions{}, errors.New("--interval must be positive")
	}
	if options.timeout < 0 {
		return questionOptions{}, errors.New("--timeout must not be negative")
	}
	return options, nil
}

func (a *App) createQuestion(ctx context.Context, s domain.Store, options questionOptions) (model.Message, error) {
	mailbox, repo, err := a.resolveMailbox(ctx, s, options.sessionID, options.directory)
	if err != nil {
		return model.Message{}, err
	}
	human, err := s.HumanMailbox(ctx)
	if err != nil {
		return model.Message{}, err
	}
	id, err := uuid.NewV7()
	if err != nil {
		return model.Message{}, fmt.Errorf("make message ID: %w", err)
	}
	m := model.Message{ID: id.String(), Context: repo, SenderMailboxID: mailbox.ID, RecipientMailboxID: human.ID, SenderLabel: mailbox.Label, RecipientLabel: human.Label, Body: options.message, Details: options.details, CreatedAt: time.Now().UTC()}
	if err := s.Create(ctx, m); err != nil {
		return model.Message{}, err
	}
	return m, nil
}

func (a *App) send(ctx context.Context, s domain.Store, args []string) error {
	options, err := a.questionOptions("send", args, false)
	if err != nil {
		return err
	}
	m, err := a.createQuestion(ctx, s, options)
	if err != nil {
		return err
	}
	if options.jsonOutput {
		stored, err := s.Get(ctx, m.ID)
		if err != nil {
			return err
		}
		return writeJSON(a.Out, stored)
	}
	return writeOnce(a.Out, []byte(m.ID+"\n"))
}

func (a *App) ask(ctx context.Context, s domain.Store, args []string, noSync bool) error {
	options, err := a.questionOptions("ask", args, true)
	if err != nil {
		return err
	}
	m, err := a.createQuestion(ctx, s, options)
	if err != nil {
		return err
	}
	nextSync := time.Time{}
	if !noSync {
		a.trySync(ctx, s, false, "message saved")
		nextSync = time.Now().Add(5 * time.Second)
	}
	err = a.waitForReply(ctx, s, m.ID, options.sessionID, options.directory, options.jsonOutput, options.timeout, options.interval, noSync, nextSync)
	if err != nil {
		return fmt.Errorf("ask %s: %w", m.ID, err)
	}
	return nil
}

func (a *App) wait(ctx context.Context, s domain.Store, args []string, noSync bool) error {
	f := flags("wait")
	timeout := f.Duration("timeout", 0, "maximum wait time")
	sessionID := f.String("session", "", "advanced session override")
	directory := f.String("dir", "", "work directory context")
	jsonOutput := f.Bool("json", false, "write JSON")
	interval := f.Duration("interval", replyRepairInterval, "repair interval")
	if err := f.Parse(args); err != nil {
		return err
	}
	if len(f.Args()) != 1 {
		return errors.New("wait needs one message ID")
	}
	if *interval <= 0 {
		return errors.New("--interval must be positive")
	}
	if *timeout < 0 {
		return errors.New("--timeout must not be negative")
	}
	return a.waitForReply(ctx, s, f.Args()[0], *sessionID, *directory, *jsonOutput, *timeout, *interval, noSync, time.Time{})
}

func (a *App) waitForReply(ctx context.Context, s domain.Store, id, sessionID, directory string, jsonOutput bool, timeout, interval time.Duration, noSync bool, nextSync time.Time) error {
	if timeout > 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, timeout)
		defer cancel()
	}
	mailbox, _, err := a.resolveMailbox(ctx, s, sessionID, directory)
	if err != nil {
		return err
	}
	var subscription domain.ChangeSubscription
	if subscribe := clientUpdates(s).Subscribe; subscribe != nil {
		subscription, err = subscribe(ctx, domain.TopicMessages, domain.TopicMailboxes)
		if err != nil {
			return fmt.Errorf("subscribe while waiting for reply: %w", err)
		}
		defer subscription.Close()
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
		if !noSync && !time.Now().Before(nextSync) {
			a.trySync(ctx, s, false, "")
			nextSync = time.Now().Add(5 * time.Second)
		}
		token := uuid.NewString()
		m, err := s.Claim(ctx, domain.Claim{ReplyTo: id, RecipientMailboxID: mailbox.ID}, token)
		if err == nil {
			if err := deliver(a.Out, m, jsonOutput); err != nil {
				_ = s.Release(context.Background(), m.ID, token)
				return err
			}
			return s.Complete(context.Background(), m.ID, token)
		}
		if !errors.Is(err, domain.ErrNotReady) && !errors.Is(err, domain.ErrClaimed) {
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
			// The reply/archive transaction can commit between the preceding List and Get.
			// Re-read the reply side before classifying the archive as cancellation.
			replies, listErr = s.List(ctx, model.Filter{ReplyTo: id, RecipientMailboxID: mailbox.ID, Limit: 1})
			if listErr != nil {
				return listErr
			}
			if len(replies) == 0 {
				return errors.New("message was archived without a reply")
			}
			continue
		}
		timer := time.NewTimer(interval)
		var changes <-chan domain.Invalidation
		if subscription != nil {
			changes = subscription.Changes()
		}
		select {
		case <-ctx.Done():
			timer.Stop()
			return ctx.Err()
		case <-changes:
			if !timer.Stop() {
				select {
				case <-timer.C:
				default:
				}
			}
		case <-timer.C:
		}
	}
}

func (a *App) poll(ctx context.Context, s domain.Store, args []string) error {
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
		m, err := s.Claim(ctx, domain.Claim{MessageID: candidate.ID, UnthreadedOnly: true}, token)
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
			prefix := ""
			if c.m.Incomplete {
				prefix = "[incomplete causal history] "
			}
			fmt.Fprintf(&b, "%s\t%s%s\n", c.m.ID, prefix, c.m.Body)
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

func (a *App) get(ctx context.Context, s domain.Store, args []string) error {
	if len(args) != 1 {
		return errors.New("get needs one message ID")
	}
	m, err := s.Get(ctx, args[0])
	if err != nil {
		return err
	}
	return writeJSON(a.Out, m)
}

func (a *App) list(ctx context.Context, s domain.Store, args []string) error {
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

func (a *App) answer(ctx context.Context, s domain.Store, args []string) error {
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

func (a *App) mailboxes(ctx context.Context, s domain.Store, args []string) error {
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

func (a *App) cancel(ctx context.Context, s domain.Store, args []string) error {
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
