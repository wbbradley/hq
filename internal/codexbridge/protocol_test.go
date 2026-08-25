package codexbridge

import (
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
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

type generatedProtocolSchema struct {
	Definitions map[string]struct {
		OneOf []struct {
			Properties struct {
				Method struct {
					Enum []string `json:"enum"`
				} `json:"method"`
			} `json:"properties"`
		} `json:"oneOf"`
	} `json:"definitions"`
}

func TestCodexV01490GeneratedSchemasContainConsumedProtocol(t *testing.T) {
	legacy := readGeneratedProtocolSchema(t, "testdata/schema/v0.149.0/codex_app_server_protocol.schemas.json")
	v2 := readGeneratedProtocolSchema(t, "testdata/schema/v0.149.0/codex_app_server_protocol.v2.schemas.json")

	assertSchemaMethods(t, v2, "ClientRequest", []string{
		"initialize", "thread/start", "thread/resume", "thread/read", "turn/start", "turn/steer", "turn/interrupt",
	})
	assertSchemaMethods(t, legacy, "ServerRequest", []string{
		requestUserInputMethod, commandApprovalMethod, fileApprovalMethod, permissionMethod, mcpElicitationMethod,
	})
	assertSchemaMethods(t, legacy, "ServerNotification", []string{
		"turn/started", "turn/completed", "turn/plan/updated", "turn/diff/updated", "item/started", "item/completed",
		"item/plan/delta", "item/commandExecution/outputDelta", "item/fileChange/outputDelta", "item/mcpToolCall/progress",
	})
}

func readGeneratedProtocolSchema(t *testing.T, path string) generatedProtocolSchema {
	t.Helper()
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var schema generatedProtocolSchema
	if err := json.Unmarshal(raw, &schema); err != nil {
		t.Fatalf("decode %s: %v", path, err)
	}
	return schema
}

func assertSchemaMethods(t *testing.T, schema generatedProtocolSchema, definition string, expected []string) {
	t.Helper()
	variants, ok := schema.Definitions[definition]
	if !ok {
		t.Fatalf("generated schema has no %s definition", definition)
	}
	methods := make([]string, 0, len(variants.OneOf))
	for _, variant := range variants.OneOf {
		methods = append(methods, variant.Properties.Method.Enum...)
	}
	for _, method := range expected {
		if !slices.Contains(methods, method) {
			t.Errorf("%s schema does not contain %q", definition, method)
		}
	}
}
