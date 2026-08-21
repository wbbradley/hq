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
