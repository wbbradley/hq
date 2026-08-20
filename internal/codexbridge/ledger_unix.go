//go:build !windows

package codexbridge

import "os"

func replaceFile(source, destination string) error {
	return os.Rename(source, destination)
}
