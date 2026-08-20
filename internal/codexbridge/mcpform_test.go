package codexbridge

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestValidateMCPFormAcceptsSupportedPrimitiveConstraints(t *testing.T) {
	schema := json.RawMessage(`{
		"type":"object",
		"properties":{
			"name":{"type":"string","minLength":2,"maxLength":4},
			"email":{"type":"string","format":"email"},
			"site":{"type":"string","format":"uri"},
			"day":{"type":"string","format":"date"},
			"when":{"type":"string","format":"date-time"},
			"count":{"type":"integer","minimum":1,"maximum":3},
			"ratio":{"type":"number","minimum":0.25,"maximum":0.75},
			"enabled":{"type":"boolean"},
			"color":{"type":"string","oneOf":[{"const":"red","title":"Red"},{"const":"blue"}]},
			"tags":{"type":"array","items":{"type":"string","enum":["a","b"]},"minItems":1,"maxItems":2},
			"modes":{"type":"array","items":{"type":"string","anyOf":[{"const":"fast"},{"const":"safe"}]}}
		},
		"required":["name","count","enabled"]
	}`)
	content := `{"name":"Ada","email":"ada@example.com","site":"https://example.com/x","day":"2026-08-20","when":"2026-08-20T12:00:00Z","count":2,"ratio":0.5,"enabled":true,"color":"red","tags":["a"],"modes":["safe"]}`
	result, err := validateMCPForm(schema, content)
	if err != nil {
		t.Fatal(err)
	}
	if result["name"] != "Ada" || result["enabled"] != true || result["count"].(json.Number).String() != "2" {
		t.Fatalf("content = %#v", result)
	}
}

func TestValidateMCPFormRejectsInvalidContent(t *testing.T) {
	schema := json.RawMessage(`{"type":"object","properties":{"name":{"type":"string","minLength":2,"maxLength":4},"email":{"type":"string","format":"email"},"count":{"type":"integer","minimum":1,"maximum":3},"ratio":{"type":"number","minimum":0.25,"maximum":0.75},"enabled":{"type":"boolean"},"tags":{"type":"array","items":{"type":"string","enum":["a","b"]},"minItems":1,"maxItems":2}},"required":["name"]}`)
	tests := []struct {
		name    string
		content string
		want    string
	}{
		{name: "missing required", content: `{}`, want: `required field "name"`},
		{name: "unknown field", content: `{"name":"Ada","extra":1}`, want: `unknown field "extra"`},
		{name: "short string", content: `{"name":"A"}`, want: "at least 2"},
		{name: "long string", content: `{"name":"Alice"}`, want: "at most 4"},
		{name: "email format", content: `{"name":"Ada","email":"nope"}`, want: "valid email"},
		{name: "fractional integer", content: `{"name":"Ada","count":1.5}`, want: "must be an integer"},
		{name: "low number", content: `{"name":"Ada","ratio":0.1}`, want: "at least"},
		{name: "wrong boolean", content: `{"name":"Ada","enabled":"yes"}`, want: "must be a boolean"},
		{name: "bad enum array", content: `{"name":"Ada","tags":["c"]}`, want: "invalid option"},
		{name: "too many items", content: `{"name":"Ada","tags":["a","b","a"]}`, want: "at most 2"},
		{name: "trailing JSON", content: `{"name":"Ada"} {}`, want: "only one JSON object"},
		{name: "null", content: `null`, want: "one JSON object"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			_, err := validateMCPForm(schema, test.content)
			if err == nil || !strings.Contains(err.Error(), test.want) {
				t.Fatalf("error = %v, want %q", err, test.want)
			}
		})
	}
}

func TestValidateMCPFormRejectsUnsupportedOrMalformedSchemas(t *testing.T) {
	tests := []struct {
		name   string
		schema string
	}{
		{name: "unknown top-level keyword", schema: `{"type":"object","properties":{},"additionalProperties":true}`},
		{name: "unknown property keyword", schema: `{"type":"object","properties":{"name":{"type":"string","pattern":".*"}}}`},
		{name: "unknown option keyword", schema: `{"type":"object","properties":{"name":{"type":"string","oneOf":[{"const":"a","value":1}]}}}`},
		{name: "missing required property", schema: `{"type":"object","properties":{},"required":["name"]}`},
		{name: "unsupported primitive", schema: `{"type":"object","properties":{"nested":{"type":"object"}}}`},
		{name: "invalid bounds", schema: `{"type":"object","properties":{"count":{"type":"integer","minimum":3,"maximum":1}}}`},
		{name: "unsupported format", schema: `{"type":"object","properties":{"name":{"type":"string","format":"hostname"}}}`},
		{name: "unconstrained array", schema: `{"type":"object","properties":{"tags":{"type":"array","items":{"type":"string"}}}}`},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if _, err := validateMCPForm(json.RawMessage(test.schema), `{}`); err == nil {
				t.Fatal("schema was accepted")
			}
		})
	}
}
