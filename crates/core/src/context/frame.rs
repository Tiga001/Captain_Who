use super::measurement::{ContextEstimatorIdentity, ContextMessageEstimate, ContextTokenEstimator};
use crate::llm::{LlmMessage, LlmMessageRole, LlmToolCall};
use crate::protocol::{
    AgentContextCheckpointGroup, AgentContextCheckpointImage, AgentContextCheckpointItem,
    AgentContextCheckpointToolCall, AgentError, AgentResult,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

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

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "run" => Some(Self::Run),
            "conversation" => Some(Self::Conversation),
            "project" => Some(Self::Project),
            _ => None,
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

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "retained" => Some(Self::Retained),
            "request_only" => Some(Self::RequestOnly),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextSource {
    BackendSystemPrompt,
    ConversationHistory,
    ConversationTrace,
    CurrentTurn,
    InputAttachment,
    ToolContinuation,
    ModelResponse,
    ToolResult,
    RuntimeExtension,
    FileTransaction,
    RuntimeGuard,
}

impl ContextSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::BackendSystemPrompt => "backend_system_prompt",
            Self::ConversationHistory => "conversation_history",
            Self::ConversationTrace => "conversation_trace",
            Self::CurrentTurn => "current_turn",
            Self::InputAttachment => "input_attachment",
            Self::ToolContinuation => "tool_continuation",
            Self::ModelResponse => "model_response",
            Self::ToolResult => "tool_result",
            Self::RuntimeExtension => "runtime_extension",
            Self::FileTransaction => "file_transaction",
            Self::RuntimeGuard => "runtime_guard",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "backend_system_prompt" => Some(Self::BackendSystemPrompt),
            "conversation_history" => Some(Self::ConversationHistory),
            "conversation_trace" => Some(Self::ConversationTrace),
            "current_turn" => Some(Self::CurrentTurn),
            "input_attachment" => Some(Self::InputAttachment),
            "tool_continuation" => Some(Self::ToolContinuation),
            "model_response" => Some(Self::ModelResponse),
            "tool_result" => Some(Self::ToolResult),
            "runtime_extension" => Some(Self::RuntimeExtension),
            "file_transaction" => Some(Self::FileTransaction),
            "runtime_guard" => Some(Self::RuntimeGuard),
            _ => None,
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

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "tool_exchange" => Some(Self::ToolExchange),
            _ => None,
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
    measurement: Option<ContextItemMeasurement>,
}

#[derive(Debug, Clone)]
struct ContextItemMeasurement {
    estimator: ContextEstimatorIdentity,
    estimate: ContextMessageEstimate,
}

