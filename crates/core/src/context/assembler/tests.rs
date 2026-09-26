mod workspace_binding;
use super::*;
use crate::conversation_trace::{
    ConversationTraceRecorder, ConversationTurnTrace, ConversationTurnTraceTerminalStatus,
    CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};
use crate::llm::{model_response_tool_call_id, LlmMessage, LlmMessagePlacement, LlmToolCall};
use crate::protocol::{AgentApprovalStatus, AgentToolCall, AgentToolResult};
use crate::world_state::{
    WorldStateDiff, WorldStateLifetime, WorldStateSectionEnvelope, WorldStateSectionId,
};
use crate::ContextJournalCursor;
use serde_json::json;

fn compaction_summary() -> ContextCompactionSummary {
    let covered_through = ContextJournalCursor::message("assistant-old");
    let prefix = crate::ContextCompactionPrefix {
        conversation_id: "conversation-1".to_string(),
        source_revision: "source-revision-1".to_string(),
        covered_through: covered_through.clone(),
        previous_summary: None,
        source_items: vec![crate::ContextCompactionSourceItem::Message {
            cursor: covered_through.clone(),
            role: "assistant".to_string(),
            content: "The user requested an old task and the agent completed it.".to_string(),
            created_at: 1,
            status: Some("sent".to_string()),
            terminal_status: None,
            terminal_error: None,
        }],
    };
    ContextCompactionSummary {
        schema_version: crate::CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
        id: "summary-1".to_string(),
        conversation_id: "conversation-1".to_string(),
        source_revision: "source-revision-1".to_string(),
        previous_summary_id: None,
        covered_through,
        content: "The user requested an old task and the agent completed it.".to_string(),
        continuity: crate::ContextContinuitySnapshot::from_prefix(&prefix).unwrap(),
        generation: crate::ContextCompactionGeneration::test(),
        source_input_tokens: 100,
        summary_input_tokens: 20,
        continuity_input_tokens: 30,
        uncovered_tail_input_tokens: 0,
        replacement_input_tokens: 50,
        created_at: 1,
    }
}

fn message(role: &str, content: &str) -> AgentChatMessage {
    AgentChatMessage {
        conversation_completion_covered: false,
        message_id: None,
        role: role.to_string(),
        content: content.to_string(),
        created_at: None,
        conversation_turn_trace: None,
        conversation_model_context_items: Vec::new(),
    }
}

fn identified_message(id: &str, role: &str, content: &str) -> AgentChatMessage {
    AgentChatMessage {
        conversation_completion_covered: false,
        message_id: Some(id.to_string()),
        role: role.to_string(),
        content: content.to_string(),
        created_at: None,
        conversation_turn_trace: None,
        conversation_model_context_items: Vec::new(),
    }
}

fn current_assistant_message(id: &str, content: &str) -> AgentChatMessage {
    AgentChatMessage {
        conversation_completion_covered: false,
        message_id: Some(id.to_string()),
        role: "assistant".to_string(),
        content: content.to_string(),
        created_at: None,
        conversation_turn_trace: Some(ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: format!("run-{id}"),
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: id.to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: Vec::new(),
        }),
        conversation_model_context_items: Vec::new(),
    }
}

