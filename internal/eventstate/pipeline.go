package eventstate

// reductionStage names one deterministic, effect-free projection step. The
// explicit pipeline makes ordering dependencies reviewable without introducing
// a second reducer abstraction or allowing individual callers to select stages.
type reductionStage struct {
	name  string
	apply func(*State)
}

type reductionPipeline []reductionStage

func (p reductionPipeline) apply(state *State) {
	for _, stage := range p {
		stage.apply(state)
	}
}

func (p reductionPipeline) stageNames() []string {
	names := make([]string, len(p))
	for index, stage := range p {
		names[index] = stage.name
	}
	return names
}

var canonicalReductionPipeline = reductionPipeline{
	{name: "local-controls", apply: (*State).classifyLocalControls},
	{name: "peer-bindings", apply: (*State).reducePeers},
	{name: "mailbox-access-classification", apply: (*State).classifyMailboxAccessEvents},
	{name: "mailbox-access-projection", apply: (*State).projectMailboxAccess},
	{name: "account-authority-classification", apply: (*State).classifyAccountEvents},
	{name: "account-projection", apply: (*State).projectAccounts},
	{name: "account-selection-classification", apply: (*State).classifyAccountSelections},
	{name: "default-account-projection", apply: (*State).projectDefaultAccount},
	{name: "domain-event-classification", apply: (*State).classifyDomainEvents},
	{name: "mailbox-projection", apply: (*State).projectMailboxes},
	{name: "named-agent-classification", apply: (*State).classifyNamedAgents},
	{name: "named-agent-projection", apply: (*State).projectNamedAgents},
	{name: "message-projection", apply: (*State).projectMessages},
	{name: "message-state", apply: (*State).applyMessageState},
	{name: "thread-projection", apply: (*State).projectThreads},
	{name: "message-order", apply: (*State).projectMessageOrder},
	{name: "conversation-order", apply: (*State).projectConversationOrder},
	{name: "harness-activity-projection", apply: (*State).projectHarnessActivities},
}

func (s *State) projectMessageOrder() {
	s.DisplayOrder = s.orderMessages()
}

func (s *State) projectConversationOrder() {
	s.ConversationOrder = s.orderConversationEvents()
}
