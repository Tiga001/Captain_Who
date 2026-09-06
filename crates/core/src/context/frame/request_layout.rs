use super::*;

/// Stable, semistable, then dynamic. Catalogs and current capability instructions precede the
/// conversation snapshot, whose content can change when compaction folds state diffs. Initial
/// user input precedes run-only bootstrap content so its historical projection can share that
/// prefix in the next run. These ranks never change journal order, retention, role, tool bindings
/// or authority. A live Skill activation and a resumed Run snapshot stay in the causal timeline.
fn band(item: &ContextItem) -> u8 {
    let metadata = &item.metadata;
    let has = |source| metadata.sources().contains(&source);
    if has(ContextSource::BackendSystemPrompt) || has(ContextSource::AutomationExecution) {
        0
    } else if has(ContextSource::RunBootstrap) && has(ContextSource::SkillCatalog) {
        1
    } else if has(ContextSource::RunBootstrap) && has(ContextSource::RuntimeGuard) {
        2
    } else if has(ContextSource::CapabilityInstructions) {
        3
    } else if has(ContextSource::WorldStateSnapshot) && metadata.scope == ContextScope::Conversation
    {
        4
    } else if has(ContextSource::ConversationSummary) {
        5
    } else if has(ContextSource::RunInput)
        || (has(ContextSource::RunBootstrap) && has(ContextSource::InputAttachment))
    {
        7
    } else if has(ContextSource::RunTimeline) {
        10
    } else if metadata.scope == ContextScope::Conversation {
        6
    } else if has(ContextSource::RunBootstrap) && has(ContextSource::SkillInstructions) {
        8
    } else if has(ContextSource::RunBootstrap) && has(ContextSource::WorldStateSnapshot) {
        9
    } else if metadata.retention == ContextRetention::RequestOnly {
        // Todo, ignored-question state, repair and file-transaction guidance retain their
        // existing contribution order at the request tail.
        11
    } else {
        10
    }
}

