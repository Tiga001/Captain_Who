use super::*;
use crate::llm::{LlmMessage, LlmToolCall};
use crate::{AgentApprovalStatus, AgentToolCall, AgentToolResult, ConversationTraceRecorder};

fn state() -> AgentConversationContextState {
    AgentConversationContextState::new(
        "revision".into(),
        "test-model".into(),
        Some(32_000),
        1024,
        ContextCapacityDetector::for_model(
            "test-model",
            crate::AgentApiStyle::OpenAiCompatible,
            &[],
        ),
        ContextFrame::new(Vec::new()),
        ConversationTimingTracker::default(),
    )
}

fn cold(recorder: &ConversationTraceRecorder) -> (AgentConversationContextState, usize) {
    let snapshot = recorder.snapshot().committed_prefix();
    let trace = snapshot.in_progress_audit_trace("run", "conversation", "assistant");
    let mut state = state();
    let count = state
        .append_trace_items(&trace, &snapshot.model_context_items, 0)
        .unwrap();
    (state, count)
}

fn seeded(recorder: &ConversationTraceRecorder) -> (AgentConversationContextState, usize) {
    let (mut state, count) = cold(recorder);
    state.seed_trace_publication_cursor(&recorder.publication(), "assistant", "run", count);
    (state, count)
}

fn call(recorder: &mut ConversationTraceRecorder, index: usize, text: &str) -> AgentToolCall {
    let call = AgentToolCall {
        id: format!("tc1_{index:043}"),
        tool: "read_file".into(),
        args: serde_json::json!({"path": "fixture.txt"}),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let sequence = recorder.record_tool_call(&call).unwrap();
    recorder
        .record_model_message(
            sequence,
            0,
            &LlmMessage::assistant(
                text,
                vec![LlmToolCall {
                    id: call.id.clone(),
                    name: call.tool.clone(),
                    args: call.args.clone(),
                }],
            ),
        )
        .unwrap();
    call
}

fn result(recorder: &mut ConversationTraceRecorder, call: &AgentToolCall) {
    let sequence = recorder
        .record_tool_result(
            call,
            &AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: true,
                result: Some(serde_json::json!({"content": "exact result"})),
                error: None,
                exact_archive_file: None,
            },
        )
        .unwrap();
    recorder
        .record_model_message(
            sequence,
            0,
            &LlmMessage::tool_result(&call.id, "exact result", false),
        )
        .unwrap();
}

#[test]
fn incremental_publication_frame_matches_cold_rebuild_at_every_closed_boundary() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_narration("initial").unwrap();
    let (mut incremental, mut count) = seeded(&recorder);
    for index in 0..100 {
        let tool = call(&mut recorder, index, "");
        let open = recorder.publication();
        assert_eq!(
            incremental
                .append_trace_publication(&open, "assistant", "run", count)
                .unwrap(),
            Some(count)
        );
        let before = incremental.frame.to_messages();
        result(&mut recorder, &tool);
        let closed = recorder.publication();
        count = incremental
            .append_trace_publication(&closed, "assistant", "run", count)
            .unwrap()
            .unwrap();
        assert_eq!(count, 1 + (index + 1) * 2);
        assert_eq!(
            &incremental.frame.to_messages()[..before.len()],
            before.as_slice()
        );
        let (mut rebuilt, rebuilt_count) = cold(&recorder);
        assert_eq!(count, rebuilt_count);
        assert_eq!(incremental.frame.to_messages(), rebuilt.frame.to_messages());
        assert_eq!(incremental.snapshot(), rebuilt.snapshot());
        assert_eq!(
            incremental
                .append_trace_publication(&closed, "assistant", "run", count)
                .unwrap(),
            Some(count)
        );
    }
}

#[test]
fn publication_wrong_identity_run_and_reset_cursor_require_cold_rebuild() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_narration("first").unwrap();
    let (mut state, count) = seeded(&recorder);
    recorder.record_narration("second").unwrap();
    let next = recorder.publication();
    let before = state.frame.to_messages();
    assert_eq!(
        state
            .append_trace_publication(&next, "other-assistant", "run", count)
            .unwrap(),
        None
    );
    let independent = recorder.snapshot().into();
    assert_eq!(
        state
            .append_trace_publication(&independent, "assistant", "run", count)
            .unwrap(),
        None
    );
    assert_eq!(state.frame.to_messages(), before);
    // A compaction/rebuild does not inherit the discarded frame's publication cursor.
    state.trace_publication_cursor = None;
    assert_eq!(
        state
            .append_trace_publication(&next, "assistant", "run", count)
            .unwrap(),
        None
    );
    assert_eq!(state.frame.to_messages(), before);
}

