//go:build windows

package syncer

func startDetachedNode(RuntimePaths) error { return ErrControlUnavailable }

func isNodeAbsent(error) bool { return false }
