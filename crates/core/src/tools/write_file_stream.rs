use super::input_stream::{JsonStringFieldEvent, TopLevelJsonStringStream};
use super::{
    ToolExecutionContext, ToolInputStreamChunk, ToolInputStreamObserver, ToolInputStreamPreview,
};
use crate::protocol::AgentFileWritePreview;
use crate::storage::models::AgentFileDraftRecord;
use crate::storage::now_ms;
use std::time::{Duration, Instant};

const PREVIEW_INTERVAL: Duration = Duration::from_millis(120);

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
    tool_call_id: Option<String>,
    received_bytes: u64,
}

pub(crate) struct WriteFileInputStreamObserver {
    context: ToolExecutionContext,
    parser: TopLevelJsonStringStream,
    phase: String,
    phase_complete: bool,
    draft_id: String,
    draft_id_complete: bool,
    content: StreamingTextMetrics,
    draft: Option<AgentFileDraftRecord>,
    metadata: StreamMetadata,
    preview_id: Option<String>,
    last_emitted_bytes: u64,
    last_emitted_at: Option<Instant>,
    disabled: bool,
}

impl WriteFileInputStreamObserver {
    pub(crate) fn new(context: ToolExecutionContext) -> Self {
        Self {
            context,
            parser: TopLevelJsonStringStream::default(),
            phase: String::new(),
            phase_complete: false,
            draft_id: String::new(),
            draft_id_complete: false,
            content: StreamingTextMetrics::default(),
            draft: None,
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
                    "phase" => self.phase.push_str(&value),
                    "draftId" => self.draft_id.push_str(&value),
                    "content" => self.content.push(&value),
                    _ => {}
                },
                JsonStringFieldEvent::Completed { field } => match field.as_str() {
                    "phase" => self.phase_complete = true,
                    "draftId" => self.draft_id_complete = true,
                    _ => {}
                },
            }
        }
    }

    fn load_draft_if_ready(&mut self) {
        if self.disabled || self.draft.is_some() || !self.phase_complete || !self.draft_id_complete
        {
            return;
        }
        if self.phase != "append" || self.draft_id.trim().is_empty() {
            self.disabled = true;
            return;
        }

        let Ok(storage) = self.context.storage() else {
            self.disabled = true;
            return;
        };
        let Ok(Some(draft)) = storage.get_agent_file_draft(self.draft_id.trim()) else {
            self.disabled = true;
            return;
        };
        let Ok(conversation_id) = self.context.conversation_id() else {
            self.disabled = true;
            return;
        };
        if draft.conversation_id != conversation_id
            || !matches!(draft.status.as_str(), "drafting" | "ready")
        {
            self.disabled = true;
            return;
        }
        self.draft = Some(draft);
    }

    fn preview(&mut self, force: bool) -> Option<AgentFileWritePreview> {
        self.load_draft_if_ready();
        let draft = self.draft.as_ref()?;
        if self.content.byte_count == 0 || self.content.byte_count == self.last_emitted_bytes {
            return None;
        }
        if !force
            && self
                .last_emitted_at
                .is_some_and(|last| last.elapsed() < PREVIEW_INTERVAL)
        {
            return None;
        }

        let preview_id = self.preview_id.get_or_insert_with(|| {
            format!(
                "{}:{}:{}",
                self.metadata.stream_id, self.metadata.attempt, self.metadata.tool_call_index
            )
        });
        let line_count = appended_line_count(draft, &self.content);
        let content_offset_bytes = self.last_emitted_bytes;
        let content_delta = self.content.take_pending_content();
        let preview = AgentFileWritePreview {
            preview_id: preview_id.clone(),
            stream_id: self.metadata.stream_id.clone(),
            attempt: self.metadata.attempt,
            tool_call_index: self.metadata.tool_call_index,
            tool_call_id: self.metadata.tool_call_id.clone(),
            draft_id: draft.id.clone(),
            file_path: draft.file_path.clone(),
            additions: draft.additions.saturating_add(self.content.line_count()),
            deletions: draft.deletions,
            line_count,
            byte_count: draft.byte_count.saturating_add(self.content.byte_count),
            generated_bytes: self.content.byte_count,
            content_offset_bytes,
            content_delta,
            updated_at: now_ms(),
        };
        self.last_emitted_bytes = self.content.byte_count;
        self.last_emitted_at = Some(Instant::now());
        Some(preview)
    }
}

impl ToolInputStreamObserver for WriteFileInputStreamObserver {
    fn on_delta(
        &mut self,
        chunk: &ToolInputStreamChunk<'_>,
    ) -> crate::protocol::AgentResult<Option<ToolInputStreamPreview>> {
        self.metadata.stream_id = chunk.stream_id.to_string();
        self.metadata.attempt = chunk.attempt;
        self.metadata.tool_call_index = chunk.tool_call_index;
        self.metadata.tool_call_id = chunk.tool_call_id.map(ToString::to_string);
        self.metadata.received_bytes = chunk.received_bytes;
        let events = self.parser.push(chunk.input_delta);
        self.consume_events(events);
        Ok(self.preview(false).map(ToolInputStreamPreview::FileWrite))
    }

    fn flush(&mut self) -> crate::protocol::AgentResult<Option<ToolInputStreamPreview>> {
        Ok(self.preview(true).map(ToolInputStreamPreview::FileWrite))
    }
}

fn appended_line_count(draft: &AgentFileDraftRecord, generated: &StreamingTextMetrics) -> u64 {
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
            .create_agent_file_draft(AgentFileDraftRecord {
                id: "draft-1".to_string(),
                conversation_id: "conversation-1".to_string(),
                project_id: None,
                run_id: "run-1".to_string(),
                file_path: "src/main.rs".to_string(),
                mode: "create".to_string(),
                status: "drafting".to_string(),
                base_revision: None,
                base_content: String::new(),
                content: String::new(),
                additions: 0,
                deletions: 0,
                line_count: 0,
                byte_count: 0,
                chunk_count: 0,
                next_chunk_index: 0,
                stats_final: false,
                summary: None,
                final_action_id: None,
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
        let mut observer = WriteFileInputStreamObserver::new(context);

        let first = observer
            .on_delta(&ToolInputStreamChunk {
                stream_id: "stream-1",
                attempt: 1,
                tool_call_index: 0,
                tool_call_id: Some("call-1"),
                input_delta: r#"{"phase":"append","draftId":"draft-1","index":0,"content":"fn main() {\n"#,
                received_bytes: 80,
            })
            .unwrap()
            .unwrap();
        let ToolInputStreamPreview::FileWrite(first) = first;
        assert_eq!(first.additions, 1);
        assert_eq!(first.line_count, 1);
        assert_eq!(first.content_offset_bytes, 0);
        assert_eq!(first.content_delta, "fn main() {\n");

        observer
            .on_delta(&ToolInputStreamChunk {
                stream_id: "stream-1",
                attempt: 1,
                tool_call_index: 0,
                tool_call_id: Some("call-1"),
                input_delta: "    println!(\\\"你好\\\");\\n}\\n\"}",
                received_bytes: 120,
            })
            .unwrap();
        let ToolInputStreamPreview::FileWrite(final_preview) = observer.flush().unwrap().unwrap();
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
                .get_agent_file_draft("draft-1")
                .unwrap()
                .unwrap()
                .byte_count,
            0
        );
    }
}