pub(super) fn ordered_items(mut items: Vec<&ContextItem>) -> Vec<&ContextItem> {
    // The summarizer has its own compact request contract, not the agent-work layout.
    if items.iter().any(|item| {
        item.metadata
            .sources()
            .contains(&ContextSource::CompactionRequest)
    }) {
        return items;
    }
    // Stable sort preserves every causal timeline and tool exchange. After compaction,
    // request_order restores the relative placement of journal records and retained overlays.
    items.sort_by_key(|item| {
        (
            band(item),
            item.metadata.request_order().unwrap_or(u64::MAX),
        )
    });
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::measurement::HeuristicTokenEstimator;
    use crate::llm::LlmImage;
    use serde_json::json;

    fn item(text: &str, source: ContextSource, scope: ContextScope) -> ContextItem {
        ContextItem::text(
            LlmMessageRole::User,
            text,
            source,
            scope,
            ContextRetention::Retained,
        )
    }

    fn bootstrap(text: &str, source: ContextSource) -> ContextItem {
        item(text, source, ContextScope::Run).with_source(ContextSource::RunBootstrap)
    }

    fn tail(text: &str, source: ContextSource) -> ContextItem {
        ContextItem::text(
            LlmMessageRole::System,
            text,
            source,
            ContextScope::Run,
            ContextRetention::RequestOnly,
        )
    }

    fn contents(frame: &ContextFrame) -> Vec<&str> {
        frame
            .model_request_items()
            .iter()
            .map(|item| item.message.content())
            .collect()
    }

    fn initial() -> ContextFrame {
        let mut prior_assistant = item(
            "旧回复",
            ContextSource::ConversationHistory,
            ContextScope::Conversation,
        );
        prior_assistant.message = LlmMessage::text(LlmMessageRole::Assistant, "旧回复");
        let mut attachment = bootstrap("当前附件", ContextSource::InputAttachment);
        attachment.message.images_mut().unwrap().push(LlmImage {
            mime_type: "image/png".into(),
            data_base64: "YWJj".into(),
        });
        ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "稳定系统",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            item(
                "旧摘要",
                ContextSource::ConversationSummary,
                ContextScope::Conversation,
            ),
            item(
                "会话初始状态",
                ContextSource::WorldStateSnapshot,
                ContextScope::Conversation,
            ),
            item(
                "旧用户",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
            ),
            prior_assistant,
            item(
                "输入前差异",
                ContextSource::WorldStateDiff,
                ContextScope::Conversation,
            ),
            item(
                "本次输入一",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
            ),
            item(
                "两输入之间差异",
                ContextSource::WorldStateDiff,
                ContextScope::Conversation,
            ),
            item(
                "本次输入二",
                ContextSource::CurrentTurn,
                ContextScope::Conversation,
            ),
            bootstrap("Run初始状态", ContextSource::WorldStateSnapshot),
            attachment,
            bootstrap("Skill目录", ContextSource::SkillCatalog),
            bootstrap("预激活Skill", ContextSource::SkillInstructions),
            bootstrap("初始协作目录", ContextSource::RuntimeGuard),
        ])
    }

    #[test]
    fn request_layout_preserves_all_content_and_causal_events_without_rewriting_journal() {
        let mut frame = initial();
        frame.mark_initial_run_input();
        let group = ContextGroup::tool_exchange("layout-call");
        frame.push(ContextItem::assistant(
            "运行叙述",
            vec![LlmToolCall {
                id: "layout-call".into(),
                name: "read_file".into(),
                args: json!({"path":"笔记.md"}),
            }],
            ContextMetadata::new(
                ContextSource::ModelResponse,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(group.clone()),
        ));
        frame.push(ContextItem::tool_result(
            "layout-call",
            "工具结果",
            false,
            ContextMetadata::new(
                ContextSource::ToolResult,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(group),
        ));
        frame.push(item(
            "运行中新Skill",
            ContextSource::SkillInstructions,
            ContextScope::Run,
        ));
        frame.push(item(
            "运行中状态差异",
            ContextSource::WorldStateDiff,
            ContextScope::Run,
        ));
        frame.push(item(
            "恢复的新epoch完整状态",
            ContextSource::WorldStateSnapshot,
            ContextScope::Run,
        ));
        frame.push(item(
            "运行中保护提示",
            ContextSource::RuntimeGuard,
            ContextScope::Run,
        ));
        frame.push(tail("Todo", ContextSource::RuntimeTodo));
        frame.push(
            tail("当前能力说明", ContextSource::RuntimeGuard)
                .with_source(ContextSource::CapabilityInstructions),
        );
        frame.push(tail("忽略状态", ContextSource::RuntimeGuard));
        frame.push(tail("修复提示", ContextSource::RuntimeGuard));
        frame.push(tail("文件事务", ContextSource::FileTransaction));
        frame.validate_cache_layout().unwrap();
        frame.validate_complete_tool_protocol().unwrap();
        let canonical = frame.to_messages();
        assert_eq!(
            contents(&frame),
            vec![
                "稳定系统",
                "Skill目录",
                "初始协作目录",
                "当前能力说明",
                "会话初始状态",
                "旧摘要",
                "旧用户",
                "旧回复",
                "输入前差异",
                "本次输入一",
                "两输入之间差异",
                "本次输入二",
                "当前附件",
                "预激活Skill",
                "Run初始状态",
                "运行叙述",
                "工具结果",
                "运行中新Skill",
                "运行中状态差异",
                "恢复的新epoch完整状态",
                "运行中保护提示",
                "Todo",
                "忽略状态",
                "修复提示",
                "文件事务",
            ]
        );
        let projected = frame.clone().into_model_request_messages();
        assert_eq!(frame.to_messages(), canonical);
        assert_eq!(projected.len(), canonical.len());
        let mut unmatched = canonical;
        for message in projected {
            let index = unmatched
                .iter()
                .position(|original| original == &message)
                .expect("layout must preserve the entire message, role, image and tool identity");
            unmatched.remove(index);
        }
        assert!(unmatched.is_empty());
    }

    #[test]
    fn cold_and_shared_baseline_layout_match_and_checkpoint_keeps_bootstrap_distinct_from_resume() {
        let mut cold = initial();
        let durable_count = 9;
        let mut durable =
            ContextFrame::new(cold.iter_items().take(durable_count).cloned().collect());
        durable.measure_incrementally(Arc::new(HeuristicTokenEstimator));
        let baseline = durable.share_measured_persistent_baseline().unwrap();
        let mut shared = ContextFrame::from_measured_baseline(baseline);
        for overlay in cold.iter_items().skip(durable_count).cloned() {
            shared.push(overlay);
        }
        cold.mark_initial_run_input();
        shared.mark_initial_run_input();
        assert_eq!(
            cold.clone().into_model_request_messages(),
            shared.clone().into_model_request_messages()
        );
        let checkpoint = shared.checkpoint_items().unwrap();
        let mut restored = ContextFrame::from_checkpoint_items(checkpoint).unwrap();
        restored.push(item(
            "恢复状态",
            ContextSource::WorldStateSnapshot,
            ContextScope::Run,
        ));
        assert_eq!(contents(&restored).last(), Some(&"恢复状态"));
        assert_eq!(
            restored
                .model_request_items()
                .iter()
                .filter(|item| item.message.content() == "预激活Skill")
                .count(),
            1
        );
        assert_eq!(
            durable
                .to_messages()
                .iter()
                .map(LlmMessage::content)
                .collect::<Vec<_>>(),
            vec![
                "稳定系统",
                "旧摘要",
                "会话初始状态",
                "旧用户",
                "旧回复",
                "输入前差异",
                "本次输入一",
                "两输入之间差异",
                "本次输入二"
            ]
        );
    }

    fn prepare_request(mut frame: ContextFrame) -> Vec<LlmMessage> {
        frame.mark_initial_run_input();
        frame.push(
            tail("当前能力说明", ContextSource::RuntimeGuard)
                .with_source(ContextSource::CapabilityInstructions),
        );
        frame.into_model_request_messages()
    }

    #[test]
    fn changed_full_and_summary_preserve_catalog_and_capability_prefix() {
        let original = initial();
        let changed = ContextFrame::new(
            original
                .iter_items()
                .cloned()
                .map(|mut item| {
                    let sources = item.metadata.sources();
                    let replacement = if item.metadata.scope == ContextScope::Conversation
                        && sources.contains(&ContextSource::WorldStateSnapshot)
                    {
                        Some("折叠状态差异后的会话基线")
                    } else if sources.contains(&ContextSource::ConversationSummary) {
                        Some("新的压缩摘要")
                    } else {
                        None
                    };
                    if let Some(content) = replacement {
                        item.message = LlmMessage::text(item.message.role(), content);
                    }
                    item
                })
                .collect(),
        );
        let before = prepare_request(original);
        let after = prepare_request(changed);
        let common_prefix = before
            .iter()
            .zip(&after)
            .take_while(|(left, right)| left == right)
            .count();
        assert_eq!(common_prefix, 4);
        assert_eq!(
            before[..common_prefix]
                .iter()
                .map(LlmMessage::content)
                .collect::<Vec<_>>(),
            ["稳定系统", "Skill目录", "初始协作目录", "当前能力说明"]
        );
    }

    #[test]
    fn user_input_prefix_survives_promotion_to_next_run_history() {
        let original = initial();
        let mut next_run = ContextFrame::new(
            original
                .iter_items()
                .filter(|item| {
                    let sources = item.metadata.sources();
                    item.metadata.scope == ContextScope::Conversation
                        || sources.contains(&ContextSource::BackendSystemPrompt)
                        || sources.contains(&ContextSource::SkillCatalog)
                        || sources.contains(&ContextSource::RuntimeGuard)
                })
                .cloned()
                .collect(),
        );
        let mut reply = item(
            "上一轮已提交回复",
            ContextSource::ConversationHistory,
            ContextScope::Conversation,
        );
        reply.message = LlmMessage::text(LlmMessageRole::Assistant, "上一轮已提交回复");
        next_run.push(reply);
        next_run.push(item(
            "下一轮用户输入",
            ContextSource::CurrentTurn,
            ContextScope::Conversation,
        ));
        next_run.push(bootstrap("下一轮Skill", ContextSource::SkillInstructions));
        next_run.push(bootstrap("下一轮状态", ContextSource::WorldStateSnapshot));
        let before = prepare_request(original);
        let after = prepare_request(next_run);
        let input_end = before
            .iter()
            .position(|message| message.content() == "本次输入二")
            .unwrap()
            + 1;
        assert_eq!(before[..input_end], after[..input_end]);
        // The previous attachment and Run bootstrap are not promoted into history. The shared
        // prefix extends through the user cohort, without claiming reuse of the whole old run.
        assert_eq!(before[input_end].content(), "当前附件");
        assert_eq!(after[input_end].content(), "上一轮已提交回复");
        for old_overlay in ["当前附件", "预激活Skill", "Run初始状态"] {
            assert!(!after.iter().any(|message| message.content() == old_overlay));
        }
    }

    #[test]
    fn summary_generation_contract_is_not_reordered() {
        let frame = ContextFrame::new(vec![
            item(
                "摘要任务",
                ContextSource::CompactionRequest,
                ContextScope::Run,
            ),
            item(
                "原摘要",
                ContextSource::ConversationSummary,
                ContextScope::Conversation,
            ),
            item(
                "被压缩的状态",
                ContextSource::WorldStateSnapshot,
                ContextScope::Conversation,
            ),
            item(
                "原始日志",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
            ),
        ]);
        assert_eq!(
            frame.clone().into_model_request_messages(),
            frame.into_messages()
        );
    }

    #[derive(Debug, Default)]
    struct RecordingEstimator(std::sync::Mutex<Vec<Vec<String>>>);

    impl ContextTokenEstimator for RecordingEstimator {
        fn identity(&self) -> ContextEstimatorIdentity {
            ContextEstimatorIdentity {
                family: "layout_verification",
                version: 1,
                variant: None,
            }
        }
        fn estimate_message(&self, message: &LlmMessage) -> ContextMessageEstimate {
            HeuristicTokenEstimator.estimate_message(message)
        }
        fn estimate_messages(&self, messages: &[&LlmMessage]) -> ContextMessageEstimate {
            self.0.lock().unwrap().push(
                messages
                    .iter()
                    .map(|message| message.content().to_string())
                    .collect(),
            );
            HeuristicTokenEstimator.estimate_messages(messages)
        }
        fn estimate_tool_definitions(&self, tools: &[crate::protocol::AgentToolDefinition]) -> u64 {
            HeuristicTokenEstimator.estimate_tool_definitions(tools)
        }
        fn request_structure_tokens(&self) -> u64 {
            HeuristicTokenEstimator.request_structure_tokens()
        }
        fn image_token_reserve_per_image(&self) -> u64 {
            HeuristicTokenEstimator.image_token_reserve_per_image()
        }
        fn full_recount_threshold_percent(&self) -> Option<u8> {
            Some(1)
        }
    }

    #[test]
    fn full_measurement_uses_sent_layout_and_invalidates_when_input_classification_changes() {
        let estimator = Arc::new(RecordingEstimator::default());
        let mut frame = initial();
        frame.measure_full(estimator.clone());
        frame.mark_initial_run_input();
        frame.measure_full(estimator.clone());
        frame.measure_full(estimator.clone());
        let measured = estimator.0.lock().unwrap();
        assert_eq!(
            measured.len(),
            2,
            "changing input classification invalidates full recount, unchanged requests reuse it"
        );
        // History and initial input now occupy adjacent bands before Skill/Run bootstrap.
        // Classification still changes, even when the projected messages remain identical.
        assert_eq!(measured[0], measured[1]);
        assert_eq!(measured[1], contents(&frame));
        let manifest = frame.manifest();
        for (request_index, item) in frame.model_request_items().iter().enumerate() {
            let original_index = frame
                .iter_items()
                .position(|original| std::ptr::eq(original, *item))
                .unwrap();
            assert_eq!(
                manifest.entries[original_index].model_request_index,
                request_index
            );
        }
    }
}
