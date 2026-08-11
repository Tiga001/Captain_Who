use super::*;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex as StdMutex;
use tokio::task::JoinSet;

pub(crate) const DEFAULT_MAX_CONCURRENT_IMAGE_ARTIFACT_READS: usize = 2;
pub(crate) const DEFAULT_MAX_CONCURRENT_MCP_MANAGEMENT_REQUESTS: usize = 16;

/// Owns every asynchronous MCP management request accepted by the stdio request loop.
///
/// The semaphore bounds concurrent work, while this tracker gives shutdown an explicit owner for
/// already-admitted tasks. Completed tasks are reaped on admission, so a long-running Host does
/// not accumulate detached `JoinHandle`s. Shutdown first closes admission, then gives accepted
/// requests a bounded grace period before aborting and joining any remainder.
pub(crate) struct McpManagementRequestTracker {
    accepting: AtomicBool,
    tasks: StdMutex<JoinSet<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct McpManagementRequestShutdown {
    pub(crate) forced: bool,
    pub(crate) task_failures: usize,
}

impl McpManagementRequestTracker {
    pub(crate) fn new() -> Self {
        Self {
            accepting: AtomicBool::new(true),
            tasks: StdMutex::new(JoinSet::new()),
        }
    }

    pub(crate) fn try_spawn<F>(&self, task: F) -> Result<(), F>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if !self.accepting.load(Ordering::Acquire) {
            return Err(task);
        }
        let mut tasks = self.tasks.lock().unwrap_or_else(|error| error.into_inner());
        while tasks.try_join_next().is_some() {}
        if !self.accepting.load(Ordering::Acquire) {
            return Err(task);
        }
        tasks.spawn(task);
        Ok(())
    }

    pub(crate) async fn shutdown(&self, grace: Duration) -> McpManagementRequestShutdown {
        self.accepting.store(false, Ordering::Release);
        let mut tasks = {
            let mut owned = self.tasks.lock().unwrap_or_else(|error| error.into_inner());
            std::mem::replace(&mut *owned, JoinSet::new())
        };
        let deadline = tokio::time::Instant::now() + grace;
        let mut forced = false;
        let mut task_failures = 0_usize;
        while !tasks.is_empty() {
            match tokio::time::timeout_at(deadline, tasks.join_next()).await {
                Ok(Some(Ok(()))) => {}
                Ok(Some(Err(_))) => task_failures = task_failures.saturating_add(1),
                Ok(None) => break,
                Err(_) => {
                    forced = true;
                    tasks.abort_all();
                    while let Some(result) = tasks.join_next().await {
                        if result.is_err() {
                            task_failures = task_failures.saturating_add(1);
                        }
                    }
                }
            }
        }
        McpManagementRequestShutdown {
            forced,
            task_failures,
        }
    }
}

pub(crate) struct ImageArtifactOutbound {
    pub(crate) message: Value,
    // Held until `run_outbound_writer` has flushed the complete base64 response. This bounds both
    // active decoding and large Values waiting behind stdout backpressure.
    pub(crate) _permit: tokio::sync::OwnedSemaphorePermit,
}

pub(crate) struct RequestDispatchers<'a> {
    pub(crate) git: &'a GitDispatcher,
    pub(crate) skills: &'a SkillsDispatcher,
    pub(crate) skill_acquisition: &'a SkillsDispatcher,
    pub(crate) image_generation_configuration: &'a ImageGenerationConfigurationDispatcher,
}

pub(crate) struct RequestOutbounds<'a> {
    pub(crate) normal: &'a mpsc::UnboundedSender<Value>,
    pub(crate) image_artifact: &'a mpsc::Sender<ImageArtifactOutbound>,
}

/// Process-wide services used directly by JSON-RPC request handlers.
///
/// Keep these dependencies grouped so new configuration domains do not turn the request-loop
/// signature into a growing service locator. Bounded filesystem dispatchers remain explicit
/// because they have independent admission and shutdown lifecycles.
#[derive(Clone)]
pub(crate) struct CoreRequestServices {
    pub(crate) storage: Arc<StorageService>,
    pub(crate) image_generation_configuration: Arc<ImageGenerationConfigurationService>,
    pub(crate) image_generation_artifacts: Arc<ManagedImageGenerationArtifactStore>,
    pub(crate) image_generation_artifact_read_admission: Arc<Semaphore>,
    pub(crate) mcp_management:
        Option<Arc<crate::application::mcp::management::McpManagementService>>,
    pub(crate) mcp_management_admission: Arc<Semaphore>,
    pub(crate) mcp_management_tasks: Arc<McpManagementRequestTracker>,
}

