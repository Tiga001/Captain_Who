use super::*;
use mycopilot_protocol_rs::{AgentDetailRequest, AgentTreeRequest};

const ROOT_CONVERSATION: &str = "tree-summary-conversation";
const ROOT_AGENT: &str = "tree-summary-root";

fn fixture() -> (tempfile::TempDir, Arc<StorageService>, AgentService) {
    let directory = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("tree.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: ROOT_CONVERSATION.into(),
            project_id: None,
            model_id: Some("model-1".into()),
            title: "Tree summary".into(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let root = storage
        .ensure_root_agent(&mycopilot_core::EnsureRootAgentInput {
            agent_id: ROOT_AGENT.into(),
            conversation_id: ROOT_CONVERSATION.into(),
            creation_request_id: "tree-summary-root-request".into(),
            task_name: "Root".into(),
        })
        .unwrap();
    assert!(root.record().model_snapshot.is_none());
    let service = AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        Arc::clone(&storage),
        None,
        2,
    )
    .unwrap();
    (directory, storage, service)
}

#[test]
fn tree_and_root_detail_do_not_load_conversation_history_for_model_labels() {
    let (directory, storage, service) = fixture();
    assert!(storage
        .load_conversation(ROOT_CONVERSATION)
        .unwrap()
        .is_some());

    // Make full timeline reads fail after startup, while preserving node/turn metadata. This
    // guards the I/O boundary regardless of history size, rather than timing a tiny fixture.
    let connection = rusqlite::Connection::open(directory.path().join("tree.sqlite")).unwrap();
    connection
        .execute_batch(
            "ALTER TABLE conversation_turn_trace_items RENAME TO unavailable_trace_items",
        )
        .unwrap();
    assert!(storage.load_conversation(ROOT_CONVERSATION).is_err());

    let tree = service
        .get_collaboration_tree(AgentTreeRequest {
            root_conversation_id: ROOT_CONVERSATION.into(),
        })
        .unwrap()
        .tree
        .unwrap();
    assert_eq!(tree.agents.len(), 1);
    let root = &tree.agents[0];
    assert_eq!(root.agent_id, ROOT_AGENT);
    assert_eq!(root.model.as_ref().unwrap().model_config_id, "model-1");
    assert_eq!(root.model.as_ref().unwrap().display_name, "Model 1");

    let detail = service
        .get_collaboration_agent(AgentDetailRequest {
            root_conversation_id: ROOT_CONVERSATION.into(),
            agent_id: ROOT_AGENT.into(),
        })
        .unwrap();
    assert_eq!(detail.summary, *root);
}

#[test]
fn tree_uses_current_root_model_label_and_preserves_child_frozen_label() {
    let (_directory, storage, service) = fixture();
    seed_root_effective_permissions(
        &storage,
        ROOT_AGENT,
        ROOT_CONVERSATION,
        "tree-summary",
        AgentPermissions::default(),
    );
    let child =
        crate::application::agent_collaboration::ChildAgentFactory::new(Arc::clone(&storage))
            .create_child(&mycopilot_core::CreateChildAgentInput {
                parent_agent_id: ROOT_AGENT.into(),
                creation_request_id: "tree-summary-child-request".into(),
                task_name: "worker".into(),
                task: "A child with a frozen model label".into(),
                template_machine_key: None,
                explicit_model_id: None,
                reasoning_effort: None,
                fork_turns: mycopilot_core::AgentForkTurns::None,
            })
            .unwrap();
    let mut settings = test_model_settings();
    settings.models[0].display_name = "Renamed model".into();
    storage.save_model_settings(settings).unwrap();

    let tree = service
        .get_collaboration_tree(AgentTreeRequest {
            root_conversation_id: ROOT_CONVERSATION.into(),
        })
        .unwrap()
        .tree
        .unwrap();
    let root = tree
        .agents
        .iter()
        .find(|node| node.agent_id == ROOT_AGENT)
        .unwrap();
    let child_summary = tree
        .agents
        .iter()
        .find(|node| node.agent_id == child.agent.agent_id)
        .unwrap();
    assert_eq!(root.model.as_ref().unwrap().display_name, "Renamed model");
    assert_eq!(
        child_summary.model.as_ref().unwrap().display_name,
        "Model 1"
    );
    let detail = service
        .get_collaboration_agent(AgentDetailRequest {
            root_conversation_id: ROOT_CONVERSATION.into(),
            agent_id: child.agent.agent_id,
        })
        .unwrap();
    assert_eq!(detail.summary, *child_summary);
}
