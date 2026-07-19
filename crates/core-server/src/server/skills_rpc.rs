use super::*;

pub(crate) struct ParsedSkillsRequest {
    pub(crate) id: JsonRpcId,
    pub(crate) operation: ParsedSkillsOperation,
}

pub(crate) enum ParsedSkillsOperation {
    List(SkillsListRequest),
    ListManagement(SkillsListManagementRequest),
    SetEnabled(SkillsSetEnabledRequest),
    InspectInstallation(SkillsInspectInstallationRequest),
    ResolveInstallationSource(SkillsResolveInstallationSourceRequest),
    CancelSourceResolution(SkillsCancelSourceResolutionRequest),
    CommitInstallation(SkillsCommitInstallationRequest),
    CancelPreparation(SkillsCancelPreparationRequest),
    InstallLocal(LocalSkillInstallRequest),
    UpdateLocal(LocalSkillUpdateRequest),
    Uninstall(ParsedSkillUninstallRequest),
}

pub(crate) enum ParsedSkillUninstallRequest {
    Exact(SkillUninstallExactRequest),
    Legacy(SkillUninstallRequest),
}

impl ParsedSkillUninstallRequest {
    pub(crate) fn skill_id(&self) -> &SkillId {
        match self {
            Self::Exact(request) => request.skill_id(),
            Self::Legacy(request) => request.skill_id(),
        }
    }
}

impl ParsedSkillsRequest {
    pub(crate) fn uses_acquisition_lane(&self) -> bool {
        matches!(
            self.operation,
            ParsedSkillsOperation::InspectInstallation(_)
                | ParsedSkillsOperation::ResolveInstallationSource(_)
                | ParsedSkillsOperation::CancelSourceResolution(_)
        )
    }

    pub(crate) fn is_source_resolution(&self) -> bool {
        matches!(
            self.operation,
            ParsedSkillsOperation::ResolveInstallationSource(_)
                | ParsedSkillsOperation::CancelSourceResolution(_)
        )
    }

    pub(crate) fn workflow_metadata(
        &self,
    ) -> Option<(mycopilot_protocol_rs::SkillInspectionPhaseDto, String)> {
        match &self.operation {
            ParsedSkillsOperation::InspectInstallation(request) => Some((
                mycopilot_protocol_rs::SkillInspectionPhaseDto::Inspect,
                request.preparation_id.clone(),
            )),
            ParsedSkillsOperation::CommitInstallation(request) => Some((
                mycopilot_protocol_rs::SkillInspectionPhaseDto::Commit,
                request.preparation_id.clone(),
            )),
            ParsedSkillsOperation::CancelPreparation(request) => Some((
                mycopilot_protocol_rs::SkillInspectionPhaseDto::Cancel,
                request.preparation_id.clone(),
            )),
            _ => None,
        }
    }

    pub(crate) fn workflow_commit_preparation_id(&self) -> Option<String> {
        match &self.operation {
            ParsedSkillsOperation::CommitInstallation(request) => {
                Some(request.preparation_id.clone())
            }
            _ => None,
        }
    }

    pub(crate) fn mutation_metadata(
        &self,
    ) -> Option<(SkillInstallationOperation, SkillMutationTarget)> {
        match &self.operation {
            ParsedSkillsOperation::List(_)
            | ParsedSkillsOperation::ListManagement(_)
            | ParsedSkillsOperation::SetEnabled(_)
            | ParsedSkillsOperation::InspectInstallation(_)
            | ParsedSkillsOperation::ResolveInstallationSource(_)
            | ParsedSkillsOperation::CancelSourceResolution(_)
            | ParsedSkillsOperation::CommitInstallation(_)
            | ParsedSkillsOperation::CancelPreparation(_) => None,
            ParsedSkillsOperation::InstallLocal(request) => Some((
                SkillInstallationOperation::Install,
                SkillMutationTarget::InstallationId(request.installation_id().clone()),
            )),
            ParsedSkillsOperation::UpdateLocal(request) => Some((
                SkillInstallationOperation::Update,
                SkillMutationTarget::SkillId(request.skill_id().clone()),
            )),
            ParsedSkillsOperation::Uninstall(request) => Some((
                SkillInstallationOperation::Uninstall,
                SkillMutationTarget::SkillId(request.skill_id().clone()),
            )),
        }
    }
}

