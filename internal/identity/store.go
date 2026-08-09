package identity

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/btcsuite/btcd/btcutil/bech32"
	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/event"
)

const keyFileVersion = 1

var (
	ErrNotInitialized = errors.New("HQ identity is not initialized; run `hq identity init`")
	ErrAlreadyExists  = errors.New("HQ identity already exists")
)

type KeyStore interface {
	Load() (Material, error)
	Initialize(io.Reader) (Material, error)
	WriteNew(Material) error
}

type FileStore struct{ Path string }

func (s FileStore) Load() (Material, error)                       { return Load(s.Path) }
func (s FileStore) Initialize(random io.Reader) (Material, error) { return Initialize(s.Path, random) }
func (s FileStore) WriteNew(material Material) error              { return WriteNew(s.Path, material) }

type Material struct {
	InstallationID string
	SecretKey      event.SecretKey
}

func (m Material) PublicKey() string { return m.SecretKey.PublicKeyHex() }

func (m Material) NPub() (string, error) { return EncodePublicKey(m.PublicKey()) }

func (m Material) Fingerprint() string {
	public := m.PublicKey()
	if len(public) > 12 {
		return public[:12]
	}
	return public
}

func (m Material) Sign(_ context.Context, content event.Content, createdAt time.Time) (event.SignedEvent, error) {
	if content.InstallationID == "" {
		content.InstallationID = m.InstallationID
	}
	if content.InstallationID != m.InstallationID {
		return event.SignedEvent{}, errors.New("event installation does not match the signer")
	}
	return event.Sign(content, createdAt, m.SecretKey)
}

type diskMaterial struct {
	Version        int    `json:"version"`
	InstallationID string `json:"installation_id"`
	SecretKey      string `json:"secret_key"`
}

func DefaultDatabasePath() (string, error) {
	if state := os.Getenv("XDG_STATE_HOME"); state != "" {
		return filepath.Join(state, "hq", "hq.db"), nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", fmt.Errorf("find home directory: %w", err)
	}
	return filepath.Join(home, ".local", "state", "hq", "hq.db"), nil
}

func DefaultConfigDirectory() (string, error) {
	if config := os.Getenv("XDG_CONFIG_HOME"); config != "" {
		return filepath.Join(config, "hq"), nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", fmt.Errorf("find home directory: %w", err)
	}
	return filepath.Join(home, ".config", "hq"), nil
}

func ResolveDatabasePath(path string) (string, error) {
	if path == "" {
		return DefaultDatabasePath()
	}
	absolute, err := filepath.Abs(path)
	if err != nil {
		return "", fmt.Errorf("resolve database path: %w", err)
	}
	return filepath.Clean(absolute), nil
}

func KeyPath(databasePath string) (string, error) {
	resolved, err := ResolveDatabasePath(databasePath)
	if err != nil {
		return "", err
	}
	return filepath.Join(filepath.Dir(resolved), "hq.key"), nil
}

func Initialize(keyPath string, random io.Reader) (Material, error) {
	if random == nil {
		random = rand.Reader
	}
	installation, err := uuid.NewV7FromReader(random)
	if err != nil {
		return Material{}, fmt.Errorf("generate installation ID: %w", err)
	}
	secret, err := generateSecret(random)
	if err != nil {
		return Material{}, err
	}
	material := Material{InstallationID: installation.String(), SecretKey: secret}
	if err := WriteNew(keyPath, material); err != nil {
		return Material{}, err
	}
	return material, nil
}

func generateSecret(random io.Reader) (event.SecretKey, error) {
	for {
		var raw [32]byte
		if _, err := io.ReadFull(random, raw[:]); err != nil {
			return event.SecretKey{}, fmt.Errorf("generate installation key: %w", err)
		}
		secret, err := event.SecretKeyFromHex(hex.EncodeToString(raw[:]))
		clear(raw[:])
		if err == nil {
			return secret, nil
		}
	}
}

