package node

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"

	"github.com/wbbradley/hq/internal/codexbridge"
	"github.com/wbbradley/hq/internal/codexsupervisor"
	hqconfig "github.com/wbbradley/hq/internal/config"
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
		supervisor.LoadLaunchDefaults = func() (domain.CodexLaunchDefaults, error) {
			settings, err := hqconfig.Load()
			return domain.CodexLaunchDefaults{Yolo: settings.Codex.Yolo}, err
		}
		database.SetProjectCommandHandler(func(commandCtx context.Context, command domain.ProjectCommand) (domain.Project, error) {
			switch command.Operation {
			case "codex.project.activate":
				var request domain.ProjectCodexActivationRequest
				if err := json.Unmarshal(command.Body, &request); err != nil {
					return domain.Project{}, err
				}
				request.ProjectID, request.ExpectedHead, request.Launch.RequestID, request.Launch.Environment = command.ProjectID, command.ExpectedHead, command.ID, os.Environ()
				result, err := supervisor.ActivateCodexProject(commandCtx, request)
				return result.Project, err
			case "codex.project.close":
				var request domain.ProjectCodexCloseRequest
				if err := json.Unmarshal(command.Body, &request); err != nil {
					return domain.Project{}, err
				}
				request.RequestID, request.ProjectID, request.ExpectedHead = command.ID, command.ProjectID, command.ExpectedHead
				return supervisor.CloseCodexProject(commandCtx, request)
			case "codex.project.handoff":
				var request domain.ProjectCodexHandoffRequest
				if err := json.Unmarshal(command.Body, &request); err != nil {
					return domain.Project{}, err
				}
				request.RequestID, request.ProjectID, request.ExpectedHead, request.Launch.RequestID, request.Launch.Environment = command.ID, command.ProjectID, command.ExpectedHead, command.ID, os.Environ()
				result, err := supervisor.HandoffCodexProject(commandCtx, request)
				return result.Project, err
			case "project.provision-worktree":
				var request domain.ProjectWorktreeRequest
				if err := json.Unmarshal(command.Body, &request); err != nil {
					return domain.Project{}, err
				}
				request.RequestID, request.ProjectID, request.HomeInstallation = command.ID, command.ProjectID, command.HomeInstallation
				return supervisor.ProvisionProjectWorktree(commandCtx, request)
			default:
				return domain.Project{}, fmt.Errorf("unsupported remote runtime operation %q", command.Operation)
			}
		})
		subscriptions := domainrpc.NewSubscriptionHub()
		database.SetChangeObserver(func(change domain.Invalidation) {
			subscriptions.Publish(change)
			supervisor.Publish(change)
		})
		supervisor.StartWorkReconciliation()
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
