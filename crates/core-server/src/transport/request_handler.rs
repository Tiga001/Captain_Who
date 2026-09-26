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
        return handle_automation_request_with_access(storage, Some(agent_service), None, request);
    }
    if is_notification_request_method(&request.method) {
        return handle_notification_request(storage, request);
    }
    if is_human_interaction_method(&request.method) {
        return handle_human_interaction_request(storage, agent_service, notification_tx, request);
    }

    match request.method.as_str() {
        "agent.workflows.request" => handle_workflow_request(storage, Some(agent_service), request),
        CORE_PING_METHOD => handle_core_ping(request.id, request.params),
        mycopilot_protocol_rs::CORE_SET_EXECUTION_ACCESS_METHOD => {
            let input = match parse_params::<mycopilot_protocol_rs::SetExecutionAccessInput>(
                request.params,
            ) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            if let Err(message) = input.validate() {
                return response_error(Some(request.id), -32602, message);
            }
            match agent_service.set_execution_access(input) {
                Ok(output) => response_success(request.id, output),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
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
        mycopilot_protocol_rs::AGENT_START_MANUAL_CONTEXT_COMPACTION_METHOD => {
            let input =
                match parse_params::<agent::AgentManualContextCompactionStartInput>(request.params)
                {
                    Ok(input) => input,
                    Err(message) => return response_error(Some(request.id), -32602, message),
                };
            match agent_service.start_manual_context_compaction(input, notification_tx) {
                Ok(output) => response_success(request.id, output),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        mycopilot_protocol_rs::AGENT_GET_MANUAL_CONTEXT_COMPACTION_STATUS_METHOD => {
            let input = match parse_params::<agent::AgentManualContextCompactionStatusInput>(
                request.params,
            ) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.get_manual_context_compaction_status(input) {
                Ok(output) => response_success(request.id, output),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        mycopilot_protocol_rs::AGENT_CANCEL_MANUAL_CONTEXT_COMPACTION_METHOD => {
            let input = match parse_params::<agent::AgentManualContextCompactionCancelInput>(
                request.params,
            ) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.cancel_manual_context_compaction(input, notification_tx) {
                Ok(output) => response_success(request.id, output),
                Err(error) => agent_service_error_response(request.id, error),
            }
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
        mycopilot_protocol_rs::AGENT_COLLABORATION_GET_SETTINGS_METHOD => {
            if let Err(message) = parse_params::<
                mycopilot_protocol_rs::AgentCollaborationSettingsGetInput,
            >(request.params)
            {
                return response_error(Some(request.id), -32602, message);
            }
            match storage.load_agent_collaboration_settings() {
                Ok(settings) => response_success(request.id, settings),
                Err(error) => response_error(Some(request.id), -32000, error),
            }
        }
        mycopilot_protocol_rs::AGENT_COLLABORATION_UPDATE_SETTINGS_METHOD => {
            let input = match parse_params::<mycopilot_protocol_rs::AgentCollaborationSettingsUpdate>(
                request.params,
            ) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match storage.update_agent_collaboration_settings(
                &mycopilot_core::AgentCollaborationSettingsUpdate {
                    enabled: input.enabled,
                    expected_revision: input.expected_revision,
                },
            ) {
                Ok(settings) => {
                    agent_service.invalidate_all_conversation_context_states();
                    let _ = notification_tx.send(serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": mycopilot_protocol_rs::AGENT_COLLABORATION_SETTINGS_CHANGED_METHOD,
                        "params": settings,
                    }));
                    response_success(request.id, settings)
                }
                Err(error) => response_error(Some(request.id), -32000, error),
            }
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
        AGENT_COLLABORATION_TEMPLATES_SET_PROJECT_ASSIGNMENT_METHOD => {
            let input = match parse_params::<AgentTemplateProjectAssignmentRequest>(request.params)
            {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.set_collaboration_template_project_assignment(input) {
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
        AGENT_GET_LOCAL_TOKEN_USAGE_METHOD => {
            handle_agent_local_token_usage(agent_service, request.id, request.params)
        }
        AGENT_CLEAR_USAGE_RECORDS_METHOD => {
            handle_agent_clear_usage_records(agent_service, request.id, request.params)
        }
        AGENT_READ_FILE_CHANGE_METHOD => {
            let input = match parse_params::<AgentFileChangeReadRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.read_file_change(
                &input.transaction_id,
                input.observer_root_conversation_id.as_deref(),
                input.offset,
                input.max_chars,
            ) {
                Ok(output) => response_success(request.id, output),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        AGENT_GET_FILE_CHANGE_DIFF_METHOD => {
            let input = match parse_params::<AgentFileChangeReadRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.get_file_change_diff(
                &input.transaction_id,
                input.observer_root_conversation_id.as_deref(),
                input.offset,
                input.max_chars,
            ) {
                Ok(output) => response_success(request.id, output),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        AGENT_GET_FILE_CHANGE_HISTORY_DIFF_METHOD => {
            let input = match parse_params::<AgentFileChangeHistoryDiffRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.get_file_change_history_diff(&input) {
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
            storage_response(request.id, storage.load_model_settings_for_edit())
        }
        STORAGE_LOAD_PROVIDER_PROFILE_UI_DESCRIPTORS_METHOD => response_success(
            request.id,
            mycopilot_core::provider_profile_ui_descriptors(),
        ),
        STORAGE_LOAD_PROVIDER_VENDOR_DESCRIPTORS_METHOD => {
            response_success(request.id, mycopilot_core::provider_vendor_descriptors())
        }
        STORAGE_RESOLVE_PROVIDER_VENDOR_MODEL_POLICY_METHOD => {
            let input = match parse_params::<mycopilot_core::ProviderVendorModelPolicyInput>(
                request.params,
            ) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            response_success(
                request.id,
                mycopilot_core::resolve_provider_vendor_model_policy(&input),
            )
        }
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
            if let Ok(preferences) = &result {
                agent_service.invalidate_all_conversation_context_states();
                let _ = notification_tx.send(serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": mycopilot_protocol_rs::AGENT_PROMPT_PREFERENCES_CHANGED_METHOD,
                    "params": {
                        "contextProfile": preferences.context_profile,
                        "updatedAt": preferences.updated_at,
                    },
                }));
            }
            storage_response(request.id, result)
        }
        STORAGE_LOAD_PROJECTS_METHOD => storage_response(request.id, storage.load_projects()),
        "storage.loadRunWorkspace" => {
            let input = match parse_params::<RunWorkspaceRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage
                    .load_run_workspace(&input.assistant_message_id, input.project_id.as_deref()),
            )
        }
        "storage.resolveRunWorkspacePath" => {
            let input = match parse_params::<RunWorkspacePathRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage.resolve_run_workspace_path(
                    &input.assistant_message_id,
                    input.project_id.as_deref(),
                    &input.file_path,
                ),
            )
        }
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
        mycopilot_protocol_rs::STORAGE_BEGIN_ATTACHMENT_IMPORT_METHOD => {
            let input = match parse_params::<mycopilot_core::AttachmentImportInput>(request.params)
            {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage
                    .begin_attachment_import(input)
                    .map(|id| json!({ "importId": id })),
            )
        }
        mycopilot_protocol_rs::STORAGE_APPEND_ATTACHMENT_IMPORT_METHOD => {
            let input = match parse_params::<AppendAttachmentImportRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage
                    .append_attachment_import(&input.import_id, input.offset, &input.data)
                    .map(|size| json!({ "receivedBytes": size })),
            )
        }
        mycopilot_protocol_rs::STORAGE_FINISH_ATTACHMENT_IMPORT_METHOD => {
            let input = match parse_params::<AttachmentImportIdRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage.finish_attachment_import(&input.import_id),
            )
        }
        mycopilot_protocol_rs::STORAGE_CANCEL_ATTACHMENT_IMPORT_METHOD => {
            let input = match parse_params::<AttachmentImportIdRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage.cancel_attachment_import(&input.import_id),
            )
        }
        mycopilot_protocol_rs::STORAGE_LOAD_INPUT_ATTACHMENT_PREVIEW_METHOD => {
            let input = match parse_params::<InputAttachmentPreviewRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            if input
                .purpose
                .as_deref()
                .is_some_and(|purpose| purpose != "display" && purpose != "thumbnail")
            {
                return response_error(
                    Some(request.id),
                    -32602,
                    "Invalid attachment preview purpose",
                );
            }
            storage_response(
                request.id,
                storage.load_input_attachment_preview_for_display(
                    &input.attachment,
                    input.purpose.as_deref() == Some("display"),
                ),
            )
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
        STORAGE_LOAD_BROWSER_DOWNLOAD_SETTINGS_METHOD => {
            storage_response(request.id, storage.load_browser_download_settings())
        }
        STORAGE_SAVE_BROWSER_DOWNLOAD_SETTINGS_METHOD => {
            let input = match parse_params::<BrowserDownloadSettingsUpdate>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.save_browser_download_settings(input))
        }
        STORAGE_REGISTER_BROWSER_DOWNLOAD_METHOD => {
            let input = match parse_params::<BrowserDownloadRegistration>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.register_browser_download(input))
        }
        STORAGE_LIST_BROWSER_DOWNLOADS_METHOD => {
            let input = match parse_params::<BrowserDownloadListInput>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.list_browser_downloads(input))
        }
        STORAGE_LOAD_BROWSER_DOWNLOAD_METHOD => {
            let input = match parse_params::<BrowserDownloadIdRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage.load_browser_download(&input.download_id),
            )
        }
        STORAGE_CLEAR_BROWSER_DOWNLOAD_HISTORY_METHOD => {
            storage_response(request.id, storage.clear_browser_download_history())
        }
        STORAGE_LOAD_BROWSER_PREFERENCES_METHOD => {
            storage_response(request.id, storage.load_browser_preferences())
        }
        STORAGE_SAVE_BROWSER_PREFERENCES_METHOD => {
            let input = match parse_params::<BrowserPreferencesUpdate>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.save_browser_preferences(input))
        }
        STORAGE_REGISTER_BROWSER_HISTORY_METHOD => {
            let input = match parse_params::<BrowserHistoryRegistration>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.register_browser_history(input))
        }
        STORAGE_UPDATE_BROWSER_HISTORY_METHOD => {
            let input = match parse_params::<BrowserHistoryMetadataUpdate>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.update_browser_history_metadata(input))
        }
        STORAGE_LIST_BROWSER_HISTORY_METHOD => {
            let input = match parse_params::<BrowserHistoryListInput>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.list_browser_history(input))
        }
        STORAGE_DELETE_BROWSER_HISTORY_METHOD => {
            let input = match parse_params::<BrowserHistoryDeleteInput>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.delete_browser_history(input))
        }
        STORAGE_SUMMARIZE_BROWSER_OWNED_DATA_METHOD => {
            let input = match parse_params::<BrowserOwnedDataRangeInput>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.summarize_browser_owned_data(input))
        }
        STORAGE_CLEAR_BROWSER_OWNED_DATA_METHOD => {
            let input = match parse_params::<BrowserOwnedDataClearInput>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.clear_browser_owned_data(input))
        }
        STORAGE_SAVE_CONVERSATION_META_METHOD => {
            let conversation = match parse_params::<ChatConversationMetaRecord>(request.params) {
                Ok(conversation) => conversation,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.save_conversation_meta_checked(conversation) {
                Ok(conversation) => storage_response(request.id, Ok(conversation)),
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
        STORAGE_SAVE_COMPOSER_DRAFT_MESSAGE_METHOD => {
            let input = match parse_params::<SaveComposerDraftMessageRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.authorize_user_conversation_write(&input.scope_id) {
                Ok(()) => storage_response(
                    request.id,
                    storage.save_composer_draft_message(
                        &input.scope_id,
                        &input.message,
                        input.updated_at,
                    ),
                ),
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BrowserDownloadIdRequest {
    pub(crate) download_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoadInputAttachmentsRequest {
    pub(crate) attachment_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AttachmentImportIdRequest {
    pub(crate) import_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AppendAttachmentImportRequest {
    pub(crate) import_id: String,
    pub(crate) offset: u64,
    pub(crate) data: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct InputAttachmentPreviewRequest {
    pub(crate) attachment: mycopilot_core::AgentInputAttachment,
    pub(crate) purpose: Option<String>,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SaveComposerDraftMessageRequest {
    pub(crate) scope_id: String,
    pub(crate) message: String,
    pub(crate) updated_at: i64,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunWorkspaceRequest {
    assistant_message_id: String,
    project_id: Option<String>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunWorkspacePathRequest {
    assistant_message_id: String,
    project_id: Option<String>,
    file_path: String,
}
