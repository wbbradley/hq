// Package conformance provides reusable behavioral tests for harness adapters.
package conformance

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/harness"
)

type Controller interface {
	FailNextLaunch(error)
	SetNextSubmissionOutcome(harness.DeliveryState, bool)
	Emit(harness.Instance, harness.OperationID, string, harness.EventPayload) error
	Ask(harness.Instance, harness.OperationID, string, harness.RequestPayload) (harness.RequestID, <-chan harness.Response, error)
	Crash(harness.Instance, error) error
}

type SubjectFactory func() (harness.Factory, Controller)

func Run(t *testing.T, newSubject SubjectFactory) {
	t.Helper()
	t.Run("new and resumed sessions", func(t *testing.T) {
		factory, _ := newSubject()
		first := launchNew(t, factory, "instance-new")
		identity := first.Session().Identity()
		if identity.Provider != factory.Provider().ID || identity.ID == "" {
			t.Fatalf("new session identity = %#v", identity)
		}
		shutdown(t, first)
		if !factory.Provider().Capabilities.Resume {
			_, err := factory.Launch(context.Background(), harness.LaunchConfig{
				InstanceID: "instance-resumed", AgentName: "agent", Directory: "/work/repo",
				SessionMode: harness.SessionResume, RequestedSession: identity.ID,
			})
			if !errors.Is(err, harness.ErrCapabilityUnavailable) {
				t.Fatalf("unsupported resume error = %v", err)
			}
			return
		}
		resumed, err := factory.Launch(context.Background(), harness.LaunchConfig{
			InstanceID: "instance-resumed", AgentName: "agent", Directory: "/work/repo",
			SessionMode: harness.SessionResume, RequestedSession: identity.ID,
		})
		if err != nil {
			t.Fatal(err)
		}
		if got := resumed.Session().Identity(); got != identity {
			t.Fatalf("resumed session identity = %#v; want %#v", got, identity)
		}
		shutdown(t, resumed)
	})

	t.Run("submit and reconcile", func(t *testing.T) {
		factory, controller := newSubject()
		instance := launchNew(t, factory, "instance-delivery")
		defer shutdown(t, instance)
		capabilities := factory.Provider().Capabilities
		reconciler, hasReconciler := instance.Session().(harness.SubmissionReconciler)
		if capabilities.SubmissionLookup && !hasReconciler {
			t.Fatal("lookup-capable session does not implement SubmissionReconciler")
		}

		controller.SetNextSubmissionOutcome(harness.DeliveryUncertain, true)
		acceptedID := harness.SubmissionID("submission-accepted-uncertain")
		uncertain, err := instance.Session().Submit(context.Background(), textSubmission(acceptedID, "accepted but response lost"))
		if err != nil || uncertain.State != harness.DeliveryUncertain || uncertain.OperationID == "" || uncertain.Validate() != nil {
			t.Fatalf("uncertain accepted result = %#v, %v", uncertain, err)
		}
		if capabilities.SubmissionLookup {
			recovered, recoverErr := reconciler.Reconcile(context.Background(), acceptedID)
			if recoverErr != nil || recovered.State != harness.RecoveryAccepted || recovered.OperationID != uncertain.OperationID || recovered.Validate() != nil {
				t.Fatalf("accepted recovery = %#v, %v", recovered, recoverErr)
			}
		} else {
			retried, retryErr := instance.Session().Submit(context.Background(), textSubmission(acceptedID, "idempotent accepted retry"))
			if retryErr != nil || retried.State != harness.DeliveryAccepted || retried.OperationID != uncertain.OperationID {
				t.Fatalf("idempotent accepted retry = %#v, %v", retried, retryErr)
			}
		}

		controller.SetNextSubmissionOutcome(harness.DeliveryUncertain, false)
		missingID := harness.SubmissionID("submission-missing-uncertain")
		missing, err := instance.Session().Submit(context.Background(), textSubmission(missingID, "not accepted"))
		if err != nil || missing.State != harness.DeliveryUncertain || missing.OperationID != "" || missing.Validate() != nil {
			t.Fatalf("uncertain missing result = %#v, %v", missing, err)
		}
		if capabilities.SubmissionLookup {
			recovered, recoverErr := reconciler.Reconcile(context.Background(), missingID)
			if recoverErr != nil || recovered != (harness.RecoveryResult{State: harness.RecoveryNotFound}) {
				t.Fatalf("missing recovery = %#v, %v", recovered, recoverErr)
			}
		}
		retried, err := instance.Session().Submit(context.Background(), textSubmission(missingID, "retry with stable ID"))
		if err != nil || retried.State != harness.DeliveryAccepted || retried.OperationID == "" {
			t.Fatalf("retried delivery = %#v, %v", retried, err)
		}

		if !capabilities.IdempotentSubmission {
			return
		}
		const concurrentRetries = 16
		results := make(chan harness.DeliveryResult, concurrentRetries)
		errorsChannel := make(chan error, concurrentRetries)
		var wait sync.WaitGroup
		for range concurrentRetries {
			wait.Add(1)
			go func() {
				defer wait.Done()
				result, submitErr := instance.Session().Submit(context.Background(), textSubmission("submission-concurrent", "same stable submission"))
				results <- result
				errorsChannel <- submitErr
			}()
		}
		wait.Wait()
		close(results)
		close(errorsChannel)
		var operationID harness.OperationID
		for result := range results {
			if operationID == "" {
				operationID = result.OperationID
			}
			if result.State != harness.DeliveryAccepted || result.OperationID != operationID {
				t.Fatalf("concurrent idempotent result = %#v; operation = %q", result, operationID)
			}
		}
		for submitErr := range errorsChannel {
			if submitErr != nil {
				t.Fatal(submitErr)
			}
		}
	})

	t.Run("active operation behavior", func(t *testing.T) {
		factory, _ := newSubject()
		instance := launchNew(t, factory, "instance-active")
		defer shutdown(t, instance)
		first, err := instance.Session().Submit(context.Background(), textSubmission("submission-first", "start work"))
		if err != nil {
			t.Fatal(err)
		}
		if !factory.Provider().Capabilities.SteerActiveOperation {
			return
		}
		steerer, ok := instance.Session().(harness.ActiveOperationSubmitter)
		if !ok {
			t.Fatal("steering-capable session does not implement ActiveOperationSubmitter")
		}
		steered, err := steerer.SubmitToActive(context.Background(), first.OperationID, textSubmission("submission-steered", "additional context"))
		if err != nil || steered.OperationID != first.OperationID || steered.State != harness.DeliveryAccepted {
			t.Fatalf("steered result = %#v, %v", steered, err)
		}
		_, err = steerer.SubmitToActive(context.Background(), "wrong-operation", textSubmission("submission-wrong-operation", "must fail"))
		if !errors.Is(err, harness.ErrOperationMismatch) {
			t.Fatalf("operation mismatch error = %v", err)
		}
	})

	t.Run("ordered events", func(t *testing.T) {
		factory, controller := newSubject()
		instance := launchNew(t, factory, "instance-events")
		defer shutdown(t, instance)
		for _, message := range []string{"one", "two", "three"} {
			if err := controller.Emit(instance, "operation-events", "item-events", harness.ProgressEvent{Message: message}); err != nil {
				t.Fatal(err)
			}
		}
		var prior time.Time
		for sequence := uint64(1); sequence <= 3; sequence++ {
			event := <-instance.Events()
			if event.Sequence != sequence || event.Session != instance.Session().Identity() || event.Operation != "operation-events" {
				t.Fatalf("event %d = %#v", sequence, event)
			}
			if !prior.IsZero() && !event.OccurredAt.After(prior) {
				t.Fatalf("event time did not increase: %s then %s", prior, event.OccurredAt)
			}
			prior = event.OccurredAt
		}
	})

	t.Run("interactive request receives exactly one response", func(t *testing.T) {
		factory, controller := newSubject()
		instance := launchNew(t, factory, "instance-request")
		defer shutdown(t, instance)
		requestID, responses, err := controller.Ask(instance, "operation-request", "item-request", harness.QuestionRequest{Prompt: "Choose", Options: []harness.QuestionOption{{Label: "yes"}}})
		if err != nil {
			t.Fatal(err)
		}
		request := <-instance.Requests()
		if request.ID != requestID || request.Session != instance.Session().Identity() {
			t.Fatalf("request = %#v", request)
		}
		response := harness.Response{RequestID: requestID, Payload: harness.AnswerResponse{Answers: []string{"yes"}}}
		if err := instance.Session().Respond(context.Background(), response); err != nil {
			t.Fatal(err)
		}
		if got := <-responses; got.RequestID != requestID {
			t.Fatalf("response = %#v", got)
		}
		if err := instance.Session().Respond(context.Background(), response); !errors.Is(err, harness.ErrRequestCompleted) {
			t.Fatalf("duplicate response error = %v", err)
		}
	})

	t.Run("concurrent shutdown", func(t *testing.T) {
		factory, _ := newSubject()
		instance := launchNew(t, factory, "instance-shutdown")
		const callers = 32
		var wait sync.WaitGroup
		errorsChannel := make(chan error, callers)
		for range callers {
			wait.Add(1)
			go func() {
				defer wait.Done()
				errorsChannel <- instance.Shutdown(context.Background())
			}()
		}
		wait.Wait()
		close(errorsChannel)
		for err := range errorsChannel {
			if err != nil {
				t.Fatal(err)
			}
		}
		if err := instance.Wait(context.Background()); err != nil {
			t.Fatal(err)
		}
		if state := instance.State(); state.Phase != harness.RuntimeStopped || state.Err != nil {
			t.Fatalf("state = %#v", state)
		}
		if _, open := <-instance.Events(); open {
			t.Fatal("event stream remained open")
		}
		if _, open := <-instance.Requests(); open {
			t.Fatal("request stream remained open")
		}
	})

	t.Run("provider and registry failures", func(t *testing.T) {
		factory, controller := newSubject()
		failure := errors.New("provider boot failed")
		controller.FailNextLaunch(failure)
		_, err := factory.Launch(context.Background(), newLaunch("instance-failed"))
		if !errors.Is(err, harness.ErrProviderUnavailable) || !errors.Is(err, failure) {
			t.Fatalf("launch error = %v", err)
		}
		registry, err := harness.NewRegistry(factory)
		if err != nil {
			t.Fatal(err)
		}
		if _, err := registry.Factory("missing-provider"); !errors.Is(err, harness.ErrUnknownProvider) {
			t.Fatalf("unknown provider error = %v", err)
		}
		if _, err := harness.NewRegistry(providerOnlyFactory{provider: harness.Provider{ID: "unsafe", DisplayName: "Unsafe"}}); err == nil {
			t.Fatal("registry accepted provider without safe submission recovery")
		}

		instance := launchNew(t, factory, "instance-crash")
		crash := errors.New("connection lost")
		if err := controller.Crash(instance, crash); err != nil {
			t.Fatal(err)
		}
		if err := instance.Wait(context.Background()); !errors.Is(err, crash) {
			t.Fatalf("wait error = %v", err)
		}
		if state := instance.State(); state.Phase != harness.RuntimeFailed || !errors.Is(state.Err, crash) {
			t.Fatalf("failed state = %#v", state)
		}
	})
}

