// Core-server Agent lifecycle regression tests for graph-aware project and conversation deletion.

use super::*;
use std::sync::Arc;

fn save_graph_bound_conversation(
    storage: &StorageService,
    project_id: &str,
    conversation_id: &str,
    agent_id: &str,
) {
    storage
        .save_project(ProjectRecord {
            id: project_id.to_string(),
            name: project_id.to_string(),
            path: None,
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: Some(project_id.to_string()),
            model_id: Some("model-1".to_string()),
            title: conversation_id.to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .ensure_root_agent(&mycopilot_core::EnsureRootAgentInput {
            agent_id: agent_id.to_string(),
            conversation_id: conversation_id.to_string(),
            creation_request_id: format!("ensure-{agent_id}"),
            task_name: "Root".to_string(),
        })
        .unwrap();
}

#[test]
fn agent_service_project_deletion_reaches_tree_aware_storage() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_graph_bound_conversation(
        &storage,
        "project-agent-tree",
        "conversation-agent-tree",
        "agent-tree-root",
    );
    let service = AgentService::new(Arc::clone(&storage));

    service.delete_project("project-agent-tree").unwrap();

    assert!(storage
        .load_projects()
        .unwrap()
        .iter()
        .all(|project| project.id != "project-agent-tree"));
    assert!(storage
        .load_conversation("conversation-agent-tree")
        .unwrap()
        .is_none());
    assert!(storage.get_agent_node("agent-tree-root").unwrap().is_none());
}

#[test]
fn agent_service_root_conversation_deletion_reaches_tree_aware_storage() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_graph_bound_conversation(
        &storage,
        "project-retained",
        "conversation-agent-root",
        "agent-conversation-root",
    );
    let service = AgentService::new(Arc::clone(&storage));

    service
        .delete_conversation("conversation-agent-root")
        .unwrap();

    assert!(storage
        .load_projects()
        .unwrap()
        .iter()
        .any(|project| project.id == "project-retained"));
    assert!(storage
        .load_conversation("conversation-agent-root")
        .unwrap()
        .is_none());
    assert!(storage
        .get_agent_node("agent-conversation-root")
        .unwrap()
        .is_none());
}
