use super::*;

impl ContextItem {
    pub(super) fn same_context_content(&self, other: &Self) -> bool {
        self.message == other.message
            && self.checkpoint_message == other.checkpoint_message
            && self.metadata == other.metadata
    }
}

impl ContextItem {
    pub(super) fn to_checkpoint(&self) -> AgentResult<AgentContextCheckpointItem> {
        if self.metadata.retention() != ContextRetention::Retained {
            return Err(AgentError::new("运行检查点不能保存 request-only 上下文。"));
        }
        let message = self.checkpoint_message.as_ref().unwrap_or(&self.message);
        if message.role != self.message.role
            || message.tool_call_id != self.message.tool_call_id
            || message.is_error != self.message.is_error
            || message.tool_calls != self.message.tool_calls
        {
            return Err(AgentError::new(
                "运行检查点投影不能改变消息角色、工具身份或错误语义。",
            ));
        }
        Ok(AgentContextCheckpointItem {
            role: message.role.as_str().to_string(),
            content: message.content.clone(),
            images: message
                .images
                .iter()
                .map(|image| AgentContextCheckpointImage {
                    mime_type: image.mime_type.clone(),
                    data_base64: image.data_base64.clone(),
                })
                .collect(),
            tool_call_id: message.tool_call_id.clone(),
            tool_calls: message
                .tool_calls
                .iter()
                .map(|call| AgentContextCheckpointToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    args: call.args.clone(),
                })
                .collect(),
            is_error: message.is_error,
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
            origin: self
                .metadata
                .origin()
                .map(|origin| AgentContextCheckpointOrigin {
                    kind: origin.kind().as_str().to_string(),
                    id: origin.id().to_string(),
                }),
        })
    }

    pub(super) fn from_checkpoint(item: AgentContextCheckpointItem) -> AgentResult<Self> {
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
        if let Some(origin) = item.origin {
            let kind = ContextOriginKind::from_str(&origin.kind)
                .ok_or_else(|| AgentError::new("运行检查点包含未知上下文来源身份类型。"))?;
            if origin.id.trim().is_empty() {
                return Err(AgentError::new("运行检查点包含空上下文来源身份。"));
            }
            metadata = metadata.with_origin(ContextOrigin {
                kind,
                id: origin.id,
            });
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
                placement: crate::llm::LlmMessagePlacement::default_for_role(role),
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
    pub(crate) origin_kind: Option<&'static str>,
    pub(crate) origin_id: Option<&'a str>,
    pub(crate) text_character_count: usize,
    pub(crate) image_count: usize,
    pub(crate) image_base64_bytes: usize,
    pub(crate) tool_call_count: usize,
    pub(crate) tool_argument_character_count: usize,
    pub(crate) tool_call_id: Option<&'a str>,
    pub(crate) is_error: bool,
}