fn conversation_world_state_records(
    effective_before_message_id: &str,
) -> Vec<AnchoredWorldStateRecord> {
    let initial = WorldStateSnapshot::new(
        "conversation-epoch-1",
        0,
        vec![
            WorldStateSectionEnvelope::model_visible(
                WorldStateSectionId::WorkspaceBinding,
                WorldStateLifetime::Conversation,
                json!({
                    "available": true,
                    "displayName": "old-workspace",
                    "rootPath": "/private/authoritative/root"
                }),
                json!({
                    "available": true,
                    "displayName": "old-workspace"
                }),
            )
            .unwrap(),
            WorldStateSectionEnvelope::host_only(
                WorldStateSectionId::extension("private.credentials").unwrap(),
                WorldStateLifetime::Conversation,
                json!({"apiKey": "host-only-secret"}),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let target = WorldStateSnapshot::new(
        "conversation-epoch-1",
        1,
        vec![
            WorldStateSectionEnvelope::model_visible(
                WorldStateSectionId::WorkspaceBinding,
                WorldStateLifetime::Conversation,
                json!({
                    "available": true,
                    "displayName": "new-workspace",
                    "rootPath": "/private/new-authoritative/root"
                }),
                json!({
                    "available": true,
                    "displayName": "new-workspace"
                }),
            )
            .unwrap(),
            WorldStateSectionEnvelope::host_only(
                WorldStateSectionId::extension("private.credentials").unwrap(),
                WorldStateLifetime::Conversation,
                json!({"apiKey": "new-host-only-secret"}),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let diff = WorldStateDiff::between(&initial, &target).unwrap();
    vec![
        AnchoredWorldStateRecord::new(WorldStateRecord::Full(initial), None).unwrap(),
        AnchoredWorldStateRecord::new(
            WorldStateRecord::Diff(diff),
            Some(effective_before_message_id.to_string()),
        )
        .unwrap(),
    ]
}

fn traced_assistant(content: &str) -> AgentChatMessage {
    let provider_call_id = "provider-history-call";
    let call_id = model_response_tool_call_id("run-previous", 0, 0, provider_call_id);
    let call = AgentToolCall {
        id: call_id.clone(),
        tool: "apply_patch".to_string(),
        args: json!({
            "request": {
                "action": "apply",
                "operation": "create",
                "filePath": "src/new.rs",
                "content": "pub fn new() {}\n"
            }
        }),
        approval_status: AgentApprovalStatus::Approved,
        reason: None,
    };
    let result = AgentToolResult {
        exact_archive_file: None,
        call_id: call_id.clone(),
        tool: call.tool.clone(),
        ok: true,
        result: Some(json!({
            "filePath": "src/new.rs",
            "status": "applied",
            "additions": 4,
            "deletions": 0
        })),
        error: None,
    };
    let mut recorder = ConversationTraceRecorder::default();
    recorder
        .record_narration("I will update the file.")
        .unwrap();
    let call_sequence = recorder
        .record_tool_call_with_identity(
            &call,
            crate::AgentToolIdentity::Builtin {
                tool_name: call.tool.clone(),
            },
        )
        .unwrap();
    recorder
        .record_model_tool_call_message(
            call_sequence,
            0,
            &LlmMessage::assistant(
                "",
                vec![LlmToolCall {
                    id: call.id.clone(),
                    name: call.tool.clone(),
                    args: call.args.clone(),
                }],
            ),
            crate::AgentProviderToolCallIdentity {
                provider_call_id: provider_call_id.to_string(),
                provider_tool_index: 0,
                runtime_call_id: call.id.clone(),
            },
        )
        .unwrap();
    let result_sequence = recorder.record_tool_result(&call, &result).unwrap();
    recorder
        .record_model_message(
            result_sequence,
            0,
            &LlmMessage::tool_result(
                call.id.clone(),
                serde_json::to_string(result.result.as_ref().unwrap()).unwrap(),
                false,
            ),
        )
        .unwrap();
    let snapshot = recorder.snapshot();
    let trace = recorder.finish(
        "run-previous",
        "conversation-1",
        "assistant-previous",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    trace
        .validate_complete_model_context(&snapshot.model_context_items)
        .unwrap();
    AgentChatMessage {
        conversation_completion_covered: false,
        message_id: Some("assistant-previous".to_string()),
        role: "assistant".to_string(),
        content: content.to_string(),
        created_at: None,
        conversation_turn_trace: Some(trace),
        conversation_model_context_items: snapshot.model_context_items,
    }
}

#[test]
fn assembles_ordered_context_with_provenance() {
    let frame = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "backend rules".to_string(),
        compaction_summary: None,
        world_state_records: Vec::new(),
        initial_run_world_state: None,
        messages: vec![
            message("user", "old question"),
            current_assistant_message("assistant-old", "old answer"),
            message("user", "current question"),
        ],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments {
            text: "attachment body".to_string(),
            images: vec![LlmImage {
                mime_type: "image/png".to_string(),
                data_base64: "abc".to_string(),
            }],
        },
    })
    .unwrap();

    let messages = frame.to_messages();
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0].role(), LlmMessageRole::System);
    assert_eq!(messages[1].content(), "old question");
    assert_eq!(messages[2].content(), "old answer");
    assert_eq!(messages[3].role(), LlmMessageRole::User);
    assert_eq!(messages[3].content(), "current question");
    assert_eq!(messages[4].content(), "attachment body");
    assert_eq!(messages[4].images().len(), 1);

    let manifest = frame.manifest();
    assert_eq!(manifest.entries[0].sources, vec!["backend_system_prompt"]);
    assert_eq!(manifest.entries[1].sources, vec!["conversation_history"]);
    assert_eq!(manifest.entries[3].sources, vec!["current_turn"]);
    assert_eq!(manifest.entries[3].scope, "conversation");
    assert_eq!(manifest.entries[3].retention, "retained");
    assert_eq!(
        manifest.entries[4].sources,
        vec!["input_attachment", "run_bootstrap"]
    );
    assert_eq!(manifest.entries[4].scope, "run");
    assert_eq!(manifest.entries[4].retention, "retained");
    assert_eq!(manifest.entries[4].image_base64_bytes, 3);
    let serialized = serde_json::to_string(&manifest).unwrap();
    assert!(!serialized.contains("current question"));
    assert!(!serialized.contains("attachment body"));
}

#[test]
fn attachment_only_current_user_message_is_sendable_and_reaches_model_context() {
    let frame = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "backend rules".to_string(),
        compaction_summary: None,
        world_state_records: Vec::new(),
        initial_run_world_state: None,
        messages: vec![message("user", "")],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments {
            text: "attachment body".to_string(),
            images: Vec::new(),
        },
    })
    .unwrap();

    let messages = frame.to_messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role(), LlmMessageRole::System);
    assert_eq!(messages[1].role(), LlmMessageRole::User);
    assert_eq!(messages[1].content(), "attachment body");
}

