use super::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single read-call proof from one durable Conversation snapshot. This is an observation, not
/// an authorization to open a file: the Host still checks the requested path and frozen workspace.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadToolFileContext {
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub assistant_message_id: String,
    pub message_found: bool,
    pub run_id: Option<String>,
    pub call: Option<ReadToolFileCall>,
    pub call_identity: Option<ReadToolFileIdentity>,
    pub result_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadToolFileCall {
    pub id: String,
    pub tool: Option<String>,
    pub path: Option<String>,
    pub file_path: Option<String>,
}

/// `None` on the context means an old receipt had no identity. A present but malformed identity
/// becomes `Some` with empty fields, preserving the Host's fail-closed behavior for that receipt.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadToolFileIdentity {
    pub r#type: Option<String>,
    pub tool_name: Option<String>,
}

impl StorageService {
    pub fn load_read_tool_file_context(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        call_id: &str,
    ) -> Result<Option<ReadToolFileContext>, String> {
        let mut connection = self.state.read_connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let result = read_context(&transaction, conversation_id, assistant_message_id, call_id)?;
        transaction.commit().map_err(storage_error)?;
        Ok(result)
    }
}

fn read_context(
    connection: &rusqlite::Connection,
    conversation_id: &str,
    assistant_message_id: &str,
    call_id: &str,
) -> Result<Option<ReadToolFileContext>, String> {
    let ids = serde_json::to_string(&[assistant_message_id]).map_err(|error| error.to_string())?;
    let Some(mut conversation) = chat_repository::get_active_conversation_in_message_scope(
        connection,
        conversation_id,
        &ids,
    )
    .map_err(storage_error)?
    else {
        return Ok(None);
    };
    // Durable Trace remains authoritative even when the Renderer has never saved a run JSON.
    // Reuse its exact history projection/legacy merge policy, scoped to one selected message.
    conversations::project_selected_read_tool_messages(connection, &mut conversation)?;
    let message = conversation
        .messages
        .iter()
        .find(|message| message.id == assistant_message_id && message.role == "assistant");
    let run: Value = serde_json::from_str(
        message
            .and_then(|message| message.agent_run_json.as_deref())
            .unwrap_or("{}"),
    )
    .map_err(|_| "The referenced read file is not available.".to_string())?;
    let items = |key| {
        run.get(key)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
    };
    let call = items("toolCalls")
        .iter()
        .find(|call| call.get("id").and_then(Value::as_str) == Some(call_id));
    let marker = items("timeline").iter().find(|item| {
        item.get("type").and_then(Value::as_str) == Some("tool_call")
            && item.get("callId").and_then(Value::as_str) == Some(call_id)
    });
    let result = items("toolResults")
        .iter()
        .rev()
        .find(|result| result.get("callId").and_then(Value::as_str) == Some(call_id));
    let string = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_owned);
    Ok(Some(ReadToolFileContext {
        conversation_id: conversation.id,
        project_id: conversation.project_id,
        assistant_message_id: assistant_message_id.to_owned(),
        message_found: message.is_some(),
        run_id: string(run.get("runId")),
        call: call.map(|call| ReadToolFileCall {
            id: call_id.to_owned(),
            tool: string(call.get("tool")),
            path: string(call.get("args").and_then(|args| args.get("path"))),
            file_path: string(call.get("args").and_then(|args| args.get("filePath"))),
        }),
        call_identity: marker
            .and_then(|marker| marker.get("identity"))
            .map(|identity| ReadToolFileIdentity {
                r#type: string(identity.get("type")),
                tool_name: string(identity.get("toolName")),
            }),
        result_path: string(
            result
                .and_then(|result| result.get("result"))
                .and_then(|result| result.get("path")),
        ),
    }))
}

#[cfg(test)]
mod tests;
