package model

// MessageCorrelation is the provider-neutral identity of one harness action.
// The values are opaque to HQ. ThreadID remains HQ's distinct canonical causal
// thread root.
type MessageCorrelation struct {
	Provider    string `json:"provider,omitempty"`
	SessionID   string `json:"session_id,omitempty"`
	OperationID string `json:"operation_id,omitempty"`
	ItemID      string `json:"item_id,omitempty"`
	RequestID   string `json:"request_id,omitempty"`
}

func (c MessageCorrelation) Empty() bool {
	return c.Provider == "" && c.SessionID == "" && c.OperationID == "" && c.ItemID == "" && c.RequestID == ""
}

func (c MessageCorrelation) IsZero() bool { return c.Empty() }

// Valid reports whether the fields form a meaningful identity. Detailed text
// and size validation belongs to the canonical event boundary.
func (c MessageCorrelation) Valid() bool {
	if c.Empty() {
		return true
	}
	if c.Provider == "" || c.SessionID == "" {
		return false
	}
	return c.OperationID != "" || (c.ItemID == "" && c.RequestID == "")
}