#[test]
fn publication_narration_absorption_rebuilds_instead_of_skipping_a_tool_call() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder
        .record_tool_turn_narration("inspecting", "provider-turn", &format!("tc1_{:043}", 0))
        .unwrap();
    let (mut state, count) = seeded(&recorder);
    let tool = call(&mut recorder, 0, "inspecting");
    let open = recorder.publication();
    assert_eq!(
        state
            .append_trace_publication(&open, "assistant", "run", count)
            .unwrap(),
        Some(count)
    );
    result(&mut recorder, &tool);
    let closed = recorder.publication();
    assert_eq!(
        state
            .append_trace_publication(&closed, "assistant", "run", count)
            .unwrap(),
        None
    );
    let (rebuilt, count) = cold(&recorder);
    assert_eq!(count, 2);
    assert_eq!(rebuilt.frame.to_messages().len(), 2);
    assert_eq!(rebuilt.frame.to_messages()[0].tool_calls().len(), 1);
}

#[test]
fn generic_narration_absorption_replays_only_the_tail_without_cold_rebuild() {
    let mut recorder = ConversationTraceRecorder::default();
    let (mut incremental, mut count) = seeded(&recorder);
    for index in 0..100 {
        recorder
            .record_tool_turn_narration(
                "inspecting",
                &format!("turn-{index}"),
                &format!("tc1_{index:043}"),
            )
            .unwrap();
        let narration = recorder.publication();
        count = incremental
            .append_trace_publication(&narration, "assistant", "run", count)
            .unwrap()
            .unwrap();
        assert_eq!(count, index * 2 + 1);
        incremental.shared_baseline().unwrap();
        let tool = call(&mut recorder, index, "inspecting");
        let open = recorder.publication();
        assert_eq!(
            incremental
                .append_trace_publication(&open, "assistant", "run", count)
                .unwrap(),
            Some(count)
        );
        result(&mut recorder, &tool);
        let closed = recorder.publication();
        count = incremental
            .append_trace_publication(&closed, "assistant", "run", count)
            .unwrap()
            .unwrap();
        assert_eq!(count, (index + 1) * 2);
        let (mut rebuilt, rebuilt_count) = cold(&recorder);
        assert_eq!(count, rebuilt_count);
        assert_eq!(incremental.frame.to_messages(), rebuilt.frame.to_messages());
        assert_eq!(incremental.snapshot(), rebuilt.snapshot());
        incremental.shared_baseline().unwrap();
    }
}

#[test]
fn narration_rewind_never_discards_a_separately_appended_user_message() {
    let mut recorder = ConversationTraceRecorder::default();
    let (mut incremental, count) = seeded(&recorder);
    recorder
        .record_tool_turn_narration("inspecting", "turn", &format!("tc1_{:043}", 0))
        .unwrap();
    let narration = recorder.publication();
    let count = incremental
        .append_trace_publication(&narration, "assistant", "run", count)
        .unwrap()
        .unwrap();
    incremental
        .append_user_message(Some("user-guidance"), "new authoritative user input", None)
        .unwrap();
    let tool = call(&mut recorder, 0, "inspecting");
    result(&mut recorder, &tool);
    let before = incremental.frame.to_messages();
    assert_eq!(
        incremental
            .append_trace_publication(&recorder.publication(), "assistant", "run", count)
            .unwrap(),
        None
    );
    assert_eq!(incremental.frame.to_messages(), before);
}

