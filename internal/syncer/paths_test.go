package syncer

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestRuntimePathsResolveDefaultsAndExplicitDatabases(t *testing.T) {
	root := t.TempDir()
	t.Setenv("XDG_STATE_HOME", filepath.Join(root, "state"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(root, "config"))
	t.Setenv("XDG_RUNTIME_DIR", filepath.Join(root, "runtime"))
	t.Setenv("HOME", filepath.Join(root, "home"))
	defaults, err := ResolveRuntimePaths("")
	if err != nil {
		t.Fatal(err)
	}
	wantDatabase := filepath.Join(root, "state", "hq", "hq.db")
	if defaults.Database != wantDatabase || defaults.IdentityKey != filepath.Join(root, "state", "hq", "hq.key") {
		t.Fatalf("default paths = %#v", defaults)
	}
	if defaults.OwnershipLock != wantDatabase+".sync.lock" || defaults.PID != wantDatabase+".node.pid" || defaults.InstanceMetadata != wantDatabase+".node.json" || defaults.StartupLog != wantDatabase+".node.log" || defaults.Socket == "" || len(defaults.Socket) > maxUnixSocketPath {
		t.Fatalf("default runtime paths = %#v", defaults)
	}
	if defaults.ConfigDirectory != filepath.Join(root, "config", "hq") {
		t.Fatalf("default config = %q", defaults.ConfigDirectory)
	}
	if defaults.Log != filepath.Join(root, "home", "logs", "hq.log") {
		t.Fatalf("default log = %q", defaults.Log)
	}

	explicitDatabase := filepath.Join(root, "isolated", "other.db")
	explicit, err := ResolveRuntimePaths(explicitDatabase)
	if err != nil {
		t.Fatal(err)
	}
	if explicit.Database != explicitDatabase || explicit.OwnershipLock == defaults.OwnershipLock || explicit.Socket == defaults.Socket || explicit.ConfigDirectory == defaults.ConfigDirectory {
		t.Fatalf("explicit paths are not isolated: %#v", explicit)
	}
	if !strings.HasPrefix(explicit.ConfigDirectory, filepath.Join(root, "config", "hq", "databases")+string(filepath.Separator)) {
		t.Fatalf("explicit config = %q", explicit.ConfigDirectory)
	}
}

func TestRuntimePathsHashLongSocketAndProtectDirectories(t *testing.T) {
	root := t.TempDir()
	t.Setenv("XDG_STATE_HOME", filepath.Join(root, "state"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(root, "config"))
	t.Setenv("XDG_RUNTIME_DIR", filepath.Join(root, "runtime"))
	database := filepath.Join(root, strings.Repeat("long-directory-", 10), "hq.db")
	paths, err := ResolveRuntimePaths(database)
	if err != nil {
		t.Fatal(err)
	}
	if len(paths.Socket) > maxUnixSocketPath || paths.Socket == database+".sync.sock" {
		t.Fatalf("long socket path = %q", paths.Socket)
	}
	second, err := ResolveRuntimePaths(database)
	if err != nil || second.Socket != paths.Socket {
		t.Fatalf("socket path is not deterministic: %q, %q, %v", paths.Socket, second.Socket, err)
	}
	if err := paths.EnsureDirectories(); err != nil {
		t.Fatal(err)
	}
	for _, directory := range []string{filepath.Dir(paths.Database), filepath.Dir(paths.Socket), paths.ConfigDirectory} {
		info, err := os.Stat(directory)
		if err != nil || info.Mode().Perm() != 0o700 {
			t.Fatalf("directory %s mode = %v, %v", directory, info.Mode().Perm(), err)
		}
	}
}

func TestExplicitDatabaseKeepsExistingParentPermissions(t *testing.T) {
	root := t.TempDir()
	t.Setenv("XDG_STATE_HOME", filepath.Join(root, "state"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(root, "config"))
	shared := filepath.Join(root, "shared")
	if err := os.Mkdir(shared, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(shared, 0o755); err != nil {
		t.Fatal(err)
	}
	paths, err := ResolveRuntimePaths(filepath.Join(shared, "custom.db"))
	if err != nil {
		t.Fatal(err)
	}
	if err := paths.EnsureDirectories(); err != nil {
		t.Fatal(err)
	}
	info, err := os.Stat(shared)
	if err != nil || info.Mode().Perm() != 0o755 {
		t.Fatalf("explicit database parent mode = %v, %v", info.Mode().Perm(), err)
	}
}
