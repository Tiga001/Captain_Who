use super::*;
use std::fs::File;
use std::path::Path;

#[cfg(not(test))]
use crate::application::mcp::approval_payload_store::MCP_APPROVAL_CREDENTIAL_SERVICE;
use crate::application::mcp::approval_payload_store::{
    durable_mcp_payload_store_or_process_only, McpApprovalPayloadStore,
};
use crate::application::mcp::authorized_stdio_connector::AuthorizedMcpStdioConnector;
use crate::application::mcp::browser_risk::BrowserRiskCoordinator;
use crate::application::mcp::builtin_capability_policy::SqliteBuiltinCapabilityPolicyStore;
use crate::application::mcp::builtin_capability_runtime::HostBuiltinCapabilityProvider;
use crate::application::mcp::managed_playwright_bridge::ManagedPlaywrightMcpRuntime;
use crate::application::mcp::management::McpManagementService;
use crate::application::mcp::registry_event_sink::McpAgentRegistryEventSink;
use crate::application::mcp::sqlite_envelope_repository::SqliteMcpApprovalEnvelopeRepository;
use crate::application::mcp::sqlite_registry::SqliteMcpRegistry;
use mycopilot_core::BuiltinCapabilityRuntime;

const MCP_APPROVAL_EXPIRY_RECONCILIATION_INTERVAL: Duration = Duration::from_secs(60);
const AGENT_COLLABORATION_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const AUTOMATION_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);

struct McpApprovalExpiryReconciler {
    cancellation: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl McpApprovalExpiryReconciler {
    fn spawn(agent_service: AgentService, payload_store: Arc<dyn McpApprovalPayloadStore>) -> Self {
        let (cancellation, mut cancelled) = oneshot::channel();
        let task = tokio::spawn(async move {
            let first_tick =
                tokio::time::Instant::now() + MCP_APPROVAL_EXPIRY_RECONCILIATION_INTERVAL;
            let mut interval =
                tokio::time::interval_at(first_tick, MCP_APPROVAL_EXPIRY_RECONCILIATION_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    _ = &mut cancelled => break,
                    _ = interval.tick() => {
                        if reconcile_expired_mcp_approvals_tick(
                            &agent_service,
                            payload_store.as_ref(),
                        )
                        .is_err()
                        {
                            eprintln!("MCP approval expiry reconciliation failed safely");
                        }
                    }
                }
            }
        });
        Self {
            cancellation: Some(cancellation),
            task: Some(task),
        }
    }

    async fn shutdown(mut self) -> Result<(), String> {
        if let Some(cancellation) = self.cancellation.take() {
            let _ = cancellation.send(());
        }
        self.task
            .take()
            .expect("MCP expiry reconciler task is owned until shutdown")
            .await
            .map_err(|_| "MCP approval expiry reconciler task failed during shutdown".to_string())
    }
}

impl Drop for McpApprovalExpiryReconciler {
    fn drop(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            let _ = cancellation.send(());
        }
    }
}

fn reconcile_expired_mcp_approvals_tick(
    agent_service: &AgentService,
    payload_store: &dyn McpApprovalPayloadStore,
) -> Result<crate::application::agent::McpApprovalExpiryReconciliation, String> {
    let summary = agent_service.reconcile_expired_mcp_approvals()?;
    agent_service.reconcile_expired_builtin_capability_approvals()?;
    agent_service.reconcile_expired_builtin_mcp_tool_approvals()?;
    payload_store
        .reconcile_expired(summary.cutoff_ms)
        .map_err(|_| "failed to reconcile expired MCP approval payloads".to_string())?;
    Ok(summary)
}

pub(crate) struct CoreServerBootstrap {
    pub(crate) storage: Arc<StorageService>,
    pub(crate) image_generation_configuration: Arc<ImageGenerationConfigurationService>,
    pub(crate) image_generation_artifacts: Arc<ManagedImageGenerationArtifactStore>,
    pub(crate) image_generation_execution: Arc<ImageGenerationExecutionService>,
    pub(crate) agent_service: AgentService,
    pub(crate) skill_services: SkillServices,
    pub(crate) git_review_service: Arc<GitReviewService>,
    pub(crate) mcp_registry: Arc<SqliteMcpRegistry>,
    pub(crate) mcp_builtin_capability_policies: Arc<SqliteBuiltinCapabilityPolicyStore>,
    pub(crate) builtin_capability_runtime: BuiltinCapabilityRuntime,
    pub(crate) builtin_capability_provider: Arc<HostBuiltinCapabilityProvider>,
    pub(crate) browser_risk_coordinator: Arc<BrowserRiskCoordinator>,
    // Fields drop in declaration order. Keep this owner last so the database lock outlives every
    // service and SQLite connection above it. File-effect deletion barriers are process-local;
    // this OS lock makes one core-server the authoritative lifecycle owner for the exact DB.
    _database_instance_lock: File,
}

