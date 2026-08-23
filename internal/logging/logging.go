// Package logging configures HQ's process-wide structured diagnostic log.
package logging

import (
	"context"
	"errors"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"sync"
)

// Open creates a debug-level structured text logger backed by a protected file.
func Open(path string) (*slog.Logger, io.Closer, error) {
	if strings.TrimSpace(path) == "" {
		return nil, nil, errors.New("HQ log path is required")
	}
	directory := filepath.Dir(path)
	if err := os.MkdirAll(directory, 0o700); err != nil {
		return nil, nil, err
	}
	file, err := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		return nil, nil, err
	}
	if err := file.Chmod(0o600); err != nil {
		_ = file.Close()
		return nil, nil, err
	}
	handler := slog.NewTextHandler(file, &slog.HandlerOptions{Level: slog.LevelDebug})
	return slog.New(handler), file, nil
}

// LineWriter turns newline-delimited subprocess diagnostics into structured records.
// It is safe for concurrent use and never retains complete environment blocks.
type LineWriter struct {
	logger  *slog.Logger
	level   slog.Level
	message string
	mu      sync.Mutex
	buffer  strings.Builder
}

func NewLineWriter(logger *slog.Logger, level slog.Level, message string) *LineWriter {
	if logger == nil {
		logger = slog.New(slog.DiscardHandler)
	}
	return &LineWriter{logger: logger, level: level, message: message}
}

func (w *LineWriter) Write(data []byte) (int, error) {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.buffer.Write(data)
	value := w.buffer.String()
	for {
		newline := strings.IndexByte(value, '\n')
		if newline < 0 {
			break
		}
		line := strings.TrimSuffix(value[:newline], "\r")
		if strings.TrimSpace(line) != "" {
			w.logger.Log(context.Background(), w.level, w.message, "line", line)
		}
		value = value[newline+1:]
	}
	w.buffer.Reset()
	w.buffer.WriteString(value)
	return len(data), nil
}
