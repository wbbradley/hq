package domain

import (
	"context"
	"encoding/json"
	"errors"
)

var ErrMutationConflict = errors.New("mutation key conflict")

type Mutation struct {
	ID            string
	Method        string
	RequestDigest string
}

type MutationLog interface {
	MutationResult(context.Context, Mutation) (json.RawMessage, bool, error)
}

type mutationContextKey struct{}

func WithMutation(ctx context.Context, mutation Mutation) context.Context {
	return context.WithValue(ctx, mutationContextKey{}, mutation)
}

func MutationFromContext(ctx context.Context) (Mutation, bool) {
	mutation, ok := ctx.Value(mutationContextKey{}).(Mutation)
	return mutation, ok
}

type mutationlessContext struct{ context.Context }

func (c mutationlessContext) Value(key any) any {
	if _, ok := key.(mutationContextKey); ok {
		return nil
	}
	return c.Context.Value(key)
}

// WithoutMutation keeps cancellation and all unrelated values while ensuring
// nested observational commits do not consume the caller's mutation receipt.
func WithoutMutation(ctx context.Context) context.Context {
	return mutationlessContext{Context: ctx}
}
