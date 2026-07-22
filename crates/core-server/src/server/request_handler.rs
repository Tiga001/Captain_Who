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

    match request.method.as_str() {
        CORE_PING_METHOD => handle_core_ping(request.id, request.params),
        OFFICE_GET_STATUS_METHOD => handle_office_status_request(agent_service, request),
        AGENT_START_CONVERSATION_TURN_METHOD => handle_agent_start_conversation_turn(
            agent_service,
            notification_tx,
            request.id,
            request.params,
        ),
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
        AGENT_GET_CONTEXT_COMPACTION_AUDIT_METHOD => {
            let input =
                match parse_params::<agent::AgentContextCompactionAuditInput>(request.params) {
                    Ok(input) => input,
                    Err(message) => return response_error(Some(request.id), -32602, message),
                };
            match agent_service.get_context_compaction_audit(input) {
                Ok(output) => response_success(request.id, output),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        AGENT_CANCEL_RUN_METHOD => {
            handle_agent_cancel_run(agent_service, request.id, request.params)
        }
        AGENT_LIST_PENDING_ACTIONS_METHOD => {
            response_success(request.id, agent_service.list_pending_actions())
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
            match agent_service.read_file_draft(&input.draft_id, input.offset, input.max_chars) {
                Ok(output) => response_success(request.id, output),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        AGENT_GET_FILE_WRITE_DIFF_METHOD => {
            let input = match parse_params::<AgentFileDraftReadRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.get_file_write_diff(&input.draft_id, input.offset, input.max_chars)
            {
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
        STORAGE_SAVE_MODEL_SETTINGS_METHOD => {
            let settings = match parse_params::<ModelSettingsRecord>(request.params) {
                Ok(settings) => settings,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            let result = storage.save_model_settings(settings).map(|_| json!(null));
            if result.is_ok() {
                agent_service.invalidate_all_conversation_context_states();
            }
            storage_response(request.id, result)
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
            storage_response(request.id, storage.load_conversations())
        }
        STORAGE_LOAD_CONVERSATION_METAS_METHOD => {
            storage_response(request.id, storage.load_conversation_metas())
        }
        STORAGE_LOAD_CONVERSATION_METHOD => {
            let input = match parse_params::<ConversationIdRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage.load_conversation(&input.conversation_id),
            )
        }
        STORAGE_FORK_CONVERSATION_METHOD => {
            let input = match parse_params::<ForkConversationInput>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.fork_conversation(input))
        }
        STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD => {
            let input = match parse_params::<AttachmentIdRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage.load_attachment_image(&input.attachment_id),
            )
        }
        STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD => {
            let input = match parse_params::<LoadInputAttachmentsRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage.load_input_attachments(&input.attachment_ids),
            )
        }
        STORAGE_SAVE_CONVERSATION_META_METHOD => {
            let conversation = match parse_params::<ChatConversationMetaRecord>(request.params) {
                Ok(conversation) => conversation,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.save_conversation_meta(conversation))
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
            storage_response(
                request.id,
                storage.upsert_chat_messages(
                    &input.conversation_id,
                    input.messages,
                    input.position_offset,
                ),
            )
        }
        STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD => {
            let input = match parse_params::<SaveChatMessageStateRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage
                    .save_chat_message_state(&input.conversation_id, input.message)
                    .map(|_| json!(null)),
            )
        }
        STORAGE_LOAD_COMPOSER_DRAFTS_METHOD => {
            storage_response(request.id, storage.load_composer_drafts())
        }
        STORAGE_SAVE_COMPOSER_DRAFT_METHOD => {
            let input = match parse_params::<SaveComposerDraftRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.save_composer_draft(input.draft))
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
pub(crate) struct SaveComposerDraftRequest {
    pub(crate) draft: ComposerDraftRecord,
}
