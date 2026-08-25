package domain

import "context"

type ChangeTopic string

const (
	TopicMessages   ChangeTopic = "messages"
	TopicMailboxes  ChangeTopic = "mailboxes"
	TopicNetwork    ChangeTopic = "network"
	TopicPeers      ChangeTopic = "peers"
	TopicHuman      ChangeTopic = "human"
	TopicRelays     ChangeTopic = "relays"
	TopicAgents     ChangeTopic = "agents"
	TopicProjects   ChangeTopic = "projects"
	TopicActivities ChangeTopic = "activities"
)

type Invalidation struct {
	Revision     uint64        `json:"revision"`
	Topics       []ChangeTopic `json:"topics,omitempty"`
	FullSnapshot bool          `json:"full_snapshot,omitempty"`
}

type ChangeLog interface {
	CurrentRevision(context.Context) (uint64, error)
}

type ChangeSubscription interface {
	Changes() <-chan Invalidation
	Close()
}

type ChangeSubscriber interface {
	Subscribe(context.Context, ...ChangeTopic) (ChangeSubscription, error)
}

type ConnectionUpdate struct {
	Diagnostic string
	Blocking   bool
}

type ClientUpdates struct {
	Subscribe func(context.Context, ...ChangeTopic) (ChangeSubscription, error)
	Initial   ConnectionUpdate
	States    <-chan ConnectionUpdate
}
