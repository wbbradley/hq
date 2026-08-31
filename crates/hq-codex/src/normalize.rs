//! Provider-private notification normalization.

use std::{collections::BTreeMap, fmt::Write as _, num::NonZeroU64};

use hq_domain::{
    ActivityKind, ActivityStatus, CONTENT_MAX_BYTES, ContentText, ErrorCode, MessageId,
    OperationId, SHORT_TEXT_MAX_BYTES, ShortText,
};
use hq_harness::{HarnessActivity, HarnessEvent, HarnessOutput, HarnessOutputKind};
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::protocol::{
    DiffNotification, ItemNotification, PlanNotification, ProgressNotification, ThreadItem,
    TurnNotification,
};

const RUNTIME: &str = "codex";

pub(crate) struct Normalizer {
    sequence: u64,
}

impl Normalizer {
    pub(crate) const fn new() -> Self {
        Self { sequence: 0 }
    }

    pub(crate) fn notification(
        &mut self,
        method: &str,
        params: Value,
        thread_id: &str,
        operations: &BTreeMap<String, OperationId>,
    ) -> Vec<HarnessEvent> {
        let event = match method {
            "turn/started" | "turn/completed" => self.turn(method, params, thread_id, operations),
            "item/completed" => self.item(params, thread_id, operations),
            "turn/plan/updated" => self.plan(params, thread_id, operations),
            "turn/diff/updated" => self.diff(params, thread_id, operations),
            "item/plan/delta"
            | "item/commandExecution/outputDelta"
            | "item/fileChange/outputDelta"
            | "item/mcpToolCall/progress" => self.progress(params, thread_id, operations),
            _ => None,
        };
        event.into_iter().collect()
    }

    fn turn(
        &mut self,
        method: &str,
        params: Value,
        thread_id: &str,
        operations: &BTreeMap<String, OperationId>,
    ) -> Option<HarnessEvent> {
        let value = parse::<TurnNotification>(params)?;
        let operation_id =
            context_operation(&value.thread_id, &value.turn.id, thread_id, operations)?;
        if method == "turn/started" {
            return self.activity(
                operation_id,
                None,
                ActivityKind::AgentTurn,
                "operation",
                ActivityStatus::Running,
                "Codex turn started",
            );
        }
        let status = turn_status(&value.turn.status)?;
        let content = match status {
            ActivityStatus::Succeeded => "Codex turn completed",
            ActivityStatus::Interrupted => "Codex turn interrupted",
            ActivityStatus::Failed(_) => "Codex turn failed",
            _ => return None,
        };
        self.activity(
            operation_id,
            None,
            ActivityKind::AgentTurn,
            "operation",
            status,
            content,
        )
    }

    fn item(
        &mut self,
        params: Value,
        thread_id: &str,
        operations: &BTreeMap<String, OperationId>,
    ) -> Option<HarnessEvent> {
        let value = parse::<ItemNotification>(params)?;
        let operation_id =
            context_operation(&value.thread_id, &value.turn_id, thread_id, operations)?;
        match completed_item(&value.item)? {
            CompletedItem::Output { body, final_answer } => {
                output(operation_id, &value.item.id, &body, final_answer)
            }
            CompletedItem::Activity {
                content,
                status,
                logical_key,
            } => self.activity(
                operation_id,
                Some(&value.item.id),
                ActivityKind::CompletedItem,
                logical_key,
                status,
                &content,
            ),
        }
    }

    fn plan(
        &mut self,
        params: Value,
        thread_id: &str,
        operations: &BTreeMap<String, OperationId>,
    ) -> Option<HarnessEvent> {
        let value = parse::<PlanNotification>(params)?;
        let operation_id =
            context_operation(&value.thread_id, &value.turn_id, thread_id, operations)?;
        self.activity(
            operation_id,
            None,
            ActivityKind::Plan,
            "plan",
            ActivityStatus::Snapshot,
            &format_plan(&value),
        )
    }

