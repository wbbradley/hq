//! Handwritten private DTO subset checked against the pinned generated schema.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireError {
    #[allow(dead_code)]
    pub code: i64,
    #[allow(dead_code)]
    pub message: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub data: Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireMessage {
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<WireError>,
}

#[derive(Serialize)]
pub(crate) struct ClientRequest<'a, T> {
    pub method: &'a str,
    pub id: u64,
    pub params: T,
}

#[derive(Serialize)]
pub(crate) struct ClientNotification<'a, T> {
    pub method: &'a str,
    pub params: T,
}

#[derive(Serialize)]
pub(crate) struct ClientResult<T> {
    pub id: Value,
    pub result: T,
}

#[derive(Serialize)]
pub(crate) struct ClientError<'a> {
    pub id: Value,
    pub error: ClientErrorBody<'a>,
}

#[derive(Serialize)]
pub(crate) struct ClientErrorBody<'a> {
    pub code: i64,
    pub message: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitializeParams<'a> {
    pub client_info: ClientInfo<'a>,
    pub capabilities: InitializeCapabilities,
}

#[derive(Serialize)]
pub(crate) struct ClientInfo<'a> {
    pub name: &'a str,
    pub title: &'a str,
    pub version: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitializeCapabilities {
    pub experimental_api: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadStartParams<'a> {
    pub cwd: &'a str,
    pub developer_instructions: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadResumeParams<'a> {
    pub thread_id: &'a str,
    pub cwd: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadReadParams<'a> {
    pub thread_id: &'a str,
    pub include_turns: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ThreadResponse {
    pub thread: Thread,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Thread {
    pub id: String,
    #[serde(default)]
    pub turns: Vec<Turn>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Turn {
    pub id: String,
    #[serde(default, deserialize_with = "string_or_default")]
    pub status: String,
    #[serde(default)]
    pub items: Vec<ThreadItem>,
    #[serde(default)]
    #[allow(dead_code)]
    pub error: Option<TurnError>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnError {
    #[serde(default, deserialize_with = "string_or_default")]
    #[allow(dead_code)]
    pub message: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadItem {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    #[serde(default, deserialize_with = "string_or_default")]
    pub client_id: String,
    #[serde(default, deserialize_with = "string_or_default")]
    pub text: String,
    #[serde(default, deserialize_with = "string_or_default")]
    pub phase: String,
    #[serde(default, deserialize_with = "string_or_default")]
    pub status: String,
    #[serde(default, deserialize_with = "string_or_default")]
    pub command: String,
    #[serde(default)]
    pub aggregated_output: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub changes: Vec<FileChange>,
    #[serde(default, deserialize_with = "string_or_default")]
    pub server: String,
    #[serde(default, deserialize_with = "string_or_default")]
    pub tool: String,
    #[serde(default, deserialize_with = "string_or_default")]
    pub query: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct FileChange {
    pub path: String,
    #[serde(default)]
    pub diff: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TextInput<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnStartParams<'a> {
    pub thread_id: &'a str,
    pub input: [TextInput<'a>; 1],
    pub client_user_message_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TurnResponse {
    pub turn: Turn,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnSteerParams<'a> {
    pub thread_id: &'a str,
    pub expected_turn_id: &'a str,
    pub input: [TextInput<'a>; 1],
    pub client_user_message_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnSteerResponse {
    pub turn_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnInterruptParams<'a> {
    pub thread_id: &'a str,
    pub turn_id: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnNotification {
    pub thread_id: String,
    pub turn: Turn,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ItemNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item: ThreadItem,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanNotification {
    pub thread_id: String,
    pub turn_id: String,
    #[serde(default)]
    pub explanation: Option<String>,
    #[serde(default)]
    pub plan: Vec<PlanStep>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PlanStep {
    pub step: String,
    pub status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiffNotification {
    pub thread_id: String,
    pub turn_id: String,
    #[serde(default)]
    pub diff: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProgressNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    #[serde(default, alias = "delta")]
    pub message: String,
}

fn string_or_default<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Option::unwrap_or_default)
}