impl ContextItem {
    pub(crate) fn new(message: LlmMessage, metadata: ContextMetadata) -> Self {
        Self {
            message,
            metadata,
            measurement: None,
        }
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

    fn measure(&mut self, estimator: &dyn ContextTokenEstimator) -> ContextMessageEstimate {
        let identity = estimator.identity();
        if let Some(measurement) = &self.measurement {
            if measurement.estimator == identity {
                return measurement.estimate;
            }
        }

        let estimate = estimator.estimate_message(&self.message);
        self.measurement = Some(ContextItemMeasurement {
            estimator: identity,
            estimate,
        });
        estimate
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ContextFrame {
    items: Vec<ContextItem>,
    revision: u64,
    measurement: Option<ContextFrameMeasurementState>,
}

#[derive(Debug, Clone)]
struct ContextFrameMeasurementState {
    estimator: Arc<dyn ContextTokenEstimator>,
    identity: ContextEstimatorIdentity,
    aggregate: ContextMessageEstimate,
    full_recount: Option<ContextFullRecount>,
}

#[derive(Debug, Clone, Copy)]
struct ContextFullRecount {
    revision: u64,
    estimate: ContextMessageEstimate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextFrameMeasurement {
    pub(crate) estimator: ContextEstimatorIdentity,
    pub(crate) estimate: ContextMessageEstimate,
    pub(crate) item_count: usize,
    pub(crate) revision: u64,
}

impl ContextFrame {
    pub(crate) fn new(items: Vec<ContextItem>) -> Self {
        let revision = u64::try_from(items.len()).unwrap_or(u64::MAX);
        Self {
            items,
            revision,
            measurement: None,
        }
    }

    pub(crate) fn push(&mut self, mut item: ContextItem) {
        if let Some(measurement) = &mut self.measurement {
            let estimate = item.measure(measurement.estimator.as_ref());
            measurement.aggregate.merge(estimate);
            measurement.full_recount = None;
        }
        self.items.push(item);
        self.revision = self.revision.saturating_add(1);
    }

    pub(crate) fn measure_incrementally(
        &mut self,
        estimator: Arc<dyn ContextTokenEstimator>,
    ) -> ContextFrameMeasurement {
        let identity = estimator.identity();
        let needs_rebuild = self
            .measurement
            .as_ref()
            .is_none_or(|measurement| measurement.identity != identity);
        if needs_rebuild {
            let aggregate =
                self.items
                    .iter_mut()
                    .fold(ContextMessageEstimate::default(), |mut total, item| {
                        total.merge(item.measure(estimator.as_ref()));
                        total
                    });
            self.measurement = Some(ContextFrameMeasurementState {
                estimator,
                identity: identity.clone(),
                aggregate,
                full_recount: None,
            });
        }

        let measurement = self
            .measurement
            .as_ref()
            .expect("context measurement must exist after rebuilding");
        ContextFrameMeasurement {
            estimator: measurement.identity.clone(),
            estimate: measurement.aggregate,
            item_count: self.items.len(),
            revision: self.revision,
        }
    }

    pub(crate) fn measure_full(
        &mut self,
        estimator: Arc<dyn ContextTokenEstimator>,
    ) -> ContextFrameMeasurement {
        let incremental = self.measure_incrementally(estimator.clone());
        if let Some(full_recount) = self
            .measurement
            .as_ref()
            .and_then(|measurement| measurement.full_recount)
            .filter(|measurement| measurement.revision == self.revision)
        {
            return ContextFrameMeasurement {
                estimate: full_recount.estimate,
                ..incremental
            };
        }

        let messages = self
            .items
            .iter()
            .map(|item| &item.message)
            .collect::<Vec<_>>();
        let estimate = estimator.estimate_messages(&messages);
        if let Some(measurement) = &mut self.measurement {
            measurement.full_recount = Some(ContextFullRecount {
                revision: self.revision,
                estimate,
            });
        }
        ContextFrameMeasurement {
            estimate,
            ..incremental
        }
    }

    pub(crate) fn checkpoint_items(&self) -> AgentResult<Vec<AgentContextCheckpointItem>> {
        self.items.iter().map(ContextItem::to_checkpoint).collect()
    }

    pub(crate) fn from_checkpoint_items(
        items: Vec<AgentContextCheckpointItem>,
    ) -> AgentResult<Self> {
        if items.is_empty() {
            return Err(AgentError::new("运行检查点没有上下文内容。"));
        }
        let items = items
            .into_iter()
            .map(ContextItem::from_checkpoint)
            .collect::<AgentResult<Vec<_>>>()?;
        Ok(Self::new(items))
    }

    pub(crate) fn validate_pending_tool_call(
        &self,
        pending_tool_call_id: &str,
    ) -> AgentResult<LlmToolCall> {
        let unresolved = self.unresolved_tool_calls()?;
        if unresolved.len() != 1 {
            return Err(AgentError::new(format!(
                "运行检查点必须恰好包含一个待审批工具调用，实际为 {} 个。",
                unresolved.len()
            )));
        }
        unresolved
            .get(pending_tool_call_id)
            .map(|(call, _)| call.clone())
            .ok_or_else(|| {
                AgentError::new(format!(
                    "运行检查点中的待审批工具调用与 `{pending_tool_call_id}` 不一致。"
                ))
            })
    }

    pub(crate) fn append_tool_continuation(
        &mut self,
        call: &LlmToolCall,
        observation: String,
        is_error: bool,
    ) -> AgentResult<()> {
        let checkpoint_call = self.validate_pending_tool_call(&call.id)?;
        if checkpoint_call.name != call.name {
            return Err(AgentError::new(format!(
                "审批续跑工具不一致：检查点为 `{}`，续跑结果为 `{}`。",
                checkpoint_call.name, call.name
            )));
        }
        let group = self
            .items
            .iter()
            .rev()
            .find(|item| {
                item.message
                    .tool_calls
                    .iter()
                    .any(|tool_call| tool_call.id == call.id)
            })
            .and_then(|item| item.metadata.group().cloned())
            .ok_or_else(|| AgentError::new("待审批工具调用缺少原子工具交换分组。"))?;

        self.push(ContextItem::tool_result(
            call.id.clone(),
            observation,
            is_error,
            ContextMetadata::new(
                ContextSource::ToolContinuation,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_source(ContextSource::ToolResult)
            .with_group(group),
        ));
        self.validate_complete_tool_protocol()
    }

    pub(crate) fn validate_complete_tool_protocol(&self) -> AgentResult<()> {
        let unresolved = self.unresolved_tool_calls()?;
        if unresolved.is_empty() {
            Ok(())
        } else {
            Err(AgentError::new(format!(
                "模型请求上下文仍有 {} 个工具调用缺少结果。",
                unresolved.len()
            )))
        }
    }

    pub(crate) fn contains_group_id(&self, group_id: &str) -> bool {
        self.items.iter().any(|item| {
            item.metadata
                .group()
                .is_some_and(|group| group.id() == group_id)
        })
    }

    pub(crate) fn contains_tool_call_id(&self, tool_call_id: &str) -> bool {
        self.items.iter().any(|item| {
            item.message
                .tool_calls
                .iter()
                .any(|call| call.id == tool_call_id)
        })
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

    fn unresolved_tool_calls(&self) -> AgentResult<BTreeMap<String, (LlmToolCall, ContextGroup)>> {
        let mut unresolved = BTreeMap::new();
        let mut seen_call_ids = BTreeSet::new();
        for item in &self.items {
            match item.message.role {
                LlmMessageRole::Assistant if !item.message.tool_calls.is_empty() => {
                    if !unresolved.is_empty() {
                        return Err(AgentError::new(
                            "工具调用协议无效：上一组工具调用尚未获得完整结果。",
                        ));
                    }
                    let group = item.metadata.group().cloned().ok_or_else(|| {
                        AgentError::new("工具调用协议无效：assistant 工具调用缺少交换分组。")
                    })?;
                    for call in &item.message.tool_calls {
                        if call.id.trim().is_empty() || call.name.trim().is_empty() {
                            return Err(AgentError::new(
                                "工具调用协议无效：工具调用 id 和名称不能为空。",
                            ));
                        }
                        if !seen_call_ids.insert(call.id.clone()) {
                            return Err(AgentError::new(format!(
                                "工具调用协议无效：工具调用 id `{}` 在当前运行中重复。",
                                call.id
                            )));
                        }
                        if unresolved
                            .insert(call.id.clone(), (call.clone(), group.clone()))
                            .is_some()
                        {
                            return Err(AgentError::new(format!(
                                "工具调用协议无效：工具调用 id `{}` 重复。",
                                call.id
                            )));
                        }
                    }
                }
                LlmMessageRole::Tool => {
                    let call_id = item.message.tool_call_id.as_deref().ok_or_else(|| {
                        AgentError::new("工具调用协议无效：工具结果缺少 tool_call_id。")
                    })?;
                    let Some((_, expected_group)) = unresolved.remove(call_id) else {
                        return Err(AgentError::new(format!(
                            "工具调用协议无效：工具结果 `{call_id}` 没有对应的未结算调用。"
                        )));
                    };
                    if item.metadata.group() != Some(&expected_group) {
                        return Err(AgentError::new(format!(
                            "工具调用协议无效：工具结果 `{call_id}` 的交换分组不匹配。"
                        )));
                    }
                }
                _ if !unresolved.is_empty() => {
                    return Err(AgentError::new(
                        "工具调用协议无效：工具调用与结果之间出现了其他消息。",
                    ));
                }
                _ => {}
            }
        }
        Ok(unresolved)
    }
}

impl Default for ContextFrame {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl ContextItem {
    fn to_checkpoint(&self) -> AgentResult<AgentContextCheckpointItem> {
        if self.metadata.retention() != ContextRetention::Retained {
            return Err(AgentError::new("运行检查点不能保存 request-only 上下文。"));
        }
        Ok(AgentContextCheckpointItem {
            role: self.message.role.as_str().to_string(),
            content: self.message.content.clone(),
            images: self
                .message
                .images
                .iter()
                .map(|image| AgentContextCheckpointImage {
                    mime_type: image.mime_type.clone(),
                    data_base64: image.data_base64.clone(),
                })
                .collect(),
            tool_call_id: self.message.tool_call_id.clone(),
            tool_calls: self
                .message
                .tool_calls
                .iter()
                .map(|call| AgentContextCheckpointToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    args: call.args.clone(),
                })
                .collect(),
            is_error: self.message.is_error,
            sources: self
                .metadata
                .sources()
                .iter()
                .map(|source| source.as_str().to_string())
                .collect(),
            scope: self.metadata.scope().as_str().to_string(),
            retention: self.metadata.retention().as_str().to_string(),
            group: self
                .metadata
                .group()
                .map(|group| AgentContextCheckpointGroup {
                    id: group.id().to_string(),
                    kind: group.kind().as_str().to_string(),
                }),
        })
    }

    fn from_checkpoint(item: AgentContextCheckpointItem) -> AgentResult<Self> {
        let role = role_from_checkpoint(&item.role)?;
        if role != LlmMessageRole::User && !item.images.is_empty() {
            return Err(AgentError::new(
                "运行检查点无效：只有 user 消息可以携带图片。",
            ));
        }
        if role != LlmMessageRole::Assistant && !item.tool_calls.is_empty() {
            return Err(AgentError::new(
                "运行检查点无效：只有 assistant 消息可以携带工具调用。",
            ));
        }
        if role == LlmMessageRole::Tool && item.tool_call_id.is_none() {
            return Err(AgentError::new(
                "运行检查点无效：tool 消息缺少 tool_call_id。",
            ));
        }
        if role != LlmMessageRole::Tool && item.tool_call_id.is_some() {
            return Err(AgentError::new(
                "运行检查点无效：非 tool 消息不能携带 tool_call_id。",
            ));
        }

        let mut sources = item.sources.into_iter();
        let first_source = sources
            .next()
            .as_deref()
            .and_then(ContextSource::from_str)
            .ok_or_else(|| AgentError::new("运行检查点无效：上下文来源为空或未知。"))?;
        let scope = ContextScope::from_str(&item.scope)
            .ok_or_else(|| AgentError::new("运行检查点包含未知上下文作用域。"))?;
        let retention = ContextRetention::from_str(&item.retention)
            .ok_or_else(|| AgentError::new("运行检查点包含未知上下文保留策略。"))?;
        if retention != ContextRetention::Retained {
            return Err(AgentError::new("运行检查点不能恢复 request-only 上下文。"));
        }
        let mut metadata = ContextMetadata::new(first_source, scope, retention);
        for source in sources {
            let source = ContextSource::from_str(&source)
                .ok_or_else(|| AgentError::new("运行检查点包含未知上下文来源。"))?;
            metadata = metadata.with_source(source);
        }
        if let Some(group) = item.group {
            let kind = ContextGroupKind::from_str(&group.kind)
                .ok_or_else(|| AgentError::new("运行检查点包含未知上下文分组类型。"))?;
            if group.id.trim().is_empty() {
                return Err(AgentError::new("运行检查点包含空上下文分组 id。"));
            }
            metadata = metadata.with_group(ContextGroup { id: group.id, kind });
        }

        Ok(Self::new(
            LlmMessage {
                role,
                content: item.content,
                images: item
                    .images
                    .into_iter()
                    .map(|image| crate::llm::LlmImage {
                        mime_type: image.mime_type,
                        data_base64: image.data_base64,
                    })
                    .collect(),
                tool_call_id: item.tool_call_id,
                tool_calls: item
                    .tool_calls
                    .into_iter()
                    .map(|call| LlmToolCall {
                        id: call.id,
                        name: call.name,
                        args: call.args,
                    })
                    .collect(),
                is_error: item.is_error,
            },
            metadata,
        ))
    }
}

fn role_from_checkpoint(value: &str) -> AgentResult<LlmMessageRole> {
    match value {
        "system" => Ok(LlmMessageRole::System),
        "user" => Ok(LlmMessageRole::User),
        "assistant" => Ok(LlmMessageRole::Assistant),
        "tool" => Ok(LlmMessageRole::Tool),
        _ => Err(AgentError::new(format!(
            "运行检查点包含未知消息角色：{value}"
        ))),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmImage;
    use serde_json::json;

    #[test]
    fn checkpoint_round_trip_preserves_messages_images_and_metadata() {
        let group = ContextGroup::tool_exchange("exchange-1");
        let mut image_message = LlmMessage::text(LlmMessageRole::User, "inspect image");
        image_message.images.push(LlmImage {
            mime_type: "image/png".to_string(),
            data_base64: "YWJj".to_string(),
        });
        let frame = ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "rules",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            ContextItem::new(
                image_message,
                ContextMetadata::new(
                    ContextSource::CurrentTurn,
                    ContextScope::Conversation,
                    ContextRetention::Retained,
                )
                .with_source(ContextSource::InputAttachment),
            ),
            ContextItem::assistant(
                "read it",
                vec![LlmToolCall {
                    id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "notes.txt" }),
                }],
                ContextMetadata::new(
                    ContextSource::ModelResponse,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(group.clone()),
            ),
            ContextItem::tool_result(
                "call-1",
                "contents",
                false,
                ContextMetadata::new(
                    ContextSource::ToolResult,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(group),
            ),
        ]);

        let checkpoint = frame.checkpoint_items().unwrap();
        let restored = ContextFrame::from_checkpoint_items(checkpoint).unwrap();

        restored.validate_complete_tool_protocol().unwrap();
        let messages = restored.to_messages();
        assert_eq!(messages[1].images[0].data_base64, "YWJj");
        assert_eq!(messages[2].tool_calls[0].args["path"], "notes.txt");
        assert_eq!(
            serde_json::to_value(frame.manifest()).unwrap(),
            serde_json::to_value(restored.manifest()).unwrap()
        );
    }

    #[test]
    fn checkpoint_rejects_request_only_context() {
        let frame = ContextFrame::new(vec![ContextItem::text(
            LlmMessageRole::User,
            "transient",
            ContextSource::RuntimeExtension,
            ContextScope::Run,
            ContextRetention::RequestOnly,
        )]);

        assert!(frame.checkpoint_items().is_err());
    }
}
