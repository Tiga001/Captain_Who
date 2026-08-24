use super::*;

pub(crate) fn handle_request(
    storage: &StorageService,
    agent_service: &AgentService,
    notification_tx: agent::CoreServerNotificationSender,
    request: JsonRpcRequest,
) -> Value {
    if request.jsonrpc != "2.0" {
        return response_error(Some(request.id), -32600, "Invalid JSON-RPC version");
    }

    if is_automation_request_method(&request.method) {
        return handle_automation_request(storage, request);
    }
    if is_notification_request_method(&request.method) {
        return handle_notification_request(storage, request);
    }

    match request.method.as_str() {
        CORE_PING_METHOD => handle_core_ping(request.id, request.params),
        OFFICE_GET_STATUS_METHOD => handle_office_status_request(agent_service, request),
        AGENT_START_CONVERSATION_TURN_METHOD => handle_agent_start_conversation_turn(
            agent_service,
            notification_tx,
            request.id,
            request.params,
        ),
        AGENT_REWRITE_CONVERSATION_TURN_METHOD => handle_agent_rewrite_conversation_turn(
            agent_service,
            notification_tx,
            request.id,
            request.params,
        ),
        AGENT_PREFLIGHT_PROVIDER_TRANSITION_METHOD => {
            handle_agent_preflight_provider_transition(agent_service, request.id, request.params)
        }
        AGENT_START_PROVIDER_TRANSITION_METHOD => handle_agent_start_provider_transition(
            agent_service,
            notification_tx,
            request.id,
            request.params,
        ),
        AGENT_GET_PROVIDER_TRANSITION_STATUS_METHOD => {
            handle_agent_get_provider_transition_status(agent_service, request.id, request.params)
        }
        AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD => {
            let input = match parse_params::<agent::AgentContextWindowSnapshotInput>(request.params)
            {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.get_context_window_snapshot(input) {
                Ok(output) => response_success(request.id, output),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        AGENT_COMMAND_SESSIONS_LIST_METHOD => {
            handle_agent_list_command_sessions(agent_service, request.id, request.params)
        }
        AGENT_COMMAND_SESSIONS_GET_METHOD => {
            handle_agent_get_command_session(agent_service, request.id, request.params)
        }
        AGENT_CANCEL_RUN_METHOD => {
            handle_agent_cancel_run(agent_service, request.id, request.params)
        }
        AGENT_STEER_RUN_METHOD => {
            handle_agent_steer_run(agent_service, notification_tx, request.id, request.params)
        }
        AGENT_LIST_PENDING_ACTIONS_METHOD => {
            response_success(request.id, agent_service.list_user_pending_actions())
        }
        AGENT_COLLABORATION_GET_TREE_METHOD => {
            let input = match parse_params::<AgentTreeRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.get_collaboration_tree(input) {
                Ok(output) => response_success(request.id, output),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        AGENT_COLLABORATION_GET_AGENT_METHOD => {
            let input = match parse_params::<AgentDetailRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.get_collaboration_agent(input) {
                Ok(output) => response_success(request.id, output),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        AGENT_COLLABORATION_LOCATE_CONVERSATION_METHOD => {
            let input = match parse_params::<AgentConversationLocatorRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.locate_collaboration_conversation(input) {
                Ok(output) => response_success(request.id, output),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        AGENT_COLLABORATION_LOAD_OBSERVER_CONVERSATION_METHOD => {
            let input = match parse_params::<AgentObserverConversationRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.load_collaboration_observer_conversation(input) {
                Ok(output) => response_success(request.id, output),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        AGENT_COLLABORATION_LIST_EVENTS_METHOD => {
            let input = match parse_params::<CollaborationEventsRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.list_collaboration_events(input) {
                Ok(output) => response_success(request.id, output),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        AGENT_COLLABORATION_TEMPLATES_LIST_METHOD => {
            let input = match parse_params::<AgentTemplateListRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.list_collaboration_templates(input) {
                Ok(output) => response_success(request.id, output),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        AGENT_COLLABORATION_TEMPLATES_CREATE_METHOD => {
            let input = match parse_params::<AgentTemplateCreateRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.create_collaboration_template(input) {
                Ok(output) => response_success(request.id, output),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        AGENT_COLLABORATION_TEMPLATES_UPDATE_METHOD => {
            let input = match parse_params::<AgentTemplateUpdateRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.update_collaboration_template(input) {
                Ok(output) => response_success(request.id, output),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        AGENT_COLLABORATION_TEMPLATES_SET_ENABLED_METHOD => {
            let input = match parse_params::<AgentTemplateSetEnabledRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.set_collaboration_template_enabled(input) {
                Ok(output) => response_success(request.id, output),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        AGENT_COLLABORATION_TEMPLATES_DELETE_METHOD => {
            let input = match parse_params::<AgentTemplateDeleteRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.delete_collaboration_template(input) {
                Ok(output) => response_success(request.id, output),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        AGENT_COLLABORATION_APPROVALS_LIST_METHOD => {
            let input = match parse_params::<CollaborationApprovalListRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.list_collaboration_approvals(input) {
                Ok(output) => response_success(request.id, output),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        AGENT_COLLABORATION_APPROVALS_DECIDE_METHOD => {
            let input = match parse_params::<CollaborationApprovalDecisionRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.decide_collaboration_approval(input, notification_tx) {
                Ok(output) => response_success(request.id, output),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        AGENT_APPROVE_ACTION_METHOD => {
            handle_agent_approve_action(agent_service, notification_tx, request.id, request.params)
        }
        AGENT_REJECT_ACTION_METHOD => {
            handle_agent_reject_action(agent_service, notification_tx, request.id, request.params)
        }
        AGENT_CANCEL_ACTION_METHOD => {
            handle_agent_cancel_action(agent_service, request.id, request.params)
        }
        AGENT_GET_USAGE_SUMMARY_METHOD => {
            handle_agent_usage_summary(agent_service, request.id, request.params)
        }
        AGENT_CLEAR_USAGE_RECORDS_METHOD => {
            handle_agent_clear_usage_records(agent_service, request.id, request.params)
        }
        AGENT_READ_FILE_DRAFT_METHOD => {
            let input = match parse_params::<AgentFileDraftReadRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.read_file_draft(
                &input.draft_id,
                input.observer_root_conversation_id.as_deref(),
                input.offset,
                input.max_chars,
            ) {
                Ok(output) => response_success(request.id, output),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        AGENT_GET_FILE_WRITE_DIFF_METHOD => {
            let input = match parse_params::<AgentFileDraftReadRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.get_file_write_diff(
                &input.draft_id,
                input.observer_root_conversation_id.as_deref(),
                input.offset,
                input.max_chars,
            ) {
                Ok(output) => response_success(request.id, output),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        SEARCH_SEARCH_CHATS_METHOD => {
            let input = match parse_params::<ChatSearchInput>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.search_chats(&input))
        }
        STORAGE_LOAD_MODEL_SETTINGS_METHOD => {
            storage_response(request.id, storage.load_model_settings())
        }
        STORAGE_LOAD_PROVIDER_PROFILE_UI_DESCRIPTORS_METHOD => response_success(
            request.id,
            mycopilot_core::provider_profile_ui_descriptors(),
        ),
        STORAGE_SAVE_MODEL_SETTINGS_METHOD => {
            let settings = match parse_params::<ModelSettingsSaveRequest>(request.params) {
                Ok(settings) => settings,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            let result = storage.save_model_settings_request(settings);
            if result.is_ok() {
                agent_service.invalidate_all_conversation_context_states();
                let _ = notification_tx.send(serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": mycopilot_protocol_rs::AGENT_COLLABORATION_RESYNC_NOTIFICATION_METHOD,
                    "params": mycopilot_protocol_rs::CollaborationResyncEnvelopeDto {
                        schema_version: mycopilot_protocol_rs::AGENT_COLLABORATION_SCHEMA_VERSION,
                        reason: mycopilot_protocol_rs::CollaborationResyncReasonDto::ModelSettingsChanged,
                    },
                }));
            }
            model_settings_save_response(request.id, result)
        }
        STORAGE_LOAD_AGENT_PROMPT_PREFERENCES_METHOD => {
            storage_response(request.id, storage.load_agent_prompt_preferences())
        }
        STORAGE_SAVE_AGENT_PROMPT_PREFERENCES_METHOD => {
            let preferences = match parse_params::<AgentPromptPreferencesRecord>(request.params) {
                Ok(preferences) => preferences,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            let result = storage.save_agent_prompt_preferences(preferences);
            if result.is_ok() {
                agent_service.invalidate_all_conversation_context_states();
            }
            storage_response(request.id, result)
        }
        STORAGE_LOAD_PROJECTS_METHOD => storage_response(request.id, storage.load_projects()),
        STORAGE_SAVE_PROJECT_METHOD => {
            let project = match parse_params::<ProjectRecord>(request.params) {
                Ok(project) => project,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.save_project(project))
        }
        STORAGE_DELETE_PROJECT_METHOD => {
            let input = match parse_params::<ProjectIdRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                agent_service
                    .delete_project(&input.project_id)
                    .map(|_| json!(null)),
            )
        }
        STORAGE_LOAD_CONVERSATIONS_METHOD => {
            storage_response(request.id, storage.load_conversation_views())
        }
        STORAGE_LOAD_CONVERSATION_METAS_METHOD => {
            storage_response(request.id, storage.load_conversation_metas())
        }
        STORAGE_LOAD_CONVERSATION_METHOD => {
            let input = match parse_params::<ConversationIdRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.authorize_user_conversation_write(&input.conversation_id) {
                Ok(()) => storage_response(
                    request.id,
                    storage.load_conversation_view(&input.conversation_id),
                ),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        STORAGE_FORK_CONVERSATION_METHOD => {
            let input = match parse_params::<ForkConversationRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            conversation_fork_response(request.id, agent_service.fork_conversation_view(input))
        }
        STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD => {
            let input = match parse_params::<AttachmentIdRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service
                .authorize_user_attachment_reads(std::slice::from_ref(&input.attachment_id))
            {
                Ok(()) => storage_response(
                    request.id,
                    storage.load_attachment_image(&input.attachment_id),
                ),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD => {
            let input = match parse_params::<LoadInputAttachmentsRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.authorize_user_attachment_reads(&input.attachment_ids) {
                Ok(()) => storage_response(
                    request.id,
                    storage.load_input_attachments(&input.attachment_ids),
                ),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        STORAGE_SAVE_CONVERSATION_META_METHOD => {
            let conversation = match parse_params::<ChatConversationMetaRecord>(request.params) {
                Ok(conversation) => conversation,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.authorize_user_conversation_write(&conversation.id) {
                Ok(()) => {
                    storage_response(request.id, storage.save_conversation_meta(conversation))
                }
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        STORAGE_DELETE_CONVERSATION_METHOD => {
            let input = match parse_params::<ConversationIdRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                agent_service
                    .delete_conversation(&input.conversation_id)
                    .map(|_| json!(null)),
            )
        }
        STORAGE_DELETE_CHAT_MESSAGES_METHOD => {
            let input = match parse_params::<DeleteChatMessagesRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                agent_service
                    .delete_chat_messages(&input.conversation_id, &input.message_ids)
                    .map(|_| json!(null)),
            )
        }
        STORAGE_UPSERT_CHAT_MESSAGES_METHOD => {
            let input = match parse_params::<UpsertChatMessagesRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.authorize_user_conversation_write(&input.conversation_id) {
                Ok(()) => storage_response(
                    request.id,
                    storage.upsert_chat_messages(
                        &input.conversation_id,
                        input.messages,
                        input.position_offset,
                    ),
                ),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD => {
            let input = match parse_params::<SaveChatMessageStateRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.authorize_user_conversation_write(&input.conversation_id) {
                Ok(()) => storage_response(
                    request.id,
                    storage
                        .save_chat_message_state(&input.conversation_id, input.message)
                        .map(|_| json!(null)),
                ),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        STORAGE_SAVE_CHAT_MESSAGE_UI_STATE_METHOD => {
            let input = match parse_params::<SaveChatMessageUiStateRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.authorize_user_conversation_write(&input.conversation_id) {
                Ok(()) => storage_response(
                    request.id,
                    storage
                        .save_chat_message_ui_state(
                            &input.conversation_id,
                            &input.message.id,
                            input.message.ui_state_json.as_deref(),
                        )
                        .map(|_| json!(null)),
                ),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        STORAGE_LOAD_COMPOSER_DRAFTS_METHOD => {
            storage_response(request.id, storage.load_composer_drafts())
        }
        STORAGE_SAVE_COMPOSER_DRAFT_METHOD => {
            let input = match parse_params::<SaveComposerDraftRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.authorize_user_conversation_write(&input.draft.scope_id) {
                Ok(()) => storage_response(request.id, storage.save_composer_draft(input.draft)),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        STORAGE_LOAD_UI_PREFERENCES_METHOD => {
            storage_response(request.id, storage.load_ui_preferences())
        }
        STORAGE_SAVE_UI_PREFERENCES_METHOD => {
            let preferences = match parse_params::<UiPreferencesRecord>(request.params) {
                Ok(preferences) => preferences,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.save_ui_preferences(preferences))
        }
        _ => response_error(Some(request.id), -32601, "Method not found"),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectIdRequest {
    pub(crate) project_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationIdRequest {
    pub(crate) conversation_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttachmentIdRequest {
    pub(crate) attachment_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoadInputAttachmentsRequest {
    pub(crate) attachment_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeleteChatMessagesRequest {
    pub(crate) conversation_id: String,
    pub(crate) message_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpsertChatMessagesRequest {
    pub(crate) conversation_id: String,
    pub(crate) messages: Vec<ChatMessageRecord>,
    pub(crate) position_offset: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveChatMessageStateRequest {
    pub(crate) conversation_id: String,
    pub(crate) message: ChatMessageStateRecord,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveChatMessageUiStateRequest {
    pub(crate) conversation_id: String,
    pub(crate) message: ChatMessageUiStateRecord,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatMessageUiStateRecord {
    pub(crate) id: String,
    pub(crate) ui_state_json: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveComposerDraftRequest {
    pub(crate) draft: ComposerDraftRecord,
}
