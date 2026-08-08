package agenthelp

import _ "embed"

// Text is the agent help shown by `hq agents`.
//
//go:embed instructions.md
var Text string
