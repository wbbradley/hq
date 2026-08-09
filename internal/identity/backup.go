package identity

import (
	"crypto/rand"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"

	"github.com/google/uuid"
)

type backupFile struct {
	Version        int    `json:"version"`
	InstallationID string `json:"installation_id"`
	EncryptedKey   string `json:"encrypted_key"`
}

func WriteBackup(path string, material Material, password []byte, random io.Reader) error {
	if len(password) == 0 {
		return errors.New("backup password is required")
	}
	if random == nil {
		random = rand.Reader
	}
	encrypted, err := EncryptNIP49(material.SecretKey, password, NIP49DefaultLogN, random)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return fmt.Errorf("create backup directory: %w", err)
	}
	file, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if err != nil {
		return fmt.Errorf("create backup: %w", err)
	}
	remove := true
	defer func() {
		_ = file.Close()
		if remove {
			_ = os.Remove(path)
		}
	}()
	if err := json.NewEncoder(file).Encode(backupFile{Version: 1, InstallationID: material.InstallationID, EncryptedKey: encrypted}); err != nil {
		return errors.New("write encrypted backup")
	}
	if err := file.Sync(); err != nil {
		return fmt.Errorf("sync encrypted backup: %w", err)
	}
	if err := file.Close(); err != nil {
		return fmt.Errorf("close encrypted backup: %w", err)
	}
	remove = false
	return nil
}

func ReadBackup(path string, password []byte) (Material, error) {
	file, err := os.Open(path)
	if err != nil {
		return Material{}, fmt.Errorf("open backup: %w", err)
	}
	defer file.Close()
	var backup backupFile
	decoder := json.NewDecoder(io.LimitReader(file, 4097))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&backup); err != nil || backup.Version != 1 {
		return Material{}, errors.New("encrypted backup is corrupt or unsupported")
	}
	parsed, err := uuid.Parse(backup.InstallationID)
	if err != nil || parsed.String() != backup.InstallationID {
		return Material{}, errors.New("encrypted backup has an invalid installation ID")
	}
	secret, err := DecryptNIP49(backup.EncryptedKey, password)
	if err != nil {
		return Material{}, err
	}
	return Material{InstallationID: backup.InstallationID, SecretKey: secret}, nil
}
