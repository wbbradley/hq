package node

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"os"

	"github.com/wbbradley/hq/internal/codexbridge"
	hqconfig "github.com/wbbradley/hq/internal/config"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/domainrpc"
	"github.com/wbbradley/hq/internal/harness"
	"github.com/wbbradley/hq/internal/harnessbridge"
	"github.com/wbbradley/hq/internal/harnesssupervisor"
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
		codexFactory := &codexbridge.HarnessFactory{Logger: logger.With("component", "codex_adapter"), Stderr: logging.NewLineWriter(logger.With("component", "codex_process"), slog.LevelWarn, "Codex app-server stderr")}
		registry, err := harness.NewRegistry(codexFactory)
		if err != nil {
			database.Close()
			return syncer.Runtime{}, err
		}
		supervisor := harnesssupervisor.New(ctx, database, codexbridge.AdaptDeliveryLedger(ledger), registry)
		supervisor.Logger = logger
		supervisor.LoadLaunchDefaults = func() (domain.HarnessLaunchDefaults, error) {
			settings, err := hqconfig.Load()
			raw, marshalErr := json.Marshal(codexbridge.CodexOptions{Yolo: settings.Codex.Yolo})
			if err == nil {
				err = marshalErr
			}
			return domain.HarnessLaunchDefaults{Harness: string(codexbridge.CodexProviderID), ProviderOptions: raw}, err
		}
		supervisor.DecodeOptions = func(provider harness.ProviderID, agentName string, raw json.RawMessage) (harness.ProviderOptions, error) {
			if provider != codexbridge.CodexProviderID {
				return nil, &harness.ProviderError{Provider: provider, Operation: "decode options", Cause: harness.ErrUnknownProvider}
			}
			var options codexbridge.CodexOptions
			if len(raw) != 0 {
				if err := json.Unmarshal(raw, &options); err != nil {
					return nil, fmt.Errorf("decode Codex provider options: %w", err)
				}
			}
			options.DeveloperInstructions = codexbridge.NamedAgentDeveloperInstructions(agentName)
			return options, nil
		}
		supervisor.Terminology = func(provider harness.ProviderID) harnessbridge.Terminology {
			if provider == codexbridge.CodexProviderID {
				return harnessbridge.Terminology{ProviderName: "Codex", SessionName: "thread", OperationName: "turn", ItemName: "item", OutputNamespace: "hq-codex-output"}
			}
			return harnessbridge.Terminology{}
		}
		database.SetProjectCommandHandler(func(commandCtx context.Context, command domain.ProjectCommand, data domain.ProjectCommandData) (domain.Project, error) {
			switch value := data.(type) {
			case *domain.ProjectHarnessActivateCommand:
				request := domain.ProjectHarnessActivationRequest(*value)
				request.ProjectID, request.ExpectedHead, request.Launch.RequestID, request.Launch.Environment = command.ProjectID, command.ExpectedHead, command.ID, os.Environ()
				result, err := supervisor.ActivateHarnessProject(commandCtx, request)
				return result.Project, err
			case *domain.ProjectHarnessCloseCommand:
				request := domain.ProjectHarnessCloseRequest(*value)
				request.RequestID, request.ProjectID, request.ExpectedHead = command.ID, command.ProjectID, command.ExpectedHead
				return supervisor.CloseHarnessProject(commandCtx, request)
			case *domain.ProjectHarnessHandoffCommand:
				request := domain.ProjectHarnessHandoffRequest(*value)
				request.RequestID, request.ProjectID, request.ExpectedHead, request.Launch.RequestID, request.Launch.Environment = command.ID, command.ProjectID, command.ExpectedHead, command.ID, os.Environ()
				result, err := supervisor.HandoffHarnessProject(commandCtx, request)
				return result.Project, err
			case *domain.ProjectProvisionWorktreeCommand:
				request := domain.ProjectWorktreeRequest(*value)
				request.RequestID, request.ProjectID, request.HomeInstallation = command.ID, command.ProjectID, command.HomeInstallation
				return supervisor.ProvisionProjectWorktree(commandCtx, request)
			default:
				return domain.Project{}, fmt.Errorf("unsupported typed runtime operation %T", data)
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
