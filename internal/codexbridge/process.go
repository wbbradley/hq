package codexbridge

import (
	"bufio"
	"fmt"
	"io"
	"log/slog"
	"os"
	"os/exec"
	"strings"
	"time"
)

type Process interface {
	Input() io.WriteCloser
	Output() io.ReadCloser
	Errors() io.ReadCloser
	Wait() error
	Kill() error
}

type ProcessStarter interface {
	Start(directory string) (Process, error)
}

type ExecStarter struct {
	Path           string
	Environment    []string
	UseEnvironment bool
	Logger         *slog.Logger
}

func (s *ExecStarter) Start(directory string) (Process, error) {
	defer func() {
		for index := range s.Environment {
			s.Environment[index] = ""
		}
		s.Environment = nil
	}()
	path := strings.TrimSpace(s.Path)
	if path == "" {
		path = "codex"
	}
	arguments := s.arguments()
	logger := s.Logger
	if logger == nil {
		logger = slog.New(slog.DiscardHandler)
	}
	logger.Info("starting Codex app-server process", "executable", path, "arguments", arguments, "directory", directory, "environment_variables", len(s.Environment))
	command := exec.Command(path, arguments...)
	command.Dir = directory
	if s.UseEnvironment {
		command.Env = make([]string, len(s.Environment))
		copy(command.Env, s.Environment)
	} else {
		command.Env = os.Environ()
	}
	input, err := command.StdinPipe()
	if err != nil {
		return nil, fmt.Errorf("open Codex app-server stdin: %w", err)
	}
	output, err := command.StdoutPipe()
	if err != nil {
		return nil, fmt.Errorf("open Codex app-server stdout: %w", err)
	}
	errorsPipe, err := command.StderrPipe()
	if err != nil {
		return nil, fmt.Errorf("open Codex app-server stderr: %w", err)
	}
	if err := command.Start(); err != nil {
		logger.Error("start Codex app-server process", "executable", path, "directory", directory, "error", err)
		return nil, fmt.Errorf("start Codex app-server: %w", err)
	}
	logger.Info("Codex app-server process started", "pid", command.Process.Pid, "directory", directory)
	// The child has received its environment. Do not retain it in the process
	// wrapper, where diagnostics or later inspection could expose credentials.
	command.Env = nil
	return &execProcess{command: command, input: input, output: output, errors: errorsPipe, logger: logger, startedAt: time.Now()}, nil
}

func (s ExecStarter) arguments() []string {
	return []string{"app-server", "--stdio"}
}

type execProcess struct {
	command   *exec.Cmd
	input     io.WriteCloser
	output    io.ReadCloser
	errors    io.ReadCloser
	logger    *slog.Logger
	startedAt time.Time
}

func (p *execProcess) Input() io.WriteCloser { return p.input }
func (p *execProcess) Output() io.ReadCloser { return p.output }
func (p *execProcess) Errors() io.ReadCloser { return p.errors }
func (p *execProcess) Wait() error {
	err := p.command.Wait()
	attributes := []any{"pid", p.command.Process.Pid, "duration", time.Since(p.startedAt)}
	if state := p.command.ProcessState; state != nil {
		attributes = append(attributes, "exit_code", state.ExitCode(), "process_state", state.String(), "user_time", state.UserTime(), "system_time", state.SystemTime())
	}
	if err != nil {
		p.logger.Error("Codex app-server process exited", append(attributes, "error", err)...)
	} else {
		p.logger.Warn("Codex app-server process exited", attributes...)
	}
	return err
}
func (p *execProcess) Kill() error {
	p.logger.Warn("killing Codex app-server process", "pid", p.command.Process.Pid)
	err := p.command.Process.Kill()
	if err != nil {
		p.logger.Error("kill Codex app-server process", "pid", p.command.Process.Pid, "error", err)
	}
	return err
}

func forwardStderr(destination io.Writer, source io.Reader) error {
	if destination == nil {
		destination = io.Discard
	}
	scanner := bufio.NewScanner(source)
	scanner.Buffer(make([]byte, 64<<10), defaultMaximumFrameBytes)
	for scanner.Scan() {
		if _, err := fmt.Fprintf(destination, "hq codex: app-server: %s\n", scanner.Text()); err != nil {
			return err
		}
	}
	return scanner.Err()
}