#[test]
fn request_boundary_world_state_cold_and_incremental_assembly_match_after_guidance() {
    let mut records = conversation_world_state_records("unused");
    records[1].effective_before_message_id = None;
    records[1].request_boundary = Some(crate::WorldStateRequestBoundary {
        run_id: "run-assistant-current".into(),
        assistant_message_id: "assistant-current".into(),
        request_index: 2,
        after_trace_sequence: Some(1),
    });
    records[1].model_observed = false;
    let mut assistant = current_assistant_message("assistant-current", "done");
    assistant.conversation_turn_trace.as_mut().unwrap().items = vec![
        crate::ConversationTurnTraceItem::AssistantNarration {
            provider_turn_id: None,
            first_tool_call_id: None,
            sequence: 0,
            content: "before-guidance".into(),
            truncated: false,
        },
        crate::ConversationTurnTraceItem::UserGuidance {
            sequence: 1,
            guidance_id: "guidance".into(),
            client_message_id: "client".into(),
            content: "new-guidance".into(),
            attachments: Vec::new(),
            folder_references: Vec::new(),
            created_at: 1,
            truncated: false,
        },
        crate::ConversationTurnTraceItem::AssistantNarration {
            provider_turn_id: None,
            first_tool_call_id: None,
            sequence: 2,
            content: "after-boundary".into(),
            truncated: false,
        },
    ];
    assistant.conversation_model_context_items = [
        ("assistant", "before-guidance"),
        ("user", "new-guidance"),
        ("assistant", "after-boundary"),
    ]
    .into_iter()
    .enumerate()
    .map(
        |(sequence, (role, content))| crate::ConversationModelContextItem {
            images: Vec::new(),
            sequence: sequence as u64,
            ordinal: 0,
            role: role.into(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            is_error: false,
        },
    )
    .collect();
    let assemble = |records: Vec<AnchoredWorldStateRecord>| {
        ContextAssembler::assemble(ContextAssemblyInput {
            system_prompt: "rules".into(),
            compaction_summary: None,
            world_state_records: records,
            initial_run_world_state: None,
            messages: vec![
                identified_message("user", "user", "input"),
                assistant.clone(),
            ],
            skill_discovery: None,
            skill_activation: None,
            attachments: ContextAttachments::default(),
        })
        .unwrap()
    };
    let cold = assemble(records.clone());
    let mut incremental = assemble(records[..1].to_vec());
    incremental
        .sync_conversation_world_state_records(&records, false)
        .unwrap();
    assert_eq!(cold.to_messages(), incremental.to_messages());
    let contents = cold.to_messages();
    let index = contents
        .iter()
        .position(|message| message.content().contains("\"recordType\":\"diff\""))
        .unwrap();
    assert!(contents[index - 1].content().contains("new-guidance"));
    assert_eq!(contents[index + 1].content(), "after-boundary");
    let manifest = cold.manifest();
    assert!(manifest.entries[index]
        .sources
        .contains(&"world_state_unobserved"));
}

#[test]
fn request_boundary_world_state_uses_summary_prefix_when_trace_anchor_was_compacted() {
    for retain_assistant_tail in [false, true] {
        let mut records = conversation_world_state_records("unused");
        records[1].effective_before_message_id = None;
        records[1].request_boundary = Some(crate::WorldStateRequestBoundary {
            run_id: "run-assistant-current".into(),
            assistant_message_id: "assistant-current".into(),
            request_index: 2,
            after_trace_sequence: Some(1),
        });
        records[1].model_observed = false;
        let mut summary = compaction_summary();
        summary.covered_through = ContextJournalCursor::trace_item("assistant-current", 1);
        summary.continuity.covered_through = summary.covered_through.clone();
        let mut messages = Vec::new();
        if retain_assistant_tail {
            let mut assistant = current_assistant_message("assistant-current", "");
            assistant.conversation_turn_trace.as_mut().unwrap().items =
                vec![crate::ConversationTurnTraceItem::AssistantNarration {
                    provider_turn_id: None,
                    first_tool_call_id: None,
                    sequence: 2,
                    content: "retained-tail".into(),
                    truncated: false,
                }];
            assistant.conversation_model_context_items =
                vec![crate::ConversationModelContextItem {
                    images: Vec::new(),
                    sequence: 2,
                    ordinal: 0,
                    role: "assistant".into(),
                    content: "retained-tail".into(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                    is_error: false,
                }];
            messages.push(assistant);
        }
        messages.push(identified_message("user-current", "user", "next-input"));
        let assemble = |records| {
            ContextAssembler::assemble(ContextAssemblyInput {
                system_prompt: "rules".into(),
                compaction_summary: Some(summary.clone()),
                world_state_records: records,
                initial_run_world_state: None,
                messages: messages.clone(),
                skill_discovery: None,
                skill_activation: None,
                attachments: ContextAttachments::default(),
            })
            .unwrap()
        };
        let cold = assemble(records.clone());
        let mut incremental = assemble(records[..1].to_vec());
        incremental
            .sync_conversation_world_state_records(&records, false)
            .unwrap();
        assert_eq!(cold.to_messages(), incremental.to_messages());
        let contents = cold.to_messages();
        assert!(contents[2].content().contains("\"recordType\":\"full\""));
        assert!(contents[3].content().contains("\"recordType\":\"diff\""));
        assert!(contents[4].content().contains(if retain_assistant_tail {
            "retained-tail"
        } else {
            "next-input"
        }));
    }
}

#[test]
fn request_boundary_world_state_rejects_a_future_trace_anchor() {
    let mut records = conversation_world_state_records("unused");
    records[1].effective_before_message_id = None;
    records[1].request_boundary = Some(crate::WorldStateRequestBoundary {
        run_id: "run-assistant-current".into(),
        assistant_message_id: "assistant-current".into(),
        request_index: 2,
        after_trace_sequence: Some(99),
    });
    let error = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".into(),
        compaction_summary: None,
        world_state_records: records,
        initial_run_world_state: None,
        messages: vec![
            identified_message("user", "user", "input"),
            current_assistant_message("assistant-current", ""),
        ],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap_err();
    assert!(error.to_string().contains("安全 trace 前缀"));
}

#[test]
fn places_world_state_full_after_summary_and_diff_immediately_before_anchor() {
    let frame = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "backend rules".to_string(),
        compaction_summary: Some(compaction_summary()),
        world_state_records: conversation_world_state_records("user-current"),
        initial_run_world_state: None,
        messages: vec![identified_message(
            "user-current",
            "user",
            "continue in the current workspace",
        )],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap();

    let messages = frame.to_messages();
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0].role(), LlmMessageRole::System);
    assert_eq!(messages[1].role(), LlmMessageRole::System);
    assert_eq!(
        messages[1].placement(),
        LlmMessagePlacement::BackendStateTimeline
    );
    assert!(messages[1].content().contains("较早对话的有损语义摘要"));
    assert!(messages[2].content().contains("\"recordType\":\"full\""));
    assert!(messages[2]
        .content()
        .contains("\"lifetime\":\"conversation\""));
    assert!(messages[2].content().contains("old-workspace"));
    assert!(messages[3].content().contains("\"recordType\":\"diff\""));
    assert!(messages[3]
        .content()
        .contains("\"lifetime\":\"conversation\""));
    assert!(messages[3].content().contains("new-workspace"));
    assert_eq!(messages[4].content(), "continue in the current workspace");
    assert!(!messages.iter().any(|message| message
        .content()
        .contains("BEGIN_UNTRUSTED_CONTINUITY_RECORDS_JSON")));

    let rendered = messages
        .iter()
        .map(|message| message.content())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!rendered.contains("/private/authoritative/root"));
    assert!(!rendered.contains("/private/new-authoritative/root"));
    assert!(!rendered.contains("private.credentials"));
    assert!(!rendered.contains("host-only-secret"));
    assert!(!rendered.contains("conversation-epoch-1"));
    assert!(!rendered.contains("world-state-sha256-v1:"));

    let manifest = frame.manifest();
    assert_eq!(manifest.entries[2].sources, vec!["world_state_snapshot"]);
    assert_eq!(manifest.entries[2].scope, "conversation");
    assert_eq!(manifest.entries[2].retention, "retained");
    assert_eq!(manifest.entries[2].origin_kind, Some("world_state_record"));
    assert_eq!(manifest.entries[3].sources, vec!["world_state_diff"]);
    assert_eq!(manifest.entries[3].scope, "conversation");
    assert_eq!(manifest.entries[3].retention, "retained");
    assert_eq!(manifest.entries[4].sources, vec!["current_turn"]);
}

