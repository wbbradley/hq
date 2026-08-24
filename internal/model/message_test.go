package model

import "testing"

func TestMessagePurposeValidationAndLegacyDefault(t *testing.T) {
	for _, purpose := range []MessagePurpose{"", MessagePurposeConversation, MessagePurposeProjectInput, MessagePurposeProtocolQuestion, MessagePurposeProtocolAnswer, MessagePurposeProjectOutput, MessagePurposeSystemNotice} {
		if !purpose.Valid() {
			t.Fatalf("purpose %q is not valid", purpose)
		}
	}
	if MessagePurpose("made-up").Valid() {
		t.Fatal("unknown purpose is valid")
	}
	if got := NormalizeMessagePurpose(""); got != MessagePurposeConversation {
		t.Fatalf("legacy purpose = %q", got)
	}
}
