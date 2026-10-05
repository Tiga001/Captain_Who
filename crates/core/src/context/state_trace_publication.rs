struct TracePublicationRenderCursor {
    identity: std::sync::Arc<()>,
    assistant_message_id: String,
    run_id: String,
    model_count: usize,
    activity_count: usize,
    // Generic ToolCall text can own a narration that was published earlier for the UI.
    unowned_narrations: std::collections::BTreeSet<String>,
    narration_rewind: Option<TraceNarrationRewind>,
}

struct TraceNarrationRewind {
    call_id: String,
    model_count: usize,
    activity_count: usize,
    frame: ContextFrame,
    after_revision: (u64, u64, usize),
}

#[cfg(test)]
#[path = "state_trace_publication_tests.rs"]
mod trace_publication_tests;

fn publication_committed_model_count(publication: &crate::ConversationTracePublication) -> usize {
    let unresolved = publication
        .trace_items()
        .iter()
        .rev()
        .find_map(|item| match item.as_ref() {
            crate::ConversationTurnTraceItem::ToolCall { sequence, .. } => Some(Some(*sequence)),
            crate::ConversationTurnTraceItem::ToolResult { .. } => Some(None),
            _ => None,
        })
        .flatten();
    unresolved.map_or(publication.model_items().len(), |sequence| {
        publication
            .model_items()
            .partition_point(|item| item.sequence < sequence)
    })
}

impl AgentConversationContextState {
    /// Called only after adopting a validated, committed publication: either a cold rebuild or
    /// an admitted historical baseline with this new Turn's committed seed appended.
    pub fn seed_trace_publication_cursor(
        &mut self,
        publication: &crate::ConversationTracePublication,
        assistant_message_id: &str,
        run_id: &str,
        activity_count: usize,
    ) {
        let model_count = publication_committed_model_count(publication);
        let retained_calls = publication.model_items()[..model_count]
            .iter()
            .filter(|item| item.role == "assistant" && !item.content.is_empty())
            .flat_map(|item| item.tool_calls.iter().map(|call| call.id.as_str()))
            .collect::<std::collections::BTreeSet<_>>();
        let through = publication
            .model_items()
            .get(model_count.wrapping_sub(1))
            .map(|item| item.sequence);
        let unowned_narrations = publication
            .trace_items()
            .iter()
            .filter_map(|item| match item.as_ref() {
                crate::ConversationTurnTraceItem::AssistantNarration {
                    sequence,
                    first_tool_call_id: Some(id),
                    ..
                } if through.is_some_and(|through| *sequence <= through)
                    && !retained_calls.contains(id.as_str()) =>
                {
                    Some(id.clone())
                }
                _ => None,
            })
            .collect();
        self.trace_publication_cursor = Some(TracePublicationRenderCursor {
            identity: publication.identity.clone(),
            assistant_message_id: assistant_message_id.into(),
            run_id: run_id.into(),
            model_count,
            activity_count,
            unowned_narrations,
            narration_rewind: None,
        });
    }

