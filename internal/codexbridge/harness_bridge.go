package codexbridge

import (
	"context"
	"errors"
	"log/slog"
	"strings"
	"time"

	"github.com/wbbradley/hq/internal/harness"
	"github.com/wbbradley/hq/internal/harnessbridge"
	"github.com/wbbradley/hq/internal/model"
)

func Run(ctx context.Context, options Options) error {
	logger := options.Logger
	if logger == nil {
		logger = slog.New(slog.DiscardHandler)
	}
	logger = logger.With("agent", options.AgentName, "directory", options.Directory)
	if strings.TrimSpace(options.Directory) == "" {
		return errors.New("Codex bridge working directory is required")
	}
	if strings.TrimSpace(options.AgentName) == "" {
		return errors.New("Codex bridge requires a durable agent name")
	}
	if options.Store == nil {
		return errors.New("Codex bridge mailbox store is required")
	}
	if options.NewThread && options.ResumeThreadID != "" {
		return errors.New("starting a new thread cannot also resume a thread")
	}
	if options.ProjectID != "" && options.ProjectReady == nil {
		return errors.New("project Codex bridge requires a ready binding callback")
	}
	if options.ProjectID != "" && options.ProjectStore == nil {
		return errors.New("project Codex bridge store is required")
	}
	ledger := options.Ledger
	if ledger == nil {
		opened, err := OpenFileLedger(options.LedgerPath)
		if err != nil {
			return err
		}
		ledger = opened
	}
	starter := options.Starter
	if starter == nil {
		starter = &ExecStarter{}
	}
	factory := &HarnessFactory{Starter: starter, Stderr: options.Stderr, Logger: logger.With("component", "codex_adapter")}
	genericOptions := harnessbridge.Options{
		Directory: options.Directory, AgentName: options.AgentName, ProjectID: options.ProjectID, NewSession: options.NewThread,
		RequestedSession: harness.SessionID(options.ResumeThreadID), InitialPrompt: options.InitialPrompt, Repository: options.Repository,
		Factory: factory, ProviderOptions: CodexOptions{Yolo: options.Yolo, DeveloperInstructions: NamedAgentDeveloperInstructions(options.AgentName)},
		Store: options.Store, ProjectStore: options.ProjectStore, Ledger: codexLedgerAdapter{ledger}, Stderr: options.Stderr,
		Sync: options.Sync, RepairInterval: options.RepairInterval, Updates: options.Updates,
		AgentLeaseDuration: options.AgentLeaseDuration, AgentRenewInterval: options.AgentRenewInterval,
		SuppressStatus: options.SuppressStatus, Logger: logger,
		Terminology: harnessbridge.Terminology{
			ProviderName: "Codex", SessionName: "thread", OperationName: "turn", ItemName: "item",
			ReadyBody: bridgeReadyBody(options), ReadyStatus: "The Codex app-server thread is connected and waiting for HQ input.",
			StoppedBody: "Codex bridge stopped", CancelledStatus: "Bridge cancelled; the app-server process is being terminated.",
			OutputNamespace: "hq-codex-output", NewSessionHint: "use --new-thread to attach Codex",
		},
		PublishStatus: func(statusContext context.Context, mailbox model.Mailbox, identity harness.SessionIdentity, body, status string, createdAt time.Time) error {
			return sendStatusAt(statusContext, options, mailbox, string(identity.ID), body, status, createdAt)
		},
	}
	if options.ProjectReady != nil {
		genericOptions.ProjectReady = func(ready harnessbridge.Ready) (harnessbridge.ProjectBinding, error) {
			binding, err := options.ProjectReady(BridgeReady{AgentName: ready.AgentName, ThreadID: string(ready.Session.ID), Directory: ready.Directory})
			return harnessbridge.ProjectBinding{
				ProjectID: binding.ProjectID, AssignmentID: binding.AssignmentID, ProjectThreadID: binding.ProjectThreadID,
				MailboxID: binding.MailboxID, ProjectName: binding.ProjectName,
			}, err
		}
	}
	if options.OnReady != nil {
		genericOptions.OnReady = func(ready harnessbridge.Ready) {
			options.OnReady(BridgeReady{AgentName: ready.AgentName, ThreadID: string(ready.Session.ID), Directory: ready.Directory})
		}
	}
	err := harnessbridge.Run(ctx, genericOptions)
	if err == nil {
		return nil
	}
	var providerErr *harness.ProviderError
	var runtimeErr *harness.RuntimeError
	if errors.As(err, &providerErr) || errors.As(err, &runtimeErr) && (runtimeErr.Action == "start session" || runtimeErr.Action == "resume session" || strings.HasPrefix(runtimeErr.Action, "validate")) {
		return bridgeAdapterLaunchError(err, options.ResumeThreadID, options.AgentName)
	}
	return err
}

type codexLedgerAdapter struct{ ledger DeliveryLedger }

func (a codexLedgerAdapter) Delivery(sessionID, messageID string) (harnessbridge.DeliveryState, bool, error) {
	record, exists, err := a.ledger.Delivery(sessionID, messageID)
	return harnessbridge.DeliveryState(record.State), exists, err
}

func (a codexLedgerAdapter) SetDelivery(sessionID, messageID string, state harnessbridge.DeliveryState) error {
	return a.ledger.SetDelivery(sessionID, messageID, DeliveryState(state))
}

func (a codexLedgerAdapter) OutputSent(sessionID, itemID string) (bool, error) {
	return a.ledger.OutputSent(sessionID, itemID)
}

func (a codexLedgerAdapter) MarkOutputSent(sessionID, itemID string) error {
	return a.ledger.MarkOutputSent(sessionID, itemID)
}

var _ harnessbridge.DeliveryLedger = codexLedgerAdapter{}
