package store

import (
	"context"
	"errors"

	"github.com/wbbradley/hq/internal/model"
)

var (
	ErrNotFound       = errors.New("message not found")
	ErrAlreadyHandled = errors.New("message has already been handled")
	ErrNotReady       = errors.New("no message is ready")
	ErrClaimed        = errors.New("message is being delivered by another process")
)

type Claim struct {
	MessageID        string
	ReplyTo          string
	Directory        string
	RecipientSession string
}

type Store interface {
	Create(context.Context, model.Message) error
	Reply(context.Context, string, model.Message) error
	Get(context.Context, string) (model.Message, error)
	List(context.Context, model.Filter) ([]model.Message, error)
	Archive(context.Context, string) error
	Claim(context.Context, Claim, string) (model.Message, error)
	Complete(context.Context, string, string) error
	Release(context.Context, string, string) error
	Close() error
}
