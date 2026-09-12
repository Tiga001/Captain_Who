impl AgentService {
    #[cfg(test)]
    pub(in crate::application::agent) fn inject_skill_script_worker_panic_once(
        &self,
        action_id: &str,
    ) {
        inject_skill_script_worker_panic_once(&self.storage, action_id);
    }

    #[cfg(test)]
    pub(in crate::application::agent) fn inject_skill_script_post_receipt_panic_once(
        &self,
        action_id: &str,
    ) {
        inject_skill_script_post_receipt_panic_once(&self.storage, action_id);
    }

    pub(in crate::application::agent) fn execute_office_operation(
        &self,
        agent_input: &AgentChatInput,
        office_operation: &mycopilot_core::AgentOfficeOperationRequest,
        skill_resources: Option<Arc<SkillResourceSession>>,
        cancellation_token: AgentCancellationToken,
        action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> AgentToolResult {
        let tool = office_tool_name(office_operation.prepared.request.document_kind);
        if office_operation.approval_status != AgentApprovalStatus::Approved {
            return AgentToolResult {
                exact_archive_file: None,
                call_id: office_operation.id.clone(),
                tool: tool.to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "office_operation_policy",
                    "code": "notAuthorized",
                })),
                error: Some("Office operation has not been authorized.".to_string()),
            };
        }
        if office_operation.schema_version != mycopilot_core::AGENT_OFFICE_OPERATION_SCHEMA_VERSION
            || !mycopilot_core::is_valid_agent_office_reason(&office_operation.reason)
            || mycopilot_core::validate_frozen_agent_office_semantic_args(office_operation).is_err()
            || office_operation.prepared.access
                != mycopilot_core::office::OfficeOperationAccess::FileWrite
            || office_operation.prepared.request.access()
                != mycopilot_core::office::OfficeOperationAccess::FileWrite
        {
            return AgentToolResult {
                exact_archive_file: None,
                call_id: office_operation.id.clone(),
                tool: tool.to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "office_operation_policy",
                    "code": "invalidApprovedSnapshot",
                    "recovery": "retry",
                })),
                error: Some(
                    "The approved Office action is not a supported file-write snapshot."
                        .to_string(),
                ),
            };
        }
        if permissions_from_input(agent_input).write == mycopilot_core::AgentWritePermission::Denied
        {
            return AgentToolResult {
                exact_archive_file: None,
                call_id: office_operation.id.clone(),
                tool: tool.to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "office_operation_policy",
                    "code": "writePermissionDenied",
                    "recovery": "changePermissions",
                })),
                error: Some(
                    "The current permission policy does not allow Office file changes.".to_string(),
                ),
            };
        }
        // Rebuild the Host-owned execution context at the last responsible moment. The Office
        // engine re-resolves every frozen path against these current run-scoped permissions and
        // attachment capabilities before it creates staging or invokes the provider.
        let skill_resources = match skill_resources {
            Some(resources) => Some(resources),
            None => match self.restore_skill_resource_session(agent_input) {
                Ok(resources) => resources,
                Err(error) => {
                    return office_skill_resource_restore_failure(
                        office_operation,
                        tool,
                        &error.to_string(),
                    );
                }
            },
        };
        let file_inputs = agent_file_input_execution_context(
            agent_input,
            skill_resources,
            Arc::clone(&self.storage),
        );
        let execution_context = mycopilot_core::office::OfficeExecutionContext::from_run_context(
            agent_input.context.as_ref(),
        )
        .with_file_inputs(file_inputs);
        match self.office_engine.execute_prepared(
            &execution_context,
            &office_operation.prepared,
            cancellation_token,
            action_cancel_flag,
        ) {
            Ok(result) => office_operation_tool_result(&office_operation.id, tool, result),
            Err(error) => office_engine_failure(
                &office_operation.id,
                tool,
                &office_operation.prepared,
                error,
            ),
        }
    }

    pub(in crate::application::agent) fn execute_skill_script(
        &self,
        agent_input: &AgentChatInput,
        script: &AgentSkillScriptRequest,
        resources: Option<&SkillResourceSession>,
        authorization_source: CommandAuthorizationSource,
        cancellation_token: AgentCancellationToken,
        action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> AgentToolResult {
        if script.approval_status != AgentApprovalStatus::Approved {
            return AgentToolResult {
                exact_archive_file: None,
                call_id: script.id.clone(),
                tool: "skills_run_script".to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "skill_script_policy",
                    "code": "notAuthorized",
                })),
                error: Some("Skill script execution has not been authorized.".to_string()),
            };
        }
        let permissions = permissions_from_input(agent_input);
        let unrestricted = permissions.read == mycopilot_core::AgentReadPermission::All
            && permissions.write == mycopilot_core::AgentWritePermission::All
            && permissions.command_safety == mycopilot_core::AgentCommandSafetyPolicy::FullAccess;
        if !unrestricted {
            return AgentToolResult {
                exact_archive_file: None,
                call_id: script.id.clone(),
                tool: "skills_run_script".to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "skill_script_policy",
                    "code": "authorizationDenied",
                    "authorizationSource": authorization_source,
                })),
                error: Some(
                    "The current permission policy does not authorize this Skill script."
                        .to_string(),
                ),
            };
        }
        let Some(resources) = resources else {
            return AgentToolResult {
                exact_archive_file: None,
                call_id: script.id.clone(),
                tool: "skills_run_script".to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "skill_script",
                    "code": "snapshotUnavailable",
                    "recovery": "reactivateSkill",
                })),
                error: Some("The activated Skill resource snapshot is unavailable.".to_string()),
            };
        };
        let verified_source = match verify_frozen_skill_script_source(script, resources) {
            Ok(verified) => verified,
            Err(error) => {
                return AgentToolResult {
                    exact_archive_file: None,
                    call_id: script.id.clone(),
                    tool: "skills_run_script".to_string(),
                    ok: false,
                    result: Some(serde_json::json!({
                        "type": "skill_script_policy",
                        "code": "sourceVerificationFailed",
                        "recovery": "reactivateSkill",
                    })),
                    error: Some(error),
                };
            }
        };
        let source_authorized = match authorization_source {
            CommandAuthorizationSource::ExplicitUser => true,
            CommandAuthorizationSource::Automatic => {
                permissions.builtin_execution
                    == mycopilot_core::AgentBuiltinExecutionPermission::AutoApprove
                    && is_verified_application_bundled_source(&verified_source)
            }
        };
        if !source_authorized {
            return AgentToolResult {
                exact_archive_file: None,
                call_id: script.id.clone(),
                tool: "skills_run_script".to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "skill_script_policy",
                    "code": "authorizationDenied",
                    "authorizationSource": authorization_source,
                })),
                error: Some(
                    "The current permission policy does not authorize this Skill script."
                        .to_string(),
                ),
            };
        }
        let Some(workspace_root) = workspace_root_optional(agent_input) else {
            return AgentToolResult {
                exact_archive_file: None,
                call_id: script.id.clone(),
                tool: "skills_run_script".to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "skill_script",
                    "code": "workspaceUnavailable",
                    "recovery": "selectWorkspace",
                })),
                error: Some("Skill script execution requires a workspace.".to_string()),
            };
        };
        match execute_skill_python_script(
            resources,
            &workspace_root,
            script,
            cancellation_token,
            action_cancel_flag,
        ) {
            Ok(result) => skill_script_tool_result(&script.id, result),
            Err(error) => skill_script_runtime_failure(&script.id, error),
        }
    }

    pub(in crate::application::agent) fn execute_skill_materialization(
        &self,
        agent_input: &AgentChatInput,
        materialization: &AgentSkillMaterializationRequest,
        resources: Option<&SkillResourceSession>,
    ) -> AgentToolResult {
        let execute = || -> Result<AgentSkillMaterializationResult, (String, Option<Value>)> {
            let plain_error = |message: &str| (message.to_string(), None);
            if materialization.approval_status != AgentApprovalStatus::Approved {
                return Err(plain_error(
                    "Skill resource materialization has not been authorized.",
                ));
            }
            if permissions_from_input(agent_input).write
                == mycopilot_core::AgentWritePermission::Denied
            {
                return Err(plain_error(
                    "Skill resource materialization requires workspace write permission.",
                ));
            }
            let resources = resources.ok_or_else(|| {
                plain_error("The activated Skill resource snapshot is unavailable.")
            })?;
            let resolver = mycopilot_core::workspace::WorkspaceResolver::from_context(
                agent_input
                    .context
                    .as_ref()
                    .and_then(|context| context.workspace.as_ref()),
            );
            let target = resolver
                .resolve_input(&materialization.destination)
                .map_err(|error| plain_error(&error))?;
            let display_destination = resolver.display_path(&target);
            let workspace_root = resolver
                .containing_root(&target)
                .map_err(|error| plain_error(&error))?
                .ok_or_else(|| {
                    plain_error("Skill resource materialization requires a workspace.")
                })?;
            let workspace_identity = resolver
                .folders()
                .iter()
                .find(|folder| {
                    folder.canonical_path.as_deref().map(std::path::Path::new)
                        == Some(workspace_root.as_path())
                })
                .and_then(|folder| folder.directory_identity.clone())
                .map(Ok)
                .unwrap_or_else(|| {
                    mycopilot_core::file_change::FileChangeDirectoryIdentity::read(&workspace_root)
                })
                .map_err(plain_error)?;
            let relative_destination = target
                .strip_prefix(&workspace_root)
                .map_err(|_| plain_error("Skill destination is outside its frozen workspace."))?
                .to_string_lossy()
                .replace('\\', "/");
            let destination = SkillMaterializationDestination::parse(relative_destination)
                .map_err(materialization_failure)?;
            let materializer = SkillResourceMaterializer::new();
            let result = if let Some(source_prefix) = materialization.source_prefix.as_deref() {
                let source = SkillPackageUri::parse(&materialization.source_uri)
                    .map_err(|error| plain_error(&error.to_string()))?;
                let source_prefix = SkillResourcePath::parse(source_prefix.to_string())
                    .map_err(|error| plain_error(&error.to_string()))?;
                let request = SkillTemplateTreeMaterializationRequest::new(
                    source,
                    source_prefix,
                    workspace_root,
                    destination,
                )
                .map_err(materialization_failure)?
                .with_workspace_identity(workspace_identity);
                let outcome = materializer
                    .materialize_template_tree(resources, &request)
                    .map_err(materialization_failure)?;
                AgentSkillMaterializationResult {
                    status: materialization_result_status(outcome.status()),
                    source_uri: outcome.source().to_string(),
                    source_prefix: Some(outcome.source_prefix().to_string()),
                    destination: display_destination.clone(),
                    source_revision: outcome.source().revision().as_str().to_string(),
                    file_count: u64::try_from(outcome.file_count()).unwrap_or(u64::MAX),
                    byte_count: outcome.byte_length(),
                    plan_digest: Some(outcome.plan_digest().to_string()),
                    error: None,
                    message: Some(materialization_success_message(outcome.status(), true)),
                }
            } else {
                let source = SkillResourceUri::parse(&materialization.source_uri)
                    .map_err(|error| plain_error(&error.to_string()))?;
                let request = SkillMaterializationRequest::new(source, workspace_root, destination)
                    .map_err(materialization_failure)?
                    .with_workspace_identity(workspace_identity);
                let outcome = materializer
                    .materialize(resources, &request)
                    .map_err(materialization_failure)?;
                AgentSkillMaterializationResult {
                    status: materialization_result_status(outcome.status()),
                    source_uri: outcome.source().to_string(),
                    source_prefix: None,
                    destination: display_destination,
                    source_revision: outcome.source().package().revision().as_str().to_string(),
                    file_count: 1,
                    byte_count: outcome.byte_length(),
                    plan_digest: Some(outcome.content_digest().to_string()),
                    error: None,
                    message: Some(materialization_success_message(outcome.status(), false)),
                }
            };
            Ok(result)
        };

        match execute() {
            Ok(result) => AgentToolResult {
                exact_archive_file: None,
                call_id: materialization.id.clone(),
                tool: "skills_materialize_resource".to_string(),
                ok: true,
                result: serde_json::to_value(result).ok(),
                error: None,
            },
            Err((error, structured)) => AgentToolResult {
                exact_archive_file: None,
                call_id: materialization.id.clone(),
                tool: "skills_materialize_resource".to_string(),
                ok: false,
                result: structured,
                error: Some(error),
            },
        }
    }

    pub(in crate::application::agent) fn queue_command_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let command_record = record.clone();
        tokio::spawn(async move {
            service
                .run_command_execution(command_record, call, guard, notifications)
                .await;
        });

        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: "approved".to_string(),
            file_change_result: None,
            command_result: None,
            tool_result: None,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        })
    }

    pub(in crate::application::agent) fn queue_mcp_tool_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let execution_record = record.clone();
        tokio::spawn(async move {
            service
                .run_mcp_tool_execution(execution_record, call, guard, notifications, false)
                .await;
        });

        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: "approved".to_string(),
            file_change_result: None,
            command_result: None,
            tool_result: None,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        })
    }

    pub(in crate::application::agent) fn queue_builtin_mcp_tool_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        grant: mycopilot_core::BuiltinMcpToolGrant,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let execution_record = record.clone();
        tokio::spawn(async move {
            service
                .run_builtin_mcp_tool_execution(execution_record, call, grant, guard, notifications)
                .await;
        });
        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: "approved".to_string(),
            file_change_result: None,
            command_result: None,
            tool_result: None,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        })
    }
}