fn is_blocking_read_method(method: &str) -> bool {
    matches!(
        method,
        AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD
            | AGENT_COMMAND_SESSIONS_LIST_METHOD
            | AGENT_COMMAND_SESSIONS_GET_METHOD
            | AGENT_LIST_PENDING_ACTIONS_METHOD
            | AGENT_GET_USAGE_SUMMARY_METHOD
            | AGENT_READ_FILE_DRAFT_METHOD
            | AGENT_GET_FILE_WRITE_DIFF_METHOD
            | SEARCH_SEARCH_CHATS_METHOD
            | STORAGE_LOAD_MODEL_SETTINGS_METHOD
            | STORAGE_LOAD_PROVIDER_PROFILE_UI_DESCRIPTORS_METHOD
            | STORAGE_LOAD_AGENT_PROMPT_PREFERENCES_METHOD
            | STORAGE_LOAD_PROJECTS_METHOD
            | STORAGE_LOAD_CONVERSATIONS_METHOD
            | STORAGE_LOAD_CONVERSATION_METAS_METHOD
            | STORAGE_LOAD_CONVERSATION_METHOD
            | STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD
            | STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD
            | STORAGE_LOAD_COMPOSER_DRAFTS_METHOD
            | STORAGE_LOAD_UI_PREFERENCES_METHOD
    )
}

pub(crate) async fn run_request_loop<R>(
    input: R,
    services: CoreRequestServices,
    agent_service: &AgentService,
    skill_services: SkillServices,
    git_review_service: Arc<GitReviewService>,
    dispatchers: &RequestDispatchers<'_>,
    outbounds: RequestOutbounds<'_>,
) -> io::Result<Option<JsonRpcId>>
where
    R: AsyncBufRead + Unpin,
{
    let storage = services.storage;
    let image_generation_configuration = services.image_generation_configuration;
    let image_generation_artifacts = services.image_generation_artifacts;
    let image_generation_artifact_read_admission =
        services.image_generation_artifact_read_admission;
    let mcp_management = services.mcp_management;
    let mcp_management_admission = services.mcp_management_admission;
    let mcp_management_tasks = services.mcp_management_tasks;
    let outbound = outbounds.normal;
    let image_artifact_outbound = outbounds.image_artifact;
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
            if is_mcp_management_method(&request.method) {
                let request_id = request.id.clone();
                let request_method = request.method.clone();
                let Some(request_service) = mcp_management.as_ref().cloned() else {
                    enqueue_outbound(
                        outbound,
                        mcp_management_unavailable_response(request_id, &request.method),
                    )?;
                    continue;
                };
                let permit = match Arc::clone(&mcp_management_admission).try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        enqueue_outbound(
                            outbound,
                            mcp_management_unavailable_response(request_id, &request.method),
                        )?;
                        continue;
                    }
                };
                let request_outbound = outbound.clone();
                let request_task = async move {
                    let _permit = permit;
                    let response = handle_mcp_management_request(request_service, request).await;
                    let _ = enqueue_outbound(&request_outbound, response);
                };
                if mcp_management_tasks.try_spawn(request_task).is_err() {
                    enqueue_outbound(
                        outbound,
                        mcp_management_unavailable_response(request_id, &request_method),
                    )?;
                }
                continue;
            }
            if request.method == IMAGE_GENERATION_READ_ARTIFACT_METHOD {
                let request_id = request.id.clone();
                let permit = match Arc::clone(&image_generation_artifact_read_admission)
                    .try_acquire_owned()
                {
                    Ok(permit) => permit,
                    Err(_) => {
                        enqueue_outbound(
                            outbound,
                            image_generation_artifact_error_response(
                                request_id,
                                mycopilot_protocol_rs::ImageGenerationArtifactErrorCodeDto::Unavailable,
                            ),
                        )?;
                        continue;
                    }
                };
                let request_store = Arc::clone(&image_generation_artifacts);
                let request_storage = Arc::clone(&storage);
                let request_outbound = image_artifact_outbound.clone();
                tokio::spawn(async move {
                    let response = handle_image_generation_artifact_request(
                        request_store,
                        request_storage,
                        request,
                    )
                    .await;
                    let _ = request_outbound
                        .send(ImageArtifactOutbound {
                            message: response,
                            _permit: permit,
                        })
                        .await;
                });
                continue;
            }
            if let Some(operation) = image_generation_configuration_operation(&request.method) {
                let request_id = request.id.clone();
                let request_service = Arc::clone(&image_generation_configuration);
                let kind = ImageGenerationConfigurationJobKind::for_operation(operation);
                if dispatchers
                    .image_generation_configuration
                    .try_submit(request_id.clone(), kind, move || {
                        handle_image_generation_configuration_request(&request_service, request)
                    })
                    .is_err()
                {
                    enqueue_outbound(
                        outbound,
                        image_generation_configuration_error_response(
                            request_id,
                            operation,
                            mycopilot_core::image_generation::ImageGenerationConfigurationError::Unavailable,
                        ),
                    )?;
                }
                continue;
            }
            if request.method == OFFICE_GET_STATUS_METHOD {
                let request_outbound = outbound.clone();
                let request_service = agent_service.clone();
                tokio::spawn(async move {
                    let request_id = request.id.clone();
                    let response = match tokio::task::spawn_blocking(move || {
                        handle_office_status_request(&request_service, request)
                    })
                    .await
                    {
                        Ok(response) => response,
                        Err(error) => response_error(
                            Some(request_id),
                            -32000,
                            format!("Office status probe task failed: {error}"),
                        ),
                    };
                    let _ = enqueue_outbound(&request_outbound, response);
                });
                continue;
            }
            if is_blocking_read_method(&request.method) {
                let request_id = request.id.clone();
                let request_storage = Arc::clone(&storage);
                let request_service = agent_service.clone();
                let request_outbound = outbound.clone();
                let request_notifications = request_outbound.clone();
                tokio::spawn(async move {
                    let response = match tokio::task::spawn_blocking(move || {
                        handle_request(
                            &request_storage,
                            &request_service,
                            request_notifications,
                            request,
                        )
                    })
                    .await
                    {
                        Ok(response) => response,
                        Err(error) => response_error(
                            Some(request_id),
                            -32000,
                            format!("Storage read task failed: {error}"),
                        ),
                    };
                    let _ = enqueue_outbound(&request_outbound, response);
                });
                continue;
            }
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