func launchNew(t *testing.T, factory harness.Factory, instanceID harness.InstanceID) harness.Instance {
	t.Helper()
	instance, err := factory.Launch(context.Background(), newLaunch(instanceID))
	if err != nil {
		t.Fatal(err)
	}
	if instance.ID() != instanceID || instance.Provider() != factory.Provider().ID || instance.State().Phase != harness.RuntimeRunning {
		t.Fatalf("launched instance = %q %q %#v", instance.ID(), instance.Provider(), instance.State())
	}
	return instance
}

func newLaunch(instanceID harness.InstanceID) harness.LaunchConfig {
	return harness.LaunchConfig{InstanceID: instanceID, AgentName: "agent", Directory: "/work/repo", SessionMode: harness.SessionNew}
}

func textSubmission(id harness.SubmissionID, text string) harness.Submission {
	return harness.Submission{ID: id, Input: []harness.InputPart{harness.TextInput{Text: text}}}
}

func shutdown(t *testing.T, instance harness.Instance) {
	t.Helper()
	if err := instance.Shutdown(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := instance.Wait(context.Background()); err != nil {
		t.Fatal(err)
	}
}

type providerOnlyFactory struct {
	provider harness.Provider
}

func (f providerOnlyFactory) Provider() harness.Provider { return f.provider }
func (providerOnlyFactory) Launch(context.Context, harness.LaunchConfig) (harness.Instance, error) {
	return nil, errors.New("not implemented")
}
