package codexbridge

import (
	"context"
	"encoding/json"
	"sync"
)

type ThreadState struct {
	mu           sync.Mutex
	threadID     string
	activeTurnID string
	changed      chan struct{}
}

func NewThreadState(threadID string) *ThreadState {
	return &ThreadState{threadID: threadID, changed: make(chan struct{})}
}

func (s *ThreadState) BindThread(threadID string) {
	s.mu.Lock()
	s.threadID = threadID
	s.mu.Unlock()
}

func (s *ThreadState) ActiveTurnID() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.activeTurnID
}

func (s *ThreadState) UpdateThread(thread Thread) {
	activeTurnID := ""
	for _, turn := range thread.Turns {
		if turn.Status == "inProgress" {
			activeTurnID = turn.ID
		}
	}
	s.setActive(activeTurnID)
}

func (s *ThreadState) SetActive(turnID string) { s.setActive(turnID) }

func (s *ThreadState) WaitForChange(ctx context.Context, activeTurnID string) error {
	for {
		s.mu.Lock()
		if s.activeTurnID != activeTurnID {
			s.mu.Unlock()
			return nil
		}
		changed := s.changed
		s.mu.Unlock()
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-changed:
		}
	}
}

func (s *ThreadState) HandleNotification(_ context.Context, notification Notification) {
	if notification.Method != "turn/started" && notification.Method != "turn/completed" {
		return
	}
	var params TurnNotification
	s.mu.Lock()
	threadID := s.threadID
	s.mu.Unlock()
	if json.Unmarshal(notification.Params, &params) != nil || params.ThreadID != threadID {
		return
	}
	if notification.Method == "turn/started" || params.Turn.Status == "inProgress" {
		s.setActive(params.Turn.ID)
		return
	}
	s.mu.Lock()
	active := s.activeTurnID
	s.mu.Unlock()
	if active == params.Turn.ID {
		s.setActive("")
	}
}

func (s *ThreadState) setActive(turnID string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.activeTurnID == turnID {
		return
	}
	s.activeTurnID = turnID
	close(s.changed)
	s.changed = make(chan struct{})
}

type NotificationHub struct {
	handlers []NotificationHandler
}

func NewNotificationHub(handlers ...NotificationHandler) *NotificationHub {
	return &NotificationHub{handlers: handlers}
}

func (h *NotificationHub) HandleNotification(ctx context.Context, notification Notification) {
	for _, handler := range h.handlers {
		if handler != nil {
			handler.HandleNotification(ctx, notification)
		}
	}
}