#[test]
fn places_initial_run_world_state_after_durable_timeline_before_attachments() {
    let run_snapshot = WorldStateSnapshot::new(
        "run-epoch-1",
        0,
        vec![
            WorldStateSectionEnvelope::model_visible(
                WorldStateSectionId::EffectiveTools,
                WorldStateLifetime::Run,
                json!({"names": ["read_file"], "executionToken": "host-secret"}),
                json!({"names": ["read_file"]}),
            )
            .unwrap(),
            WorldStateSectionEnvelope::host_only(
                WorldStateSectionId::ModelCapabilities,
                WorldStateLifetime::Run,
                json!({"imageInput": false, "providerSecret": "hidden"}),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let frame = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".to_string(),
        compaction_summary: None,
        world_state_records: Vec::new(),
        initial_run_world_state: Some(run_snapshot),
        messages: vec![identified_message("user-1", "user", "inspect the file")],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments {
            text: "ATTACHMENT_MARKER".to_string(),
            images: Vec::new(),
        },
    })
    .unwrap();

    let messages = frame.to_messages();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[1].content(), "inspect the file");
    assert!(messages[2].content().contains("read_file"));
    assert!(messages[2].content().contains("\"lifetime\":\"run\""));
    assert!(!messages[2].content().contains("host-secret"));
    assert!(!messages[2].content().contains("providerSecret"));
    assert_eq!(messages[3].content(), "ATTACHMENT_MARKER");
    let manifest = frame.manifest();
    assert_eq!(
        manifest.entries[2].sources,
        vec!["world_state_snapshot", "run_bootstrap"]
    );
    assert_eq!(manifest.entries[2].scope, "run");
    assert_eq!(manifest.entries[2].retention, "retained");
    assert_eq!(
        manifest.entries[3].sources,
        vec!["input_attachment", "run_bootstrap"]
    );
}

#[test]
fn direct_library_messages_without_a_world_state_ledger_keep_the_current_shape() {
    let frame = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".to_string(),
        compaction_summary: None,
        world_state_records: Vec::new(),
        initial_run_world_state: None,
        messages: vec![identified_message(
            "user-direct-library",
            "user",
            "direct library message",
        )],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap();

    let messages = frame.to_messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role(), LlmMessageRole::System);
    assert_eq!(messages[1].content(), "direct library message");
    assert!(frame.manifest().entries.iter().all(|entry| !entry
        .sources
        .iter()
        .any(|source| source.starts_with("world_state"))));
}

