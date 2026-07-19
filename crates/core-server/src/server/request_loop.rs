use super::*;

pub(crate) struct RequestDispatchers<'a> {
    pub(crate) git: &'a GitDispatcher,
    pub(crate) skills: &'a SkillsDispatcher,
    pub(crate) skill_acquisition: &'a SkillsDispatcher,
}

pub(crate) async fn run_request_loop<R>(
    input: R,
    storage: Arc<StorageService>,
    agent_service: &AgentService,
    skill_services: SkillServices,
    git_review_service: Arc<GitReviewService>,
    dispatchers: &RequestDispatchers<'_>,
    outbound: &mpsc::UnboundedSender<Value>,
) -> io::Result<Option<JsonRpcId>>
where
    R: AsyncBufRead + Unpin,
{
    let mut lines = input.lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(request) => request,
            Err(error) => {
                enqueue_outbound(
                    outbound,
                    serde_json::to_value(mycopilot_protocol_rs::error(
                        None,
                        -32700,
                        format!("Parse error: {error}"),
                    ))
                    .expect("JSON-RPC parse error response must serialize"),
                )?;
                continue;
            }
        };

        if request.jsonrpc == "2.0" && request.method == CORE_SHUTDOWN_METHOD {
            return Ok(Some(request.id));
        }

        if request.jsonrpc == "2.0" {
            if let Some(priority) = git_request_priority(&request.method) {
                let request_id = request.id.clone();
                let request_storage = Arc::clone(&storage);
                let request_service = Arc::clone(&git_review_service);
                if let Err(error) =
                    dispatchers
                        .git
                        .try_submit(priority, request_id.clone(), move || {
                            handle_git_request(&request_storage, &request_service, request)
                        })
                {
                    enqueue_outbound(
                        outbound,
                        response_error(Some(request_id), error.code(), error.message()),
                    )?;
                }
                continue;
            }
            if is_skills_method(&request.method) {
                let request_id = request.id.clone();
                let request = match parse_skills_request(request) {
                    Ok(request) => request,
                    Err(response) => {
                        enqueue_outbound(outbound, response)?;
                        continue;
                    }
                };
                let request_storage = Arc::clone(&storage);
                let request_catalog = Arc::clone(&skill_services.catalog);
                let request_installations = Arc::clone(&skill_services.installations);
                let request_workflow = Arc::clone(&skill_services.workflow);
                let request_source_resolution = Arc::clone(&skill_services.source_resolution);
                let request_outbound = outbound.clone();
                let acquisition_lane = request.uses_acquisition_lane();
                let source_resolution_request = request.is_source_resolution();
                let workflow_metadata = request.workflow_metadata();
                let workflow_commit_preparation_id = request.workflow_commit_preparation_id();
                let mutation_metadata = request.mutation_metadata();
                let submit_result = if let Some(preparation_id) = workflow_commit_preparation_id {
                    dispatchers.skills.try_submit_workflow_commit(
                        request_id.clone(),
                        preparation_id,
                        move || {
                            handle_parsed_skills_request(
                                &request_storage,
                                &request_catalog,
                                &request_installations,
                                Some(&request_workflow),
                                Some(&request_source_resolution),
                                Some(&request_outbound),
                                request,
                            )
                        },
                    )
                } else {
                    match mutation_metadata.clone() {
                        Some((operation, target)) => dispatchers.skills.try_submit_mutation(
                            request_id.clone(),
                            operation,
                            target,
                            move || {
                                handle_parsed_skills_request(
                                    &request_storage,
                                    &request_catalog,
                                    &request_installations,
                                    Some(&request_workflow),
                                    Some(&request_source_resolution),
                                    Some(&request_outbound),
                                    request,
                                )
                            },
                        ),
                        None if acquisition_lane => dispatchers.skill_acquisition.try_submit(
                            request_id.clone(),
                            move || {
                                handle_parsed_skills_request(
                                    &request_storage,
                                    &request_catalog,
                                    &request_installations,
                                    Some(&request_workflow),
                                    Some(&request_source_resolution),
                                    Some(&request_outbound),
                                    request,
                                )
                            },
                        ),
                        None => dispatchers.skills.try_submit(request_id.clone(), move || {
                            handle_parsed_skills_request(
                                &request_storage,
                                &request_catalog,
                                &request_installations,
                                Some(&request_workflow),
                                Some(&request_source_resolution),
                                Some(&request_outbound),
                                request,
                            )
                        }),
                    }
                };
                if let Err(error) = submit_result {
                    let response = match mutation_metadata {
                        Some((operation, target)) => {
                            mutation_admission_error_response(request_id, operation, target, error)
                        }
                        None if workflow_metadata.is_some() => {
                            let (phase, preparation_id) =
                                workflow_metadata.expect("checked Skill workflow request metadata");
                            skill_inspection_error_response(
                                request_id,
                                dispatch_failure(phase, preparation_id, error.message()),
                            )
                        }
                        None if source_resolution_request => {
                            skill_source_resolution_error_response(
                                request_id,
                                resolution_dispatch_failure(error.message()),
                            )
                        }
                        None => response_error(Some(request_id), error.code(), error.message()),
                    };
                    enqueue_outbound(outbound, response)?;
                }
                continue;
            }
        }

        let response = handle_request(&storage, agent_service, outbound.clone(), request);
        enqueue_outbound(outbound, response)?;
    }
    Ok(None)
}