#[test]
fn narration_rewind_preserves_world_state_observation_metadata() {
    let mut recorder = ConversationTraceRecorder::default();
    let (mut incremental, count) = seeded(&recorder);
    let snapshot = crate::WorldStateSnapshot::new("epoch", 0, Vec::new()).unwrap();
    let mut record = crate::AnchoredWorldStateRecord::new(
        crate::world_state::WorldStateRecord::Full(snapshot),
        None,
    )
    .unwrap();
    record.model_observed = false;
    incremental
        .sync_conversation_world_state_records(&[record])
        .unwrap();
    recorder
        .record_tool_turn_narration("inspecting", "turn", &format!("tc1_{:043}", 0))
        .unwrap();
    let count = incremental
        .append_trace_publication(&recorder.publication(), "assistant", "run", count)
        .unwrap()
        .unwrap();
    incremental.mark_conversation_world_state_observed();
    let tool = call(&mut recorder, 0, "inspecting");
    result(&mut recorder, &tool);
    let before = incremental.frame.to_messages();
    assert_eq!(
        incremental
            .append_trace_publication(&recorder.publication(), "assistant", "run", count)
            .unwrap(),
        None
    );
    assert_eq!(incremental.frame.to_messages(), before);
    assert!(incremental
        .frame
        .conversation_world_state_records()
        .iter()
        .all(|record| record.model_observed));
}

#[test]
fn publication_host_render_work_scales_with_new_payload_including_generic_narration() {
    use crate::context::trace_renderer::take_model_render_work;
    // Full-reference rendering is deliberately quadratic (and includes quadratic legacy lookups).
    // Keep the 500-turn comparison opt-in; the normal suite retains both 100-turn regressions.
    let step_counts: &[usize] = if std::env::var_os("BENCH_TRACE_HOST_RENDER_WORK").is_some() {
        &[100, 500]
    } else {
        &[100]
    };
    for generic in [false, true] {
        let mut previous_work = None;
        for &steps in step_counts {
            let mut recorder = ConversationTraceRecorder::default();
            let (mut incremental, mut count) = seeded(&recorder);
            let mut hot = (0_usize, 0_usize);
            let mut full = (0_usize, 0_usize);
            let mut compare = |recorder: &ConversationTraceRecorder| {
                let publication = recorder.publication();
                take_model_render_work();
                count = incremental
                    .append_trace_publication(&publication, "assistant", "run", count)
                    .unwrap()
                    .expect("every warm publication uses the suffix path");
                let work = take_model_render_work();
                hot.0 += work.0;
                hot.1 += work.1;
                // Include the actual full renderer, not a calculated quadratic estimate.
                let (rebuilt, full_count) = cold(recorder);
                let work = take_model_render_work();
                full.0 += work.0;
                full.1 += work.1;
                assert_eq!(count, full_count);
                assert_eq!(incremental.frame.to_messages(), rebuilt.frame.to_messages());
                incremental.shared_baseline().unwrap();
            };
            for index in 0..steps {
                if generic {
                    recorder
                        .record_tool_turn_narration(
                            "inspecting",
                            &format!("turn-{index}"),
                            &format!("tc1_{index:043}"),
                        )
                        .unwrap();
                    compare(&recorder);
                }
                let tool = call(
                    &mut recorder,
                    index,
                    if generic { "inspecting" } else { "" },
                );
                compare(&recorder);
                result(&mut recorder, &tool);
                compare(&recorder);
            }
            assert_eq!(hot.0, steps * if generic { 3 } else { 2 });
            assert!(full.0 > hot.0 * steps / 2);
            if let Some((old_rows, old_bytes)) = previous_work {
                assert_eq!(hot.0, old_rows * 5);
                assert_eq!(hot.1, old_bytes * 5);
            }
            previous_work = Some(hot);
            eprintln!("host_render_work generic={generic} steps={steps} suffix_rows={} suffix_content_bytes={} full_rows={} full_content_bytes={}", hot.0, hot.1, full.0, full.1);
        }
    }
}

#[test]
fn closed_publication_without_model_coverage_does_not_mutate_cached_frame() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_narration("initial").unwrap();
    let (mut state, count) = seeded(&recorder);
    let tool = call(&mut recorder, 0, "");
    let open = recorder.publication();
    state
        .append_trace_publication(&open, "assistant", "run", count)
        .unwrap();
    recorder
        .record_tool_result(
            &tool,
            &AgentToolResult {
                call_id: tool.id.clone(),
                tool: tool.tool.clone(),
                ok: true,
                result: Some(serde_json::json!({"content": "missing model row"})),
                error: None,
                exact_archive_file: None,
            },
        )
        .unwrap();
    let before = state.frame.to_messages();
    assert!(state
        .append_trace_publication(&recorder.publication(), "assistant", "run", count)
        .is_err());
    assert_eq!(state.frame.to_messages(), before);
}
