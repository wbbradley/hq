package syncer

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"time"

	"github.com/wbbradley/hq/internal/localwire"
)

type InstanceMetadata struct {
	PID        int       `json:"pid"`
	Build      string    `json:"build"`
	InstanceID string    `json:"instance_id"`
	StartedAt  time.Time `json:"started_at"`
	Database   string    `json:"database"`
	Socket     string    `json:"socket"`
}

func writeRuntimeMetadata(paths RuntimePaths, peer localwire.PeerMetadata) error {
	metadata := InstanceMetadata{
		PID: os.Getpid(), Build: peer.Build, InstanceID: peer.InstanceID, StartedAt: peer.StartedAt,
		Database: paths.Database, Socket: paths.Socket,
	}
	raw, err := json.Marshal(metadata)
	if err != nil {
		return err
	}
	raw = append(raw, '\n')
	if err := writeRuntimeFile(paths.PID, []byte(strconv.Itoa(metadata.PID)+"\n")); err != nil {
		return fmt.Errorf("write node PID: %w", err)
	}
	if err := writeRuntimeFile(paths.InstanceMetadata, raw); err != nil {
		_ = os.Remove(paths.PID)
		return fmt.Errorf("write node metadata: %w", err)
	}
	return nil
}

func ReadInstanceMetadata(paths RuntimePaths) (InstanceMetadata, error) {
	raw, err := os.ReadFile(paths.InstanceMetadata)
	if err != nil {
		return InstanceMetadata{}, err
	}
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	var metadata InstanceMetadata
	if err := decoder.Decode(&metadata); err != nil {
		return InstanceMetadata{}, fmt.Errorf("decode node metadata: %w", err)
	}
	if metadata.PID <= 0 || metadata.InstanceID == "" || metadata.Database != paths.Database || metadata.Socket != paths.Socket || metadata.StartedAt.IsZero() {
		return InstanceMetadata{}, errors.New("node metadata is invalid")
	}
	return metadata, nil
}

func removeRuntimeMetadata(paths RuntimePaths, instanceID string) error {
	metadata, err := ReadInstanceMetadata(paths)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	if metadata.InstanceID != instanceID {
		return nil
	}
	var removeErr error
	for _, path := range []string{paths.PID, paths.InstanceMetadata} {
		if err := os.Remove(path); err != nil && !errors.Is(err, os.ErrNotExist) {
			removeErr = errors.Join(removeErr, err)
		}
	}
	return removeErr
}

func writeRuntimeFile(path string, raw []byte) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	file, err := os.CreateTemp(filepath.Dir(path), ".hq-runtime-*")
	if err != nil {
		return err
	}
	temporary := file.Name()
	remove := true
	defer func() {
		_ = file.Close()
		if remove {
			_ = os.Remove(temporary)
		}
	}()
	if err := file.Chmod(0o600); err != nil {
		return err
	}
	if _, err := file.Write(raw); err != nil {
		return err
	}
	if err := file.Sync(); err != nil {
		return err
	}
	if err := file.Close(); err != nil {
		return err
	}
	if err := os.Rename(temporary, path); err != nil {
		return err
	}
	remove = false
	return nil
}
