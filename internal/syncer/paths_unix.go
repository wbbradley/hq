//go:build !windows

package syncer

import (
	"fmt"
	"os"
	"path/filepath"
)

func fallbackRuntimeDirectory() string {
	return filepath.Join(os.TempDir(), fmt.Sprintf("hq-%d", os.Getuid()))
}
