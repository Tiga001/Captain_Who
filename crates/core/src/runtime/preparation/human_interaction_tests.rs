use super::*;
use crate::human_interaction::HumanInteractionSettings;
use crate::tools::human_interaction::HUMAN_INTERACTION_TOOL_NAMES;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Policy {
    enabled: bool,
    reads: AtomicUsize,
}

impl HumanInteractionPolicySource for Policy {
    fn snapshot(&self) -> AgentResult<HumanInteractionSettings> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(HumanInteractionSettings {
            enabled: self.enabled,
            revision: 1,
            updated_at: 1,
        })
    }
}

struct NoReports;
impl crate::AutomationReportSink for NoReports {
    fn record(&self, _: crate::AutomationReportKind, _: &str) -> Result<(), String> {
        panic!("capability preparation must never execute an automation report")
    }
}

fn root_input() -> AgentChatInput {
    let mut input: AgentChatInput = serde_json::from_value(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "unused",
        "model": "model-1",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.context = Some(AgentRunContext {
        conversation_id: Some("conversation-root".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    });
    input
}

fn policy(enabled: bool) -> Arc<Policy> {
    Arc::new(Policy {
        enabled,
        reads: AtomicUsize::new(0),
    })
}

fn services(policy: Option<Arc<Policy>>) -> RuntimeCapabilityServices {
    RuntimeCapabilityServices {
        web_search_policy: None,
        host_actions_available: false,
        office_engine: None,
        image_generation_execution: None,
        skill_installation_prepare: None,
        skill_installation_commit: None,
        skill_activation_resolver: None,
        skill_resources: None,
        mcp_tools: None,
        builtin_capabilities: None,
        agent_collaboration: None,
        agent_collaboration_policy: None,
        automation_report_sink: None,
        human_interaction_policy: policy
            .map(|policy| policy as Arc<dyn HumanInteractionPolicySource>),
        human_interaction_execution_ready: false,
        human_interaction_async_execution_ready: false,
    }
}

fn assert_no_human_tools(capabilities: &PreparedRuntimeCapabilities) {
    for name in HUMAN_INTERACTION_TOOL_NAMES {
        assert!(!capabilities.tool_registry.contains_tool(name));
        assert!(!capabilities.initial_tool_set.contains(name));
        assert!(capabilities
            .tool_definitions
            .iter()
            .all(|tool| tool.name != name));
    }
}

fn assert_inapplicable_human_state(capabilities: &PreparedRuntimeCapabilities) {
    let sections = capabilities
        .runtime_extensions
        .conversation_world_state_sections()
        .unwrap();
    let section = sections
        .iter()
        .find(|section| section.id.as_str() == "human.interaction")
        .unwrap();
    assert_eq!(
        section.lifetime,
        crate::world_state::WorldStateLifetime::Conversation
    );
    assert_eq!(
        section.model_projection,
        Some(json!({
            "available":false, "reason":"identity_not_applicable"
        }))
    );
    assert!(capabilities
        .runtime_extensions
        .world_state_sections()
        .unwrap()
        .iter()
        .all(|section| section.id.as_str() != "human.interaction"));
}

fn has_human_shell(capabilities: &PreparedRuntimeCapabilities) -> bool {
    capabilities
        .runtime_extensions
        .snapshots()
        .unwrap()
        .iter()
        .any(|snapshot| snapshot.extension_id == "human.interaction")
}

#[test]
fn human_interaction_root_shell_does_not_change_stable_prefix_or_enable_unready_tools() {
    let input = root_input();
    let baseline =
        prepare_runtime_capabilities_with_skills(&input, "run-root", &[], services(None)).unwrap();
    let unavailable = baseline
        .runtime_extensions
        .conversation_world_state_sections()
        .unwrap();
    let human = unavailable
        .iter()
        .find(|section| section.id.as_str() == "human.interaction")
        .unwrap();
    assert_eq!(
        human.model_projection,
        Some(json!({"available":false,"reason":"host_unavailable"}))
    );
    for enabled in [false, true] {
        let source = policy(enabled);
        let capabilities = prepare_runtime_capabilities_with_skills(
            &input,
            "run-root",
            &[],
            services(Some(source.clone())),
        )
        .unwrap();
        assert!(has_human_shell(&capabilities));
        assert_eq!(source.reads.load(Ordering::SeqCst), 1);
        assert_no_human_tools(&capabilities);
        assert_eq!(
            capabilities.initial_tool_set.stable_revision(),
            baseline.initial_tool_set.stable_revision()
        );
        assert_eq!(
            capabilities.initial_tool_set.revision(),
            baseline.initial_tool_set.revision()
        );
    }
}

#[test]
fn human_interaction_root_restores_shell_when_disabled_or_policy_service_is_missing() {
    let input = root_input();
    let original = prepare_runtime_capabilities_with_skills(
        &input,
        "run-root",
        &[],
        services(Some(policy(true))),
    )
    .unwrap();
    let snapshots = original.runtime_extensions.snapshots().unwrap();
    for source in [Some(policy(false)), None] {
        let restored = prepare_runtime_capabilities_with_skills(
            &input,
            "run-root",
            &snapshots,
            services(source),
        )
        .unwrap();
        assert!(has_human_shell(&restored));
        assert_no_human_tools(&restored);
        assert_eq!(restored.runtime_extensions.snapshots().unwrap(), snapshots);
    }
}

fn child_input() -> AgentChatInput {
    let mut input = root_input();
    let context = input.context.as_mut().unwrap();
    context.conversation_id = Some("conversation-child".to_string());
    context.collaboration_identity = Some(crate::AgentCollaborationIdentity {
        agent_id: "agent-child".to_string(),
        root_agent_id: "agent-root".to_string(),
        root_conversation_id: "conversation-root".to_string(),
        parent_agent_id: "agent-parent".to_string(),
        parent_task_name: "parent".to_string(),
        parent_task_path: "/root/parent".to_string(),
        conversation_id: "conversation-child".to_string(),
        task_name: "child".to_string(),
        task_path: "/root/parent/child".to_string(),
        source_agent_id: "agent-parent".to_string(),
        source_kind: crate::AgentMailboxKind::Task,
        source_task_name: "parent".to_string(),
        source_task_path: "/root/parent".to_string(),
        source_agent_message_id: "mailbox-task".to_string(),
        entrusted_task: "bounded child task".to_string(),
        template_instructions: None,
    });
    input
}

#[test]
fn human_interaction_child_never_mounts_or_reads_accidentally_supplied_root_policy() {
    let source = policy(true);
    let mut ready_services = services(Some(source.clone()));
    ready_services.human_interaction_execution_ready = true;
    ready_services.human_interaction_async_execution_ready = true;
    let capabilities =
        prepare_runtime_capabilities_with_skills(&child_input(), "run-child", &[], ready_services)
            .unwrap();
    assert_no_human_tools(&capabilities);
    assert!(!has_human_shell(&capabilities));
    assert_inapplicable_human_state(&capabilities);
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    let root = prepare_runtime_capabilities_with_skills(
        &root_input(),
        "run-root",
        &[],
        services(Some(source)),
    )
    .unwrap();
    assert!(prepare_runtime_capabilities_with_skills(
        &child_input(),
        "run-child",
        &root.runtime_extensions.snapshots().unwrap(),
        services(None)
    )
    .is_err());
}

#[test]
fn human_interaction_automation_metadata_or_report_sink_each_exclude_the_module() {
    for through_sink in [false, true] {
        let source = policy(true);
        let mut input = root_input();
        let mut host = services(Some(source.clone()));
        host.human_interaction_execution_ready = true;
        host.human_interaction_async_execution_ready = true;
        if through_sink {
            host.automation_report_sink = Some(Arc::new(NoReports));
        } else {
            let mut preferences: AgentPromptPreferences =
                serde_json::from_value(json!({})).unwrap();
            preferences.automation_execution_context = Some(
                AgentAutomationExecutionContext::new(
                    "automation-1",
                    "automation-run-1",
                    1,
                    None,
                    "manual",
                )
                .unwrap(),
            );
            input.prompt_preferences = Some(preferences);
        }
        let capabilities =
            prepare_runtime_capabilities_with_skills(&input, "run-auto", &[], host).unwrap();
        assert_no_human_tools(&capabilities);
        assert!(!has_human_shell(&capabilities));
        assert_inapplicable_human_state(&capabilities);
        assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn human_interaction_unowned_inputs_never_receive_human_capability() {
    let mut input = root_input();
    input.context = None;
    let source = policy(true);
    let capabilities = prepare_runtime_capabilities_with_skills(
        &input,
        "unowned",
        &[],
        services(Some(source.clone())),
    )
    .unwrap();
    assert_no_human_tools(&capabilities);
    assert!(!has_human_shell(&capabilities));
    assert_inapplicable_human_state(&capabilities);
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
}

#[test]
fn human_interaction_policy_read_failure_does_not_prevent_ordinary_root_preparation() {
    struct Unavailable;
    impl HumanInteractionPolicySource for Unavailable {
        fn snapshot(&self) -> AgentResult<HumanInteractionSettings> {
            Err(AgentError::new("settings temporarily unavailable"))
        }
    }
    let mut host = services(None);
    host.human_interaction_policy = Some(Arc::new(Unavailable));
    let mut capabilities =
        prepare_runtime_capabilities_with_skills(&root_input(), "run-root", &[], host).unwrap();
    assert!(has_human_shell(&capabilities));
    assert_no_human_tools(&capabilities);
    capabilities
        .runtime_extensions
        .prepare_model_request()
        .unwrap();
    assert_no_human_tools(&capabilities);
}

#[test]
fn human_interaction_ready_root_exposes_only_sync_and_keeps_stable_prefix() {
    let baseline =
        prepare_runtime_capabilities_with_skills(&root_input(), "root", &[], services(None))
            .unwrap();
    let mut host = services(Some(policy(true)));
    host.human_interaction_execution_ready = true;
    let capabilities =
        prepare_runtime_capabilities_with_skills(&root_input(), "root", &[], host).unwrap();
    assert!(capabilities.initial_tool_set.contains("request_user_input"));
    assert!(!capabilities
        .initial_tool_set
        .contains("request_user_input_async"));
    assert_eq!(
        baseline.initial_tool_set.stable_revision(),
        capabilities.initial_tool_set.stable_revision()
    );
    for input in [child_input(), {
        let mut input = root_input();
        input.context = None;
        input
    }] {
        let source = policy(true);
        let mut host = services(Some(source.clone()));
        host.human_interaction_execution_ready = true;
        let capabilities =
            prepare_runtime_capabilities_with_skills(&input, "descendant", &[], host).unwrap();
        assert_no_human_tools(&capabilities);
        assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn human_interaction_ready_automation_remains_unmounted() {
    let source = policy(true);
    let mut host = services(Some(source.clone()));
    host.human_interaction_execution_ready = true;
    host.human_interaction_async_execution_ready = true;
    host.automation_report_sink = Some(Arc::new(NoReports));
    let capabilities =
        prepare_runtime_capabilities_with_skills(&root_input(), "automation", &[], host).unwrap();
    assert_no_human_tools(&capabilities);
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
}

#[test]
fn human_interaction_both_ready_share_policy_and_do_not_change_stable_prefix() {
    let baseline =
        prepare_runtime_capabilities_with_skills(&root_input(), "root", &[], services(None))
            .unwrap();
    let mut host = services(Some(policy(true)));
    host.human_interaction_execution_ready = true;
    host.human_interaction_async_execution_ready = true;
    let ready = prepare_runtime_capabilities_with_skills(&root_input(), "root", &[], host).unwrap();
    for name in HUMAN_INTERACTION_TOOL_NAMES {
        assert!(ready.initial_tool_set.contains(name));
        assert!(ready.tool_registry.contains_tool(name));
    }
    assert_eq!(
        baseline.initial_tool_set.stable_revision(),
        ready.initial_tool_set.stable_revision()
    );
    for task_path in [
        "/root/child",
        "/root/child/grandchild",
        "/root/child/grandchild/greatgrandchild",
    ] {
        let mut child = child_input();
        child
            .context
            .as_mut()
            .unwrap()
            .collaboration_identity
            .as_mut()
            .unwrap()
            .task_path = task_path.into();
        let source = policy(true);
        let mut host = services(Some(source.clone()));
        host.human_interaction_execution_ready = true;
        host.human_interaction_async_execution_ready = true;
        let child = prepare_runtime_capabilities_with_skills(&child, "child", &[], host).unwrap();
        assert_no_human_tools(&child);
        assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    }
}