    fn diff(
        &mut self,
        params: Value,
        thread_id: &str,
        operations: &BTreeMap<String, OperationId>,
    ) -> Option<HarnessEvent> {
        let value = parse::<DiffNotification>(params)?;
        let operation_id =
            context_operation(&value.thread_id, &value.turn_id, thread_id, operations)?;
        self.activity(
            operation_id,
            None,
            ActivityKind::Diff,
            "diff",
            ActivityStatus::Snapshot,
            value
                .diff
                .as_deref()
                .filter(|content| !content.is_empty())
                .unwrap_or("(no changes)"),
        )
    }

    fn progress(
        &mut self,
        params: Value,
        thread_id: &str,
        operations: &BTreeMap<String, OperationId>,
    ) -> Option<HarnessEvent> {
        let value = parse::<ProgressNotification>(params)?;
        let operation_id =
            context_operation(&value.thread_id, &value.turn_id, thread_id, operations)?;
        if value.item_id.is_empty() || value.message.is_empty() {
            return None;
        }
        self.activity(
            operation_id,
            Some(&value.item_id),
            ActivityKind::Progress,
            "progress",
            ActivityStatus::Running,
            &value.message,
        )
    }

    fn activity(
        &mut self,
        operation_id: OperationId,
        item: Option<&str>,
        kind: ActivityKind,
        logical_key: &str,
        status: ActivityStatus,
        content: &str,
    ) -> Option<HarnessEvent> {
        self.sequence = self.sequence.saturating_add(1).max(1);
        let (content, truncated) = bounded(content, CONTENT_MAX_BYTES);
        Some(HarnessEvent::Activity(HarnessActivity {
            operation_id,
            item: item.and_then(|value| short(value).ok()),
            kind,
            logical_key: short(logical_key).ok()?,
            runtime: short(RUNTIME).ok()?,
            sequence: NonZeroU64::new(self.sequence)?,
            status,
            content: ContentText::new(if content.is_empty() {
                "Codex activity".to_owned()
            } else {
                content
            })
            .ok()?,
            truncated,
        }))
    }
}

fn output(
    operation_id: OperationId,
    item_id: &str,
    body: &str,
    final_answer: bool,
) -> Option<HarnessEvent> {
    let (body, _) = bounded(body, CONTENT_MAX_BYTES);
    Some(HarnessEvent::Output(HarnessOutput {
        output_id: stable_message_id(item_id.as_bytes()),
        operation_id,
        kind: if final_answer {
            HarnessOutputKind::FinalAnswer
        } else {
            HarnessOutputKind::Update
        },
        status: if final_answer {
            ActivityStatus::Succeeded
        } else {
            ActivityStatus::Running
        },
        body: ContentText::new(body).ok()?,
    }))
}

fn parse<T: DeserializeOwned>(params: Value) -> Option<T> {
    serde_json::from_value(params).ok()
}

fn context_operation(
    observed_thread: &str,
    turn_id: &str,
    expected_thread: &str,
    operations: &BTreeMap<String, OperationId>,
) -> Option<OperationId> {
    (observed_thread == expected_thread && !turn_id.is_empty())
        .then(|| operations.get(turn_id).copied())
        .flatten()
}

fn turn_status(value: &str) -> Option<ActivityStatus> {
    match value {
        "completed" => Some(ActivityStatus::Succeeded),
        "failed" => ErrorCode::new("codex_turn_failed")
            .ok()
            .map(ActivityStatus::Failed),
        "interrupted" => Some(ActivityStatus::Interrupted),
        _ => None,
    }
}

enum CompletedItem {
    Output {
        body: String,
        final_answer: bool,
    },
    Activity {
        content: String,
        status: ActivityStatus,
        logical_key: &'static str,
    },
}

fn completed_item(item: &ThreadItem) -> Option<CompletedItem> {
    match item.kind.as_str() {
        "agentMessage" if !item.text.trim().is_empty() => Some(CompletedItem::Output {
            body: item.text.clone(),
            final_answer: item.phase == "final_answer",
        }),
        "commandExecution" if !item.command.trim().is_empty() => command_item(item),
        "fileChange" => file_item(item),
        "mcpToolCall" | "dynamicToolCall" | "collabAgentToolCall" => tool_item(item),
        "webSearch" if !item.query.trim().is_empty() => Some(CompletedItem::Activity {
            content: item.query.clone(),
            status: ActivityStatus::Succeeded,
            logical_key: "web-search",
        }),
        _ => None,
    }
}