impl CoreServerBootstrap {
    pub(crate) fn initialize() -> io::Result<Self> {
        let database_path = absolute_path(database_path()?)?;
        let database_instance_lock = acquire_database_instance_lock(&database_path)?;
        let uses_development_credentials = uses_development_image_generation_credentials();
        let skill_store_root = skill_store_root(&database_path);
        let storage =
            Arc::new(StorageService::open(&database_path).map_err(|error| {
                io::Error::other(format!("failed to initialize storage: {error}"))
            })?);
        let mcp_registry =
            Arc::new(SqliteMcpRegistry::open(&database_path).map_err(|_| {
                io::Error::other("failed to initialize the persistent MCP Registry")
            })?);
        let mcp_builtin_capability_policies = Arc::new(
            SqliteBuiltinCapabilityPolicyStore::open(&database_path).map_err(|_| {
                io::Error::other("failed to initialize built-in MCP capability policy storage")
            })?,
        );
        let (builtin_capability_runtime, builtin_capability_provider) =
            HostBuiltinCapabilityProvider::runtime_and_provider(
                Arc::clone(&mcp_builtin_capability_policies),
                Some(Arc::clone(&storage)),
            )
            .map_err(|error| {
                io::Error::other(format!(
                    "failed to initialize built-in MCP capability runtime: {error}"
                ))
            })?;
        let browser_risk_coordinator =
            BrowserRiskCoordinator::new(builtin_capability_runtime.clone());
        let image_generation_configuration = Arc::new(ImageGenerationConfigurationService::new(
            Arc::clone(&storage),
            image_generation_credential_store(&database_path, uses_development_credentials)?,
        ));
        let provider_continuation_vault = match provider_continuation_credential_store(
            &database_path,
            uses_development_credentials,
        ) {
            Ok(credentials) => match ProviderContinuationVaultFactory::open_or_provision(
                Arc::clone(&storage),
                credentials,
            ) {
                Ok(vault) => Some(Arc::new(vault)),
                Err(error) => {
                    // Provider continuation is an optional Host capability. Runs that do not
                    // require private replay remain available; any required persist/replay path
                    // fails closed inside Core before a provider or Host side effect.
                    eprintln!(
                        "Provider continuation vault is unavailable: {}",
                        error.code()
                    );
                    None
                }
            },
            Err(_) => {
                // The credential backend error is deliberately redacted at this boundary.
                eprintln!("Provider continuation credential store is unavailable");
                None
            }
        };
        if let Err(error) = image_generation_configuration.reconcile_credentials() {
            // The service remains available so settings can expose a structured, retryable error.
            // Credential-store errors are deliberately redacted by the domain boundary.
            eprintln!("failed to reconcile image-generation credentials: {error}");
        }
        let mut image_generation_adapters = ImageGenerationAdapterRegistry::new();
        image_generation_adapters
            .register(Arc::new(SmartMlSeedreamProviderFactory::default()))
            .map_err(|error| {
                io::Error::other(format!(
                    "failed to register image-generation provider adapter: {error}"
                ))
            })?;
        let image_generation_artifacts = Arc::new(
            ManagedImageGenerationArtifactStore::new(
                image_generation_artifact_store_root(&database_path),
                ImageArtifactStoreConfig::default(),
            )
            .map_err(|error| {
                io::Error::other(format!(
                    "failed to initialize image-generation Artifact store: {error}"
                ))
            })?,
        );
        let image_generation_execution = Arc::new(
            ImageGenerationExecutionService::new(
                Arc::clone(&image_generation_configuration),
                Arc::new(image_generation_adapters),
                image_generation_artifacts.clone(),
                Arc::clone(&storage),
                ImageGenerationExecutionLimits::default(),
            )
            .map_err(|error| {
                io::Error::other(format!(
                    "failed to initialize image-generation execution service: {error}"
                ))
            })?,
        );
        let git_review_service = Arc::new(GitReviewService::new());
        let skills_service = Arc::new(
            SkillsService::new()
                .with_bundled_source()
                .and_then(|service| service.with_installed_source(skill_store_root.clone()))
                .map_err(|error| {
                    io::Error::other(format!("failed to initialize Skills: {error}"))
                })?,
        );
        let skill_installation_service = Arc::new(
            SkillInstallationService::new(skill_store_root.clone()).map_err(|error| {
                io::Error::other(format!(
                    "failed to initialize Skill installation service: {error}"
                ))
            })?,
        );
        let mut skill_installation_workflow = SkillInstallationWorkflow::new(
            SkillInstallationService::new(skill_store_root).map_err(|error| {
                io::Error::other(format!(
                    "failed to initialize Skill installation workflow: {error}"
                ))
            })?,
        );
        let github_transport: Arc<dyn GitHubAcquisitionTransport> =
            Arc::new(SharedGitHubTransport::new(Arc::new(
                ReqwestGitHubTransport::new().map_err(|error| {
                    io::Error::other(format!(
                        "failed to initialize public GitHub Skill transport: {error}"
                    ))
                })?,
            )));
        let github_acquisition = GitHubWorkflowAcquisitionAdapter::new(Arc::new(
            GitHubSkillAcquirer::with_transport(Arc::clone(&github_transport)),
        ));
        skill_installation_workflow
            .register_adapter(Arc::new(github_acquisition))
            .map_err(|error| {
                io::Error::other(format!(
                    "failed to register GitHub Skill acquisition: {error}"
                ))
            })?;
        let mut skill_source_resolution = SkillSourceResolutionService::with_session_store(
            skill_installation_workflow.session_store(),
        );
        skill_source_resolution
            .register_resolver(Arc::new(GitHubInstallationSourceResolver::new(Arc::clone(
                &github_transport,
            ))))
            .map_err(|error| {
                io::Error::other(format!(
                    "failed to register GitHub Skill source resolver: {error}"
                ))
            })?;
        let skill_installation_workflow = Arc::new(skill_installation_workflow);
        let skill_source_resolution = Arc::new(skill_source_resolution);
        let agent_skill_installation_prepare = Arc::new(
            AgentSkillInstallationInspectionAdapter::with_pending_root(
                Arc::clone(&skill_source_resolution),
                Arc::clone(&skill_installation_workflow),
                skill_installation_pending_root(&database_path),
            )
            .map_err(|error| {
                io::Error::other(format!(
                    "failed to initialize pending Skill installation store: {error}"
                ))
            })?,
        );
        let agent_service =
            AgentService::try_new_deferred_startup_reconciliation_with_provider_continuation_vault(
                storage.clone(),
                provider_continuation_vault,
            )
            .map_err(|error| {
                io::Error::other(format!("failed to initialize Agent service: {error}"))
            })?
            .with_skills_service(Arc::clone(&skills_service))
            .with_image_generation_execution(Arc::clone(&image_generation_execution))
            .with_skill_installation(
                agent_skill_installation_prepare,
                Arc::clone(&skill_installation_service),
                Arc::clone(&skill_installation_workflow),
            )
            .with_builtin_capabilities(builtin_capability_runtime.clone())
            .with_browser_risk_coordinator(Arc::clone(&browser_risk_coordinator));
        let skill_services = SkillServices {
            catalog: skills_service,
            installations: skill_installation_service,
            workflow: skill_installation_workflow,
            source_resolution: skill_source_resolution,
        };
        Ok(Self {
            storage,
            image_generation_configuration,
            image_generation_artifacts,
            image_generation_execution,
            agent_service,
            skill_services,
            git_review_service,
            mcp_registry,
            mcp_builtin_capability_policies,
            builtin_capability_runtime,
            builtin_capability_provider,
            browser_risk_coordinator,
            _database_instance_lock: database_instance_lock,
        })
    }
}

