//! RPC sequencing scopes come from Core-owned identities. They are not execution permission:
//! handlers still perform their final identity, revision, epoch and lease checks.
use super::*;
use mycopilot_protocol_rs::*;

// Decode only the frozen owner envelope of this exact Action. Unknown fields, including the
// private model checkpoint, are skipped without materializing or cloning their contents.
#[derive(serde::Deserialize)]
struct FrozenRpcInput {
    context: Option<FrozenRpcContext>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrozenRpcContext {
    conversation_id: Option<String>,
    project_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RpcSequencingPolicy {
    Ordered,
    /// Later owner lookups must observe the result before publishing their own resource set.
    OwnershipChange,
    TurnPreparation {
        conversation_id: String,
    },
    /// Issued only for the current Core-owned Preparing Run, never from a DTO hint alone.
    CancelPreparation {
        conversation_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RpcResourceScope {
    Global,
    /// An audited human Turn may create its Conversation. Its unknown owner keeps the full
    /// global fence, while account publication can invalidate its frozen access token and an
    /// exact Core Preparing owner can authorize cancellation of this started handler.
    GlobalTurnPreparation {
        conversation_id: String,
    },
    /// Writer-linearized account grant publication. It must reach queued/preparing Turns so
    /// their frozen account token can be invalidated before final admission.
    ExecutionAccess,
    Overview,
    Resources {
        keys: Vec<String>,
        fairness_root: String,
        policy: RpcSequencingPolicy,
    },
}

impl AgentService {
    pub(crate) fn rpc_resource_scope(
        &self,
        method: &str,
        params: Option<&Value>,
    ) -> Result<RpcResourceScope, String> {
        if method == CORE_SET_EXECUTION_ACCESS_METHOD {
            return Ok(RpcResourceScope::ExecutionAccess);
        }
        // These are consistent overviews, not read-your-own-write barriers. Exact reads below
        // join their conversation's sequence; configuration and unknown operations stay global.
        if matches!(
            method,
            STORAGE_LOAD_CONVERSATION_METAS_METHOD
                | STORAGE_LOAD_RUNNING_CONVERSATION_SUMMARIES_METHOD
                | HUMAN_INTERACTION_GET_ATTENTION_METHOD
                | NOTIFICATION_SUMMARY_METHOD
                | OFFICE_GET_STATUS_METHOD
        ) {
            return Ok(RpcResourceScope::Overview);
        }
        let Some(params) = params else {
            return Ok(RpcResourceScope::Global);
        };
        let text = |key| params.get(key).and_then(Value::as_str);
        if method == STORAGE_DELETE_PROJECT_METHOD {
            let Some(project) = text("projectId") else {
                return Ok(RpcResourceScope::Global);
            };
            if !self
                .storage
                .load_projects()?
                .iter()
                .any(|record| record.id == project)
            {
                return Ok(RpcResourceScope::Global);
            }
            return Ok(RpcResourceScope::Resources {
                keys: vec![format!("project-marker:{project}")],
                fairness_root: format!("project:{project}"),
                policy: RpcSequencingPolicy::OwnershipChange,
            });
        }
        let mut extra_keys = Vec::new();
        let mut policy = RpcSequencingPolicy::Ordered;
        let mut preparing_identity = None;
        let conversation = match method {
            AGENT_APPROVE_ACTION_METHOD
            | AGENT_REJECT_ACTION_METHOD
            | AGENT_CANCEL_ACTION_METHOD => {
                let (Some(run), Some(action)) = (text("runId"), text("actionId")) else {
                    return Ok(RpcResourceScope::Global);
                };
                let action_key = pending_action_storage_id(run, action);
                let Some(record) = self.storage.get_pending_agent_action(&action_key)? else {
                    return Ok(RpcResourceScope::Global);
                };
                if record.run_id != run {
                    return Ok(RpcResourceScope::Global);
                }
                let frozen: FrozenRpcInput = serde_json::from_str(&record.agent_input_json)
                    .map_err(|_| "Pending Action frozen owner is unavailable".to_string())?;
                let Some(context) = frozen.context.filter(|context| {
                    context.conversation_id.is_some()
                        && context.conversation_id == record.conversation_id
                }) else {
                    return Ok(RpcResourceScope::Global);
                };
                if let Some(project) = context.project_id {
                    extra_keys.push(format!("project-member:{project}"));
                }
                extra_keys.extend([format!("run:{run}"), format!("action:{action_key}")]);
                record.conversation_id
            }
            AGENT_CANCEL_RUN_METHOD | AGENT_STEER_RUN_METHOD => {
                let Some(run) = (if method == AGENT_STEER_RUN_METHOD {
                    text("expectedRunId")
                } else {
                    text("runId")
                }) else {
                    return Ok(RpcResourceScope::Global);
                };
                // All successful values originate in Core-owned run/usage/action/session state.
                // A missing/retired identity, including a failed durable fallback, stays Global.
                extra_keys.push(format!("run:{run}"));
                let preparing = self.preparing.snapshot_run(run);
                preparing_identity = preparing.clone();
                if method == AGENT_CANCEL_RUN_METHOD {
                    if let Some(entry) = preparing
                        .as_ref()
                        .filter(|entry| entry.phase == preparing::PreparingPhase::Preparing)
                    {
                        policy = RpcSequencingPolicy::CancelPreparation {
                            conversation_id: entry.token.conversation_id.clone(),
                        };
                    }
                }
                let mut frozen_owners = Vec::new();
                if let Some(entry) = preparing {
                    frozen_owners.push((entry.token.conversation_id, entry.project_id));
                }
                if let Some(owner) = self
                    .active_runs
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(run)
                    .map(|control| (control.conversation_id.clone(), control.project_id.clone()))
                {
                    frozen_owners.push(owner);
                }
                if let Some(owner) = self
                    .usage_contexts
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(run)
                    .map(|state| {
                        (
                            state.context.conversation_id.clone(),
                            state.context.project_id.clone(),
                        )
                    })
                {
                    frozen_owners.push(owner);
                }
                {
                    let actions = self
                        .pending_actions
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    for action in actions
                        .values()
                        .filter(|action| action.snapshot.run_id == run)
                    {
                        let Some(conversation) = agent_input_conversation_id(&action.agent_input)
                            .filter(|id| action.snapshot.conversation_id.as_deref() == Some(*id))
                        else {
                            return Ok(RpcResourceScope::Global);
                        };
                        frozen_owners.push((
                            conversation.to_owned(),
                            agent_input_project_id(&action.agent_input).map(str::to_owned),
                        ));
                    }
                }
                let Some((conversation, _)) = frozen_owners.first() else {
                    // A durable Trace/session may prove the Conversation but not its original
                    // Project. Without the complete frozen effect scope, retain the global fence.
                    return Ok(RpcResourceScope::Global);
                };
                let conversation = conversation.clone();
                if frozen_owners
                    .iter()
                    .any(|(owner, _)| owner != &conversation)
                {
                    return Ok(RpcResourceScope::Global);
                }
                extra_keys.extend(
                    frozen_owners
                        .into_iter()
                        .filter_map(|(_, project)| project)
                        .map(|project| format!("project-member:{project}")),
                );
                Some(conversation)
            }
            HUMAN_INTERACTION_SUBMIT_METHOD | HUMAN_INTERACTION_IGNORE_METHOD => {
                let (Some(conversation), Some(request)) =
                    (text("conversationId"), text("requestId"))
                else {
                    return Ok(RpcResourceScope::Global);
                };
                let record = self
                    .storage
                    .get_human_interaction_request(conversation, request)
                    .map_err(|error| error.to_string())?;
                extra_keys.push(format!("interaction:{}", record.request_id));
                Some(record.conversation_id)
            }
            AGENT_START_CONVERSATION_TURN_METHOD
            | AGENT_CONTINUE_CONVERSATION_TURN_METHOD
            | AGENT_START_PROVIDER_TRANSITION_METHOD
            | AGENT_START_MANUAL_CONTEXT_COMPACTION_METHOD
            | AGENT_CANCEL_MANUAL_CONTEXT_COMPACTION_METHOD
            | STORAGE_DELETE_CONVERSATION_METHOD
            | STORAGE_DELETE_CHAT_MESSAGES_METHOD
            | STORAGE_UPSERT_CHAT_MESSAGES_METHOD
            | STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD
            | STORAGE_SAVE_CHAT_MESSAGE_UI_STATE_METHOD
            | STORAGE_LOAD_CONVERSATION_METHOD
            | STORAGE_LOAD_READ_TOOL_FILE_CONTEXT_METHOD
            | AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD
            | AGENT_GET_MANUAL_CONTEXT_COMPACTION_STATUS_METHOD
            | AGENT_GET_PROVIDER_TRANSITION_STATUS_METHOD => {
                text("conversationId").map(str::to_owned)
            }
            AGENT_REWRITE_CONVERSATION_TURN_METHOD => params
                .get("turn")
                .and_then(|turn| turn.get("conversationId"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            STORAGE_SAVE_CONVERSATION_META_METHOD => text("id").map(str::to_owned),
            STORAGE_SAVE_COMPOSER_DRAFT_METHOD => params
                .get("draft")
                .and_then(|draft| draft.get("scopeId"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            STORAGE_SAVE_COMPOSER_DRAFT_MESSAGE_METHOD => text("scopeId").map(str::to_owned),
            // Unaudited multi-resource mutations (workflow, collaboration management,
            // account/configuration) are not inferred to be local from an arbitrary DTO field.
            _ => return Ok(RpcResourceScope::Global),
        };
        let Some(conversation) = conversation else {
            return Ok(RpcResourceScope::Global);
        };
        let Some(meta) = self.storage.load_conversation_meta(&conversation)? else {
            if matches!(
                method,
                AGENT_START_CONVERSATION_TURN_METHOD
                    | AGENT_CONTINUE_CONVERSATION_TURN_METHOD
                    | AGENT_REWRITE_CONVERSATION_TURN_METHOD
            ) {
                return Ok(RpcResourceScope::GlobalTurnPreparation {
                    conversation_id: conversation,
                });
            }
            if matches!(policy, RpcSequencingPolicy::CancelPreparation { .. }) {
                if let Some(entry) = preparing_identity {
                    // Only two production paths register Preparing: a human root, or a trusted
                    // AgentWake whose factory already proved its durable node/Conversation.
                    // Deletion waits for Preparing to drain before removing either. Thus an
                    // exact current entry with neither node nor admitted Trace is a new human
                    // root; the DTO's conversation/project hints are not authority here.
                    let has_node = self
                        .storage
                        .get_agent_node_by_conversation(&conversation)
                        .map_err(|error| error.to_string())?
                        .is_some();
                    let has_trace = self
                        .storage
                        .get_conversation_turn_trace(&entry.assistant_message_id)?
                        .is_some();
                    let project_valid = match entry.project_id.as_deref() {
                        None => true,
                        Some(project) => self
                            .storage
                            .load_projects()?
                            .iter()
                            .any(|record| record.id == project),
                    };
                    if entry.token.conversation_id == conversation
                        && !has_node
                        && !has_trace
                        && project_valid
                    {
                        extra_keys.extend([
                            format!("conversation:{conversation}"),
                            format!("tree:{conversation}"),
                        ]);
                        extra_keys.sort();
                        extra_keys.dedup();
                        return Ok(RpcResourceScope::Resources {
                            keys: extra_keys,
                            fairness_root: conversation,
                            policy,
                        });
                    }
                }
            }
            return Ok(RpcResourceScope::Global);
        };
        if matches!(
            method,
            AGENT_START_CONVERSATION_TURN_METHOD
                | AGENT_CONTINUE_CONVERSATION_TURN_METHOD
                | AGENT_REWRITE_CONVERSATION_TURN_METHOD
        ) {
            policy = RpcSequencingPolicy::TurnPreparation {
                conversation_id: conversation.clone(),
            };
        } else if matches!(
            method,
            STORAGE_DELETE_CONVERSATION_METHOD | STORAGE_DELETE_CHAT_MESSAGES_METHOD
        ) || (method == STORAGE_SAVE_CONVERSATION_META_METHOD
            && meta.project_id.as_deref() != text("projectId"))
        {
            policy = RpcSequencingPolicy::OwnershipChange;
        }
        if let Some(project) = meta.project_id.as_ref() {
            extra_keys.push(format!("project-member:{project}"));
        }
        // Moving metadata must also sequence with deletion of the destination. Existence is
        // checked from Core; the DTO alone never creates a scope or bypasses final storage CAS.
        if method == STORAGE_SAVE_CONVERSATION_META_METHOD {
            if let Some(project) =
                text("projectId").filter(|target| meta.project_id.as_deref() != Some(*target))
            {
                if !self
                    .storage
                    .load_projects()?
                    .iter()
                    .any(|record| record.id == project)
                {
                    return Ok(RpcResourceScope::Global);
                }
                extra_keys.push(format!("project-member:{project}"));
            }
        }
        let node = self
            .storage
            .get_agent_node_by_conversation(&conversation)
            .map_err(|error| error.to_string())?;
        let fairness_root = node.as_ref().map_or_else(
            || conversation.clone(),
            |node| node.root_conversation_id.clone(),
        );
        if matches!(
            method,
            AGENT_CANCEL_RUN_METHOD
                | STORAGE_DELETE_CONVERSATION_METHOD
                | STORAGE_DELETE_CHAT_MESSAGES_METHOD
        ) {
            // A user stop may cancel descendants; resolve the root from the graph instead of
            // pretending a Run DTO is a single-conversation effect.
            extra_keys.push(format!("tree:{fairness_root}"));
            if let Some(node) = node.as_ref() {
                for member in self
                    .storage
                    .list_agent_tree(&node.root_agent_id)
                    .map_err(|error| error.to_string())?
                {
                    if let Some(project) = member.project_id {
                        extra_keys.push(format!("project-member:{project}"));
                    }
                }
            }
        }
        extra_keys.push(format!("conversation:{conversation}"));
        extra_keys.sort();
        extra_keys.dedup();
        Ok(RpcResourceScope::Resources {
            keys: extra_keys,
            fairness_root,
            policy,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::storage::models::ChatConversationMetaRecord;
    use serde_json::json;

    fn fixture() -> (tempfile::TempDir, AgentService) {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            StorageService::open_with_model_credentials(
                &directory.path().join("rpc-resource-test.sqlite"),
                Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
            )
            .unwrap(),
        );
        for id in ["actual", "forged"] {
            storage
                .save_conversation_meta(ChatConversationMetaRecord {
                    id: id.into(),
                    project_id: None,
                    model_id: None,
                    title: id.into(),
                    created_at: 1,
                    updated_at: 1,
                    pinned_at: None,
                    archived_at: None,
                    unread_at: None,
                })
                .unwrap();
        }
        (directory, AgentService::new_authorized_for_test(storage))
    }

    #[test]
    fn scope_allowlist_never_promotes_unknown_or_missing_conversations() {
        let (_directory, service) = fixture();
        assert!(
            matches!(service.rpc_resource_scope(AGENT_START_CONVERSATION_TURN_METHOD,
            Some(&json!({"conversationId":"actual","runId":"forged-run"}))).unwrap(),
            RpcResourceScope::Resources { keys, .. } if keys == ["conversation:actual"])
        );
        for method in ["unknown.mutation", STORAGE_DELETE_PROJECT_METHOD] {
            assert_eq!(
                service
                    .rpc_resource_scope(method, Some(&json!({"conversationId":"actual"})))
                    .unwrap(),
                RpcResourceScope::Global
            );
        }
        assert_eq!(
            service
                .rpc_resource_scope(CORE_SET_EXECUTION_ACCESS_METHOD, None)
                .unwrap(),
            RpcResourceScope::ExecutionAccess
        );
        assert_eq!(
            service
                .rpc_resource_scope(
                    AGENT_START_CONVERSATION_TURN_METHOD,
                    Some(&json!({"conversationId":"absent"}))
                )
                .unwrap(),
            RpcResourceScope::GlobalTurnPreparation {
                conversation_id: "absent".into()
            }
        );
        assert_eq!(
            service
                .rpc_resource_scope(STORAGE_LOAD_CONVERSATION_METAS_METHOD, None)
                .unwrap(),
            RpcResourceScope::Overview
        );
    }

    #[test]
    fn office_status_is_an_ownerless_observation_but_exact_reads_still_sequence() {
        let (_directory, service) = fixture();
        assert_eq!(
            service
                .rpc_resource_scope(OFFICE_GET_STATUS_METHOD, None)
                .unwrap(),
            RpcResourceScope::Overview
        );
        assert!(matches!(
            service
                .rpc_resource_scope(
                    STORAGE_LOAD_CONVERSATION_METHOD,
                    Some(&json!({"conversationId":"actual"})),
                )
                .unwrap(),
            RpcResourceScope::Resources { .. }
        ));
        assert_eq!(
            service.rpc_resource_scope(STORAGE_LOAD_READ_TOOL_FILE_CONTEXT_METHOD, Some(&json!({"conversationId":"actual","assistantMessageId":"message","callId":"read"}))).unwrap(),
            service.rpc_resource_scope(STORAGE_LOAD_CONVERSATION_METHOD, Some(&json!({"conversationId":"actual"}))).unwrap(),
            "a narrow proof read retains the exact Conversation ordering boundary"
        );
        assert_eq!(
            service.rpc_resource_scope("unknown.read", None).unwrap(),
            RpcResourceScope::Global
        );
    }

    #[test]
    fn approval_uses_frozen_action_owner_and_ignores_forged_conversation_hint() {
        let (_directory, service) = fixture();
        for project in ["frozen-project", "current-project"] {
            service
                .storage
                .save_project(
                    mycopilot_core::storage::models::ProjectRecord::without_folders(
                        project, project, 1,
                    ),
                )
                .unwrap();
        }
        let mut meta = service
            .storage
            .load_conversation_meta("actual")
            .unwrap()
            .unwrap();
        meta.project_id = Some("current-project".into());
        service.storage.save_conversation_meta(meta).unwrap();
        let action_id = pending_action_storage_id("actual-run", "actual-action");
        service
            .storage
            .store_pending_agent_action(AgentPendingActionRecord {
                action_id: action_id.clone(),
                run_id: "actual-run".into(),
                conversation_id: Some("actual".into()),
                assistant_message_id: None,
                action_type: "command".into(),
                tool_name: "run_command".into(),
                tool_call_id: None,
                status: "pending".into(),
                target_status: None,
                action_json: "{}".into(),
                agent_input_json:
                    json!({"context":{"conversationId":"actual", "projectId":"frozen-project"}})
                        .to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();
        let scope = service.rpc_resource_scope(AGENT_APPROVE_ACTION_METHOD,
            Some(&json!({"runId":"actual-run","actionId":"actual-action","conversationId":"forged"}))).unwrap();
        let RpcResourceScope::Resources {
            keys,
            fairness_root,
            ..
        } = scope
        else {
            panic!("durable owner should resolve")
        };
        assert!(keys.contains(&"conversation:actual".into()));
        assert!(!keys.contains(&"conversation:forged".into()));
        assert!(keys.contains(&"project-member:frozen-project".into()));
        assert!(keys.contains(&"project-member:current-project".into()));
        assert_eq!(fairness_root, "actual");
        assert_eq!(service.rpc_resource_scope(AGENT_APPROVE_ACTION_METHOD,
            Some(&json!({"runId":"forged-run","actionId":"actual-action","conversationId":"actual"}))).unwrap(), RpcResourceScope::Global);
    }
    #[test]
    fn project_delete_and_metadata_move_use_verified_project_membership() {
        let (_directory, service) = fixture();
        for project in ["source-project", "target-project"] {
            service
                .storage
                .save_project(
                    mycopilot_core::storage::models::ProjectRecord::without_folders(
                        project, project, 1,
                    ),
                )
                .unwrap();
        }
        let mut meta = service
            .storage
            .load_conversation_meta("actual")
            .unwrap()
            .unwrap();
        meta.project_id = Some("source-project".into());
        service.storage.save_conversation_meta(meta).unwrap();
        let RpcResourceScope::Resources { keys, .. } = service
            .rpc_resource_scope(
                STORAGE_LOAD_CONVERSATION_METHOD,
                Some(&json!({"conversationId":"actual", "projectId":"forged"})),
            )
            .unwrap()
        else {
            panic!("existing metadata is authoritative")
        };
        assert!(keys.contains(&"project-member:source-project".into()));
        assert!(!keys.contains(&"project-member:forged".into()));
        let RpcResourceScope::Resources { keys, .. } = service
            .rpc_resource_scope(
                STORAGE_SAVE_CONVERSATION_META_METHOD,
                Some(&json!({"id":"actual", "projectId":"target-project"})),
            )
            .unwrap()
        else {
            panic!("verified move resolves both project memberships")
        };
        assert!(keys.contains(&"project-member:source-project".into()));
        assert!(keys.contains(&"project-member:target-project".into()));
        assert!(
            matches!(service.rpc_resource_scope(STORAGE_DELETE_PROJECT_METHOD,
            Some(&json!({"projectId":"source-project", "conversationId":"forged"}))).unwrap(),
            RpcResourceScope::Resources { keys, .. } if keys == ["project-marker:source-project"])
        );
        assert_eq!(
            service
                .rpc_resource_scope(
                    STORAGE_DELETE_PROJECT_METHOD,
                    Some(&json!({"projectId":"absent"}))
                )
                .unwrap(),
            RpcResourceScope::Global
        );
        assert!(
            matches!(service.rpc_resource_scope(STORAGE_DELETE_CONVERSATION_METHOD,
            Some(&json!({"conversationId":"actual"}))).unwrap(),
            RpcResourceScope::Resources { keys, .. } if keys.contains(&"tree:actual".into()))
        );
    }

    #[test]
    fn steering_uses_preparing_run_identity_instead_of_a_forged_conversation() {
        let (_directory, service) = fixture();
        assert_eq!(
            service
                .rpc_resource_scope(
                    AGENT_CANCEL_RUN_METHOD,
                    Some(&json!({"runId":"preparing-run", "conversationId":"actual"}))
                )
                .unwrap(),
            RpcResourceScope::Global,
            "a future Run hint cannot interrupt an unregistered preparation"
        );
        let _preparing = service
            .preparing
            .register(
                "actual",
                None,
                "preparing-run",
                "assistant",
                AgentCancellationToken::new(),
                Some(0),
            )
            .unwrap();
        assert!(matches!(service.rpc_resource_scope(AGENT_STEER_RUN_METHOD,
            Some(&json!({"expectedRunId":"preparing-run", "conversationId":"forged"}))).unwrap(),
            RpcResourceScope::Resources { keys, .. } if keys.contains(&"conversation:actual".into()) && !keys.contains(&"conversation:forged".into())));
        assert_eq!(
            service
                .rpc_resource_scope(
                    AGENT_STEER_RUN_METHOD,
                    Some(&json!({"expectedRunId":"missing-run", "conversationId":"actual"}))
                )
                .unwrap(),
            RpcResourceScope::Global
        );
        assert!(matches!(service.rpc_resource_scope(AGENT_CANCEL_RUN_METHOD,
            Some(&json!({"runId":"preparing-run", "conversationId":"forged"}))).unwrap(),
            RpcResourceScope::Resources { keys, policy: RpcSequencingPolicy::CancelPreparation { conversation_id }, .. }
                if keys.contains(&"tree:actual".into()) && conversation_id == "actual"));
        _preparing.mark_committing().unwrap();
        assert!(
            matches!(
                service
                    .rpc_resource_scope(
                        AGENT_CANCEL_RUN_METHOD,
                        Some(&json!({"runId":"preparing-run", "conversationId":"forged"}))
                    )
                    .unwrap(),
                RpcResourceScope::Resources {
                    policy: RpcSequencingPolicy::Ordered,
                    ..
                }
            ),
            "commit must preserve the normal ordering boundary"
        );
    }

    #[test]
    fn missing_or_forged_human_question_owner_does_not_grant_a_local_scope() {
        let (_directory, service) = fixture();
        for method in [
            HUMAN_INTERACTION_SUBMIT_METHOD,
            HUMAN_INTERACTION_IGNORE_METHOD,
        ] {
            assert!(service
                .rpc_resource_scope(
                    method,
                    Some(&json!({"conversationId":"actual", "requestId":"unknown-request"}))
                )
                .is_err());
            assert_eq!(
                service
                    .rpc_resource_scope(method, Some(&json!({"conversationId":"actual"})))
                    .unwrap(),
                RpcResourceScope::Global
            );
        }
    }

    #[test]
    fn preparing_run_preserves_its_frozen_project_after_conversation_movement() {
        let (_directory, service) = fixture();
        for project in ["frozen-project", "current-project"] {
            service
                .storage
                .save_project(
                    mycopilot_core::storage::models::ProjectRecord::without_folders(
                        project, project, 1,
                    ),
                )
                .unwrap();
        }
        let mut meta = service
            .storage
            .load_conversation_meta("actual")
            .unwrap()
            .unwrap();
        meta.project_id = Some("current-project".into());
        service.storage.save_conversation_meta(meta).unwrap();
        let _preparing = service
            .preparing
            .register(
                "actual",
                Some("frozen-project"),
                "run-before-move",
                "assistant",
                AgentCancellationToken::new(),
                Some(0),
            )
            .unwrap();
        for (method, params) in [
            (
                AGENT_CANCEL_RUN_METHOD,
                json!({"runId":"run-before-move", "projectId":"forged"}),
            ),
            (
                AGENT_STEER_RUN_METHOD,
                json!({"expectedRunId":"run-before-move", "projectId":"forged"}),
            ),
        ] {
            let RpcResourceScope::Resources { keys, .. } =
                service.rpc_resource_scope(method, Some(&params)).unwrap()
            else {
                panic!("Core frozen Run identity should resolve")
            };
            assert!(keys.contains(&"project-member:frozen-project".into()));
            assert!(keys.contains(&"project-member:current-project".into()));
            assert!(!keys.contains(&"project-member:forged".into()));
        }
    }

    #[test]
    fn new_conversation_cancel_requires_exact_core_preparing_identity() {
        let (_directory, service) = fixture();
        service
            .storage
            .save_project(
                mycopilot_core::storage::models::ProjectRecord::without_folders(
                    "preparing-project",
                    "preparing-project",
                    1,
                ),
            )
            .unwrap();
        let params = json!({
            "runId": "new-run",
            "conversationId": "forged",
            "projectId": "forged"
        });
        assert_eq!(
            service
                .rpc_resource_scope(AGENT_CANCEL_RUN_METHOD, Some(&params))
                .unwrap(),
            RpcResourceScope::Global,
            "DTO hints alone cannot cancel a future Run"
        );
        let preparing = service
            .preparing
            .register(
                "new-conversation",
                Some("preparing-project"),
                "new-run",
                "new-assistant",
                AgentCancellationToken::new(),
                None,
            )
            .unwrap();
        assert!(service
            .storage
            .load_conversation_meta("new-conversation")
            .unwrap()
            .is_none());
        assert_eq!(
            service
                .rpc_resource_scope(AGENT_CANCEL_RUN_METHOD, Some(&params))
                .unwrap(),
            RpcResourceScope::Resources {
                keys: vec![
                    "conversation:new-conversation".into(),
                    "project-member:preparing-project".into(),
                    "run:new-run".into(),
                    "tree:new-conversation".into(),
                ],
                fairness_root: "new-conversation".into(),
                policy: RpcSequencingPolicy::CancelPreparation {
                    conversation_id: "new-conversation".into(),
                },
            }
        );
        for (method, params) in [
            (
                AGENT_CANCEL_RUN_METHOD,
                json!({"runId":"unregistered-run", "conversationId":"new-conversation"}),
            ),
            (
                AGENT_STEER_RUN_METHOD,
                json!({"expectedRunId":"new-run", "conversationId":"new-conversation"}),
            ),
            (
                "unknown.mutation",
                json!({"runId":"new-run", "conversationId":"new-conversation"}),
            ),
        ] {
            assert_eq!(
                service.rpc_resource_scope(method, Some(&params)).unwrap(),
                RpcResourceScope::Global
            );
        }
        preparing.mark_committing().unwrap();
        assert_eq!(
            service
                .rpc_resource_scope(AGENT_CANCEL_RUN_METHOD, Some(&params))
                .unwrap(),
            RpcResourceScope::Global,
            "a missing-metadata commit retains the ordinary global fence"
        );
    }
}