fn command_item(item: &ThreadItem) -> Option<CompletedItem> {
    let status = item_status(&item.status)?;
    let mut content = item.command.clone();
    if let Some(output) = item
        .aggregated_output
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        content.push('\n');
        content.push_str(output);
    }
    if let Some(exit_code) = item.exit_code {
        let _ = write!(content, "\nExit code: {exit_code}");
    }
    Some(CompletedItem::Activity {
        content,
        status,
        logical_key: "command",
    })
}

fn file_item(item: &ThreadItem) -> Option<CompletedItem> {
    let status = item_status(&item.status)?;
    let content = if item.changes.is_empty() {
        "File changes completed".to_owned()
    } else {
        item.changes
            .iter()
            .map(|change| {
                if change.diff.is_empty() {
                    change.path.clone()
                } else {
                    format!("{}\n{}", change.path, change.diff)
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    Some(CompletedItem::Activity {
        content,
        status,
        logical_key: "file-change",
    })
}

fn tool_item(item: &ThreadItem) -> Option<CompletedItem> {
    let status = item_status(&item.status)?;
    let name = match item.kind.as_str() {
        "mcpToolCall" => format!("{}/{}", item.server, item.tool),
        "collabAgentToolCall" => format!("collab/{}", item.tool),
        _ => item.tool.clone(),
    };
    (!name.trim_matches('/').is_empty()).then_some(CompletedItem::Activity {
        content: name,
        status,
        logical_key: "tool",
    })
}

fn item_status(value: &str) -> Option<ActivityStatus> {
    match value {
        "completed" => Some(ActivityStatus::Succeeded),
        "failed" => ErrorCode::new("codex_item_failed")
            .ok()
            .map(ActivityStatus::Failed),
        "declined" => Some(ActivityStatus::Interrupted),
        _ => None,
    }
}

fn format_plan(value: &PlanNotification) -> String {
    let mut lines = value
        .explanation
        .iter()
        .filter(|line| !line.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>();
    lines.extend(value.plan.iter().filter_map(|step| {
        let text = step.step.trim();
        if text.is_empty() {
            return None;
        }
        let marker = match step.status.as_str() {
            "completed" => "[x]",
            "inProgress" => "[~]",
            _ => "[ ]",
        };
        Some(format!("- {marker} {text}"))
    }));
    if lines.is_empty() {
        "(no plan)".to_owned()
    } else {
        lines.join("\n")
    }
}

fn short(value: &str) -> Result<ShortText, ()> {
    let (value, _) = bounded(value, SHORT_TEXT_MAX_BYTES);
    ShortText::new(value).map_err(|_| ())
}

fn bounded(value: &str, maximum: usize) -> (String, bool) {
    if value.len() <= maximum {
        return (value.to_owned(), false);
    }
    let mut end = maximum;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_owned(), true)
}

fn stable_message_id(value: &[u8]) -> MessageId {
    let mut digest = Sha256::new();
    digest.update(b"hq.codex.output.v1\0");
    digest.update(value);
    MessageId::from_bytes(digest.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn turn_lifecycle_is_typed_independently_from_generic_status() {
        let operation = OperationId::from_bytes([7; 32]);
        let operations = BTreeMap::from([("turn-1".to_owned(), operation)]);
        let mut normalizer = Normalizer::new();

        let events = normalizer.notification(
            "turn/started",
            json!({
                "threadId": "thread-1",
                "turn": {"id": "turn-1", "status": "inProgress", "items": []}
            }),
            "thread-1",
            &operations,
        );

        assert!(matches!(
            events.as_slice(),
            [HarnessEvent::Activity(HarnessActivity {
                kind: ActivityKind::AgentTurn,
                status: ActivityStatus::Running,
                ..
            })]
        ));
    }
}
