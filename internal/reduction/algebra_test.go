package reduction

import (
	"math/rand/v2"
	"slices"
	"testing"
)

func TestSetUnionSemilatticeLaws(t *testing.T) {
	algebra := SetUnion[int]()
	samples := []Set[int]{
		{},
		NewSet(1),
		NewSet(1, 2),
		NewSet(2, 3, 4),
	}
	if err := CheckLaws(algebra, samples); err != nil {
		t.Fatal(err)
	}
}

func TestFoldToleratesChunkingPermutationAndDuplicates(t *testing.T) {
	algebra := SetUnion[int]()
	input := []Set[int]{NewSet(1, 2), NewSet(2, 3), NewSet(4), NewSet(1, 4)}
	want := Fold(algebra, input)
	for range 100 {
		shuffled := slices.Clone(input)
		rand.Shuffle(len(shuffled), func(i, j int) { shuffled[i], shuffled[j] = shuffled[j], shuffled[i] })
		shuffled = append(shuffled, shuffled[rand.IntN(len(shuffled))])
		if got := Fold(algebra, shuffled); !got.Equal(want) {
			t.Fatalf("fold = %v, want %v", got.Values(func(a, b int) bool { return a < b }), want.Values(func(a, b int) bool { return a < b }))
		}
	}
}

type testGraph map[string][]string

func (g testGraph) Parents(id string) []string { return slices.Clone(g[id]) }

func TestCausalRelationAndFrontier(t *testing.T) {
	graph := testGraph{
		"root":  nil,
		"left":  {"root"},
		"right": {"root"},
		"join":  {"left", "right"},
	}
	checks := []struct {
		left, right string
		want        Relation
	}{
		{"root", "join", Before},
		{"join", "root", After},
		{"left", "right", Concurrent},
		{"left", "left", Equal},
	}
	for _, check := range checks {
		if got := Relate(graph, check.left, check.right); got != check.want {
			t.Errorf("Relate(%q, %q) = %s, want %s", check.left, check.right, got, check.want)
		}
	}
	frontier := Maxima(graph, NewSet("root", "left", "right", "join"))
	if !frontier.Equal(NewSet("join")) {
		t.Fatalf("frontier = %v", frontier.Values(func(a, b string) bool { return a < b }))
	}
}

func TestMissingParentsRemainConcurrentUntilResolved(t *testing.T) {
	graph := testGraph{"child": {"missing"}}
	if got := Relate(graph, "missing", "child"); got != Before {
		t.Fatalf("known edge relation = %s", got)
	}
	if got := Relate(graph, "unknown", "child"); got != Concurrent {
		t.Fatalf("unknown relation = %s", got)
	}
}
