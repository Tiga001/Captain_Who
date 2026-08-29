use super::input_stream::{JsonStringFieldEvent, TopLevelJsonStringStream};
use super::{
    ToolExecutionContext, ToolInputStreamChunk, ToolInputStreamObserver, ToolInputStreamPreview,
};
use crate::protocol::{
    AgentError, AgentFileChangePreview, AgentResult, AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
};
use crate::storage::models::AgentFileChangeRecord;
use crate::storage::now_ms;
use std::time::{Duration, Instant};

const PREVIEW_INTERVAL: Duration = Duration::from_millis(120);
const MAX_PREVIEW_PATH_BYTES: usize = 16 * 1024;

#[derive(Default)]
struct StreamingTextMetrics {
    byte_count: u64,
    newline_count: u64,
    has_content: bool,
    ends_with_newline: bool,
    pending_content: String,
}

impl StreamingTextMetrics {
    fn push(&mut self, value: &str) {
        if value.is_empty() {
            return;
        }
        self.byte_count = self.byte_count.saturating_add(value.len() as u64);
        self.newline_count = self
            .newline_count
            .saturating_add(value.bytes().filter(|byte| *byte == b'\n').count() as u64);
        self.has_content = true;
        self.ends_with_newline = value.ends_with('\n');
        self.pending_content.push_str(value);
    }

    fn line_count(&self) -> u64 {
        self.newline_count
            .saturating_add(u64::from(self.has_content && !self.ends_with_newline))
    }

    fn take_pending_content(&mut self) -> String {
        std::mem::take(&mut self.pending_content)
    }
}

#[derive(Default)]
struct StreamMetadata {
    stream_id: String,
    attempt: usize,
    tool_call_index: usize,
    received_bytes: u64,
}

pub(crate) struct FileChangeInputStreamObserver {
    context: ToolExecutionContext,
    parser: TopLevelJsonStringStream,
    action: String,
    action_complete: bool,
    transaction_id: String,
    transaction_id_complete: bool,
    file_path: String,
    file_path_complete: bool,
    content: StreamingTextMetrics,
    transaction: Option<AgentFileChangeRecord>,
    metadata: StreamMetadata,
    preview_id: Option<String>,
    last_emitted_bytes: u64,
    last_emitted_at: Option<Instant>,
    disabled: bool,
}

impl FileChangeInputStreamObserver {
    pub(crate) fn apply_patch(context: ToolExecutionContext) -> Self {
        Self {
            context,
            parser: TopLevelJsonStringStream::nested_object("request"),
            action: String::new(),
            action_complete: false,
            transaction_id: String::new(),
            transaction_id_complete: false,
            file_path: String::new(),
            file_path_complete: false,
            content: StreamingTextMetrics::default(),
            transaction: None,
            metadata: StreamMetadata::default(),
            preview_id: None,
            last_emitted_bytes: 0,
            last_emitted_at: None,
            disabled: false,
        }
    }

    fn consume_events(&mut self, events: Vec<JsonStringFieldEvent>) {
        for event in events {
            match event {
                JsonStringFieldEvent::Delta { field, value } => match field.as_str() {
                    "action" => self.action.push_str(&value),
                    "transactionId" => self.transaction_id.push_str(&value),
                    "filePath" => self.file_path.push_str(&value),
                    "content" => self.content.push(&value),
                    _ => {}
                },
                JsonStringFieldEvent::Completed { field } => match field.as_str() {
                    "action" => self.action_complete = true,
                    "transactionId" => self.transaction_id_complete = true,
                    "filePath" => self.file_path_complete = true,
                    _ => {}
                },
            }
        }
    }

