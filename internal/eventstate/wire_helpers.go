package eventstate

import (
	"encoding/json"

	"github.com/wbbradley/hq/internal/eventwire"
	"github.com/wbbradley/hq/internal/model"
)

func decodePayload(raw json.RawMessage, target any) error {
	return eventwire.DecodePayload(raw, target)
}

func validateMessageCorrelation(correlation model.MessageCorrelation) error {
	return eventwire.ValidateMessageCorrelation(correlation)
}

func decodeTextPayload(raw json.RawMessage, schema int) (eventwire.TextPayload, error) {
	return eventwire.DecodeTextPayload(raw, schema)
}
