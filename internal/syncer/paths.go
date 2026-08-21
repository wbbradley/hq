package syncer

import (
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"github.com/wbbradley/hq/internal/identity"
)

type RuntimePaths struct {
	Database                 string
	IdentityKey              string
	OwnershipLock            string
	Socket                   string
	PID                      string
	InstanceMetadata         string
	Log                      string
	ConfigDirectory          string
	RuntimeDirectory         string
	protectDatabaseDirectory bool
}

const maxUnixSocketPath = 103

func ResolveRuntimePaths(databasePath string) (RuntimePaths, error) {
	database, err := identity.ResolveDatabasePath(databasePath)
	if err != nil {
		return RuntimePaths{}, err
	}
	key, err := identity.KeyPath(database)
	if err != nil {
		return RuntimePaths{}, err
	}
	configRoot, err := identity.DefaultConfigDirectory()
	if err != nil {
		return RuntimePaths{}, err
	}
	defaultDatabase, err := identity.DefaultDatabasePath()
	if err != nil {
		return RuntimePaths{}, err
	}
	digest := storageDigest(database)
	configDirectory := configRoot
	if database != defaultDatabase {
		configDirectory = filepath.Join(configRoot, "databases", digest)
	}
	runtimeDirectory := os.Getenv("XDG_RUNTIME_DIR")
	if runtimeDirectory != "" {
		runtimeDirectory = filepath.Join(runtimeDirectory, "hq")
	} else {
		runtimeDirectory = filepath.Join(filepath.Dir(defaultDatabase), "run")
	}
	socket := database + ".sync.sock"
	if len(socket) > maxUnixSocketPath {
		socket = filepath.Join(runtimeDirectory, digest+".sock")
		if len(socket) > maxUnixSocketPath {
			runtimeDirectory = fallbackRuntimeDirectory()
			socket = filepath.Join(runtimeDirectory, digest+".sock")
		}
	}
	return RuntimePaths{
		Database: database, IdentityKey: key,
		OwnershipLock: database + ".sync.lock", Socket: socket,
		PID: database + ".node.pid", InstanceMetadata: database + ".node.json",
		Log: database + ".node.log", ConfigDirectory: configDirectory,
		RuntimeDirectory: runtimeDirectory, protectDatabaseDirectory: database == defaultDatabase,
	}, nil
}

func (p RuntimePaths) EnsureDirectories() error {
	databaseDirectory := filepath.Dir(p.Database)
	if err := ensureDirectory(databaseDirectory, p.protectDatabaseDirectory); err != nil {
		return err
	}
	if socketDirectory := filepath.Dir(p.Socket); socketDirectory != databaseDirectory {
		if err := ensureDirectory(socketDirectory, true); err != nil {
			return err
		}
	}
	if err := ensureDirectory(p.ConfigDirectory, true); err != nil {
		return err
	}
	return nil
}

func ensureDirectory(directory string, protectExisting bool) error {
	_, statErr := os.Stat(directory)
	created := errors.Is(statErr, os.ErrNotExist)
	if statErr != nil && !created {
		return fmt.Errorf("inspect HQ directory %s: %w", directory, statErr)
	}
	if err := os.MkdirAll(directory, 0o700); err != nil {
		return fmt.Errorf("create HQ directory %s: %w", directory, err)
	}
	if created || protectExisting {
		if err := os.Chmod(directory, 0o700); err != nil {
			return fmt.Errorf("protect HQ directory %s: %w", directory, err)
		}
	}
	return nil
}

func storageDigest(database string) string {
	sum := sha256.Sum256([]byte(filepath.Clean(database)))
	return hex.EncodeToString(sum[:16])
}
