package codexbridge

import (
	"bufio"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"
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
	Yolo           bool
	Environment    []string
	UseEnvironment bool
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
	command := exec.Command(path, s.arguments()...)
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
		return nil, fmt.Errorf("start Codex app-server: %w", err)
	}
	// The child has received its environment. Do not retain it in the process
	// wrapper, where diagnostics or later inspection could expose credentials.
	command.Env = nil
	return &execProcess{command: command, input: input, output: output, errors: errorsPipe}, nil
}

func (s ExecStarter) arguments() []string {
	arguments := make([]string, 0, 3)
	if s.Yolo {
		arguments = append(arguments, "--yolo")
	}
	return append(arguments, "app-server", "--stdio")
}

type execProcess struct {
	command *exec.Cmd
	input   io.WriteCloser
	output  io.ReadCloser
	errors  io.ReadCloser
}

func (p *execProcess) Input() io.WriteCloser { return p.input }
func (p *execProcess) Output() io.ReadCloser { return p.output }
func (p *execProcess) Errors() io.ReadCloser { return p.errors }
func (p *execProcess) Wait() error           { return p.command.Wait() }
func (p *execProcess) Kill() error           { return p.command.Process.Kill() }

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
