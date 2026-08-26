// Package eventstate defines the effect-free contracts shared by batch and
// incremental canonical-event reduction.
package eventstate

import (
	"maps"
	"slices"
	"sort"
)

type EventID string
type ResourceKind string
type AggregateKind string
type ProjectionKind string

type ResourceKey struct {
	Kind ResourceKind
	ID   string
}

type AggregateKey struct {
	Kind AggregateKind
	ID   string
}

type ProjectionKey struct {
	Kind ProjectionKind
	ID   string
}

type Fact[P any] struct {
	ID          EventID
	Parents     []EventID
	Authorities []EventID
	Resources   []ResourceKey
	Payload     P
}

func NewFact[P any](id EventID, parents, authorities []EventID, resources []ResourceKey, payload P) Fact[P] {
	return Fact[P]{
		ID: id, Parents: slices.Clone(parents), Authorities: slices.Clone(authorities),
		Resources: slices.Clone(resources), Payload: payload,
	}
}

type CausalQuery interface {
	Parents(EventID) []EventID
	Known(EventID) bool
}

type ReadinessStatus string

const (
	ReadinessReady   ReadinessStatus = "ready"
	ReadinessMissing ReadinessStatus = "missing"
)

type Readiness struct {
	Status  ReadinessStatus
	Missing []EventID
}

func EvaluateReadiness(graph CausalQuery, id EventID) Readiness {
	missing := make(map[EventID]struct{})
	visiting := make(map[EventID]bool)
	visited := make(map[EventID]bool)
	var visit func(EventID)
	visit = func(current EventID) {
		if visited[current] || visiting[current] {
			return
		}
		visiting[current] = true
		for _, parent := range graph.Parents(current) {
			if !graph.Known(parent) {
				missing[parent] = struct{}{}
				continue
			}
			visit(parent)
		}
		delete(visiting, current)
		visited[current] = true
	}
	visit(id)
	if len(missing) == 0 {
		return Readiness{Status: ReadinessReady}
	}
	ids := make([]EventID, 0, len(missing))
	for id := range missing {
		ids = append(ids, id)
	}
	sort.Slice(ids, func(i, j int) bool { return ids[i] < ids[j] })
	return Readiness{Status: ReadinessMissing, Missing: ids}
}

type AuthorizationStatus string

const (
	AuthorizationAuthorized   AuthorizationStatus = "authorized"
	AuthorizationUnauthorized AuthorizationStatus = "unauthorized"
	AuthorizationUnknown      AuthorizationStatus = "unknown"
)

type Decision[T any] struct {
	Value         T
	Readiness     Readiness
	Authorization AuthorizationStatus
	Reason        string
	Support       []EventID
}

type ProjectionDelta[T any] struct {
	Aggregate AggregateKey
	Upsert    map[ProjectionKey]T
	Delete    []ProjectionKey
	Support   []EventID
}

func (d ProjectionDelta[T]) Clone() ProjectionDelta[T] {
	return ProjectionDelta[T]{
		Aggregate: d.Aggregate, Upsert: maps.Clone(d.Upsert),
		Delete: slices.Clone(d.Delete), Support: slices.Clone(d.Support),
	}
}
