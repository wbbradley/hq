package event

import (
	"strings"

	"github.com/wbbradley/hq/internal/model"
)

// projectLegacySchema1Message is the only compatibility boundary allowed to
// interpret historical line-oriented message details. It changes only the
// read-side projection; signed schema-1 bytes remain untouched.
func projectLegacySchema1Message(payload TextPayload) TextPayload {
	lines := strings.Split(payload.Details, "\n")
	remove := make([]bool, len(lines))
	values := make(map[string]string)
	indices := make(map[string]int)

	for index, line := range lines {
		trimmed := strings.TrimSpace(line)
		for _, field := range legacyMessageFields {
			value, found := strings.CutPrefix(trimmed, field.prefix)
			if !found {
				continue
			}
			value = legacyProjectedValue(value)
			if _, exists := values[field.name]; !exists {
				values[field.name], indices[field.name] = value, index
			}
			break
		}
	}

	projectFields := legacyProjectFields(payload.Purpose, values)
	legacyHarnessShape := values["provider"] != "" && values["session"] != "" || values["codex_session"] != ""
	if legacyHarnessShape {
		markLegacyFields(remove, indices, "kind", "phase", "provider", "session", "operation", "item", "request", "message", "mailbox", "codex_session", "codex_operation")
	}
	if len(projectFields.fields) != 0 {
		markLegacyFields(remove, indices, "kind")
	}
	if legacyHarnessShape || len(projectFields.fields) != 0 {
		if kind := model.PresentationKind(values["kind"]); kind.Valid() && kind != "" {
			payload.Presentation = kind
		}
		if payload.Presentation == "" && values["phase"] == "final_answer" {
			payload.Presentation = model.PresentationFinalAnswer
		}
	}

	correlation := model.MessageCorrelation{
		Provider: values["provider"], SessionID: values["session"], OperationID: values["operation"],
		ItemID: values["item"], RequestID: values["request"],
	}
	if correlation.SessionID == "" && values["codex_session"] != "" {
		correlation.Provider, correlation.SessionID = "codex", values["codex_session"]
	}
	if correlation.OperationID == "" {
		correlation.OperationID = values["codex_operation"]
	}
	if legacyHarnessShape && validateMessageCorrelation(correlation) == nil {
		payload.Correlation = correlation
	}

	var sections []model.TechnicalSection
	if legacyHarnessShape && (values["phase"] != "" || values["mailbox"] != "") {
		section := model.TechnicalSection{Namespace: "hq.legacy.harness"}
		appendLegacyField(&section, "phase", "Phase", values["phase"])
		appendLegacyField(&section, "mailbox_id", "HQ mailbox", values["mailbox"])
		sections = append(sections, section)
	}

	if len(projectFields.fields) != 0 {
		section := model.TechnicalSection{Namespace: projectFields.namespace}
		for _, field := range projectFields.fields {
			appendLegacyField(&section, field.key, field.label, values[field.name])
			if index, ok := indices[field.name]; ok {
				remove[index] = true
			}
		}
		sections = append(sections, section)
	}

	payload.Details = visibleLegacyDetails(lines, remove)
	payload.TechnicalSections = sections
	return payload
}

type legacyMessageField struct {
	name   string
	prefix string
}

var legacyMessageFields = []legacyMessageField{
	{name: "kind", prefix: "Kind:"},
	{name: "phase", prefix: "Phase:"},
	{name: "provider", prefix: "Harness provider:"},
	{name: "session", prefix: "Harness session:"},
	{name: "operation", prefix: "Harness operation:"},
	{name: "item", prefix: "Harness item:"},
	{name: "request", prefix: "Harness request:"},
	{name: "message", prefix: "HQ message:"},
	{name: "mailbox", prefix: "HQ mailbox:"},
	{name: "codex_session", prefix: "Codex thread:"},
	{name: "codex_operation", prefix: "Codex turn:"},
	{name: "project", prefix: "Project:"},
	{name: "project_assignment", prefix: "Project assignment:"},
	{name: "project_thread", prefix: "Project thread:"},
	{name: "late", prefix: "Late from inactive assignment:"},
	{name: "current_assignment", prefix: "Current assignment:"},
	{name: "current_agent", prefix: "Current agent:"},
	{name: "current_project_thread", prefix: "Current project thread:"},
	{name: "resource", prefix: "Resource:"},
	{name: "previous_health", prefix: "Previous health:"},
	{name: "current_health", prefix: "Current health:"},
	{name: "health_details", prefix: "Health details:"},
	{name: "pending_message", prefix: "Pending message:"},
	{name: "lifecycle", prefix: "Lifecycle:"},
	{name: "archived", prefix: "Archived:"},
}

