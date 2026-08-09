package identity

import (
	"bytes"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/event"
)

func TestInitializeLoadAndPublicEncoding(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state", "hq.key")
	material, err := Initialize(path, bytes.NewReader(bytes.Repeat([]byte{3}, 64)))
	if err != nil {
		t.Fatal(err)
	}
	loaded, err := Load(path)
	if err != nil || loaded != material {
		t.Fatalf("loaded = %#v, %v", loaded, err)
	}
	info, err := os.Stat(path)
	if err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("key mode = %v, %v", info.Mode().Perm(), err)
	}
	npub, err := material.NPub()
	if err != nil {
		t.Fatal(err)
	}
	public, err := DecodePublicKey(npub)
	if err != nil || public != material.PublicKey() {
		t.Fatalf("decoded public key = %q, %v", public, err)
	}
	if _, err := Initialize(path, bytes.NewReader(bytes.Repeat([]byte{4}, 64))); !errors.Is(err, ErrAlreadyExists) {
		t.Fatalf("second init = %v", err)
	}
}

func TestLoadRejectsCorruptAndLooseFilesWithoutSecretError(t *testing.T) {
	path := filepath.Join(t.TempDir(), "hq.key")
	if err := os.WriteFile(path, []byte(`{"version":1,"installation_id":"bad","secret_key":"super-secret"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	_, err := Load(path)
	if err == nil || strings.Contains(err.Error(), "super-secret") {
		t.Fatalf("corrupt error = %v", err)
	}
	if err := os.Chmod(path, 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := Load(path); err == nil || !strings.Contains(err.Error(), "0600") {
		t.Fatalf("loose mode error = %v", err)
	}
}

func TestResetRemovesIdentityAndDatabase(t *testing.T) {
	dir := t.TempDir()
	db, key := filepath.Join(dir, "hq.db"), filepath.Join(dir, "hq.key")
	for _, path := range []string{db, db + "-wal", db + "-shm", key} {
		if err := os.WriteFile(path, []byte("x"), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	if err := Reset(db, key); err != nil {
		t.Fatal(err)
	}
	for _, path := range []string{db, db + "-wal", db + "-shm", key} {
		if _, err := os.Stat(path); !errors.Is(err, os.ErrNotExist) {
			t.Fatalf("%s remains: %v", path, err)
		}
	}
}

func TestMaterialRejectsWrongInstallation(t *testing.T) {
	material := Material{InstallationID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d01", SecretKey: event.MustSecretKeyFromHex("1")}
	_, err := material.Sign(t.Context(), event.Content{InstallationID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d02"}, time.Unix(1, 0))
	if err == nil {
		t.Fatal("signed another installation's event")
	}
}

func TestEncryptedBackupRoundTrip(t *testing.T) {
	dir := t.TempDir()
	material := Material{InstallationID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d01", SecretKey: event.MustSecretKeyFromHex("3")}
	path := filepath.Join(dir, "backup.json")
	if err := WriteBackup(path, material, []byte("backup password"), bytes.NewReader(bytes.Repeat([]byte{9}, 40))); err != nil {
		t.Fatal(err)
	}
	got, err := ReadBackup(path, []byte("backup password"))
	if err != nil || got != material {
		t.Fatalf("backup = %#v, %v", got, err)
	}
	if _, err := ReadBackup(path, []byte("wrong")); err == nil {
		t.Fatal("wrong backup password succeeded")
	}
	info, _ := os.Stat(path)
	if info.Mode().Perm() != 0o600 {
		t.Fatalf("backup mode = %o", info.Mode().Perm())
	}
}

func TestXDGPaths(t *testing.T) {
	t.Setenv("XDG_STATE_HOME", "/tmp/hq-state-test")
	t.Setenv("XDG_CONFIG_HOME", "/tmp/hq-config-test")
	database, err := DefaultDatabasePath()
	if err != nil || database != "/tmp/hq-state-test/hq/hq.db" {
		t.Fatalf("database path = %q, %v", database, err)
	}
	config, err := DefaultConfigDirectory()
	if err != nil || config != "/tmp/hq-config-test/hq" {
		t.Fatalf("config path = %q, %v", config, err)
	}
}
