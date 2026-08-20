package codexbridge

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"
)

const deliveryLedgerVersion = 1

type DeliveryState string

const (
	DeliveryPending   DeliveryState = "pending"
	DeliveryUncertain DeliveryState = "uncertain"
	DeliveryAccepted  DeliveryState = "accepted"
)

type DeliveryRecord struct {
	State     DeliveryState `json:"state"`
	UpdatedAt time.Time     `json:"updated_at"`
}

type DeliveryLedger interface {
	// Delivery states are local checkpoints, not a cross-process lock. Callers
	// must reconcile uncertain records against Codex history before retrying.
	Delivery(threadID, messageID string) (DeliveryRecord, bool, error)
	SetDelivery(threadID, messageID string, state DeliveryState) error
	OutputSent(threadID, itemID string) (bool, error)
	MarkOutputSent(threadID, itemID string) error
}

type ledgerState struct {
	Version    int                                  `json:"version"`
	Deliveries map[string]map[string]DeliveryRecord `json:"deliveries"`
	Outputs    map[string]map[string]bool           `json:"outputs"`
}

func emptyLedgerState() ledgerState {
	return ledgerState{
		Version:    deliveryLedgerVersion,
		Deliveries: make(map[string]map[string]DeliveryRecord),
		Outputs:    make(map[string]map[string]bool),
	}
}

type FileLedger struct {
	mu    sync.Mutex
	path  string
	state ledgerState
	now   func() time.Time
}

func OpenFileLedger(path string) (*FileLedger, error) {
	if path == "" {
		return nil, errors.New("delivery ledger path is required")
	}
	ledger := &FileLedger{path: filepath.Clean(path), state: emptyLedgerState(), now: time.Now}
	raw, err := os.ReadFile(ledger.path)
	if errors.Is(err, os.ErrNotExist) {
		return ledger, nil
	}
	if err != nil {
		return nil, fmt.Errorf("read delivery ledger: %w", err)
	}
	if err := json.Unmarshal(raw, &ledger.state); err != nil {
		return nil, fmt.Errorf("decode delivery ledger: %w", err)
	}
	if ledger.state.Version != deliveryLedgerVersion {
		return nil, fmt.Errorf("delivery ledger version %d is unsupported; expected %d", ledger.state.Version, deliveryLedgerVersion)
	}
	if ledger.state.Deliveries == nil {
		ledger.state.Deliveries = make(map[string]map[string]DeliveryRecord)
	}
	if ledger.state.Outputs == nil {
		ledger.state.Outputs = make(map[string]map[string]bool)
	}
	for threadID, deliveries := range ledger.state.Deliveries {
		if threadID == "" {
			return nil, errors.New("delivery ledger contains an empty thread ID")
		}
		for messageID, record := range deliveries {
			if messageID == "" || !validDeliveryState(record.State) {
				return nil, fmt.Errorf("delivery ledger contains invalid record for thread %q message %q", threadID, messageID)
			}
		}
	}
	return ledger, nil
}

func (l *FileLedger) Delivery(threadID, messageID string) (DeliveryRecord, bool, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	record, ok := l.state.Deliveries[threadID][messageID]
	return record, ok, nil
}

