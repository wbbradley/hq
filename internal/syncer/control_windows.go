//go:build windows

package syncer

import (
	"context"
	"io"

	"github.com/wbbradley/hq/internal/localwire"
)

func startControl(context.Context, RuntimePaths, chan<- struct{}, context.CancelFunc, context.CancelFunc, func() string, localwire.PeerMetadata) (io.Closer, error) {
	return nil, ErrControlUnavailable
}

func controlCommand(string, string, any) (localwire.HandshakeResponse, error) {
	return localwire.HandshakeResponse{}, ErrControlUnavailable
}