func markLegacyFields(remove []bool, indices map[string]int, names ...string) {
	for _, name := range names {
		if index, ok := indices[name]; ok {
			remove[index] = true
		}
	}
}

type legacyProjectField struct {
	name  string
	key   string
	label string
}

type legacyProjectSection struct {
	namespace string
	fields    []legacyProjectField
}

func legacyProjectFields(purpose model.MessagePurpose, values map[string]string) legacyProjectSection {
	if purpose == model.MessagePurposeProjectOutput && values["project"] != "" && values["project_assignment"] != "" && values["project_thread"] != "" {
		return legacyProjectSection{namespace: "hq.legacy.project_output_provenance", fields: []legacyProjectField{
			{name: "project", key: "project_id", label: "Project"},
			{name: "project_assignment", key: "assignment_id", label: "Project assignment"},
			{name: "project_thread", key: "project_thread_id", label: "Project thread"},
			{name: "late", key: "late", label: "Late from inactive assignment"},
			{name: "current_assignment", key: "current_assignment_id", label: "Current assignment"},
			{name: "current_agent", key: "current_agent", label: "Current agent"},
			{name: "current_project_thread", key: "current_project_thread_id", label: "Current project thread"},
		}}
	}
	if purpose != model.MessagePurposeSystemNotice || values["project"] == "" {
		return legacyProjectSection{}
	}
	if values["resource"] != "" && values["previous_health"] != "" && values["current_health"] != "" {
		return legacyProjectSection{namespace: "hq.legacy.project_notice", fields: []legacyProjectField{
			{name: "project", key: "project_id", label: "Project"},
			{name: "resource", key: "resource_id", label: "Resource"},
			{name: "previous_health", key: "previous_health", label: "Previous health"},
			{name: "current_health", key: "current_health", label: "Current health"},
			{name: "health_details", key: "health_details", label: "Health details"},
		}}
	}
	if values["pending_message"] != "" && values["lifecycle"] != "" && values["archived"] != "" {
		return legacyProjectSection{namespace: "hq.legacy.project_notice", fields: []legacyProjectField{
			{name: "project", key: "project_id", label: "Project"},
			{name: "pending_message", key: "pending_message_id", label: "Pending message"},
			{name: "lifecycle", key: "lifecycle", label: "Lifecycle"},
			{name: "archived", key: "archived", label: "Archived"},
		}}
	}
	return legacyProjectSection{}
}

func appendLegacyField(section *model.TechnicalSection, key, label, value string) {
	if value == "" {
		return
	}
	section.Fields = append(section.Fields, model.TechnicalField{Key: key, Label: label, Value: value})
}

func legacyProjectedValue(value string) string {
	value = strings.TrimSpace(value)
	if value == "(none)" {
		return ""
	}
	return value
}

func visibleLegacyDetails(lines []string, remove []bool) string {
	visible := make([]string, 0, len(lines))
	for index, line := range lines {
		if !remove[index] {
			visible = append(visible, line)
		}
	}
	for len(visible) != 0 && strings.TrimSpace(visible[0]) == "" {
		visible = visible[1:]
	}
	for len(visible) != 0 && strings.TrimSpace(visible[len(visible)-1]) == "" {
		visible = visible[:len(visible)-1]
	}
	return strings.Join(visible, "\n")
}

func cloneTechnicalSections(sections []model.TechnicalSection) []model.TechnicalSection {
	if sections == nil {
		return nil
	}
	cloned := make([]model.TechnicalSection, len(sections))
	for index, section := range sections {
		cloned[index] = section
		cloned[index].Fields = append([]model.TechnicalField(nil), section.Fields...)
	}
	return cloned
}
