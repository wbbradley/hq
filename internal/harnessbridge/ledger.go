package harnessbridge

import "sync"

// MemoryLedger is a process-local delivery checkpoint implementation suitable
// for tests and callers that do not configure durable recovery state.
type MemoryLedger struct {
	mu         sync.Mutex
	deliveries map[string]DeliveryState
	outputs    map[string]bool
}

func NewMemoryLedger() *MemoryLedger {
	return &MemoryLedger{deliveries: make(map[string]DeliveryState), outputs: make(map[string]bool)}
}

func checkpointKey(sessionID, id string) string { return sessionID + "\x00" + id }

func (l *MemoryLedger) Delivery(sessionID, messageID string) (DeliveryState, bool, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	state, exists := l.deliveries[checkpointKey(sessionID, messageID)]
	return state, exists, nil
}

func (l *MemoryLedger) SetDelivery(sessionID, messageID string, state DeliveryState) error {
	l.mu.Lock()
	l.deliveries[checkpointKey(sessionID, messageID)] = state
	l.mu.Unlock()
	return nil
}

func (l *MemoryLedger) OutputSent(sessionID, itemID string) (bool, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.outputs[checkpointKey(sessionID, itemID)], nil
}

func (l *MemoryLedger) MarkOutputSent(sessionID, itemID string) error {
	l.mu.Lock()
	l.outputs[checkpointKey(sessionID, itemID)] = true
	l.mu.Unlock()
	return nil
}

var _ DeliveryLedger = (*MemoryLedger)(nil)