func WriteNew(path string, material Material) error {
	if _, err := uuid.Parse(material.InstallationID); err != nil {
		return errors.New("installation ID must be a UUID")
	}
	if _, err := event.SecretKeyFromHex(hex.EncodeToString(material.SecretKey[:])); err != nil {
		return errors.New("installation key is invalid")
	}
	directory := filepath.Dir(path)
	if err := os.MkdirAll(directory, 0o700); err != nil {
		return fmt.Errorf("create identity directory: %w", err)
	}
	file, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if errors.Is(err, os.ErrExist) {
		return ErrAlreadyExists
	}
	if err != nil {
		return fmt.Errorf("create identity file: %w", err)
	}
	remove := true
	defer func() {
		_ = file.Close()
		if remove {
			_ = os.Remove(path)
		}
	}()
	disk := diskMaterial{Version: keyFileVersion, InstallationID: material.InstallationID, SecretKey: hex.EncodeToString(material.SecretKey[:])}
	encoder := json.NewEncoder(file)
	if err := encoder.Encode(disk); err != nil {
		return fmt.Errorf("write identity file: %w", err)
	}
	if err := file.Sync(); err != nil {
		return fmt.Errorf("sync identity file: %w", err)
	}
	if err := file.Close(); err != nil {
		return fmt.Errorf("close identity file: %w", err)
	}
	remove = false
	return nil
}

func Load(path string) (Material, error) {
	file, err := os.Open(path)
	if errors.Is(err, os.ErrNotExist) {
		return Material{}, ErrNotInitialized
	}
	if err != nil {
		return Material{}, fmt.Errorf("open identity file: %w", err)
	}
	defer file.Close()
	if info, err := file.Stat(); err != nil {
		return Material{}, fmt.Errorf("inspect identity file: %w", err)
	} else if runtime.GOOS != "windows" && info.Mode().Perm() != 0o600 {
		return Material{}, fmt.Errorf("identity file mode is %04o; want 0600", info.Mode().Perm())
	}
	var disk diskMaterial
	decoder := json.NewDecoder(io.LimitReader(file, 4097))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&disk); err != nil {
		return Material{}, errors.New("identity file is corrupt")
	}
	if disk.Version != keyFileVersion {
		return Material{}, fmt.Errorf("unsupported identity file version %d", disk.Version)
	}
	parsedID, err := uuid.Parse(disk.InstallationID)
	if err != nil || parsedID.String() != strings.ToLower(disk.InstallationID) {
		return Material{}, errors.New("identity file has an invalid installation ID")
	}
	secret, err := event.SecretKeyFromHex(disk.SecretKey)
	if err != nil {
		return Material{}, errors.New("identity file has an invalid key")
	}
	return Material{InstallationID: parsedID.String(), SecretKey: secret}, nil
}

func Reset(databasePath, keyPath string) error {
	for _, path := range []string{databasePath, databasePath + "-wal", databasePath + "-shm", keyPath} {
		if err := os.Remove(path); err != nil && !errors.Is(err, os.ErrNotExist) {
			return fmt.Errorf("remove %s: %w", filepath.Base(path), err)
		}
	}
	return nil
}

func EncodePublicKey(publicKey string) (string, error) {
	raw, err := hex.DecodeString(publicKey)
	if err != nil || len(raw) != 32 || publicKey != strings.ToLower(publicKey) {
		return "", errors.New("public key must be 32-byte lowercase hex")
	}
	return bech32.EncodeFromBase256("npub", raw)
}

func DecodePublicKey(value string) (string, error) {
	if len(value) == 64 {
		if _, err := EncodePublicKey(value); err != nil {
			return "", err
		}
		return value, nil
	}
	hrp, raw, err := bech32.DecodeToBase256(value)
	if err != nil || hrp != "npub" || len(raw) != 32 {
		return "", errors.New("public key must be npub or 32-byte lowercase hex")
	}
	return hex.EncodeToString(raw), nil
}