pub(crate) fn parse_skills_request(request: JsonRpcRequest) -> Result<ParsedSkillsRequest, Value> {
    let id = request.id;
    if request.jsonrpc != "2.0" {
        return Err(response_error(Some(id), -32600, "Invalid JSON-RPC version"));
    }

    let operation = match request.method.as_str() {
        SKILLS_LIST_METHOD => ParsedSkillsOperation::List(
            parse_params::<SkillsListRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?,
        ),
        SKILLS_LIST_MANAGEMENT_METHOD => ParsedSkillsOperation::ListManagement(
            parse_params::<SkillsListManagementRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?,
        ),
        SKILLS_SET_ENABLED_METHOD => ParsedSkillsOperation::SetEnabled(
            parse_params::<SkillsSetEnabledRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?,
        ),
        SKILLS_INSPECT_INSTALLATION_METHOD => ParsedSkillsOperation::InspectInstallation(
            parse_params::<SkillsInspectInstallationRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?,
        ),
        SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD => {
            ParsedSkillsOperation::ResolveInstallationSource(
                parse_params::<SkillsResolveInstallationSourceRequest>(request.params)
                    .map_err(|message| response_error(Some(id.clone()), -32602, message))?,
            )
        }
        SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD => ParsedSkillsOperation::CancelSourceResolution(
            parse_params::<SkillsCancelSourceResolutionRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?,
        ),
        SKILLS_COMMIT_INSTALLATION_METHOD => ParsedSkillsOperation::CommitInstallation(
            parse_params::<SkillsCommitInstallationRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?,
        ),
        SKILLS_CANCEL_PREPARATION_METHOD => ParsedSkillsOperation::CancelPreparation(
            parse_params::<SkillsCancelPreparationRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?,
        ),
        SKILLS_INSTALL_LOCAL_METHOD => {
            let input = parse_params::<SkillsInstallLocalRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?;
            let installation_id = SkillInstallationId::parse(input.installation_id)
                .map_err(|error| invalid_skill_installation_params(&id, error.to_string()))?;
            let directory = parse_absolute_skill_directory(&id, input.directory)?;
            ParsedSkillsOperation::InstallLocal(LocalSkillInstallRequest::new(
                installation_id,
                directory,
            ))
        }
        SKILLS_UPDATE_LOCAL_METHOD => {
            let input = parse_params::<SkillsUpdateLocalRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?;
            let skill_id = SkillId::parse(input.skill_id)
                .map_err(|error| invalid_skill_installation_params(&id, error.to_string()))?;
            let expected_revision = SkillRevision::parse(input.expected_revision)
                .map_err(|error| invalid_skill_installation_params(&id, error.to_string()))?;
            let directory = parse_absolute_skill_directory(&id, input.directory)?;
            ParsedSkillsOperation::UpdateLocal(LocalSkillUpdateRequest::new(
                skill_id,
                expected_revision,
                directory,
            ))
        }
        SKILLS_UNINSTALL_METHOD => {
            let input = parse_params::<SkillsUninstallRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?;
            let skill_id = SkillId::parse(input.skill_id)
                .map_err(|error| invalid_skill_installation_params(&id, error.to_string()))?;
            let uninstall = if input
                .expected_revision
                .starts_with(SKILL_INSTALLATION_REVISION_PREFIX)
            {
                let expected_revision = SkillInstallationRevision::parse(input.expected_revision)
                    .map_err(|error| {
                    invalid_skill_installation_params(&id, error.to_string())
                })?;
                ParsedSkillUninstallRequest::Exact(SkillUninstallExactRequest::new(
                    skill_id,
                    expected_revision,
                ))
            } else {
                let expected_revision = SkillRevision::parse(input.expected_revision)
                    .map_err(|error| invalid_skill_installation_params(&id, error.to_string()))?;
                ParsedSkillUninstallRequest::Legacy(SkillUninstallRequest::new(
                    skill_id,
                    expected_revision,
                ))
            };
            ParsedSkillsOperation::Uninstall(uninstall)
        }
        _ => return Err(response_error(Some(id), -32601, "Method not found")),
    };
    Ok(ParsedSkillsRequest { id, operation })
}

