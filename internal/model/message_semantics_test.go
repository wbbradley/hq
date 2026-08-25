package model

import (
	"encoding/json"
	"testing"
)

func TestPresentationKindValidation(t *testing.T) {
	for _, kind := range []PresentationKind{"", PresentationUpdate, PresentationFinalAnswer, PresentationStatus, PresentationNotice} {
		if !kind.Valid() {
			t.Fatalf("presentation kind %q rejected", kind)
		}
	}
	if PresentationKind("question").Valid() {
		t.Fatal("unknown presentation kind accepted")
	}
}

func TestTechnicalSectionsPreserveOrderAndLabels(t *testing.T) {
	sections := []TechnicalSection{{
		Namespace: "hq.harness.output",
		Fields: []TechnicalField{
			{Key: "status", Label: "Status", Value: "failed"},
			{Key: "diagnostic", Value: "provider error"},
		},
	}}
	raw, err := json.Marshal(sections)
	if err != nil {
		t.Fatal(err)
	}
	const want = `[{"namespace":"hq.harness.output","fields":[{"key":"status","label":"Status","value":"failed"},{"key":"diagnostic","value":"provider error"}]}]`
	if string(raw) != want {
		t.Fatalf("technical JSON = %s, want %s", raw, want)
	}
}
