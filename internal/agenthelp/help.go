package agenthelp

import _ "embed"

// Text is the agent help shown by `hq agents`.
//
//go:embed instructions.md
var Text string

//go:embed commands.md
var commands string

//go:embed sync-semantics.md
var syncSemantics string

//go:embed delivery-semantics.md
var deliverySemantics string

var topics = map[string]string{
	"commands":           commands,
	"sync-semantics":     syncSemantics,
	"delivery-semantics": deliverySemantics,
}

// Topic returns the focused agent help for name.
func Topic(name string) (string, bool) {
	text, ok := topics[name]
	return text, ok
}