    /// Append only exact model rows that became committed since the previously adopted Runtime
    /// publication. None asks the Host for its existing cold rebuild (compaction, foreign lineage,
    /// historical narration replacement or new image hydration). No state changes on that path.
    pub fn append_trace_publication(
        &mut self,
        publication: &crate::ConversationTracePublication,
        assistant_message_id: &str,
        run_id: &str,
        expected_activity_count: usize,
    ) -> AgentResult<Option<usize>> {
        let Some(cursor) = self.trace_publication_cursor.as_ref() else {
            return Ok(None);
        };
        if cursor.assistant_message_id != assistant_message_id
            || cursor.run_id != run_id
            || cursor.activity_count != expected_activity_count
            || !(std::sync::Arc::ptr_eq(&cursor.identity, &publication.identity)
                || publication
                    .parent
                    .as_ref()
                    .is_some_and(|parent| std::sync::Arc::ptr_eq(parent, &cursor.identity)))
        {
            return Ok(None);
        }
        let model_count = publication_committed_model_count(publication);
        if model_count < cursor.model_count {
            return Ok(None);
        }
        let committed_trace_count = publication
            .trace_items()
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, item)| match item.as_ref() {
                crate::ConversationTurnTraceItem::ToolCall { .. } => Some(index),
                crate::ConversationTurnTraceItem::ToolResult { .. } => {
                    Some(publication.trace_items().len())
                }
                _ => None,
            })
            .unwrap_or(publication.trace_items().len());
        let expected_last = publication.trace_items()[..committed_trace_count]
            .iter()
            .rev()
            .find(|item| item.is_model_visible())
            .map(|item| item.sequence());
        let actual_last = publication.model_items()[..model_count]
            .last()
            .map(|item| item.sequence);
        if expected_last != actual_last {
            return Err(AgentError::new(
                "模型上下文日志必须覆盖每个已闭合的轨迹项目。",
            ));
        }
        let new_suffix = &publication.model_items()[cursor.model_count..model_count];
        if new_suffix.iter().any(|item| !item.images.is_empty()) {
            return Ok(None);
        }
        let retained_calls = new_suffix
            .iter()
            .filter(|item| item.role == "assistant" && !item.content.is_empty())
            .flat_map(|item| item.tool_calls.iter().map(|call| call.id.as_str()))
            .collect::<std::collections::BTreeSet<_>>();
        let absorbs_narration = retained_calls
            .iter()
            .any(|id| cursor.unowned_narrations.contains(*id));
        let rewind = if absorbs_narration {
            match cursor.narration_rewind.as_ref() {
                Some(rewind)
                    if retained_calls.contains(rewind.call_id.as_str())
                        && cursor.activity_count == rewind.activity_count + 1
                        && self.frame.content_revision() == rewind.after_revision
                        && retained_calls
                            .iter()
                            .filter(|id| cursor.unowned_narrations.contains(**id))
                            .count()
                            == 1 =>
                {
                    Some(rewind)
                }
                _ => return Ok(None),
            }
        } else {
            None
        };
        let suffix_start = rewind.map_or(cursor.model_count, |rewind| rewind.model_count);
        let suffix = &publication.model_items()[suffix_start..model_count];
        let mut unowned_narrations = Vec::new();
        let mut appended = Vec::with_capacity(suffix.len());
        for item in suffix {
            item.validate().map_err(AgentError::new)?;
            let index = publication
                .trace_items()
                .binary_search_by_key(&item.sequence, |item| item.sequence())
                .map_err(|_| AgentError::new("模型上下文日志缺少对应轨迹。"))?;
            let trace_item = publication.trace_items()[index].as_ref();
            crate::conversation_trace::validate_model_item_against_trace(item, trace_item)
                .map_err(AgentError::new)?;
            if let crate::ConversationTurnTraceItem::AssistantNarration {
                first_tool_call_id: Some(id),
                ..
            } = trace_item
            {
                if retained_calls.contains(id.as_str()) {
                    continue;
                }
                unowned_narrations.push(id.clone());
            }
            if matches!(
                trace_item,
                crate::ConversationTurnTraceItem::BackendState {
                    placement:
                        crate::conversation_trace::ConversationBackendStatePlacement::AfterMessage,
                    ..
                }
            ) {
                continue;
            }
            appended.push(super::trace_renderer::model_context_item(
                item,
                assistant_message_id,
                trace_item,
            )?);
        }
        // New model rows end at a closed exchange; validate them before touching the cached frame.
        ContextFrame::new(appended.clone()).validate_complete_tool_protocol()?;
        let activity_count =
            rewind.map_or(cursor.activity_count, |rewind| rewind.activity_count) + appended.len();
        let replacement_frame = rewind.map(|rewind| rewind.frame.clone());
        let consumed_narration = rewind.map(|rewind| rewind.call_id.clone());
        let capture_rewind =
            if rewind.is_none() && appended.len() == 1 && unowned_narrations.len() == 1 {
                Some((
                    unowned_narrations[0].clone(),
                    cursor.model_count,
                    cursor.activity_count,
                ))
            } else {
                None
            };
        let changes_frame = !appended.is_empty() || replacement_frame.is_some();
        if let Some(frame) = replacement_frame {
            self.frame = frame;
        }
        // Flush the older body into immutable measured chunks before saving this one-item
        // checkpoint. Cloning it shares history instead of copying all prior tool output.
        let next_rewind = if let Some((call_id, model_count, activity_count)) = capture_rewind {
            self.detector.prepare_frame(&mut self.frame);
            self.frame.share_measured_persistent_baseline()?;
            Some(TraceNarrationRewind {
                call_id,
                model_count,
                activity_count,
                frame: self.frame.clone(),
                after_revision: (0, 0, 0),
            })
        } else {
            None
        };
        for item in appended {
            self.frame.push(item);
        }
        let cursor = self
            .trace_publication_cursor
            .as_mut()
            .expect("checked cursor");
        cursor.identity = publication.identity.clone();
        cursor.model_count = model_count;
        cursor.activity_count = activity_count;
        if let Some(id) = consumed_narration {
            cursor.unowned_narrations.remove(&id);
        }
        cursor.unowned_narrations.extend(unowned_narrations);
        if changes_frame {
            cursor.narration_rewind = next_rewind.map(|mut rewind| {
                rewind.after_revision = self.frame.content_revision();
                rewind
            });
        }
        Ok(Some(activity_count))
    }
}
