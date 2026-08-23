package codexbridge

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestCodexCompatibilityFixturesRemainValidJSONRPC(t *testing.T) {
	fixtures, err := filepath.Glob("testdata/v*/*.json")
	if err != nil || len(fixtures) != 12 {
		t.Fatalf("fixtures = %v, %v", fixtures, err)
	}
	for _, fixture := range fixtures {
		raw, err := os.ReadFile(fixture)
		if err != nil {
			t.Fatal(err)
		}
		var envelope rpcEnvelope
		if err := json.Unmarshal(raw, &envelope); err != nil {
			t.Fatalf("%s: %v", fixture, err)
		}
		if envelope.JSONRPC != "" || (len(envelope.ID) == 0 && envelope.Method == "") {
			t.Fatalf("%s: invalid envelope %#v", fixture, envelope)
		}
	}
}

func TestCodexV01490ThreadStartResponseIncludesEffectiveYoloSettings(t *testing.T) {
	raw, err := os.ReadFile("testdata/v0.149.0/thread-start-response.json")
	if err != nil {
		t.Fatal(err)
	}
	var envelope struct {
		Result ThreadResponse `json:"result"`
	}
	if err := json.Unmarshal(raw, &envelope); err != nil {
		t.Fatal(err)
	}
	if envelope.Result.Thread.ID == "" || envelope.Result.ApprovalPolicy != approvalPolicyNever || envelope.Result.Sandbox.Type != sandboxTypeDangerFullAccess {
		t.Fatalf("thread start response = %#v", envelope.Result)
	}
}
