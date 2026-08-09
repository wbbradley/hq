//go:build windows

package syncer

import (
	"context"
	"io"
)

func startControl(context.Context, string, chan<- struct{}, context.CancelFunc, func() string) (io.Closer, error) {
	return nil, ErrControlUnavailable
}

func controlCommand(string, string) (string, error) { return "", ErrControlUnavailable }
