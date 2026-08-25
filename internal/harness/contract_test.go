package harness_test

import (
	"errors"
	"testing"

	"github.com/wbbradley/hq/internal/harness"
	"github.com/wbbradley/hq/internal/harness/fake"
)

func TestDeliveryAndRecoveryResultsRequireExplicitProof(t *testing.T) {
	validDeliveries := []harness.DeliveryResult{
		{State: harness.DeliveryRejected},
		{State: harness.DeliveryAccepted, OperationID: "operation-1"},
		{State: harness.DeliveryUncertain},
		{State: harness.DeliveryUncertain, OperationID: "operation-possibly-accepted"},
	}
	for _, result := range validDeliveries {
		if err := result.Validate(); err != nil {
			t.Errorf("valid delivery %#v: %v", result, err)
		}
	}
	for _, result := range []harness.DeliveryResult{
		{},
		{State: harness.DeliveryRejected, OperationID: "impossible"},
		{State: harness.DeliveryAccepted},
	} {
		if err := result.Validate(); err == nil {
			t.Errorf("invalid delivery accepted: %#v", result)
		}
	}

	for _, result := range []harness.RecoveryResult{
		{State: harness.RecoveryNotFound},
		{State: harness.RecoveryAccepted, OperationID: "operation-1"},
	} {
		if err := result.Validate(); err != nil {
			t.Errorf("valid recovery %#v: %v", result, err)
		}
	}
	for _, result := range []harness.RecoveryResult{
		{},
		{State: harness.RecoveryNotFound, OperationID: "impossible"},
		{State: harness.RecoveryAccepted},
	} {
		if err := result.Validate(); err == nil {
			t.Errorf("invalid recovery accepted: %#v", result)
		}
	}
}

func TestRegistryIsDeterministicAndRejectsDuplicates(t *testing.T) {
	alpha := fake.NewFactory("alpha")
	zeta := fake.NewFactory("zeta")
	registry, err := harness.NewRegistry(zeta, alpha)
	if err != nil {
		t.Fatal(err)
	}
	providers := registry.Providers()
	if len(providers) != 2 || providers[0].ID != "alpha" || providers[1].ID != "zeta" {
		t.Fatalf("providers = %#v", providers)
	}
	if err := registry.Register(fake.NewFactory("alpha")); err == nil {
		t.Fatal("duplicate provider registration succeeded")
	}
	if _, err := registry.Factory("missing"); !errors.Is(err, harness.ErrUnknownProvider) {
		t.Fatalf("unknown provider error = %v", err)
	}
}

func TestNormalizedActivityPayloadsStayTyped(t *testing.T) {
	exitCode := 0
	payloads := []harness.EventPayload{
		harness.OperationStatusEvent{Status: harness.OperationRunning},
		harness.PlanEvent{Text: "plan"},
		harness.DiffEvent{Text: "diff"},
		harness.CommandEvent{Command: "go test ./...", Output: "ok", ExitCode: &exitCode, Status: harness.OperationCompleted},
		harness.FileChangeEvent{Path: "main.go", Summary: "updated", Status: harness.OperationCompleted},
		harness.ToolEvent{Name: "search", Summary: "found matches", Status: harness.OperationCompleted},
		harness.ProgressEvent{Message: "working"},
		harness.OutputEvent{Text: "answer", Final: true},
	}
	if len(payloads) != 8 {
		t.Fatalf("payloads = %#v", payloads)
	}
}