#[cfg(test)]
mod mcp_management_task_tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_joins_completed_management_requests_and_closes_admission() {
        let tracker = McpManagementRequestTracker::new();
        assert!(
            tracker.try_spawn(async {}).is_ok(),
            "open tracker accepts a bounded request"
        );

        let report = tracker.shutdown(Duration::from_secs(1)).await;
        assert_eq!(
            report,
            McpManagementRequestShutdown {
                forced: false,
                task_failures: 0,
            }
        );
        assert!(
            tracker.try_spawn(async {}).is_err(),
            "shutdown permanently closes management admission"
        );
    }

    #[tokio::test]
    async fn shutdown_aborts_and_joins_a_request_that_exceeds_the_grace_period() {
        let tracker = McpManagementRequestTracker::new();
        let (entered_tx, entered_rx) = oneshot::channel();
        assert!(
            tracker
                .try_spawn(async move {
                    let _ = entered_tx.send(());
                    std::future::pending::<()>().await;
                })
                .is_ok(),
            "open tracker accepts a bounded request"
        );
        entered_rx
            .await
            .expect("management request entered its pending phase");

        let report = tracker.shutdown(Duration::ZERO).await;
        assert!(report.forced);
        assert_eq!(report.task_failures, 1);
    }
}

pub(crate) async fn run_outbound_writer<W>(
    mut writer: W,
    mut outbound: mpsc::UnboundedReceiver<Value>,
    mut image_artifact_outbound: mpsc::Receiver<ImageArtifactOutbound>,
    mut finish: oneshot::Receiver<()>,
) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut outbound_open = true;
    let mut image_artifact_outbound_open = true;
    loop {
        tokio::select! {
            _ = &mut finish => {
                // Closing preserves already queued messages but prevents lingering agent tasks
                // from keeping shutdown open or appending notifications after the final response.
                outbound.close();
                image_artifact_outbound.close();
                // Release the largest retained Values first. The dedicated lane is capped at two,
                // so this cannot starve the normal queue during shutdown.
                while let Some(message) = image_artifact_outbound.recv().await {
                    write_outbound_message(&mut writer, message.message).await?;
                }
                while let Some(message) = outbound.recv().await {
                    write_outbound_message(&mut writer, message).await?;
                }
                return Ok(());
            }
            message = image_artifact_outbound.recv(), if image_artifact_outbound_open => {
                match message {
                    Some(message) => {
                        write_outbound_message(&mut writer, message.message).await?;
                    }
                    None => {
                        image_artifact_outbound_open = false;
                        if !outbound_open {
                            return Ok(());
                        }
                    }
                }
            }
            message = outbound.recv(), if outbound_open => {
                match message {
                    Some(message) => {
                        write_outbound_message(&mut writer, message).await?;
                    }
                    None => {
                        outbound_open = false;
                        if !image_artifact_outbound_open {
                            return Ok(());
                        }
                    }
                }
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
        GIT_INSPECT_REPOSITORY_METHOD
        | GIT_GET_REVIEW_SUMMARY_METHOD
        | GIT_GET_TURN_DIFF_SUMMARIES_METHOD => Some(GitJobPriority::Medium),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_reads_use_the_non_blocking_dispatch_path() {
        assert!(is_blocking_read_method(
            STORAGE_LOAD_CONVERSATION_METAS_METHOD
        ));
        assert!(is_blocking_read_method(STORAGE_LOAD_CONVERSATION_METHOD));
        assert!(is_blocking_read_method(STORAGE_LOAD_UI_PREFERENCES_METHOD));
        assert!(is_blocking_read_method(STORAGE_LOAD_COMPOSER_DRAFTS_METHOD));
        assert!(is_blocking_read_method(AGENT_COMMAND_SESSIONS_LIST_METHOD));
        assert!(is_blocking_read_method(AGENT_COMMAND_SESSIONS_GET_METHOD));
        assert!(!is_blocking_read_method(
            STORAGE_SAVE_CONVERSATION_META_METHOD
        ));
        assert!(!is_blocking_read_method(STORAGE_DELETE_CONVERSATION_METHOD));
    }
}
