package syncer

import (
	"errors"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/localwire"
)

func TestRuntimeMetadataWriteRefreshAndOwnedRemoval(t *testing.T) {
	t.Setenv("XDG_STATE_HOME", filepath.Join(t.TempDir(), "state"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(t.TempDir(), "config"))
	paths, err := ResolveRuntimePaths(filepath.Join(t.TempDir(), "hq.db"))
	if err != nil {
		t.Fatal(err)
	}
	first := localwire.PeerMetadata{Build: "one", InstanceID: "instance-one", StartedAt: time.Unix(10, 0).UTC()}
	if err := writeRuntimeMetadata(paths, first); err != nil {
		t.Fatal(err)
	}
	metadata, err := ReadInstanceMetadata(paths)
	if err != nil || metadata.InstanceID != first.InstanceID || metadata.Build != first.Build || metadata.PID != os.Getpid() {
		t.Fatalf("metadata = %#v, %v", metadata, err)
	}
	for _, path := range []string{paths.PID, paths.InstanceMetadata} {
		info, err := os.Stat(path)
		if err != nil || info.Mode().Perm() != 0o600 {
			t.Fatalf("runtime file %s mode = %v, %v", path, info.Mode().Perm(), err)
		}
	}
	second := localwire.PeerMetadata{Build: "two", InstanceID: "instance-two", StartedAt: time.Unix(20, 0).UTC()}
	if err := writeRuntimeMetadata(paths, second); err != nil {
		t.Fatal(err)
	}
	if err := removeRuntimeMetadata(paths, first.InstanceID); err != nil {
		t.Fatal(err)
	}
	if metadata, err = ReadInstanceMetadata(paths); err != nil || metadata.InstanceID != second.InstanceID {
		t.Fatalf("new owner's metadata was removed: %#v, %v", metadata, err)
	}
	if err := removeRuntimeMetadata(paths, second.InstanceID); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(paths.InstanceMetadata); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("metadata remains: %v", err)
	}
	if _, err := os.Stat(paths.PID); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("PID remains: %v", err)
	}
}
