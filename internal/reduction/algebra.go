// Package reduction contains small, lawful building blocks for pure reducers.
//
// The package deliberately uses explicit algebra dictionaries instead of
// reflection or method-heavy frameworks. A JoinSemilattice must obey identity,
// associativity, commutativity, and idempotence. Those laws make folds insensitive
// to batching, duplication, and arrival order.
package reduction

import (
	"fmt"
	"slices"
)

// Set is a persistent-style set. Mutating operations return a new value and do
// not retain caller-owned maps or modify the receiver.
type Set[K comparable] struct {
	items map[K]struct{}
}

func NewSet[K comparable](values ...K) Set[K] {
	result := Set[K]{items: make(map[K]struct{}, len(values))}
	for _, value := range values {
		result.items[value] = struct{}{}
	}
	return result
}

func (s Set[K]) Len() int { return len(s.items) }

func (s Set[K]) Contains(value K) bool {
	_, ok := s.items[value]
	return ok
}

func (s Set[K]) Add(values ...K) Set[K] {
	result := s.clone()
	for _, value := range values {
		result.items[value] = struct{}{}
	}
	return result
}

func (s Set[K]) Remove(values ...K) Set[K] {
	result := s.clone()
	for _, value := range values {
		delete(result.items, value)
	}
	return result
}

func (s Set[K]) Union(other Set[K]) Set[K] {
	result := s.clone()
	for value := range other.items {
		result.items[value] = struct{}{}
	}
	return result
}

func (s Set[K]) Equal(other Set[K]) bool {
	if len(s.items) != len(other.items) {
		return false
	}
	for value := range s.items {
		if !other.Contains(value) {
			return false
		}
	}
	return true
}

func (s Set[K]) Values(less func(K, K) bool) []K {
	result := make([]K, 0, len(s.items))
	for value := range s.items {
		result = append(result, value)
	}
	if less != nil {
		slices.SortFunc(result, func(a, b K) int {
			if less(a, b) {
				return -1
			}
			if less(b, a) {
				return 1
			}
			return 0
		})
	}
	return result
}

func (s Set[K]) clone() Set[K] {
	result := Set[K]{items: make(map[K]struct{}, len(s.items))}
	for value := range s.items {
		result.items[value] = struct{}{}
	}
	return result
}

// JoinSemilattice is an explicit dictionary for a lawful, unordered merge.
type JoinSemilattice[T any] struct {
	Empty func() T
	Join  func(T, T) T
	Equal func(T, T) bool
}

func SetUnion[K comparable]() JoinSemilattice[Set[K]] {
	return JoinSemilattice[Set[K]]{
		Empty: func() Set[K] { return NewSet[K]() },
		Join:  func(a, b Set[K]) Set[K] { return a.Union(b) },
		Equal: func(a, b Set[K]) bool { return a.Equal(b) },
	}
}

func Fold[T any](algebra JoinSemilattice[T], values []T) T {
	result := algebra.Empty()
	for _, value := range values {
		result = algebra.Join(result, value)
	}
	return result
}

// CheckLaws exhaustively checks semilattice laws over the supplied examples.
// It is useful in tests for every domain algebra instance.
func CheckLaws[T any](algebra JoinSemilattice[T], samples []T) error {
	empty := algebra.Empty()
	for i, a := range samples {
		if !algebra.Equal(algebra.Join(empty, a), a) || !algebra.Equal(algebra.Join(a, empty), a) {
			return fmt.Errorf("identity law failed for sample %d", i)
		}
		if !algebra.Equal(algebra.Join(a, a), a) {
			return fmt.Errorf("idempotence law failed for sample %d", i)
		}
		for j, b := range samples {
			if !algebra.Equal(algebra.Join(a, b), algebra.Join(b, a)) {
				return fmt.Errorf("commutativity law failed for samples %d and %d", i, j)
			}
			for k, c := range samples {
				left := algebra.Join(algebra.Join(a, b), c)
				right := algebra.Join(a, algebra.Join(b, c))
				if !algebra.Equal(left, right) {
					return fmt.Errorf("associativity law failed for samples %d, %d, and %d", i, j, k)
				}
			}
		}
	}
	return nil
}
