package fake_test

import (
	"testing"

	"github.com/wbbradley/hq/internal/harness"
	"github.com/wbbradley/hq/internal/harness/conformance"
	"github.com/wbbradley/hq/internal/harness/fake"
)

func TestFakeHarnessConformance(t *testing.T) {
	t.Run("full capabilities", func(t *testing.T) {
		conformance.Run(t, func() (harness.Factory, conformance.Controller) {
			factory := fake.NewFactory("home-built")
			return factory, factory
		})
	})
	t.Run("idempotent recovery only", func(t *testing.T) {
		conformance.Run(t, func() (harness.Factory, conformance.Controller) {
			factory := fake.NewFactory("home-built-idempotent")
			factory.SetCapabilities(harness.Capabilities{IdempotentSubmission: true})
			return factory, factory
		})
	})
	t.Run("lookup recovery only", func(t *testing.T) {
		conformance.Run(t, func() (harness.Factory, conformance.Controller) {
			factory := fake.NewFactory("home-built-lookup")
			factory.SetCapabilities(harness.Capabilities{SubmissionLookup: true})
			return factory, factory
		})
	})
}