#[test]
fn rejects_missing_world_state_anchor_and_broken_revision_chain() {
    let messages = vec![identified_message("user-current", "user", "continue")];
    let missing_anchor = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".to_string(),
        compaction_summary: None,
        world_state_records: conversation_world_state_records("missing-message"),
        initial_run_world_state: None,
        messages: messages.clone(),
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap_err();
    assert!(missing_anchor.to_string().contains("不存在的消息 anchor"));

    let mut broken_records = conversation_world_state_records("user-current");
    let WorldStateRecord::Diff(diff) = &mut broken_records[1].record else {
        unreachable!("test fixture has a diff as its second record");
    };
    diff.base_revision = format!("{}{}", crate::WORLD_STATE_REVISION_PREFIX, "0".repeat(64));
    let broken_chain = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".to_string(),
        compaction_summary: None,
        world_state_records: broken_records,
        initial_run_world_state: None,
        messages,
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap_err();
    assert!(broken_chain.to_string().contains("base revision mismatch"));
}

#[test]
fn places_attachments_before_each_activated_skill() {
    let activation = AgentSkillActivation {
        activation_revision: "activation-sha256-v1:ordered".to_string(),
        skills: vec![
            AgentActivatedSkill {
                id: "workspace:w:first".to_string(),
                name: "first".to_string(),
                revision: "skill-sha256-v1:first".to_string(),
                source: "workspace".to_string(),
                instructions: "FIRST_SKILL_MARKER".to_string(),
                source_bytes: 18,
                resources: None,
            },
            AgentActivatedSkill {
                id: "workspace:w:second".to_string(),
                name: "second".to_string(),
                revision: "skill-sha256-v1:second".to_string(),
                source: "workspace".to_string(),
                instructions: "SECOND_SKILL_MARKER".to_string(),
                source_bytes: 19,
                resources: None,
            },
        ],
    };
    let frame = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".to_string(),
        compaction_summary: None,
        world_state_records: Vec::new(),
        initial_run_world_state: None,
        messages: vec![message("user", "current question")],
        skill_discovery: None,
        skill_activation: Some(activation),
        attachments: ContextAttachments {
            text: "ATTACHMENT_MARKER".to_string(),
            images: Vec::new(),
        },
    })
    .unwrap();

    let messages = frame.to_messages();
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[1].content(), "current question");
    assert_eq!(messages[2].content(), "ATTACHMENT_MARKER");
    assert!(messages[3].content().contains("FIRST_SKILL_MARKER"));
    assert!(messages[4].content().contains("SECOND_SKILL_MARKER"));
    assert!(messages[3].content().contains("\"source\":\"workspace\""));
    assert!(!messages[3].content().contains("description"));

    let manifest = frame.manifest();
    assert_eq!(
        manifest.entries[2].sources,
        vec!["input_attachment", "run_bootstrap"]
    );
    for (index, id) in [(3, "workspace:w:first"), (4, "workspace:w:second")] {
        assert_eq!(
            manifest.entries[index].sources,
            vec!["skill_instructions", "run_bootstrap"]
        );
        assert_eq!(manifest.entries[index].scope, "run");
        assert_eq!(manifest.entries[index].retention, "retained");
        assert_eq!(manifest.entries[index].origin_kind, Some("skill"));
        assert_eq!(manifest.entries[index].origin_id, Some(id));
    }
}

#[test]
fn places_discovery_metadata_before_full_skill_instructions_without_leaking_identity() {
    let discovery = AgentSkillDiscoverySnapshot {
        schema_version: crate::skills::AGENT_SKILL_DISCOVERY_SCHEMA_VERSION,
        catalog_revision: "skill-enabled-catalog-sha256-v1:test".to_string(),
        prompt_token_budget: crate::skills::DEFAULT_SKILL_DISCOVERY_PROMPT_TOKENS,
        skills: vec![crate::skills::AgentDiscoverableSkill {
            activation_ref: crate::skills::derive_skill_activation_ref(
                "skill-enabled-catalog-sha256-v1:test",
                "bundled:application:documents",
                "skill-package-sha256-v1:documents",
            ),
            id: "bundled:application:documents".to_string(),
            revision: "skill-package-sha256-v1:documents".to_string(),
            name: "documents".to_string(),
            description: "Create documents.".to_string(),
            source_kind: "bundled".to_string(),
        }],
        max_activated_skills: 8,
        max_total_source_bytes: 512 * 1024,
    };
    let activation = AgentSkillActivation {
        activation_revision: "activation-sha256-v1:documents".to_string(),
        skills: vec![AgentActivatedSkill {
            id: "bundled:application:documents".to_string(),
            name: "documents".to_string(),
            revision: "skill-package-sha256-v1:documents".to_string(),
            source: "bundled:application".to_string(),
            instructions: "FULL_DOCUMENT_SKILL_INSTRUCTIONS".to_string(),
            source_bytes: 32,
            resources: None,
        }],
    };
    let activation_ref = discovery.skills[0].activation_ref.clone();
    let frame = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".to_string(),
        compaction_summary: None,
        world_state_records: Vec::new(),
        initial_run_world_state: None,
        messages: vec![message("user", "current question")],
        skill_discovery: Some(discovery),
        skill_activation: Some(activation),
        attachments: ContextAttachments {
            text: "ATTACHMENT_BEFORE_SKILLS".to_string(),
            images: Vec::new(),
        },
    })
    .unwrap();

    let messages = frame.to_messages();
    assert_eq!(messages[1].content(), "current question");
    assert_eq!(messages[2].content(), "ATTACHMENT_BEFORE_SKILLS");
    assert!(messages[3].content().contains("backend_available_skills"));
    assert!(messages[3]
        .content()
        .contains(&format!("\"ref\":\"{activation_ref}\"")));
    assert!(!messages[3]
        .content()
        .contains("bundled:application:documents"));
    assert!(!messages[3]
        .content()
        .contains("skill-package-sha256-v1:documents"));
    assert!(messages[4]
        .content()
        .contains("FULL_DOCUMENT_SKILL_INSTRUCTIONS"));
    assert_eq!(
        frame.manifest().entries[2].sources,
        vec!["input_attachment", "run_bootstrap"]
    );
    assert_eq!(
        frame.manifest().entries[3].sources,
        vec!["skill_catalog", "run_bootstrap"]
    );
    assert_eq!(
        frame.manifest().entries[4].sources,
        vec!["skill_instructions", "run_bootstrap"]
    );
}

