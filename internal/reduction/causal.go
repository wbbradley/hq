package reduction

// Parents exposes the causal predecessors of an identifier. Implementations
// return known edges even when the parent fact itself has not arrived yet.
type Parents[K comparable] interface {
	Parents(K) []K
}

type Relation string

const (
	Equal      Relation = "equal"
	Before     Relation = "before"
	After      Relation = "after"
	Concurrent Relation = "concurrent"
)

func Relate[K comparable](graph Parents[K], left, right K) Relation {
	if left == right {
		return Equal
	}
	if ancestor(graph, left, right) {
		return Before
	}
	if ancestor(graph, right, left) {
		return After
	}
	return Concurrent
}

func Maxima[K comparable](graph Parents[K], values Set[K]) Set[K] {
	items := values.Values(nil)
	result := NewSet[K]()
	for _, candidate := range items {
		maximal := true
		for _, other := range items {
			if candidate != other && Relate(graph, candidate, other) == Before {
				maximal = false
				break
			}
		}
		if maximal {
			result = result.Add(candidate)
		}
	}
	return result
}

func ancestor[K comparable](graph Parents[K], target, descendant K) bool {
	seen := NewSet[K]()
	stack := append([]K(nil), graph.Parents(descendant)...)
	for len(stack) > 0 {
		id := stack[len(stack)-1]
		stack = stack[:len(stack)-1]
		if id == target {
			return true
		}
		if seen.Contains(id) {
			continue
		}
		seen = seen.Add(id)
		stack = append(stack, graph.Parents(id)...)
	}
	return false
}