    fn load_transaction_if_ready(&mut self) {
        if self.disabled || self.transaction.is_some() || !self.action_complete {
            return;
        }

        if self.action == "apply" {
            if self.file_path_complete
                && (self.file_path.trim().is_empty()
                    || self.file_path.len() > MAX_PREVIEW_PATH_BYTES)
            {
                self.disabled = true;
            }
            return;
        }

        if self.action != "append" {
            self.disabled = true;
            return;
        }
        if !self.transaction_id_complete {
            return;
        }
        if self.transaction_id.trim().is_empty() {
            self.disabled = true;
            return;
        }

        let Ok(storage) = self.context.storage() else {
            self.disabled = true;
            return;
        };
        let Ok(conversation_id) = self.context.conversation_id() else {
            self.disabled = true;
            return;
        };
        let Ok(run_id) = self.context.run_id() else {
            self.disabled = true;
            return;
        };
        let Ok(Some(transaction)) = storage.get_agent_file_change_for_owner(
            self.transaction_id.trim(),
            conversation_id,
            self.context.project_id(),
            run_id,
            "apply_patch",
        ) else {
            self.disabled = true;
            return;
        };
        if !matches!(transaction.status.as_str(), "drafting" | "ready")
            || transaction.expires_at <= now_ms()
        {
            self.disabled = true;
            return;
        }
        self.transaction = Some(transaction);
    }

    fn preview(&mut self, force: bool) -> AgentResult<Option<AgentFileChangePreview>> {
        self.load_transaction_if_ready();
        let direct =
            self.action == "apply" && self.file_path_complete && !self.file_path.trim().is_empty();
        if !direct && self.transaction.is_none() {
            return Ok(None);
        }
        if self.content.byte_count == 0 || self.content.byte_count == self.last_emitted_bytes {
            return Ok(None);
        }
        if !force
            && self
                .last_emitted_at
                .is_some_and(|last| last.elapsed() < PREVIEW_INTERVAL)
        {
            return Ok(None);
        }

        let preview_id = self.preview_id.get_or_insert_with(|| {
            format!(
                "{}:{}:{}",
                self.metadata.stream_id, self.metadata.attempt, self.metadata.tool_call_index
            )
        });
        let (transaction_id, file_path, additions, deletions, line_count, byte_count) =
            if let Some(transaction) = self.transaction.as_ref() {
                (
                    transaction.id.clone(),
                    transaction.file_path.clone(),
                    transaction
                        .additions
                        .saturating_add(self.content.line_count()),
                    transaction.deletions,
                    appended_line_count(transaction, &self.content),
                    transaction
                        .byte_count
                        .saturating_add(self.content.byte_count),
                )
            } else {
                (
                    preview_id.clone(),
                    self.file_path.clone(),
                    self.content.line_count(),
                    0,
                    self.content.line_count(),
                    self.content.byte_count,
                )
            };
        let content_offset_bytes = self.last_emitted_bytes;
        let content_delta = self.content.take_pending_content();
        let preview = AgentFileChangePreview {
            schema_version: AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
            preview_id: preview_id.clone(),
            stream_id: self.metadata.stream_id.clone(),
            attempt: self.metadata.attempt,
            tool_call_index: self.metadata.tool_call_index,
            tool_call_id: None,
            transaction_id,
            file_path,
            additions,
            deletions,
            line_count,
            byte_count,
            generated_bytes: self.content.byte_count,
            content_offset_bytes,
            content_delta,
            updated_at: now_ms(),
        };
        self.last_emitted_bytes = self.content.byte_count;
        self.last_emitted_at = Some(Instant::now());
        preview.validate().map_err(AgentError::new)?;
        Ok(Some(preview))
    }
}

impl ToolInputStreamObserver for FileChangeInputStreamObserver {
    fn on_delta(
        &mut self,
        chunk: &ToolInputStreamChunk<'_>,
    ) -> crate::protocol::AgentResult<Option<ToolInputStreamPreview>> {
        self.metadata.stream_id = chunk.stream_id.to_string();
        self.metadata.attempt = chunk.attempt;
        self.metadata.tool_call_index = chunk.tool_call_index;
        self.metadata.received_bytes = chunk.received_bytes;
        let events = self.parser.push(chunk.input_delta);
        self.consume_events(events);
        Ok(self.preview(false)?.map(ToolInputStreamPreview::FileChange))
    }

