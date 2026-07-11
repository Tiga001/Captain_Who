use crate::llm::{LlmMessage, LlmMessageRole, LlmToolCall};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextScope {
    Run,
    Conversation,
    // Reserved for the later project-memory source without enabling it today.
    #[allow(dead_code)]
    Project,
}

impl ContextScope {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Conversation => "conversation",
            Self::Project => "project",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextRetention {
    Retained,
    RequestOnly,
}

impl ContextRetention {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Retained => "retained",
            Self::RequestOnly => "request_only",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextSource {
    BackendSystemPrompt,
    ConversationHistory,
    CurrentTurn,
    InputAttachment,
    ApprovalDecision,
    ToolContinuation,
    ModelResponse,
    ToolResult,
    RuntimeHook,
    FileTransaction,
    RuntimeGuard,
}

impl ContextSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::BackendSystemPrompt => "backend_system_prompt",
            Self::ConversationHistory => "conversation_history",
            Self::CurrentTurn => "current_turn",
            Self::InputAttachment => "input_attachment",
            Self::ApprovalDecision => "approval_decision",
            Self::ToolContinuation => "tool_continuation",
            Self::ModelResponse => "model_response",
            Self::ToolResult => "tool_result",
            Self::RuntimeHook => "runtime_hook",
            Self::FileTransaction => "file_transaction",
            Self::RuntimeGuard => "runtime_guard",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextGroupKind {
    ToolExchange,
}

impl ContextGroupKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ToolExchange => "tool_exchange",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextGroup {
    id: String,
    kind: ContextGroupKind,
}

impl ContextGroup {
    pub(crate) fn tool_exchange(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: ContextGroupKind::ToolExchange,
        }
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn kind(&self) -> ContextGroupKind {
        self.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextMetadata {
    sources: Vec<ContextSource>,
    scope: ContextScope,
    retention: ContextRetention,
    group: Option<ContextGroup>,
}

impl ContextMetadata {
    pub(crate) fn new(
        source: ContextSource,
        scope: ContextScope,
        retention: ContextRetention,
    ) -> Self {
        Self {
            sources: vec![source],
            scope,
            retention,
            group: None,
        }
    }

    pub(crate) fn with_source(mut self, source: ContextSource) -> Self {
        if !self.sources.contains(&source) {
            self.sources.push(source);
        }
        self
    }

    pub(crate) fn with_group(mut self, group: ContextGroup) -> Self {
        self.group = Some(group);
        self
    }

    pub(crate) fn sources(&self) -> &[ContextSource] {
        &self.sources
    }

    pub(crate) fn scope(&self) -> ContextScope {
        self.scope
    }

    pub(crate) fn retention(&self) -> ContextRetention {
        self.retention
    }

    pub(crate) fn group(&self) -> Option<&ContextGroup> {
        self.group.as_ref()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ContextItem {
    message: LlmMessage,
    metadata: ContextMetadata,
}

impl ContextItem {
    pub(crate) fn new(message: LlmMessage, metadata: ContextMetadata) -> Self {
        Self { message, metadata }
    }

    pub(crate) fn text(
        role: LlmMessageRole,
        content: impl Into<String>,
        source: ContextSource,
        scope: ContextScope,
        retention: ContextRetention,
    ) -> Self {
        Self::new(
            LlmMessage::text(role, content),
            ContextMetadata::new(source, scope, retention),
        )
    }

    pub(crate) fn assistant(
        content: impl Into<String>,
        tool_calls: Vec<LlmToolCall>,
        metadata: ContextMetadata,
    ) -> Self {
        Self::new(LlmMessage::assistant(content, tool_calls), metadata)
    }

    pub(crate) fn tool_result(
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
        metadata: ContextMetadata,
    ) -> Self {
        Self::new(
            LlmMessage::tool_result(tool_call_id, content, is_error),
            metadata,
        )
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ContextFrame {
    items: Vec<ContextItem>,
}

impl ContextFrame {
    pub(crate) fn new(items: Vec<ContextItem>) -> Self {
        Self { items }
    }

    pub(crate) fn push(&mut self, item: ContextItem) {
        self.items.push(item);
    }

    #[cfg(test)]
    pub(crate) fn to_messages(&self) -> Vec<LlmMessage> {
        self.items.iter().map(|item| item.message.clone()).collect()
    }

    pub(crate) fn into_messages(self) -> Vec<LlmMessage> {
        self.items.into_iter().map(|item| item.message).collect()
    }

    pub(crate) fn manifest(&self) -> ContextManifest<'_> {
        ContextManifest {
            entries: self
                .items
                .iter()
                .enumerate()
                .map(|(index, item)| ContextManifestEntry {
                    index,
                    role: item.message.role.as_str(),
                    sources: item
                        .metadata
                        .sources()
                        .iter()
                        .map(|source| source.as_str())
                        .collect(),
                    scope: item.metadata.scope().as_str(),
                    retention: item.metadata.retention().as_str(),
                    group_id: item.metadata.group().map(ContextGroup::id),
                    group_kind: item.metadata.group().map(|group| group.kind().as_str()),
                    text_character_count: item.message.content.chars().count(),
                    image_count: item.message.images.len(),
                    image_base64_bytes: item
                        .message
                        .images
                        .iter()
                        .map(|image| image.data_base64.len())
                        .sum(),
                    tool_call_count: item.message.tool_calls.len(),
                    tool_argument_character_count: item
                        .message
                        .tool_calls
                        .iter()
                        .filter_map(|call| serde_json::to_string(&call.args).ok())
                        .map(|args| args.chars().count())
                        .sum(),
                    tool_call_id: item.message.tool_call_id.as_deref(),
                    is_error: item.message.is_error,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextManifest<'a> {
    pub(crate) entries: Vec<ContextManifestEntry<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextManifestEntry<'a> {
    pub(crate) index: usize,
    pub(crate) role: &'static str,
    pub(crate) sources: Vec<&'static str>,
    pub(crate) scope: &'static str,
    pub(crate) retention: &'static str,
    pub(crate) group_id: Option<&'a str>,
    pub(crate) group_kind: Option<&'static str>,
    pub(crate) text_character_count: usize,
    pub(crate) image_count: usize,
    pub(crate) image_base64_bytes: usize,
    pub(crate) tool_call_count: usize,
    pub(crate) tool_argument_character_count: usize,
    pub(crate) tool_call_id: Option<&'a str>,
    pub(crate) is_error: bool,
}