pub(crate) fn invalid_skill_installation_params(id: &JsonRpcId, reason: String) -> Value {
    response_error(
        Some(id.clone()),
        -32602,
        format!("Invalid params: {reason}"),
    )
}

pub(crate) fn parse_absolute_skill_directory(
    id: &JsonRpcId,
    value: String,
) -> Result<PathBuf, Value> {
    let directory = PathBuf::from(value);
    if !directory.is_absolute() {
        return Err(invalid_skill_installation_params(
            id,
            "Skill directory must be an absolute path".to_string(),
        ));
    }
    Ok(directory)
}

#[cfg(test)]
pub(crate) fn handle_skills_request(
    storage: &StorageService,
    skills_service: &SkillsService,
    skill_installation_service: &SkillInstallationService,
    request: JsonRpcRequest,
) -> Value {
    match parse_skills_request(request) {
        Ok(request) => handle_parsed_skills_request(
            storage,
            skills_service,
            skill_installation_service,
            None,
            None,
            None,
            request,
        ),
        Err(response) => response,
    }
}

pub(crate) fn handle_parsed_skills_request(
    storage: &StorageService,
    skills_service: &SkillsService,
    skill_installation_service: &SkillInstallationService,
    skill_installation_workflow: Option<&SkillInstallationWorkflow>,
    skill_source_resolution: Option<&SkillSourceResolutionService>,
    notification_tx: Option<&mpsc::UnboundedSender<Value>>,
    request: ParsedSkillsRequest,
) -> Value {
    match request.operation {
        ParsedSkillsOperation::List(input) => {
            let result = match input.project_id.as_deref() {
                Some(project_id) => {
                    resolve_project_path(storage, project_id).and_then(|workspace| {
                        skills_service
                            .list_with_workspace(project_id, &workspace)
                            .map_err(|error| error.to_string())
                    })
                }
                None => skills_service.list().map_err(|error| error.to_string()),
            };
            match result.and_then(|catalog| enabled_catalog_response(storage, &catalog)) {
                Ok(catalog) => response_success(request.id, catalog),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        ParsedSkillsOperation::ListManagement(input) => {
            // Reserved for a future project-scoped management extension. The
            // settings-page inventory is global in schema v1.
            let _ = input.project_id;
            let result = skills_service
                .list()
                .map_err(|_| SkillManagementFailure::list_unavailable())
                .and_then(|catalog| {
                    management_response(
                        storage,
                        &catalog,
                        skill_installation_service,
                        skill_installation_workflow,
                    )
                });
            match result {
                Ok(response) => response_success(request.id, response),
                Err(error) => skill_management_error_response(request.id, error),
            }
        }
        ParsedSkillsOperation::SetEnabled(input) => {
            let result = skills_service
                .list()
                .map_err(|_| SkillManagementFailure::set_enabled_unavailable())
                .and_then(|catalog| {
                    set_enabled_response(
                        storage,
                        &catalog,
                        skill_installation_service,
                        skill_installation_workflow,
                        &input,
                    )
                });
            match result {
                Ok((response, changed)) => {
                    if changed {
                        notify_skills_changed(
                            storage,
                            skills_service,
                            skill_installation_service,
                            skill_installation_workflow,
                            notification_tx,
                            SkillsChangedReasonDto::EnablementChanged,
                            Some(input.skill_id),
                        );
                    }
                    response_success(request.id, response)
                }
                Err(error) => skill_management_error_response(request.id, error),
            }
        }
        ParsedSkillsOperation::InspectInstallation(input) => {
            let Some(workflow) = skill_installation_workflow else {
                return response_error(
                    Some(request.id),
                    -32603,
                    "Skill installation workflow is unavailable.",
                );
            };
            let result = preparation_request(input)
                .and_then(|request| {
                    workflow.inspect(&request).map_err(|error| {
                        workflow_failure(
                            mycopilot_protocol_rs::SkillInspectionPhaseDto::Inspect,
                            &error,
                        )
                    })
                })
                .and_then(|preview| preview_response(&preview));
            match result {
                Ok(preview) => response_success(request.id, preview),
                Err(error) => skill_inspection_error_response(request.id, error),
            }
        }
        ParsedSkillsOperation::ResolveInstallationSource(input) => {
            let Some(service) = skill_source_resolution else {
                return skill_source_resolution_error_response(
                    request.id,
                    resolution_dispatch_failure("Skill source resolution is unavailable."),
                );
            };
            let result = resolution_request(input)
                .and_then(|(resolution_id, locator)| {
                    service
                        .resolve_registered(resolution_id, &locator)
                        .map_err(resolution_failure)
                })
                .and_then(|registered| resolution_response(&registered));
            match result {
                Ok(response) => response_success(request.id, response),
                Err(error) => skill_source_resolution_error_response(request.id, error),
            }
        }
        ParsedSkillsOperation::CancelSourceResolution(input) => {
            let Some(service) = skill_source_resolution else {
                return skill_source_resolution_error_response(
                    request.id,
                    resolution_dispatch_failure("Skill source resolution is unavailable."),
                );
            };
            let result = cancellation_resolution_id(input).and_then(|resolution_id| {
                let outcome = service
                    .cancel_registered_resolution(&resolution_id)
                    .map_err(resolution_failure)?;
                source_resolution_cancellation_response(&resolution_id, outcome)
            });
            match result {
                Ok(response) => response_success(request.id, response),
                Err(error) => skill_source_resolution_error_response(request.id, error),
            }
        }
        ParsedSkillsOperation::CommitInstallation(input) => {
            let Some(workflow) = skill_installation_workflow else {
                return response_error(
                    Some(request.id),
                    -32603,
                    "Skill installation workflow is unavailable.",
                );
            };
            let result = commit_request(input).and_then(|commit| {
                workflow.commit(&commit).map_err(|error| {
                    workflow_failure(
                        mycopilot_protocol_rs::SkillInspectionPhaseDto::Commit,
                        &error,
                    )
                })
            });
            match result.and_then(|result| {
                let response = commit_response(&result)?;
                if !result.replayed() {
                    let reason = match result.mutation().operation() {
                        SkillInstallationOperation::Install => {
                            Some(SkillsChangedReasonDto::Installed)
                        }
                        SkillInstallationOperation::Update => Some(SkillsChangedReasonDto::Updated),
                        _ => None,
                    };
                    if let Some(reason) = reason {
                        notify_skills_changed(
                            storage,
                            skills_service,
                            skill_installation_service,
                            skill_installation_workflow,
                            notification_tx,
                            reason,
                            Some(result.mutation().skill_id().as_str().to_string()),
                        );
                    }
                }
                Ok(response)
            }) {
                Ok(response) => response_success(request.id, response),
                Err(error) => skill_workflow_commit_error_response_with_invalidation(
                    storage,
                    skills_service,
                    skill_installation_service,
                    skill_installation_workflow,
                    notification_tx,
                    request.id,
                    error,
                ),
            }
        }
        ParsedSkillsOperation::CancelPreparation(input) => {
            let Some(workflow) = skill_installation_workflow else {
                return response_error(
                    Some(request.id),
                    -32603,
                    "Skill installation workflow is unavailable.",
                );
            };
            let preparation_id = match cancellation_preparation_id(input) {
                Ok(preparation_id) => preparation_id,
                Err(error) => return skill_inspection_error_response(request.id, error),
            };
            match workflow.cancel(&preparation_id) {
                Ok(outcome) => {
                    response_success(request.id, cancellation_response(&preparation_id, outcome))
                }
                Err(error) if is_missing_preparation(&error) => {
                    response_success(request.id, absent_cancellation_response(&preparation_id))
                }
                Err(error) => skill_inspection_error_response(
                    request.id,
                    workflow_failure(
                        mycopilot_protocol_rs::SkillInspectionPhaseDto::Cancel,
                        &error,
                    ),
                ),
            }
        }
        ParsedSkillsOperation::InstallLocal(input) => {
            let result = skill_installation_service.install_local_directory(&input);
            if matches!(
                result.as_ref().map(SkillInstallationMutation::outcome),
                Ok(mycopilot_core::skills::SkillInstallationOutcome::Installed)
            ) {
                let skill_id = result
                    .as_ref()
                    .expect("matched successful installation")
                    .skill_id()
                    .as_str()
                    .to_string();
                notify_skills_changed(
                    storage,
                    skills_service,
                    skill_installation_service,
                    skill_installation_workflow,
                    notification_tx,
                    SkillsChangedReasonDto::Installed,
                    Some(skill_id),
                );
            }
            skill_mutation_response_with_invalidation(
                storage,
                skills_service,
                skill_installation_service,
                skill_installation_workflow,
                notification_tx,
                request.id,
                result,
            )
        }
        ParsedSkillsOperation::UpdateLocal(input) => {
            let result = skill_installation_service.update_local_directory(&input);
            if matches!(
                result.as_ref().map(SkillInstallationMutation::outcome),
                Ok(mycopilot_core::skills::SkillInstallationOutcome::Updated)
            ) {
                notify_skills_changed(
                    storage,
                    skills_service,
                    skill_installation_service,
                    skill_installation_workflow,
                    notification_tx,
                    SkillsChangedReasonDto::Updated,
                    Some(input.skill_id().as_str().to_string()),
                );
            }
            skill_mutation_response_with_invalidation(
                storage,
                skills_service,
                skill_installation_service,
                skill_installation_workflow,
                notification_tx,
                request.id,
                result,
            )
        }
        ParsedSkillsOperation::Uninstall(input) => {
            let result = match &input {
                ParsedSkillUninstallRequest::Exact(request) => {
                    skill_installation_service.uninstall_exact(request)
                }
                ParsedSkillUninstallRequest::Legacy(request) => {
                    skill_installation_service.uninstall(request)
                }
            };
            if result.is_ok() {
                // The managed-store mutation is the authoritative commit. SQLite is
                // a derived preference store: cleanup must never turn a
                // completed uninstall into a false failure. Idempotent
                // uninstall retries repeat the same reconciliation.
                best_effort_delete_skill_enablement(storage, input.skill_id().as_str());
            }
            if matches!(
                result.as_ref().map(SkillInstallationMutation::outcome),
                Ok(mycopilot_core::skills::SkillInstallationOutcome::Uninstalled)
            ) {
                notify_skills_changed(
                    storage,
                    skills_service,
                    skill_installation_service,
                    skill_installation_workflow,
                    notification_tx,
                    SkillsChangedReasonDto::Uninstalled,
                    Some(input.skill_id().as_str().to_string()),
                );
            }
            skill_mutation_response_with_invalidation(
                storage,
                skills_service,
                skill_installation_service,
                skill_installation_workflow,
                notification_tx,
                request.id,
                result,
            )
        }
    }
}

pub(crate) fn best_effort_delete_skill_enablement(storage: &StorageService, skill_id: &str) {
    // Uninstalled installation IDs are durably retired and cannot be reused,
    // so deleting their derived preference row cannot create state-token ABA.
    let _ = storage.delete_skill_enablement_override(skill_id);
}

pub(crate) fn skill_mutation_response_with_invalidation(
    storage: &StorageService,
    skills_service: &SkillsService,
    skill_installation_service: &SkillInstallationService,
    skill_installation_workflow: Option<&SkillInstallationWorkflow>,
    notification_tx: Option<&mpsc::UnboundedSender<Value>>,
    id: JsonRpcId,
    result: Result<SkillInstallationMutation, SkillInstallationServiceError>,
) -> Value {
    if let Err(error) = &result {
        if let Ok(failure) = installation_failure(error) {
            notify_skills_changed_if_commit_outcome_uncertain(
                storage,
                skills_service,
                skill_installation_service,
                skill_installation_workflow,
                notification_tx,
                failure.commit_may_have_succeeded(),
                failure.skill_id(),
            );
        }
    }
    skill_mutation_response(id, result)
}

pub(crate) fn skill_workflow_commit_error_response_with_invalidation(
    storage: &StorageService,
    skills_service: &SkillsService,
    skill_installation_service: &SkillInstallationService,
    skill_installation_workflow: Option<&SkillInstallationWorkflow>,
    notification_tx: Option<&mpsc::UnboundedSender<Value>>,
    id: JsonRpcId,
    failure: SkillInspectionFailure,
) -> Value {
    notify_skills_changed_if_commit_outcome_uncertain(
        storage,
        skills_service,
        skill_installation_service,
        skill_installation_workflow,
        notification_tx,
        failure.commit_may_have_succeeded(),
        failure.skill_id(),
    );
    skill_inspection_error_response(id, failure)
}

pub(crate) fn notify_skills_changed_if_commit_outcome_uncertain(
    storage: &StorageService,
    skills_service: &SkillsService,
    skill_installation_service: &SkillInstallationService,
    skill_installation_workflow: Option<&SkillInstallationWorkflow>,
    notification_tx: Option<&mpsc::UnboundedSender<Value>>,
    commit_may_have_succeeded: bool,
    skill_id: Option<&str>,
) {
    if !commit_may_have_succeeded {
        return;
    }
    // This is an invalidation, not a success claim. The durable store must be
    // re-read because a post-publication fsync failure cannot prove whether the
    // requested mutation became visible.
    notify_skills_changed(
        storage,
        skills_service,
        skill_installation_service,
        skill_installation_workflow,
        notification_tx,
        SkillsChangedReasonDto::CatalogChanged,
        skill_id.map(str::to_string),
    );
}

pub(crate) fn notify_skills_changed(
    storage: &StorageService,
    skills_service: &SkillsService,
    skill_installation_service: &SkillInstallationService,
    skill_installation_workflow: Option<&SkillInstallationWorkflow>,
    notification_tx: Option<&mpsc::UnboundedSender<Value>>,
    reason: SkillsChangedReasonDto,
    skill_id: Option<String>,
) {
    let Some(notification_tx) = notification_tx else {
        return;
    };
    let management_revision = skills_service
        .list()
        .ok()
        .and_then(|catalog| {
            management_response(
                storage,
                &catalog,
                skill_installation_service,
                skill_installation_workflow,
            )
            .ok()
        })
        .map(|response| response.management_revision)
        .unwrap_or_else(|| "unavailable".to_string());
    let notification = SkillsChangedNotification {
        schema_version: SKILL_MANAGEMENT_SCHEMA_VERSION,
        management_revision,
        reason,
        skill_id,
    };
    let _ = notification_tx.send(json!({
        "jsonrpc": "2.0",
        "method": SKILLS_CHANGED_NOTIFICATION_METHOD,
        "params": notification,
    }));
}

pub(crate) fn skill_mutation_response(
    id: JsonRpcId,
    result: Result<SkillInstallationMutation, SkillInstallationServiceError>,
) -> Value {
    match result {
        Ok(mutation) => match mutation_response(&mutation) {
            Ok(response) => response_success(id, response),
            Err(message) => response_error(Some(id), -32603, message),
        },
        Err(error) => match installation_failure(&error) {
            Ok(failure) => {
                let message = failure.to_string();
                serde_json::to_value(error_with_data(
                    Some(id),
                    SKILL_INSTALLATION_ERROR_CODE,
                    message,
                    serde_json::to_value(failure.into_data())
                        .expect("Skill installation error data must serialize"),
                ))
                .expect("JSON-RPC Skill installation error response must serialize")
            }
            Err(mapping_error) => response_error(Some(id), -32603, mapping_error),
        },
    }
}

pub(crate) fn skill_management_error_response(
    id: JsonRpcId,
    failure: SkillManagementFailure,
) -> Value {
    let message = failure.to_string();
    serde_json::to_value(error_with_data(
        Some(id),
        SKILL_MANAGEMENT_ERROR_CODE,
        message,
        serde_json::to_value(failure.into_data())
            .expect("Skill management error data must serialize"),
    ))
    .expect("JSON-RPC Skill management error response must serialize")
}

pub(crate) fn skill_inspection_error_response(
    id: JsonRpcId,
    failure: SkillInspectionFailure,
) -> Value {
    let message = failure.to_string();
    serde_json::to_value(error_with_data(
        Some(id),
        SKILL_INSPECTION_ERROR_CODE,
        message,
        serde_json::to_value(failure.into_data())
            .expect("Skill inspection error data must serialize"),
    ))
    .expect("JSON-RPC Skill inspection error response must serialize")
}

pub(crate) fn skill_source_resolution_error_response(
    id: JsonRpcId,
    failure: SkillSourceResolutionFailure,
) -> Value {
    let message = failure.to_string();
    serde_json::to_value(error_with_data(
        Some(id),
        SKILL_SOURCE_RESOLUTION_ERROR_CODE,
        message,
        serde_json::to_value(failure.into_data())
            .expect("Skill source resolution error data must serialize"),
    ))
    .expect("JSON-RPC Skill source resolution error response must serialize")
}
