package node

import (
	"context"
	"errors"
	"fmt"
	"io"

	"github.com/wbbradley/hq/internal/codexbridge"
	"github.com/wbbradley/hq/internal/codexsupervisor"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/domainrpc"
	"github.com/wbbradley/hq/internal/localwire"
	"github.com/wbbradley/hq/internal/logging"
	"github.com/wbbradley/hq/internal/store"
	"github.com/wbbradley/hq/internal/syncer"
)

type StoreOpener func(string) (*store.SQLite, error)

type Runner struct {
	Open StoreOpener
}

func Run(ctx context.Context, databasePath string) error {
	return (Runner{}).Run(ctx, databasePath)
}

func (r Runner) Run(ctx context.Context, databasePath string) error {
	paths, err := syncer.ResolveRuntimePaths(databasePath)
	if err != nil {
		return err
	}
	logger, logCloser, err := logging.Open(paths.Log)
	if err != nil {
		return fmt.Errorf("open HQ daemon log: %w", err)
	}
	defer logCloser.Close()
	logger = logger.With("service", "hq")
	logger.Info("HQ node initializing", "database", paths.Database, "log_path", paths.Log)
	opener := r.Open
	if opener == nil {
		opener = store.Open
	}
	factory := func(context.Context) (syncer.Runtime, error) {
		logger.Debug("opening HQ database", "database", paths.Database)
		database, err := opener(paths.Database)
		if err != nil {
			logger.Error("open HQ database", "database", paths.Database, "error", err)
			return syncer.Runtime{}, err
		}
		engine := &syncer.Engine{State: database, Codec: database.WireCodec(nil, nil)}
		ledger, err := codexbridge.OpenFileLedger(paths.Database + ".codexbridge.json")
		if err != nil {
			database.Close()
			logger.Error("open Codex delivery ledger", "error", err)
			return syncer.Runtime{}, err
		}
		supervisor := codexsupervisor.New(ctx, database, ledger)
		supervisor.Logger = logger
		subscriptions := domainrpc.NewSubscriptionHub()
		database.SetChangeObserver(func(change domain.Invalidation) {
			subscriptions.Publish(change)
			supervisor.Publish(change)
		})
		service := domainrpc.Service{
			Store: database, Subscriptions: subscriptions, Runtime: supervisor,
			Synchronize: func(context.Context) error {
				return syncer.Wake(paths.Database)
			},
		}
		domainMode := &localwire.ModeConfig{Supported: localwire.DomainVersions, Handler: service.Handle}
		return syncer.Runtime{Engine: engine, Domain: domainMode, Closer: runtimeCloser{supervisor, database}}, nil
	}
	return (syncer.Daemon{
		RuntimeFactory: factory,
		Coordinator:    syncer.FileCoordinator{DatabasePath: paths.Database},
		DatabasePath:   paths.Database,
		Logger:         logger,
	}).Run(ctx)
}

type runtimeCloser []io.Closer

func (c runtimeCloser) Close() error {
	var result error
	for _, closer := range c {
		if closer != nil {
			result = errors.Join(result, closer.Close())
		}
	}
	return result
}