#[test]
fn renders_timing_on_user_messages_without_decorating_assistant_history() {
    let mut first_user = message("user", "historical question");
    first_user.created_at = Some(0);
    let mut historical_assistant =
        current_assistant_message("assistant-historical", "historical answer");
    historical_assistant.created_at = Some(1_000);
    let mut current_user = message("user", "follow up");
    current_user.created_at = Some(2_000);
    let frame = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".to_string(),
        compaction_summary: None,
        world_state_records: Vec::new(),
        initial_run_world_state: None,
        messages: vec![first_user, historical_assistant, current_user],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap();

    let messages = frame.to_messages();
    assert!(messages[1].content().contains(&format!(
        "user_message_created_at: {}",
        crate::context::format_message_created_at(0).unwrap()
    )));
    assert!(messages[1].content().ends_with("historical question"));
    assert_eq!(messages[2].content(), "historical answer");
    assert!(!messages[2]
        .content()
        .contains("<backend_conversation_timing>"));
    assert!(messages[3].content().contains(&format!(
        "previous_assistant_message_created_at: {}",
        crate::context::format_message_created_at(1_000).unwrap()
    )));
    assert!(messages[3].content().contains(&format!(
        "user_message_created_at: {}",
        crate::context::format_message_created_at(2_000).unwrap()
    )));
    assert!(messages[3].content().ends_with("follow up"));
}

#[test]
fn normalizes_supported_messages_and_rejects_unknown_roles() {
    let frame = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".to_string(),
        compaction_summary: None,
        world_state_records: Vec::new(),
        initial_run_world_state: None,
        messages: vec![
            message(" user ", " hello "),
            message("user", " "),
            message("system", "history rules"),
        ],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap();
    let messages = frame.to_messages();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1].content(), "hello");
    assert_eq!(messages[2].content(), "history rules");

    let missing_trace = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".to_string(),
        compaction_summary: None,
        world_state_records: Vec::new(),
        initial_run_world_state: None,
        messages: vec![message(
            "assistant",
            "ASSISTANT_HISTORY_CANARY_MUST_NOT_ENTER_ERROR",
        )],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap_err();
    assert!(missing_trace
        .to_string()
        .contains("Assistant 历史消息缺少当前 ConversationTurnTrace"));
    assert!(!missing_trace.to_string().contains("CANARY"));

    let error = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".to_string(),
        compaction_summary: None,
        world_state_records: Vec::new(),
        initial_run_world_state: None,
        messages: vec![message("tool", "result")],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap_err();
    assert!(error.to_string().contains("不支持的消息角色"));
}

#[test]
fn assembles_conversation_trace_before_final_reply_and_terminal_before_next_user() {
    let frame = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".to_string(),
        compaction_summary: None,
        world_state_records: Vec::new(),
        initial_run_world_state: None,
        messages: vec![
            message("user", "create a file"),
            traced_assistant("Created src/new.rs."),
            message("user", "what changed?"),
        ],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap();

    frame.validate_complete_tool_protocol().unwrap();
    let messages = frame.to_messages();
    // The file content is redacted from the trace, so the truncated run keeps its terminal record.
    assert_eq!(messages.len(), 8);
    assert_eq!(messages[1].content(), "create a file");
    assert_eq!(messages[2].content(), "I will update the file.");
    assert_eq!(messages[3].role(), LlmMessageRole::Assistant);
    let file_change_call = messages[3].tool_calls().next().unwrap();
    assert_eq!(file_change_call.name, "apply_patch");
    assert_eq!(file_change_call.args["request"]["filePath"], "src/new.rs");
    assert_eq!(messages[4].role(), LlmMessageRole::Tool);
    assert_eq!(
        messages[4].tool_call_id(),
        Some(file_change_call.id.as_str())
    );
    assert_eq!(messages[5].content(), "Created src/new.rs.");
    assert_eq!(messages[6].role(), LlmMessageRole::System);
    assert_eq!(
        messages[6].placement(),
        crate::llm::LlmMessagePlacement::BackendStateTimeline
    );
    let record: serde_json::Value = serde_json::from_str(messages[6].content()).unwrap();
    assert_eq!(record["recordType"], "historical_agent_activity_terminal");
    assert_eq!(record["terminalStatus"], "completed");
    assert_eq!(record["traceTruncated"], true);
    assert_eq!(messages[7].role(), LlmMessageRole::User);
    assert_eq!(messages[7].content(), "what changed?");

    let manifest = frame.manifest();
    for index in [2, 3, 4] {
        assert_eq!(manifest.entries[index].sources, vec!["conversation_trace"]);
        assert_eq!(manifest.entries[index].scope, "conversation");
        assert_eq!(manifest.entries[index].retention, "retained");
    }
    assert_eq!(manifest.entries[5].sources, vec!["conversation_history"]);
    assert_eq!(
        manifest.entries[6].sources,
        vec!["conversation_trace", "backend_state"]
    );
    assert_eq!(manifest.entries[7].sources, vec!["current_turn"]);

    let checkpoint = frame.checkpoint_items().unwrap();
    let restored = ContextFrame::from_checkpoint_items(checkpoint).unwrap();
    restored.validate_complete_tool_protocol().unwrap();
    assert_eq!(
        serde_json::to_value(frame.manifest()).unwrap(),
        serde_json::to_value(restored.manifest()).unwrap()
    );
}