pub(crate) async fn run_core_server(bootstrap: &CoreServerBootstrap) -> io::Result<()> {
    bootstrap
        .image_generation_execution
        .reconcile_interrupted()
        .await
        .map_err(|error| {
            io::Error::other(format!(
                "failed to reconcile interrupted image-generation executions: {error}"
            ))
        })?;
    bootstrap
        .agent_service
        .reconcile_interrupted_image_generation_tool_audits()
        .await
        .map_err(|error| {
            io::Error::other(format!(
                "failed to reconcile interrupted image-generation Agent audits: {error}"
            ))
        })?;
    // The persistent Registry has already migrated and reconciled before the
    // runtime begins. The connector performs one final exact launch-spec
    // authorization check immediately before every stdio spawn.
    let mcp_registry = Arc::clone(&bootstrap.mcp_registry);
    let mcp_connector: Arc<dyn McpConnector> =
        Arc::new(AuthorizedMcpStdioConnector::new(Arc::clone(&mcp_registry)));
    let mcp_event_sink = Arc::new(McpAgentRegistryEventSink::new());
    let mcp_manager = Arc::new(
        McpConnectionManager::new(
            mcp_registry.clone(),
            mcp_connector,
            mcp_event_sink.clone(),
            McpManagerPolicy::default(),
        )
        .map_err(|_| io::Error::other("failed to initialize the MCP connection manager"))?,
    );
    let managed_playwright_runtime = ManagedPlaywrightMcpRuntime::new()
        .map_err(|_| io::Error::other("failed to initialize managed Playwright MCP runtime"))?;
    bootstrap
        .builtin_capability_provider
        .attach_managed_runtime(Arc::clone(&managed_playwright_runtime))
        .map_err(|_| io::Error::other("failed to attach managed Playwright MCP runtime"))?;
    let managed_playwright_bridge = managed_playwright_runtime.bridge();
    let mcp_management = Arc::new(
        McpManagementService::new(
            Arc::clone(&mcp_registry),
            Arc::clone(&mcp_manager),
            Arc::clone(&bootstrap.mcp_builtin_capability_policies),
            bootstrap.builtin_capability_runtime.clone(),
        )
        .with_managed_playwright_runtime(Arc::clone(&managed_playwright_runtime))
        .with_browser_risk_coordinator(Arc::clone(&bootstrap.browser_risk_coordinator)),
    );
    let mcp_payload_store = mcp_approval_payload_store(Arc::clone(&bootstrap.storage));
    let _ = mcp_payload_store.reconcile_expired(mycopilot_core::storage::now_ms());
    let mcp_bridge = Arc::new(
        McpRuntimeBridge::with_payload_store_and_registry_security_gate(
            Arc::clone(&mcp_manager),
            Arc::clone(&mcp_payload_store),
            mcp_event_sink.clone(),
        ),
    );
    let agent_service = bootstrap
        .agent_service
        .clone()
        .with_mcp_tool_invoker(mcp_bridge.clone())
        .with_mcp_startup_inspector(mcp_bridge);
    mcp_event_sink
        .bind(agent_service.clone())
        .map_err(io::Error::other)?;
    let mcp_startup_manager = Arc::clone(&mcp_manager);
    let mcp_startup = tokio::spawn(async move {
        let failures = mcp_startup_manager
            .start_enabled()
            .await
            .into_iter()
            .filter(|result| result.error.is_some())
            .count();
        if failures > 0 {
            eprintln!(
                "{failures} optional MCP server(s) failed to start; built-in Agent tools remain available"
            );
        }
    });
    agent_service
        .reconcile_startup_mcp_actions()
        .map_err(io::Error::other)?;
    agent_service
        .reconcile_expired_builtin_capability_approvals()
        .map_err(io::Error::other)?;
    agent_service
        .reconcile_expired_builtin_mcp_tool_approvals()
        .map_err(io::Error::other)?;
    agent_service
        .reconcile_startup_orphaned_conversation_traces()
        .map_err(io::Error::other)?;
    let mcp_approval_expiry_reconciler =
        McpApprovalExpiryReconciler::spawn(agent_service.clone(), mcp_payload_store);

    // Freeze the notifier cut before any request or collaboration dispatcher can mutate durable
    // state. Events committed after this read are replayed; older events are covered by the global
    // startup resync which makes every already-mounted root store rehydrate.
    let collaboration_event_startup_cursor = bootstrap
        .storage
        .latest_global_agent_collaboration_event_sequence()
        .map_err(io::Error::other)?;
    let automation_event_startup_cursor = bootstrap
        .storage
        .latest_automation_event_sequence()
        .map_err(io::Error::other)?;
    let (outbound_tx, outbound_rx) = mpsc::unbounded_channel::<Value>();
    managed_playwright_bridge
        .attach_outbound(outbound_tx.clone())
        .map_err(|_| io::Error::other("failed to attach managed Playwright Host bridge"))?;
    outbound_tx
        .send(collaboration_resync_notification())
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "outbound channel is closed"))?;
    outbound_tx
        .send(automation_resync_notification(
            automation_event_startup_cursor,
        )?)
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "outbound channel is closed"))?;
    agent_service
        .start_collaboration_dispatcher(outbound_tx.clone())
        .map_err(io::Error::other)?;
    let (image_artifact_outbound_tx, image_artifact_outbound_rx) =
        mpsc::channel::<ImageArtifactOutbound>(DEFAULT_MAX_CONCURRENT_IMAGE_ARTIFACT_READS);
    let (finish_outbound_tx, finish_outbound_rx) = oneshot::channel();
    let writer = tokio::spawn(run_outbound_writer(
        io::stdout(),
        outbound_rx,
        image_artifact_outbound_rx,
        finish_outbound_rx,
    ));
    let mcp_changed_notifier = tokio::spawn(run_mcp_changed_notifier(
        mcp_event_sink.subscribe_safe_events(),
        Arc::clone(&mcp_management),
        outbound_tx.clone(),
    ));
    let collaboration_event_notifier = tokio::spawn(run_collaboration_event_notifier(
        Arc::clone(&bootstrap.storage),
        outbound_tx.clone(),
        collaboration_event_startup_cursor,
    ));
    let automation_event_notifier = tokio::spawn(run_automation_event_notifier(
        Arc::clone(&bootstrap.storage),
        outbound_tx.clone(),
        automation_event_startup_cursor,
    ));
    // The scheduler is constructed only after the final MCP-injected AgentService has completed
    // startup reconciliation and the authoritative resync cut has been published.
    let automation_scheduler_wake =
        crate::application::automation::AutomationSchedulerWake::default();
    let mut automation_scheduler = crate::application::automation::AutomationScheduler::start(
        Arc::clone(&bootstrap.storage),
        agent_service.clone(),
        outbound_tx.clone(),
        automation_scheduler_wake.clone(),
    )
    .await
    .map_err(io::Error::other)?;
    let git_dispatcher = GitDispatcher::new(outbound_tx.clone());
    let skill_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
    let skill_acquisition_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
    let image_generation_configuration_dispatcher =
        ImageGenerationConfigurationDispatcher::new(outbound_tx.clone());
    let request_dispatchers = RequestDispatchers {
        git: &git_dispatcher,
        skills: &skill_dispatcher,
        skill_acquisition: &skill_acquisition_dispatcher,
        image_generation_configuration: &image_generation_configuration_dispatcher,
    };
    let mcp_management_tasks = Arc::new(McpManagementRequestTracker::new());
    let browser_risk_tasks = Arc::new(McpManagementRequestTracker::new());

    let input_result = run_request_loop(
        BufReader::new(io::stdin()),
        CoreRequestServices {
            storage: Arc::clone(&bootstrap.storage),
            automation_scheduler_wake,
            image_generation_configuration: Arc::clone(&bootstrap.image_generation_configuration),
            image_generation_artifacts: Arc::clone(&bootstrap.image_generation_artifacts),
            image_generation_artifact_read_admission: Arc::new(Semaphore::new(
                DEFAULT_MAX_CONCURRENT_IMAGE_ARTIFACT_READS,
            )),
            mcp_management: Some(Arc::clone(&mcp_management)),
            managed_playwright_bridge: Some(Arc::clone(&managed_playwright_bridge)),
            managed_playwright_runtime: Some(Arc::clone(&managed_playwright_runtime)),
            browser_risk_coordinator: Some(Arc::clone(&bootstrap.browser_risk_coordinator)),
            browser_risk_admission: Arc::new(Semaphore::new(
                DEFAULT_MAX_CONCURRENT_BROWSER_RISK_REQUESTS,
            )),
            browser_risk_tasks: Arc::clone(&browser_risk_tasks),
            mcp_management_admission: Arc::new(Semaphore::new(
                DEFAULT_MAX_CONCURRENT_MCP_MANAGEMENT_REQUESTS,
            )),
            mcp_management_tasks: Arc::clone(&mcp_management_tasks),
        },
        &agent_service,
        bootstrap.skill_services.clone(),
        Arc::clone(&bootstrap.git_review_service),
        &request_dispatchers,
        RequestOutbounds {
            normal: &outbound_tx,
            image_artifact: &image_artifact_outbound_tx,
        },
    )
    .await;

    // Stop future Automation claims before any shared Agent or MCP shutdown begins. Already
    // admitted HumanRoot turns continue through the normal AgentService shutdown path below.
    automation_scheduler.stop_admissions().await;

    // `core.shutdown` settles the managed runtime inside the request loop while reverse bridge
    // completions can still arrive from Main. EOF or a request-loop failure means that responder
    // is no longer available, so close the bridge first and then perform the same idempotent,
    // bounded runtime cleanup. In every exit path this happens before the outbound writer closes.
    if !matches!(input_result.as_ref(), Ok(Some(_))) {
        managed_playwright_bridge.close_now();
    }
    bootstrap.browser_risk_coordinator.cancel_all();
    let browser_risk_requests_shutdown = browser_risk_tasks.shutdown(Duration::from_secs(1)).await;
    managed_playwright_runtime.shutdown().await;

    mcp_management.begin_shutdown();
    mcp_changed_notifier.abort();
    let _ = mcp_changed_notifier.await;
    collaboration_event_notifier.abort();
    let _ = collaboration_event_notifier.await;
    // Optional connection discovery must never delay admission or outlive Host shutdown.
    // Aborting this coordinator does not replace Manager cleanup; stop_all below remains the
    // process-owned close authority for every connection that reached the Manager.
    mcp_startup.abort();
    let _ = mcp_startup.await;
    let mcp_approval_expiry_shutdown = mcp_approval_expiry_reconciler.shutdown().await;
    let mcp_action_invalidation =
        agent_service.invalidate_process_bound_mcp_actions_before_shutdown();

    // Admission has stopped. Settle accepted filesystem jobs while active agents are cancelled in
    // parallel; queued jobs receive cancellation errors and running jobs get a bounded grace
    // period. The outbound writer remains live for every final response and notification.
    let (
        git_dispatcher_result,
        skill_dispatcher_result,
        skill_acquisition_dispatcher_result,
        image_generation_configuration_dispatcher_result,
        image_generation_execution_shutdown,
        collaboration_dispatcher_shutdown,
        (cancelled_runs, timed_out),
        mcp_management_requests_shutdown,
        mcp_shutdown,
    ) = tokio::join!(
        git_dispatcher.shutdown(),
        skill_dispatcher.shutdown(),
        skill_acquisition_dispatcher.shutdown(),
        image_generation_configuration_dispatcher.shutdown(),
        bootstrap
            .image_generation_execution
            .shutdown(Duration::from_secs(2)),
        agent_service.shutdown_collaboration_dispatcher(),
        agent_service.shutdown_active_runs(Duration::from_secs(2)),
        mcp_management_tasks.shutdown(Duration::from_secs(2)),
        mcp_manager.shutdown(Duration::from_secs(2))
    );

    automation_scheduler.finish_shutdown().await;
    automation_event_notifier.abort();
    let _ = automation_event_notifier.await;

    if let Err(error) = collaboration_dispatcher_shutdown {
        eprintln!("collaboration dispatcher shutdown failed: {error}");
    }

    let mut outbound_error = None;
    if let Ok(Some(shutdown_id)) = &input_result {
        if let Err(error) = enqueue_outbound(
            &outbound_tx,
            response_success(
                shutdown_id.clone(),
                CoreShutdownResponse {
                    cancelled_runs,
                    timed_out,
                },
            ),
        ) {
            outbound_error = Some(error);
        }
    }
    drop(outbound_tx);
    drop(image_artifact_outbound_tx);
    // A timed-out agent may still own an outbound sender. Tell the writer to close its receiver
    // and drain everything accepted so far instead of waiting for every producer clone to drop.
    // The shutdown response above is therefore flushed, while late notifications are rejected.
    let _ = finish_outbound_tx.send(());

    let writer_result = writer
        .await
        .map_err(|error| io::Error::other(format!("outbound writer stopped: {error}")))?;
    input_result?;
    mcp_approval_expiry_shutdown.map_err(io::Error::other)?;
    git_dispatcher_result.map_err(|error| io::Error::other(error.to_string()))?;
    skill_dispatcher_result.map_err(|error| io::Error::other(error.to_string()))?;
    skill_acquisition_dispatcher_result.map_err(|error| io::Error::other(error.to_string()))?;
    image_generation_configuration_dispatcher_result
        .map_err(|error| io::Error::other(error.to_string()))?;
    if image_generation_execution_shutdown.timed_out {
        eprintln!(
            "image-generation execution shutdown timed out after cancelling {} active execution(s)",
            image_generation_execution_shutdown.cancelled_executions
        );
    }
    let mcp_shutdown_failures = mcp_shutdown
        .results
        .iter()
        .filter(|result| result.error.is_some())
        .count();
    if mcp_shutdown.forced {
        eprintln!(
            "MCP graceful shutdown reached its deadline; forced transport cleanup was required"
        );
    }
    if !mcp_shutdown.cleanup_complete {
        eprintln!("MCP forced cleanup did not settle every transport before the Host deadline");
    }
    if mcp_shutdown_failures > 0 {
        eprintln!("{mcp_shutdown_failures} MCP server(s) did not shut down cleanly");
    }
    if mcp_management_requests_shutdown.forced {
        eprintln!(
            "MCP management request shutdown reached its deadline; remaining requests were aborted"
        );
    }
    if mcp_management_requests_shutdown.task_failures > 0 {
        eprintln!(
            "{} MCP management request task(s) did not join cleanly",
            mcp_management_requests_shutdown.task_failures
        );
    }
    if browser_risk_requests_shutdown.forced {
        eprintln!(
            "browser risk authorization shutdown reached its deadline; remaining requests were aborted"
        );
    }
    if browser_risk_requests_shutdown.task_failures > 0 {
        eprintln!(
            "{} browser risk authorization task(s) did not join cleanly",
            browser_risk_requests_shutdown.task_failures
        );
    }
    match &mcp_action_invalidation {
        Ok(summary) if summary.payload_invalidation_failures > 0 => eprintln!(
            "{} process-bound MCP approval payload(s) could not be invalidated cleanly",
            summary.payload_invalidation_failures
        ),
        Ok(_) => {}
        Err(_) => eprintln!("process-bound MCP approvals could not be settled safely at shutdown"),
    }
    if let Some(error) = outbound_error {
        return Err(error);
    }
    mcp_action_invalidation.map_err(io::Error::other)?;
    writer_result
}

