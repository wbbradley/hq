package main

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/signal"
	"syscall"

	"github.com/wbbradley/hq/internal/cli"
)

var version = "dev"

func main() {
	if len(os.Args) == 2 && (os.Args[1] == "version" || os.Args[1] == "--version") {
		fmt.Fprintln(os.Stdout, "hq", version)
		return
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	err := cli.New().Run(ctx, os.Args[1:])
	if err == nil {
		return
	}
	if errors.Is(err, cli.ErrNoMessages) {
		os.Exit(3)
	}
	fmt.Fprintln(os.Stderr, "hq:", err)
	os.Exit(1)
}
