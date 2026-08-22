package config

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"github.com/wbbradley/hq/internal/identity"
)

const fileName = "config.json"

// Settings contains local, non-secret user preferences. These settings are
// installation-independent and are not synchronized through HQ.
type Settings struct {
	Codex CodexSettings `json:"codex"`
}

type CodexSettings struct {
	Yolo bool `json:"yolo"`
}

func Path() (string, error) {
	directory, err := identity.DefaultConfigDirectory()
	if err != nil {
		return "", err
	}
	return filepath.Join(directory, fileName), nil
}

func Load() (Settings, error) {
	path, err := Path()
	if err != nil {
		return Settings{}, err
	}
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return Settings{}, nil
	}
	if err != nil {
		return Settings{}, fmt.Errorf("read HQ config: %w", err)
	}
	var settings Settings
	if err := json.Unmarshal(data, &settings); err != nil {
		return Settings{}, fmt.Errorf("parse HQ config %s: %w", path, err)
	}
	return settings, nil
}

func Save(settings Settings) error {
	path, err := Path()
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return fmt.Errorf("create HQ config directory: %w", err)
	}
	data, err := json.MarshalIndent(settings, "", "  ")
	if err != nil {
		return fmt.Errorf("encode HQ config: %w", err)
	}
	data = append(data, '\n')
	temporary, err := os.CreateTemp(filepath.Dir(path), ".hq-config-*")
	if err != nil {
		return fmt.Errorf("create temporary HQ config: %w", err)
	}
	temporaryPath := temporary.Name()
	defer os.Remove(temporaryPath)
	if err := temporary.Chmod(0o600); err != nil {
		temporary.Close()
		return fmt.Errorf("secure temporary HQ config: %w", err)
	}
	if _, err := temporary.Write(data); err != nil {
		temporary.Close()
		return fmt.Errorf("write HQ config: %w", err)
	}
	if err := temporary.Sync(); err != nil {
		temporary.Close()
		return fmt.Errorf("sync HQ config: %w", err)
	}
	if err := temporary.Close(); err != nil {
		return fmt.Errorf("close HQ config: %w", err)
	}
	if err := os.Rename(temporaryPath, path); err != nil {
		return fmt.Errorf("replace HQ config: %w", err)
	}
	return nil
}
