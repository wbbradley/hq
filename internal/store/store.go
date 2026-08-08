package store

import (
	"context"
	"errors"

	"github.com/wbbradley/hq/internal/model"
)

var (
	ErrNotFound       = errors.New("question not found")
	ErrAlreadyHandled = errors.New("question has already been handled")
	ErrNotReady       = errors.New("question has no answer ready")
	ErrClaimed        = errors.New("answer is being delivered by another process")
)

type Store interface {
	Create(context.Context, model.Question) error
	Get(context.Context, string) (model.Question, error)
	List(context.Context, model.Filter) ([]model.Question, error)
	Answer(context.Context, string, string) error
	Cancel(context.Context, string) error
	ClaimAnswer(context.Context, string, string) (model.Question, error)
	CompleteAnswer(context.Context, string, string) error
	ReleaseAnswer(context.Context, string, string) error
	Close() error
}