async fn run_collaboration_event_notifier(
    storage: Arc<StorageService>,
    outbound: mpsc::UnboundedSender<Value>,
    initial_cursor: u64,
) {
    let mut cursor = initial_cursor;
    let mut interval = tokio::time::interval(AGENT_COLLABORATION_EVENT_POLL_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let storage = Arc::clone(&storage);
        let page = tokio::task::spawn_blocking(move || {
            storage.list_global_agent_collaboration_events(cursor, 256)
        })
        .await;
        let Ok(Ok(events)) = page else {
            continue;
        };
        for event in events {
            cursor = event.global_sequence;
            let notification = serde_json::json!({
                "jsonrpc": "2.0",
                "method": mycopilot_protocol_rs::AGENT_COLLABORATION_EVENT_NOTIFICATION_METHOD,
                "params": application::agent::collaboration_event_dto(event),
            });
            if outbound.send(notification).is_err() {
                return;
            }
        }
    }
}

pub(crate) async fn run_automation_event_notifier(
    storage: Arc<StorageService>,
    outbound: mpsc::UnboundedSender<Value>,
    initial_cursor: i64,
) {
    let mut cursor = initial_cursor;
    let mut interval = tokio::time::interval(AUTOMATION_EVENT_POLL_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let request_storage = Arc::clone(&storage);
        let page = tokio::task::spawn_blocking(move || {
            request_storage.list_automation_events_after(cursor, 256)
        })
        .await;
        let Ok(Ok(events)) = page else {
            continue;
        };
        for event in events {
            cursor = event.sequence;
            let Ok(params) = application::automation::automation_event_dto(event) else {
                continue;
            };
            let notification = serde_json::json!({
                "jsonrpc": "2.0",
                "method": mycopilot_protocol_rs::AUTOMATION_EVENT_NOTIFICATION_METHOD,
                "params": params,
            });
            if outbound.send(notification).is_err() {
                return;
            }
        }
    }
}

