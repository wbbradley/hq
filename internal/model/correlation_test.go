package model

import (
	"encoding/json"
	"testing"
)

func TestMessageCorrelationShapeAndCombination(t *testing.T) {
	correlation := MessageCorrelation{
		Provider: "home-built", SessionID: "session-1", OperationID: "operation-1",
		ItemID: "item-1", RequestID: "request-1",
	}
	if !correlation.Valid() || correlation.Empty() {
		t.Fatalf("complete correlation = %#v", correlation)
	}
	raw, err := json.Marshal(correlation)
	if err != nil {
		t.Fatal(err)
	}
	const want = `{"provider":"home-built","session_id":"session-1","operation_id":"operation-1","item_id":"item-1","request_id":"request-1"}`
	if string(raw) != want {
		t.Fatalf("correlation JSON = %s, want %s", raw, want)
	}
	for _, valid := range []MessageCorrelation{
		{},
		{Provider: "provider", SessionID: "session"},
		{Provider: "provider", SessionID: "session", OperationID: "operation"},
	} {
		if !valid.Valid() {
			t.Fatalf("valid correlation rejected: %#v", valid)
		}
	}
	for _, invalid := range []MessageCorrelation{
		{Provider: "provider"},
		{SessionID: "session"},
		{OperationID: "operation"},
		{Provider: "provider", SessionID: "session", ItemID: "item"},
		{Provider: "provider", SessionID: "session", RequestID: "request"},
	} {
		if invalid.Valid() {
			t.Fatalf("invalid correlation accepted: %#v", invalid)
		}
	}
}