#[test]
fn failed_run_terminal_record_is_backend_state_not_assistant_voice() {
    let mut failed = traced_assistant("Partial answer.");
    let trace = failed.conversation_turn_trace.as_mut().unwrap();
    trace.terminal_status = ConversationTurnTraceTerminalStatus::Failed;
    trace.terminal_error = Some("provider timeout".to_string());
    let frame = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".to_string(),
        compaction_summary: None,
        world_state_records: Vec::new(),
        initial_run_world_state: None,
        messages: vec![
            message("user", "create a file"),
            failed,
            message("user", "retry"),
        ],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap();

    let messages = frame.to_messages();
    assert_eq!(messages.len(), 8);
    assert_eq!(messages[5].content(), "Partial answer.");
    let terminal = &messages[6];
    assert_eq!(terminal.role(), LlmMessageRole::System);
    assert_eq!(
        terminal.placement(),
        crate::llm::LlmMessagePlacement::BackendStateTimeline
    );
    let record: serde_json::Value = serde_json::from_str(terminal.content()).unwrap();
    assert_eq!(record["recordType"], "historical_agent_activity_terminal");
    assert_eq!(record["terminalStatus"], "failed");
    assert_eq!(record["terminalError"], "provider timeout");
    assert!(!messages
        .iter()
        .any(|message| message.role() == LlmMessageRole::Assistant
            && message
                .content()
                .contains("historical_agent_activity_terminal")));
    assert_eq!(
        frame.manifest().entries[6].sources,
        vec!["conversation_trace", "backend_state"]
    );

    let restored = ContextFrame::from_checkpoint_items(frame.checkpoint_items().unwrap()).unwrap();
    assert_eq!(
        restored.to_messages()[6].placement(),
        crate::llm::LlmMessagePlacement::BackendStateTimeline
    );
}

#[test]
fn keeps_trace_when_historical_assistant_final_text_is_empty() {
    let mut historical_assistant = traced_assistant("");
    historical_assistant.created_at = Some(0);
    let frame = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".to_string(),
        compaction_summary: None,
        world_state_records: Vec::new(),
        initial_run_world_state: None,
        messages: vec![
            message("user", "do the work"),
            historical_assistant,
            message("user", "continue"),
        ],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap();

    let messages = frame.to_messages();
    assert!(messages
        .iter()
        .any(|message| message.content() == "I will update the file."));
    assert!(messages.iter().any(|message| message
        .content()
        .contains("historical_agent_activity_terminal")));
    assert!(!messages.iter().any(|message| message.content().is_empty()
        && message.role() == LlmMessageRole::Assistant
        && message.tool_calls().is_empty()));
    assert!(!messages
        .iter()
        .any(|message| message.content().starts_with("[Message created at:")));
}

#[test]
fn assembles_compaction_summary_before_uncovered_tail() {
    let frame = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".to_string(),
        compaction_summary: Some(compaction_summary()),
        world_state_records: Vec::new(),
        initial_run_world_state: None,
        messages: vec![message("user", "continue from the summary")],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap();

    let messages = frame.to_messages();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].role(), LlmMessageRole::System);
    assert_eq!(messages[1].role(), LlmMessageRole::System);
    assert_eq!(
        messages[1].placement(),
        LlmMessagePlacement::BackendStateTimeline
    );
    assert!(messages[1].content().contains("old task"));
    assert!(messages[1].content().contains("较早对话的有损语义摘要"));
    assert!(messages[1].content().contains("当前用户消息在冲突时优先"));
    assert!(messages[1].content().contains("conversation_history"));
    assert_eq!(messages[2].content(), "continue from the summary");
    assert!(!messages.iter().any(|message| message
        .content()
        .contains("BEGIN_UNTRUSTED_CONTINUITY_RECORDS_JSON")));
    assert_eq!(
        frame.manifest().entries[1].sources,
        vec!["conversation_summary"]
    );
    assert_eq!(frame.manifest().entries[1].role, "system");
    assert_eq!(
        frame.manifest().entries[1].origin_kind,
        Some("compaction_summary")
    );
    assert_eq!(frame.manifest().entries[1].origin_id, Some("summary-1"));
    let persisted = compaction_summary();
    persisted.continuity.validate().unwrap();
    assert!(!persisted.continuity.archived_counts.is_empty());
}