fn automation_resync_notification(last_sequence: i64) -> io::Result<Value> {
    let last_sequence = u64::try_from(last_sequence)
        .map_err(|_| io::Error::other("automation event sequence is negative"))?;
    Ok(serde_json::json!({
        "jsonrpc": "2.0",
        "method": mycopilot_protocol_rs::AUTOMATION_RESYNC_NOTIFICATION_METHOD,
        "params": mycopilot_protocol_rs::AutomationResyncDto {
            schema_version: mycopilot_protocol_rs::AUTOMATION_SCHEMA_VERSION,
            reason: mycopilot_protocol_rs::AutomationResyncReasonDto::CoreStarted,
            last_sequence,
            occurred_at: mycopilot_core::storage::now_ms(),
        },
    }))
}

fn collaboration_resync_notification() -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": mycopilot_protocol_rs::AGENT_COLLABORATION_RESYNC_NOTIFICATION_METHOD,
        "params": mycopilot_protocol_rs::CollaborationResyncEnvelopeDto {
            schema_version: mycopilot_protocol_rs::AGENT_COLLABORATION_SCHEMA_VERSION,
            reason: mycopilot_protocol_rs::CollaborationResyncReasonDto::CoreStarted,
        },
    })
}

#[derive(Clone)]
pub(crate) struct SkillServices {
    pub(crate) catalog: Arc<SkillsService>,
    pub(crate) installations: Arc<SkillInstallationService>,
    pub(crate) workflow: Arc<SkillInstallationWorkflow>,
    pub(crate) source_resolution: Arc<SkillSourceResolutionService>,
}

