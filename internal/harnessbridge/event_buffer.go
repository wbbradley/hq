package harnessbridge

import (
	"context"
	"sync"
)

type bufferedEventWork struct {
	work        eventWork
	coalesceKey string
}

type eventBuffer struct {
	mu       sync.Mutex
	capacity int
	items    []bufferedEventWork
	closed   bool
	changed  chan struct{}
}

func newEventBuffer(capacity int) *eventBuffer {
	if capacity < 1 {
		capacity = 1
	}
	return &eventBuffer{capacity: capacity, changed: make(chan struct{})}
}

func (b *eventBuffer) enqueue(ctx context.Context, work eventWork) error {
	key := work.coalesceKey()
	for {
		b.mu.Lock()
		if b.closed {
			b.mu.Unlock()
			return context.Canceled
		}
		if key != "" {
			for index := len(b.items) - 1; index >= 0; index-- {
				if b.items[index].coalesceKey != key {
					continue
				}
				copy(b.items[index:], b.items[index+1:])
				b.items[len(b.items)-1] = bufferedEventWork{work: work, coalesceKey: key}
				b.signalLocked()
				b.mu.Unlock()
				return nil
			}
		}
		if len(b.items) < b.capacity {
			b.items = append(b.items, bufferedEventWork{work: work, coalesceKey: key})
			b.signalLocked()
			b.mu.Unlock()
			return nil
		}
		changed := b.changed
		b.mu.Unlock()
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-changed:
		}
	}
}

func (b *eventBuffer) dequeue() (eventWork, bool) {
	for {
		b.mu.Lock()
		if len(b.items) != 0 {
			work := b.items[0].work
			copy(b.items, b.items[1:])
			b.items = b.items[:len(b.items)-1]
			b.signalLocked()
			b.mu.Unlock()
			return work, true
		}
		if b.closed {
			b.mu.Unlock()
			return eventWork{}, false
		}
		changed := b.changed
		b.mu.Unlock()
		<-changed
	}
}

func (b *eventBuffer) close() {
	b.mu.Lock()
	if !b.closed {
		b.closed = true
		b.signalLocked()
	}
	b.mu.Unlock()
}

func (b *eventBuffer) signalLocked() {
	close(b.changed)
	b.changed = make(chan struct{})
}