#[test]
fn compacted_uncovered_tail_recovers_running_command_receipt_from_durable_trace() {
    let call_id = model_response_tool_call_id("run-command", 0, 0, "provider-command-call");
    let call = AgentToolCall {
        id: call_id.clone(),
        tool: "run_command".to_string(),
        args: json!({ "command": "python3 server.py" }),
        approval_status: AgentApprovalStatus::Approved,
        reason: None,
    };
    let result = AgentToolResult {
        exact_archive_file: None,
        call_id: call_id.clone(),
        tool: "run_command".to_string(),
        ok: true,
        result: Some(json!({
            "status": "running",
            "sessionId": "cmd_0123456789abcdef0123456789abcdef",
            "output": "server listening on port 3000",
            "startedAt": 1_725_000_000_000_i64,
            "latestSequence": 3,
            "outputTruncated": false,
        })),
        error: None,
    };
    let mut recorder = ConversationTraceRecorder::default();
    let call_sequence = recorder
        .record_tool_call_with_identity(
            &call,
            crate::AgentToolIdentity::Builtin {
                tool_name: call.tool.clone(),
            },
        )
        .unwrap();
    recorder
        .record_model_tool_call_message(
            call_sequence,
            0,
            &LlmMessage::assistant(
                "",
                vec![LlmToolCall {
                    id: call.id.clone(),
                    name: call.tool.clone(),
                    args: call.args.clone(),
                }],
            ),
            crate::AgentProviderToolCallIdentity {
                provider_call_id: "provider-command-call".to_string(),
                provider_tool_index: 0,
                runtime_call_id: call.id.clone(),
            },
        )
        .unwrap();
    let result_sequence = recorder.record_tool_result(&call, &result).unwrap();
    recorder
        .record_model_message(
            result_sequence,
            0,
            &LlmMessage::tool_result(
                call.id.clone(),
                serde_json::to_string(result.result.as_ref().unwrap()).unwrap(),
                false,
            ),
        )
        .unwrap();
    let snapshot = recorder.snapshot();
    let trace = recorder.finish(
        "run-command",
        "conversation-1",
        "assistant-command",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    let historical_assistant = AgentChatMessage {
        conversation_completion_covered: false,
        message_id: Some("assistant-command".to_string()),
        role: "assistant".to_string(),
        content: "The server is running in a managed Session.".to_string(),
        created_at: None,
        conversation_turn_trace: Some(trace),
        conversation_model_context_items: snapshot.model_context_items,
    };

    let frame = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".to_string(),
        compaction_summary: Some(compaction_summary()),
        world_state_records: Vec::new(),
        initial_run_world_state: None,
        messages: vec![
            message("user", "start the server"),
            historical_assistant,
            message("user", "check whether it is still healthy"),
        ],
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap();

    frame.validate_complete_tool_protocol().unwrap();
    let tool_result = frame
        .to_messages()
        .into_iter()
        .find(|message| message.role() == LlmMessageRole::Tool)
        .expect("the durable run_command result must be reconstructed after reload");
    let observation: serde_json::Value = serde_json::from_str(tool_result.content()).unwrap();
    assert_eq!(observation["status"], "running");
    assert_eq!(
        observation["sessionId"],
        "cmd_0123456789abcdef0123456789abcdef"
    );
    assert_eq!(observation["output"], "server listening on port 3000");
    assert_eq!(observation["startedAt"], 1_725_000_000_000_i64);
    assert_eq!(observation["latestSequence"], 3);
    assert_eq!(observation["outputTruncated"], false);
}

#[test]
fn summary_only_context_is_valid_after_covering_the_latest_completed_turn() {
    let frame = ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: "rules".to_string(),
        compaction_summary: Some(compaction_summary()),
        world_state_records: Vec::new(),
        initial_run_world_state: None,
        messages: Vec::new(),
        skill_discovery: None,
        skill_activation: None,
        attachments: ContextAttachments::default(),
    })
    .unwrap();

    assert_eq!(frame.to_messages().len(), 2);
}

#[test]
fn backend_state_after_a_covered_message_does_not_replay_its_terminal_record() {
    let content =
        json!({"type":"human_interaction_status","requestId":"request-1","status":"ignored"})
            .to_string();
    let mut recorder = ConversationTraceRecorder::default();
    recorder
        .record_backend_state(
            0,
            "ignored:request-1",
            &content,
            20,
            crate::ConversationBackendStatePlacement::AfterMessage,
        )
        .unwrap();
    let snapshot = recorder.snapshot();
    let mut trace = snapshot.in_progress_audit_trace("run-1", "conversation-1", "assistant-old");
    trace.terminal_status = ConversationTurnTraceTerminalStatus::Completed;
    for summary in [None, Some(compaction_summary())] {
        let covered = summary.is_some();
        let frame = ContextAssembler::assemble(ContextAssemblyInput {
            system_prompt: "rules".to_string(),
            compaction_summary: summary,
            world_state_records: Vec::new(),
            initial_run_world_state: None,
            messages: vec![AgentChatMessage {
                message_id: Some("assistant-old".to_string()),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: Some(10),
                conversation_turn_trace: Some(trace.clone()),
                conversation_model_context_items: snapshot.model_context_items.clone(),
                conversation_completion_covered: covered,
            }],
            skill_discovery: None,
            skill_activation: None,
            attachments: ContextAttachments::default(),
        })
        .unwrap();
        let messages = frame.to_messages();
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.content() == content)
                .count(),
            1
        );
        assert_eq!(
            messages
                .iter()
                .filter(|message| message
                    .content()
                    .contains("historical_agent_activity_terminal"))
                .count(),
            usize::from(!covered)
        );
    }
}