pub(crate) async fn run_outbound_writer<W>(
    mut writer: W,
    mut outbound: mpsc::UnboundedReceiver<Value>,
    mut finish: oneshot::Receiver<()>,
) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    loop {
        tokio::select! {
            biased;
            _ = &mut finish => {
                // Closing preserves already queued messages but prevents lingering agent tasks
                // from keeping shutdown open or appending notifications after the final response.
                outbound.close();
                while let Some(message) = outbound.recv().await {
                    write_outbound_message(&mut writer, message).await?;
                }
                return Ok(());
            }
            message = outbound.recv() => {
                let Some(message) = message else {
                    return Ok(());
                };
                write_outbound_message(&mut writer, message).await?;
            }
        }
    }
}

pub(crate) async fn write_outbound_message<W>(writer: &mut W, message: Value) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    writer.write_all(message.to_string().as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

pub(crate) fn enqueue_outbound(
    outbound: &mpsc::UnboundedSender<Value>,
    message: Value,
) -> io::Result<()> {
    outbound
        .send(message)
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "outbound writer is unavailable"))
}

pub(crate) fn git_request_priority(method: &str) -> Option<GitJobPriority> {
    match method {
        GIT_MUTATE_REVIEW_FILE_METHOD => Some(GitJobPriority::High),
        GIT_INSPECT_REPOSITORY_METHOD | GIT_GET_REVIEW_SUMMARY_METHOD => {
            Some(GitJobPriority::Medium)
        }
        GIT_GET_REVIEW_FILE_DIFF_METHOD | GIT_GET_REVIEW_FILE_CONTENT_METHOD => {
            Some(GitJobPriority::Low)
        }
        _ => None,
    }
}

pub(crate) fn is_skills_method(method: &str) -> bool {
    matches!(
        method,
        SKILLS_LIST_METHOD
            | SKILLS_INSPECT_INSTALLATION_METHOD
            | SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD
            | SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD
            | SKILLS_COMMIT_INSTALLATION_METHOD
            | SKILLS_CANCEL_PREPARATION_METHOD
            | SKILLS_INSTALL_LOCAL_METHOD
            | SKILLS_UPDATE_LOCAL_METHOD
            | SKILLS_UNINSTALL_METHOD
            | SKILLS_LIST_MANAGEMENT_METHOD
            | SKILLS_SET_ENABLED_METHOD
    )
}
