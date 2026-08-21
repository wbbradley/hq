package node

import (
	"context"

	"github.com/wbbradley/hq/internal/domainrpc"
	"github.com/wbbradley/hq/internal/localwire"
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
	opener := r.Open
	if opener == nil {
		opener = store.Open
	}
	factory := func(context.Context) (syncer.Runtime, error) {
		database, err := opener(paths.Database)
		if err != nil {
			return syncer.Runtime{}, err
		}
		engine := &syncer.Engine{State: database, Codec: database.WireCodec(nil, nil)}
		service := domainrpc.Service{
			Store: database,
			Synchronize: func(context.Context) error {
				return syncer.Wake(paths.Database)
			},
		}
		domainMode := &localwire.ModeConfig{Supported: localwire.DomainVersions, Handler: service.Handle}
		return syncer.Runtime{Engine: engine, Domain: domainMode, Closer: database}, nil
	}
	return (syncer.Daemon{
		RuntimeFactory: factory,
		Coordinator:    syncer.FileCoordinator{DatabasePath: paths.Database},
		DatabasePath:   paths.Database,
	}).Run(ctx)
}
