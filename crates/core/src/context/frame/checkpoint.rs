use super::*;

fn same_tool_call_identity(live: &LlmMessage, projected: &LlmMessage) -> bool {
    live.tool_calls().len() == projected.tool_calls().len()
        && live
            .tool_calls()
            .zip(projected.tool_calls())
            .all(|(live, projected)| live.id == projected.id && live.name == projected.name)
}

impl ContextItem {
    pub(super) fn same_context_content(&self, other: &Self) -> bool {
        self.message == other.message
            && self.context_image_refs == other.context_image_refs
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
        if message.role() != self.message.role()
            || message.tool_call_id() != self.message.tool_call_id()
            || message.is_error() != self.message.is_error()
            || !same_tool_call_identity(&self.message, message)
        {
            return Err(AgentError::new(
                "运行检查点投影不能改变消息角色、工具身份或错误语义。",
            ));
        }
        if message
            .assistant_turn()
            .and_then(LlmAssistantTurn::provider_continuation)
            .is_some()
        {
            return Err(AgentError::new(
                "运行检查点不能持久化 Provider raw continuation。",
            ));
        }
        let effective_tool_calls = message.tool_calls().cloned().collect::<Vec<_>>();
        let provider_identities = if effective_tool_calls.is_empty() {
            BTreeMap::new()
        } else {
            message
                .assistant_turn()
                .ok_or_else(|| {
                    AgentError::new("运行检查点中的 Assistant Tool Call 缺少完整 Assistant Turn。")
                })?
                .checkpoint_identity()?
                .tool_call_identities
                .into_iter()
                .map(|identity| (identity.runtime_call_id.clone(), identity))
                .collect::<BTreeMap<_, _>>()
        };
        Ok(AgentContextCheckpointItem {
            role: message.role().as_str().to_string(),
            content: message.content().to_string(),
            context_image_refs: self.context_image_refs.clone(),
            images: message
                .images()
                .iter()
                .map(|image| AgentContextCheckpointImage {
                    mime_type: image.mime_type.clone(),
                    data_base64: image.data_base64.clone(),
                })
                .collect(),
            tool_call_id: message.tool_call_id().map(str::to_string),
            tool_calls: effective_tool_calls
                .into_iter()
                .map(|call| {
                    let provider_identity = provider_identities
                        .get(call.id.as_str())
                        .cloned()
                        .ok_or_else(|| {
                            AgentError::new(
                                "运行检查点中的 Assistant Tool Call 缺少 Provider 身份映射。",
                            )
                        })?;
                    Ok(AgentContextCheckpointToolCall {
                        id: call.id,
                        name: call.name,
                        args: call.args,
                        provider_identity,
                    })
                })
                .collect::<AgentResult<Vec<_>>>()?,
            is_error: message.is_error(),
            sources: self
                .metadata
                .sources()
                .iter()
                .map(|source| source.as_str().to_string())
                .collect(),
            scope: self.metadata.scope().as_str().to_string(),
            retention: self.metadata.retention().as_str().to_string(),
            request_order: self.metadata.request_order(),
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
        if role != LlmMessageRole::User
            && (!item.images.is_empty() || !item.context_image_refs.is_empty())
        {
            return Err(AgentError::new(
                "运行检查点无效：只有 user 消息可以携带图片。",
            ));
        }
        for reference in &item.context_image_refs {
            reference.validate()?;
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
        if let Some(order) = item.request_order {
            metadata = metadata.with_request_order(order);
        }
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

        let message = match role {
            LlmMessageRole::System | LlmMessageRole::User => {
                let mut message = LlmMessage::text(role, item.content);
                message
                    .images_mut()
                    .expect("text checkpoint messages support images")
                    .extend(item.images.into_iter().map(|image| crate::llm::LlmImage {
                        mime_type: image.mime_type,
                        data_base64: image.data_base64,
                    }));
                message
            }
            LlmMessageRole::Assistant => {
                if item.tool_calls.is_empty() {
                    LlmMessage::text(LlmMessageRole::Assistant, item.content)
                } else {
                    let max_provider_index = item
                        .tool_calls
                        .iter()
                        .map(|call| call.provider_identity.provider_tool_index)
                        .max()
                        .map(|index| usize::try_from(index).unwrap_or(usize::MAX));
                    if max_provider_index
                        .is_some_and(|index| index > item.tool_calls.len().saturating_add(1_024))
                    {
                        return Err(AgentError::new(
                            "运行检查点的 Provider Tool index 超出安全恢复范围。",
                        ));
                    }
                    let mut provider_calls = (0..max_provider_index.map_or(0, |index| index + 1))
                        .map(|index| LlmToolCall {
                            id: format!("checkpoint-omitted-provider-call-{index}"),
                            name: "checkpoint_omitted_provider_call".to_string(),
                            args: serde_json::Value::Null,
                        })
                        .collect::<Vec<_>>();
                    let mut runtime_bindings = Vec::with_capacity(item.tool_calls.len());
                    let mut previous_provider_index = None;
                    for call in item.tool_calls {
                        let provider_identity = call.provider_identity;
                        let provider_index = usize::try_from(provider_identity.provider_tool_index)
                            .map_err(|_| {
                                AgentError::new("运行检查点的 Provider Tool index 无效。")
                            })?;
                        if previous_provider_index
                            .is_some_and(|previous| previous >= provider_index)
                            || provider_identity.runtime_call_id != call.id
                        {
                            return Err(AgentError::new(
                                "运行检查点的 Provider/Runtime Tool Call 映射无序或不一致。",
                            ));
                        }
                        previous_provider_index = Some(provider_index);
                        let provider_call = LlmToolCall {
                            id: provider_identity.provider_call_id,
                            name: call.name.clone(),
                            args: call.args.clone(),
                        };
                        let runtime_call = LlmToolCall {
                            id: call.id,
                            name: call.name,
                            args: call.args,
                        };
                        runtime_bindings.push(LlmRuntimeToolCallBinding::new(
                            provider_index,
                            &provider_call,
                            runtime_call,
                        ));
                        provider_calls[provider_index] = provider_call;
                    }
                    let turn =
                        LlmAssistantTurn::from_split_projection(item.content, provider_calls)
                            .with_runtime_tool_bindings(runtime_bindings)?;
                    LlmMessage::from_assistant_turn(turn)
                }
            }
            LlmMessageRole::Tool => {
                let tool_call_id = item.tool_call_id.ok_or_else(|| {
                    AgentError::new("运行检查点无效：tool 消息缺少 tool_call_id。")
                })?;
                LlmMessage::tool_result(tool_call_id, item.content, item.is_error)
            }
        };

        Ok(Self::new(message, metadata).with_context_image_refs(item.context_image_refs))
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
    /// Placement in the model request; `index` remains the canonical planning cursor.
    pub(crate) model_request_index: usize,
    pub(crate) request_order: Option<u64>,
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

#[cfg(test)]
mod image_reference_tests {
    use super::*;

    #[test]
    fn checkpoint_preserves_immutable_image_references_for_rehydration() {
        let reference = crate::ConversationContextImageRef {
            attachment_id: "historical-image".to_string(),
            mime_type: "image/png".to_string(),
            sha256: "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
                .to_string(),
        };
        let mut message = LlmMessage::text(LlmMessageRole::User, "image material");
        message.images_mut().unwrap().push(crate::llm::LlmImage {
            mime_type: "image/png".to_string(),
            data_base64: "YWJj".to_string(),
        });
        let item = ContextItem::new(
            message,
            ContextMetadata::new(
                ContextSource::InputAttachment,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
        )
        .with_context_image_refs(vec![reference.clone()]);
        let checkpoint = item.to_checkpoint().unwrap();
        assert_eq!(checkpoint.context_image_refs, vec![reference]);
        let restored = ContextItem::from_checkpoint(checkpoint.clone()).unwrap();
        assert!(item.same_context_content(&restored));
        let mut wrong_role = checkpoint.clone();
        wrong_role.role = "assistant".to_string();
        wrong_role.images.clear();
        assert!(ContextItem::from_checkpoint(wrong_role).is_err());
        let mut invalid_reference = checkpoint;
        invalid_reference.context_image_refs[0].sha256 = "invalid".to_string();
        assert!(ContextItem::from_checkpoint(invalid_reference).is_err());
    }
}
