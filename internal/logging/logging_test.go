package logging

import (
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestOpenWritesProtectedStructuredDebugLog(t *testing.T) {
	path := filepath.Join(t.TempDir(), "logs", "hq.log")
	logger, closer, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	logger.Debug("daemon starting", "database", "/data/hq.db")
	if err := closer.Close(); err != nil {
		t.Fatal(err)
	}
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if text := string(raw); !strings.Contains(text, "level=DEBUG") || !strings.Contains(text, `msg="daemon starting"`) || !strings.Contains(text, "database=/data/hq.db") {
		t.Fatalf("log = %q", text)
	}
	for target, want := range map[string]os.FileMode{filepath.Dir(path): 0o700, path: 0o600} {
		info, err := os.Stat(target)
		if err != nil {
			t.Fatal(err)
		}
		if info.Mode().Perm() != want {
			t.Fatalf("mode for %s = %v; want %v", target, info.Mode().Perm(), want)
		}
	}
}

func TestLineWriterBuffersFragmentsAndLogsCompleteLines(t *testing.T) {
	path := filepath.Join(t.TempDir(), "hq.log")
	logger, closer, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	writer := NewLineWriter(logger, slog.LevelWarn, "subprocess stderr")
	_, _ = writer.Write([]byte("first fragment"))
	_, _ = writer.Write([]byte(" complete\nsecond\n"))
	if err := closer.Close(); err != nil {
		t.Fatal(err)
	}
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	text := string(raw)
	if strings.Count(text, `msg="subprocess stderr"`) != 2 || !strings.Contains(text, `line="first fragment complete"`) || !strings.Contains(text, "line=second") {
		t.Fatalf("log = %q", text)
	}
}
