//go:build !windows

package hqclient

import (
	"context"
	"io"
	"net"
)

func dial(ctx context.Context, socket string) (io.ReadWriteCloser, error) {
	return (&net.Dialer{}).DialContext(ctx, "unix", socket)
}
