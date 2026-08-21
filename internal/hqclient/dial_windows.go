//go:build windows

package hqclient

import (
	"context"
	"io"

	"github.com/wbbradley/hq/internal/syncer"
)

func dial(context.Context, string) (io.ReadWriteCloser, error) {
	return nil, syncer.ErrControlUnavailable
}
