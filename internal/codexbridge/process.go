package codexbridge

import (
	"bufio"
	"fmt"
	"io"
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
	Path string
}

func (s ExecStarter) Start(directory string) (Process, error) {
	path := strings.TrimSpace(s.Path)
	if path == "" {
		path = "codex"
	}
	command := exec.Command(path, "app-server", "--stdio")
	command.Dir = directory
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
	return &execProcess{command: command, input: input, output: output, errors: errorsPipe}, nil
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