    fn flush(&mut self) -> crate::protocol::AgentResult<Option<ToolInputStreamPreview>> {
        Ok(self.preview(true)?.map(ToolInputStreamPreview::FileChange))
    }
}

fn appended_line_count(draft: &AgentFileChangeRecord, generated: &StreamingTextMetrics) -> u64 {
    let existing_newlines = draft.content.bytes().filter(|byte| *byte == b'\n').count() as u64;
    let has_content = !draft.content.is_empty() || generated.has_content;
    let ends_with_newline = if generated.has_content {
        generated.ends_with_newline
    } else {
        draft.content.ends_with('\n')
    };
    existing_newlines
        .saturating_add(generated.newline_count)
        .saturating_add(u64::from(has_content && !ends_with_newline))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AgentCommandPermission, AgentPatchPermission, AgentPermissions, AgentReadPermission,
        AgentRunContext, AgentWorkspaceContext, AgentWritePermission,
    };
    use crate::storage::models::ChatConversationRecord;
    use crate::storage::service::StorageService;
    use std::fs;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[test]
    fn streams_utf8_append_metrics_without_changing_the_draft() {
        let fixture = tempdir().unwrap();
        let workspace = fixture.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let storage = Arc::new(StorageService::open(&fixture.path().join("app.db")).unwrap());
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-1".to_string(),
                project_id: None,
                model_id: None,
                title: "Test".to_string(),
                messages: Vec::new(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        storage
            .create_agent_file_change(AgentFileChangeRecord {
                schema_version: crate::file_change::FILE_CHANGE_SCHEMA_VERSION,
                id: "draft-1".to_string(),
                conversation_id: "conversation-1".to_string(),
                project_id: None,
                run_id: "run-1".to_string(),
                source_tool_name: "apply_patch".to_string(),
                source_tool_call_id: "call-begin".to_string(),
                source_tool_arguments_digest: crate::file_change::proposal_digest(
                    &serde_json::json!({
                        "request": {
                            "action": "begin",
                            "operation": "create",
                            "filePath": "src/main.rs",
                            "observationId": "fobs-preview-fixture"
                        }
                    }),
                )
                .unwrap(),
                permission_revision: "permission-1".to_string(),
                tool_set_revision: "tool-set-1".to_string(),
                provider_wire_revision: "provider-protocol-v1".to_string(),
                observation_id: "fobs_preview_fixture".to_string(),
                observation_json: "{}".to_string(),
                file_path: "src/main.rs".to_string(),
                operation: "create".to_string(),
                strategy: None,
                status: "drafting".to_string(),
                base_revision: None,
                base_content: String::new(),
                content: String::new(),
                draft_revision: 0,
                next_mutation_index: 0,
                additions: 0,
                deletions: 0,
                line_count: 0,
                byte_count: 0,
                mutation_count: 0,
                stats_final: false,
                summary: None,
                final_action_id: None,
                final_action_arguments_digest: None,
                final_permission_revision: None,
                final_tool_set_revision: None,
                final_provider_wire_revision: None,
                created_at: 1,
                updated_at: 1,
                expires_at: i64::MAX,
            })
            .unwrap();
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-1".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some(workspace.to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                command_safety: Default::default(),
                patch: AgentPatchPermission::RequireApproval,
                builtin_execution: Default::default(),
            },
        }))
        .with_runtime_services("run-1".to_string(), Some(storage.clone()));
        let mut observer = FileChangeInputStreamObserver::apply_patch(context);

        let first = observer
            .on_delta(&ToolInputStreamChunk {
                stream_id: "stream-1",
                attempt: 1,
                tool_call_index: 0,
                input_delta: r#"{"request":{"action":"append","transactionId":"draft-1","index":0,"expectedDraftRevision":0,"content":"fn main() {\n"#,
                received_bytes: 80,
            })
            .unwrap()
            .unwrap();
        let ToolInputStreamPreview::FileChange(first) = first;
        assert_eq!(first.additions, 1);
        assert_eq!(first.line_count, 1);
        assert_eq!(first.content_offset_bytes, 0);
        assert_eq!(first.content_delta, "fn main() {\n");

        observer
            .on_delta(&ToolInputStreamChunk {
                stream_id: "stream-1",
                attempt: 1,
                tool_call_index: 0,
                input_delta: "    println!(\\\"你好\\\");\\n}\\n\"}}",
                received_bytes: 120,
            })
            .unwrap();
        let ToolInputStreamPreview::FileChange(final_preview) = observer.flush().unwrap().unwrap();
        assert_eq!(final_preview.additions, 3);
        assert_eq!(final_preview.line_count, 3);
        assert!(final_preview.generated_bytes > 20);
        assert_eq!(final_preview.content_offset_bytes, first.generated_bytes);
        assert_eq!(
            final_preview.content_delta,
            "    println!(\"\u{4f60}\u{597d}\");\n}\n"
        );
        assert_eq!(
            storage
                .get_agent_file_change("draft-1")
                .unwrap()
                .unwrap()
                .byte_count,
            0
        );

        let mut staged_record = storage.get_agent_file_change("draft-1").unwrap().unwrap();
        staged_record.id = "transaction-apply-1".to_string();
        staged_record.source_tool_name = "apply_patch".to_string();
        staged_record.source_tool_call_id = "call-begin-apply".to_string();
        staged_record.observation_id = "fobs_preview_fixture".to_string();
        staged_record.observation_json = "{}".to_string();
        storage.create_agent_file_change(staged_record).unwrap();

        let mut staged = FileChangeInputStreamObserver::apply_patch(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some("conversation-1".to_string()),
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("workspace".to_string()),
                    root_path: Some(workspace.to_string_lossy().to_string()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    read: AgentReadPermission::WorkspaceOnly,
                    write: AgentWritePermission::WorkspaceOnly,
                    command: AgentCommandPermission::RequireApproval,
                    command_safety: Default::default(),
                    patch: AgentPatchPermission::RequireApproval,
                    builtin_execution: Default::default(),
                },
            }))
            .with_runtime_services("run-1".to_string(), Some(storage.clone())),
        );
        let staged_preview = staged
            .on_delta(&ToolInputStreamChunk {
                stream_id: "stream-apply",
                attempt: 1,
                tool_call_index: 2,
                input_delta: r#"{"request":{"action":"append","transactionId":"transaction-apply-1","index":0,"expectedDraftRevision":0,"content":"你好\n"}}"#,
                received_bytes: 128,
            })
            .unwrap()
            .unwrap();
        let ToolInputStreamPreview::FileChange(staged_preview) = staged_preview;
        assert_eq!(staged_preview.transaction_id, "transaction-apply-1");
        assert_eq!(staged_preview.content_delta, "你好\n");
        assert_eq!(
            storage
                .get_agent_file_change("transaction-apply-1")
                .unwrap()
                .unwrap()
                .byte_count,
            0,
            "a provisional preview must never commit the streamed chunk",
        );

        let mut direct = FileChangeInputStreamObserver::apply_patch(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some("conversation-1".to_string()),
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("workspace".to_string()),
                    root_path: Some(workspace.to_string_lossy().to_string()),
                }),
                attachment_library: None,
                permissions: AgentPermissions::default(),
            }))
            .with_runtime_services("run-1".to_string(), Some(storage)),
        );
        let direct_preview = direct
            .on_delta(&ToolInputStreamChunk {
                stream_id: "stream-direct",
                attempt: 1,
                tool_call_index: 3,
                input_delta: r#"{"request":{"action":"apply","operation":"create","filePath":"src/direct.rs","observationId":"fobs_private","content":"fn main() {}\n"}}"#,
                received_bytes: 160,
            })
            .unwrap()
            .unwrap();
        let ToolInputStreamPreview::FileChange(direct_preview) = direct_preview;
        assert_eq!(direct_preview.file_path, "src/direct.rs");
        assert_eq!(direct_preview.line_count, 1);
        assert_eq!(direct_preview.content_delta, "fn main() {}\n");
    }
}
