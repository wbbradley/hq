package eventstate

import (
	"slices"
	"testing"
)

func TestCanonicalReductionPipelineCoversEveryDomainOnce(t *testing.T) {
	want := []string{
		"local-controls",
		"peer-bindings",
		"mailbox-access-classification",
		"mailbox-access-projection",
		"account-authority-classification",
		"account-projection",
		"account-selection-classification",
		"default-account-projection",
		"domain-event-classification",
		"mailbox-projection",
		"named-agent-classification",
		"named-agent-projection",
		"message-projection",
		"message-state",
		"thread-projection",
		"message-order",
		"conversation-order",
		"harness-activity-projection",
	}
	if got := canonicalReductionPipeline.stageNames(); !slices.Equal(got, want) {
		t.Fatalf("canonical reduction pipeline\nwant: %#v\ngot:  %#v", want, got)
	}
}
