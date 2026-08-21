package syncer

const (
	wakeMethod    = "lifecycle/wake"
	statusMethod  = "lifecycle/status"
	stopMethod    = "lifecycle/stop"
	restartMethod = "lifecycle/restart"
)

type lifecycleStatus struct {
	State string `json:"state"`
}

type lifecycleAcknowledgement struct {
	State string `json:"state"`
}
