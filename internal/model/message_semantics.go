package model

// PresentationKind controls the small set of behavioral message presentation
// modes. Empty means ordinary conversation text.
type PresentationKind string

const (
	PresentationUpdate      PresentationKind = "update"
	PresentationFinalAnswer PresentationKind = "final-answer"
	PresentationStatus      PresentationKind = "status"
	PresentationNotice      PresentationKind = "notice"
)

func (kind PresentationKind) Valid() bool {
	switch kind {
	case "", PresentationUpdate, PresentationFinalAnswer, PresentationStatus, PresentationNotice:
		return true
	default:
		return false
	}
}

// TechnicalSection is ordered diagnostic/display metadata. Namespace records
// provenance, keys are stable machine names, and labels are display-only.
// Domain behavior must never depend on these values.
type TechnicalSection struct {
	Namespace string           `json:"namespace"`
	Fields    []TechnicalField `json:"fields"`
}

type TechnicalField struct {
	Key   string `json:"key"`
	Label string `json:"label,omitempty"`
	Value string `json:"value"`
}