func (l *FileLedger) SetDelivery(threadID, messageID string, state DeliveryState) error {
	if !validDeliveryState(state) {
		return fmt.Errorf("invalid delivery state %q", state)
	}
	if threadID == "" || messageID == "" {
		return errors.New("thread ID and message ID are required")
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	next := cloneLedgerState(l.state)
	if next.Deliveries[threadID] == nil {
		next.Deliveries[threadID] = make(map[string]DeliveryRecord)
	}
	next.Deliveries[threadID][messageID] = DeliveryRecord{State: state, UpdatedAt: l.now().UTC()}
	if err := l.persist(next); err != nil {
		return err
	}
	l.state = next
	return nil
}

func (l *FileLedger) OutputSent(threadID, itemID string) (bool, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.state.Outputs[threadID][itemID], nil
}

func (l *FileLedger) MarkOutputSent(threadID, itemID string) error {
	if threadID == "" || itemID == "" {
		return errors.New("thread ID and item ID are required")
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	next := cloneLedgerState(l.state)
	if next.Outputs[threadID] == nil {
		next.Outputs[threadID] = make(map[string]bool)
	}
	next.Outputs[threadID][itemID] = true
	if err := l.persist(next); err != nil {
		return err
	}
	l.state = next
	return nil
}

func (l *FileLedger) persist(state ledgerState) error {
	raw, err := json.MarshalIndent(state, "", "  ")
	if err != nil {
		return fmt.Errorf("encode delivery ledger: %w", err)
	}
	directory := filepath.Dir(l.path)
	if err := os.MkdirAll(directory, 0o700); err != nil {
		return fmt.Errorf("create delivery ledger directory: %w", err)
	}
	temporary, err := os.CreateTemp(directory, ".hq-codex-ledger-*")
	if err != nil {
		return fmt.Errorf("create delivery ledger temporary file: %w", err)
	}
	temporaryPath := temporary.Name()
	defer os.Remove(temporaryPath)
	if err := temporary.Chmod(0o600); err != nil {
		temporary.Close()
		return fmt.Errorf("secure delivery ledger: %w", err)
	}
	if _, err := temporary.Write(raw); err != nil {
		temporary.Close()
		return fmt.Errorf("write delivery ledger: %w", err)
	}
	if err := temporary.Sync(); err != nil {
		temporary.Close()
		return fmt.Errorf("sync delivery ledger: %w", err)
	}
	if err := temporary.Close(); err != nil {
		return fmt.Errorf("close delivery ledger: %w", err)
	}
	if err := replaceFile(temporaryPath, l.path); err != nil {
		return fmt.Errorf("replace delivery ledger: %w", err)
	}
	return nil
}

func validDeliveryState(state DeliveryState) bool {
	return state == DeliveryPending || state == DeliveryUncertain || state == DeliveryAccepted
}

func cloneLedgerState(state ledgerState) ledgerState {
	clone := emptyLedgerState()
	for threadID, records := range state.Deliveries {
		clone.Deliveries[threadID] = make(map[string]DeliveryRecord, len(records))
		for messageID, record := range records {
			clone.Deliveries[threadID][messageID] = record
		}
	}
	for threadID, outputs := range state.Outputs {
		clone.Outputs[threadID] = make(map[string]bool, len(outputs))
		for itemID, sent := range outputs {
			clone.Outputs[threadID][itemID] = sent
		}
	}
	return clone
}

type MemoryLedger struct {
	mu    sync.Mutex
	state ledgerState
}

func NewMemoryLedger() *MemoryLedger { return &MemoryLedger{state: emptyLedgerState()} }

func (l *MemoryLedger) Delivery(threadID, messageID string) (DeliveryRecord, bool, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	record, ok := l.state.Deliveries[threadID][messageID]
	return record, ok, nil
}

func (l *MemoryLedger) SetDelivery(threadID, messageID string, state DeliveryState) error {
	if !validDeliveryState(state) {
		return fmt.Errorf("invalid delivery state %q", state)
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.state.Deliveries[threadID] == nil {
		l.state.Deliveries[threadID] = make(map[string]DeliveryRecord)
	}
	l.state.Deliveries[threadID][messageID] = DeliveryRecord{State: state, UpdatedAt: time.Now().UTC()}
	return nil
}

func (l *MemoryLedger) OutputSent(threadID, itemID string) (bool, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.state.Outputs[threadID][itemID], nil
}

func (l *MemoryLedger) MarkOutputSent(threadID, itemID string) error {
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.state.Outputs[threadID] == nil {
		l.state.Outputs[threadID] = make(map[string]bool)
	}
	l.state.Outputs[threadID][itemID] = true
	return nil
}
