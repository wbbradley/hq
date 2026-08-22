package config

import (
	"os"
	"path/filepath"
	"testing"
)

func TestSettingsRoundTripAndMissingDefault(t *testing.T) {
	root := t.TempDir()
	t.Setenv("XDG_CONFIG_HOME", root)
	settings, err := Load()
	if err != nil || settings.Codex.Yolo {
		t.Fatalf("missing settings = %#v, %v", settings, err)
	}
	settings.Codex.Yolo = true
	if err := Save(settings); err != nil {
		t.Fatal(err)
	}
	loaded, err := Load()
	if err != nil || !loaded.Codex.Yolo {
		t.Fatalf("loaded settings = %#v, %v", loaded, err)
	}
	info, err := os.Stat(filepath.Join(root, "hq", fileName))
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Fatalf("config mode = %o", info.Mode().Perm())
	}
}
