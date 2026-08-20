package codexbridge

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestCodexV01480FixturesRemainValidJSONRPC(t *testing.T) {
	fixtures, err := filepath.Glob("testdata/v0.148.0/*.json")
	if err != nil || len(fixtures) != 11 {
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
