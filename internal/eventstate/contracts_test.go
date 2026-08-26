package eventstate

import (
	"slices"
	"testing"
)

type graphFixture map[EventID][]EventID

func (g graphFixture) Parents(id EventID) []EventID { return slices.Clone(g[id]) }
func (g graphFixture) Known(id EventID) bool        { _, ok := g[id]; return ok }

func TestEvaluateReadinessReportsMissingDependencies(t *testing.T) {
	graph := graphFixture{
		"root":  nil,
		"child": {"root", "missing"},
	}
	got := EvaluateReadiness(graph, "child")
	if got.Status != ReadinessMissing || !slices.Equal(got.Missing, []EventID{"missing"}) {
		t.Fatalf("readiness = %#v", got)
	}
	graph["missing"] = nil
	if got := EvaluateReadiness(graph, "child"); got.Status != ReadinessReady || len(got.Missing) != 0 {
		t.Fatalf("resolved readiness = %#v", got)
	}
}

func TestFactCopiesCallerOwnedReferences(t *testing.T) {
	parents := []EventID{"one"}
	authorities := []EventID{"one"}
	resources := []ResourceKey{{Kind: "message", ID: "m1"}}
	fact := NewFact("fact", parents, authorities, resources, "payload")
	parents[0], authorities[0], resources[0].ID = "changed", "changed", "changed"
	if fact.Parents[0] != "one" || fact.Authorities[0] != "one" || fact.Resources[0].ID != "m1" {
		t.Fatalf("fact retained caller-owned storage: %#v", fact)
	}
}

func TestProjectionDeltaIsExplicitlyScoped(t *testing.T) {
	delta := ProjectionDelta[string]{
		Aggregate: AggregateKey{Kind: "thread", ID: "t1"},
		Upsert:    map[ProjectionKey]string{{Kind: "thread", ID: "t1"}: "open"},
		Delete:    []ProjectionKey{{Kind: "thread-answer", ID: "old"}},
		Support:   []EventID{"fact"},
	}
	clone := delta.Clone()
	clone.Upsert[ProjectionKey{Kind: "thread", ID: "t1"}] = "changed"
	clone.Delete[0].ID = "changed"
	if delta.Upsert[ProjectionKey{Kind: "thread", ID: "t1"}] != "open" || delta.Delete[0].ID != "old" {
		t.Fatalf("clone aliases input: original=%#v clone=%#v", delta, clone)
	}
}
