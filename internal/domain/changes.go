package domain

import "context"

type ChangeTopic string

const (
	TopicMessages  ChangeTopic = "messages"
	TopicMailboxes ChangeTopic = "mailboxes"
	TopicNetwork   ChangeTopic = "network"
	TopicPeers     ChangeTopic = "peers"
	TopicHuman     ChangeTopic = "human"
	TopicRelays    ChangeTopic = "relays"
)

type Invalidation struct {
	Revision     uint64        `json:"revision"`
	Topics       []ChangeTopic `json:"topics,omitempty"`
	FullSnapshot bool          `json:"full_snapshot,omitempty"`
}

type ChangeLog interface {
	CurrentRevision(context.Context) (uint64, error)
}
