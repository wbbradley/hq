//go:build windows

package syncer

import (
	"os"
	"path/filepath"
)

func fallbackRuntimeDirectory() string {
	return filepath.Join(os.TempDir(), "hq")
}