pub(crate) fn skill_store_root(database_path: &std::path::Path) -> PathBuf {
    database_path
        .parent()
        .map(|parent| parent.join("skills"))
        .unwrap_or_else(|| PathBuf::from("skills"))
}

pub(crate) fn skill_installation_pending_root(database_path: &std::path::Path) -> PathBuf {
    database_path
        .parent()
        .map(|parent| parent.join("skill-installation-transactions"))
        .unwrap_or_else(|| PathBuf::from("skill-installation-transactions"))
}

pub(crate) fn image_generation_artifact_store_root(database_path: &std::path::Path) -> PathBuf {
    database_path
        .parent()
        .map(|parent| parent.join("image-generation-artifacts"))
        .unwrap_or_else(|| PathBuf::from("image-generation-artifacts"))
}

#[cfg(target_os = "macos")]
const MACOS_CORE_SERVER_SIGNING_REQUIREMENT: &str =
    r#"anchor apple generic and identifier "com.mycopilot.next.core-server""#;

/// Selects a credential backend without ever probing a legacy Keychain item.
///
/// A frequently rebuilt, ad-hoc-signed helper has no stable macOS code identity. Giving that
/// helper direct Keychain access makes the operating system ask the user to approve every new
/// CDHash. Development and unsigned local packages therefore use a private application file
/// store. Only an Apple-issued, fixed-identifier build is allowed to use the non-interactive
/// production Keychain adapter.
fn image_generation_credential_store(
    database_path: &Path,
    uses_development_credentials: bool,
) -> io::Result<Arc<dyn CredentialStore>> {
    #[cfg(target_os = "macos")]
    {
        if !uses_development_credentials {
            return Ok(Arc::new(
                NonInteractiveMacCredentialStore::image_generation(),
            ));
        }

        DevelopmentFileCredentialStore::new(image_generation_development_credential_store_root(
            database_path,
        ))
        .map(|store| Arc::new(store) as Arc<dyn CredentialStore>)
        .map_err(|error| {
            io::Error::other(format!(
                "failed to initialize development image-generation credential store: {error}"
            ))
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (database_path, uses_development_credentials);
        Ok(Arc::new(SystemCredentialStore::image_generation()))
    }
}

/// Selects a credential backend dedicated to the provider-continuation master key.
///
/// This namespace is distinct from model API tokens and image-generation credentials.
/// Development builds use a private durable file store so encrypted continuation rows remain
/// recoverable across Core Server restarts without probing an unstable Keychain identity.
fn provider_continuation_credential_store(
    database_path: &Path,
    uses_development_credentials: bool,
) -> io::Result<Arc<dyn CredentialStore>> {
    #[cfg(target_os = "macos")]
    {
        if !uses_development_credentials {
            return NonInteractiveMacCredentialStore::new(PROVIDER_CONTINUATION_CREDENTIAL_SERVICE)
                .map(|store| Arc::new(store) as Arc<dyn CredentialStore>)
                .map_err(|error| {
                    io::Error::other(format!(
                        "failed to initialize Provider continuation credential store: {error}"
                    ))
                });
        }

        DevelopmentFileCredentialStore::new(
            provider_continuation_development_credential_store_root(database_path),
        )
        .map(|store| Arc::new(store) as Arc<dyn CredentialStore>)
        .map_err(|error| {
            io::Error::other(format!(
                "failed to initialize development Provider continuation credential store: {error}"
            ))
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (database_path, uses_development_credentials);
        SystemCredentialStore::new(PROVIDER_CONTINUATION_CREDENTIAL_SERVICE)
            .map(|store| Arc::new(store) as Arc<dyn CredentialStore>)
            .map_err(|error| {
                io::Error::other(format!(
                    "failed to initialize Provider continuation credential store: {error}"
                ))
            })
    }
}

/// Selects the native credential backend dedicated to the MCP approval master key.
///
/// Unlike image generation, MCP payload encryption never falls back to the development file
/// credential store. On macOS an unsigned or ad-hoc-signed helper receives no Keychain adapter;
/// the caller then uses a process-only payload store.
#[cfg(test)]
fn mcp_approval_credential_store() -> Option<Arc<dyn CredentialStore>> {
    // Unit-test binaries must never probe or mutate the developer's real Keychain/Credential
    // Manager. Durable recovery tests inject InMemoryCredentialStore directly into the factory.
    None
}

#[cfg(not(test))]
fn mcp_approval_credential_store() -> Option<Arc<dyn CredentialStore>> {
    #[cfg(target_os = "macos")]
    {
        if !macos_core_server_has_stable_signing_identity() {
            return None;
        }
        NonInteractiveMacCredentialStore::new(MCP_APPROVAL_CREDENTIAL_SERVICE)
            .ok()
            .map(|store| Arc::new(store) as Arc<dyn CredentialStore>)
    }

    #[cfg(not(target_os = "macos"))]
    {
        SystemCredentialStore::new(MCP_APPROVAL_CREDENTIAL_SERVICE)
            .ok()
            .map(|store| Arc::new(store) as Arc<dyn CredentialStore>)
    }
}

fn mcp_approval_payload_store(storage: Arc<StorageService>) -> Arc<dyn McpApprovalPayloadStore> {
    let repository = Arc::new(SqliteMcpApprovalEnvelopeRepository::new(storage));
    durable_mcp_payload_store_or_process_only(repository, mcp_approval_credential_store())
}

fn uses_development_image_generation_credentials() -> bool {
    #[cfg(target_os = "macos")]
    {
        !macos_core_server_has_stable_signing_identity()
    }

    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn image_generation_development_credential_store_root(database_path: &Path) -> PathBuf {
    database_path
        .parent()
        .map(|parent| parent.join("image-generation-development-credentials-v1"))
        .unwrap_or_else(|| PathBuf::from("image-generation-development-credentials-v1"))
}

#[cfg(target_os = "macos")]
pub(crate) fn provider_continuation_development_credential_store_root(
    database_path: &Path,
) -> PathBuf {
    database_path
        .parent()
        .map(|parent| parent.join("provider-continuation-development-credentials-v1"))
        .unwrap_or_else(|| PathBuf::from("provider-continuation-development-credentials-v1"))
}

#[cfg(target_os = "macos")]
fn macos_core_server_has_stable_signing_identity() -> bool {
    use security_framework::os::macos::code_signing::{Flags, SecCode, SecRequirement};

    let Ok(requirement) = MACOS_CORE_SERVER_SIGNING_REQUIREMENT.parse::<SecRequirement>() else {
        return false;
    };
    SecCode::for_self(Flags::NONE)
        .and_then(|code| code.check_validity(Flags::NONE, &requirement))
        .is_ok()
}

pub(crate) fn absolute_path(path: PathBuf) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir().map(|current_directory| current_directory.join(path))
    }
}

#[cfg(test)]
mod mcp_payload_bootstrap_tests {
    use super::*;
    use crate::application::mcp::approval_payload_store::{
        InMemoryMcpApprovalPayloadStore, McpApprovalPayloadPersistence,
        UnavailableMcpApprovalPayloadStore,
    };
    use tempfile::tempdir;

    #[test]
    fn collaboration_startup_resync_is_a_strict_global_invalidation() {
        assert_eq!(
            collaboration_resync_notification(),
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": mycopilot_protocol_rs::AGENT_COLLABORATION_RESYNC_NOTIFICATION_METHOD,
                "params": {
                    "schemaVersion": mycopilot_protocol_rs::AGENT_COLLABORATION_SCHEMA_VERSION,
                    "reason": "core_started"
                }
            })
        );
    }

    #[test]
    fn automation_startup_resync_carries_the_frozen_event_cut() {
        let notification = automation_resync_notification(7).unwrap();
        assert_eq!(
            notification["method"],
            mycopilot_protocol_rs::AUTOMATION_RESYNC_NOTIFICATION_METHOD
        );
        assert_eq!(notification["params"]["schemaVersion"], 1);
        assert_eq!(notification["params"]["reason"], "core_started");
        assert_eq!(notification["params"]["lastSequence"], 7);
        assert!(notification["params"]["occurredAt"].as_i64().is_some());
        assert!(automation_resync_notification(-1).is_err());
    }

    #[tokio::test]
    async fn collaboration_notifier_replays_a_commit_after_the_frozen_startup_cut() {
        let directory = tempdir().unwrap();
        let storage = Arc::new(
            StorageService::open(&directory.path().join("collaboration-events.sqlite")).unwrap(),
        );
        storage
            .save_conversation_meta(
                mycopilot_core::storage::models::ChatConversationMetaRecord {
                    id: "conversation-root".to_string(),
                    project_id: None,
                    model_id: None,
                    title: "Root".to_string(),
                    created_at: 1,
                    updated_at: 1,
                    pinned_at: None,
                    archived_at: None,
                    unread_at: None,
                },
            )
            .unwrap();
        storage
            .ensure_root_agent(&mycopilot_core::EnsureRootAgentInput {
                agent_id: "agent-root".to_string(),
                conversation_id: "conversation-root".to_string(),
                creation_request_id: "ensure-root".to_string(),
                task_name: "Root".to_string(),
            })
            .unwrap();
        let startup_cut = storage
            .latest_global_agent_collaboration_event_sequence()
            .unwrap();
        storage
            .transition_agent_lifecycle(
                "agent-root",
                1,
                mycopilot_core::AgentLifecycle::Active,
                mycopilot_core::AgentLifecycle::Archived,
            )
            .unwrap();

        let (outbound, mut notifications) = mpsc::unbounded_channel();
        let notifier = tokio::spawn(run_collaboration_event_notifier(
            Arc::clone(&storage),
            outbound,
            startup_cut,
        ));
        let notification = tokio::time::timeout(Duration::from_secs(1), notifications.recv())
            .await
            .unwrap()
            .unwrap();
        notifier.abort();
        let _ = notifier.await;

        assert_eq!(
            notification["method"],
            mycopilot_protocol_rs::AGENT_COLLABORATION_EVENT_NOTIFICATION_METHOD
        );
        assert_eq!(notification["params"]["rootAgentId"], "agent-root");
        assert_eq!(notification["params"]["sequence"], 2);
    }

    #[test]
    fn test_bootstrap_never_probes_native_credentials_and_uses_process_only_payloads() {
        assert!(mcp_approval_credential_store().is_none());
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("test.sqlite");
        let storage = Arc::new(StorageService::open(&database_path).unwrap());
        assert_eq!(
            mcp_approval_payload_store(storage).persistence(),
            McpApprovalPayloadPersistence::ProcessOnly
        );
    }

    #[test]
    fn expiry_tick_reconciles_the_host_payload_store_after_agent_state() {
        let directory = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&directory.path().join("test.sqlite")).unwrap());
        let service = AgentService::new(storage);
        let unavailable = UnavailableMcpApprovalPayloadStore;
        assert!(
            reconcile_expired_mcp_approvals_tick(&service, &unavailable).is_err(),
            "an unavailable payload store must be observed by the synchronized tick"
        );
    }

    #[tokio::test]
    async fn expiry_reconciler_cancellation_is_explicit_and_awaited_without_a_timer_sleep() {
        let directory = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&directory.path().join("test.sqlite")).unwrap());
        let service = AgentService::new(storage);
        let reconciler = McpApprovalExpiryReconciler::spawn(
            service,
            Arc::new(InMemoryMcpApprovalPayloadStore::default()),
        );
        reconciler.shutdown().await.unwrap();
    }
}
