use super::*;
use std::fs::{self, File, OpenOptions};
use std::path::Path;

pub(crate) struct CoreServerBootstrap {
    pub(crate) storage: Arc<StorageService>,
    pub(crate) image_generation_configuration: Arc<ImageGenerationConfigurationService>,
    pub(crate) image_generation_artifacts: Arc<ManagedImageGenerationArtifactStore>,
    pub(crate) image_generation_execution: Arc<ImageGenerationExecutionService>,
    pub(crate) agent_service: AgentService,
    pub(crate) skill_services: SkillServices,
    pub(crate) git_review_service: Arc<GitReviewService>,
    // Fields drop in declaration order. Keep this owner last so the database lock outlives every
    // service and SQLite connection above it. File-effect deletion barriers are process-local;
    // this OS lock makes one core-server the authoritative lifecycle owner for the exact DB.
    _database_instance_lock: File,
}

impl CoreServerBootstrap {
    pub(crate) fn initialize() -> io::Result<Self> {
        let database_path = absolute_path(database_path())?;
        let database_instance_lock = acquire_database_instance_lock(&database_path)?;
        let skill_store_root = skill_store_root(&database_path);
        let storage =
            Arc::new(StorageService::open(&database_path).map_err(|error| {
                io::Error::other(format!("failed to initialize storage: {error}"))
            })?);
        let image_generation_configuration = Arc::new(ImageGenerationConfigurationService::new(
            Arc::clone(&storage),
            image_generation_credential_store(&database_path)?,
        ));
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
        let github_acquisition_transport: Arc<dyn GitHubAcquisitionTransport> =
            Arc::new(ReqwestGitHubTransport::new().map_err(|error| {
                io::Error::other(format!(
                    "failed to initialize public GitHub Skill transport: {error}"
                ))
            })?);
        let github_acquisition = GitHubWorkflowAcquisitionAdapter::new(Arc::new(
            GitHubSkillAcquirer::with_transport(github_acquisition_transport),
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
        let github_resolution_transport: Arc<dyn GitHubAcquisitionTransport> = Arc::new(
            ReqwestGitHubTransport::new_for_source_resolution().map_err(|error| {
                io::Error::other(format!(
                    "failed to initialize GitHub Skill source resolution transport: {error}"
                ))
            })?,
        );
        skill_source_resolution
            .register_resolver(Arc::new(GitHubInstallationSourceResolver::new(
                github_resolution_transport,
            )))
            .map_err(|error| {
                io::Error::other(format!(
                    "failed to register GitHub Skill source resolver: {error}"
                ))
            })?;
        let agent_service = AgentService::try_new_deferred_startup_reconciliation(storage.clone())
            .map_err(|error| {
                io::Error::other(format!("failed to initialize Agent service: {error}"))
            })?
            .with_skills_service(Arc::clone(&skills_service))
            .with_image_generation_execution(Arc::clone(&image_generation_execution));
        let skill_services = SkillServices {
            catalog: skills_service,
            installations: skill_installation_service,
            workflow: Arc::new(skill_installation_workflow),
            source_resolution: Arc::new(skill_source_resolution),
        };
        Ok(Self {
            storage,
            image_generation_configuration,
            image_generation_artifacts,
            image_generation_execution,
            agent_service,
            skill_services,
            git_review_service,
            _database_instance_lock: database_instance_lock,
        })
    }
}

pub(crate) fn acquire_database_instance_lock(database_path: &Path) -> io::Result<File> {
    let parent = database_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let canonical_parent = fs::canonicalize(parent)?;
    let file_name = database_path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "storage database path has no file name: {}",
                database_path.display()
            ),
        )
    })?;
    let database_identity = if database_path.exists() {
        fs::canonicalize(database_path)?
    } else {
        canonical_parent.join(file_name)
    };
    let lock_file_name = format!(
        ".{}.core-server.lock",
        database_identity
            .file_name()
            .expect("database identity retains a file name")
            .to_string_lossy()
    );
    let lock_path = database_identity.with_file_name(lock_file_name);
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    if let Err(error) = lock.try_lock() {
        let (kind, message) = match error {
            fs::TryLockError::WouldBlock => (
                io::ErrorKind::AlreadyExists,
                format!(
                    "another core-server already owns storage `{}` (instance lock `{}`)",
                    database_identity.display(),
                    lock_path.display()
                ),
            ),
            fs::TryLockError::Error(error) => (
                error.kind(),
                format!(
                    "failed to acquire the core-server instance lock `{}` for storage `{}`: {error}",
                    lock_path.display(),
                    database_identity.display()
                ),
            ),
        };
        return Err(io::Error::new(kind, message));
    }
    Ok(lock)
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
    bootstrap
        .agent_service
        .reconcile_startup_orphaned_conversation_traces()
        .map_err(io::Error::other)?;
    let (outbound_tx, outbound_rx) = mpsc::unbounded_channel::<Value>();
    let (image_artifact_outbound_tx, image_artifact_outbound_rx) =
        mpsc::channel::<ImageArtifactOutbound>(DEFAULT_MAX_CONCURRENT_IMAGE_ARTIFACT_READS);
    let (finish_outbound_tx, finish_outbound_rx) = oneshot::channel();
    let writer = tokio::spawn(run_outbound_writer(
        io::stdout(),
        outbound_rx,
        image_artifact_outbound_rx,
        finish_outbound_rx,
    ));
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

    let input_result = run_request_loop(
        BufReader::new(io::stdin()),
        CoreRequestServices {
            storage: Arc::clone(&bootstrap.storage),
            image_generation_configuration: Arc::clone(&bootstrap.image_generation_configuration),
            image_generation_artifacts: Arc::clone(&bootstrap.image_generation_artifacts),
            image_generation_artifact_read_admission: Arc::new(Semaphore::new(
                DEFAULT_MAX_CONCURRENT_IMAGE_ARTIFACT_READS,
            )),
        },
        &bootstrap.agent_service,
        bootstrap.skill_services.clone(),
        Arc::clone(&bootstrap.git_review_service),
        &request_dispatchers,
        RequestOutbounds {
            normal: &outbound_tx,
            image_artifact: &image_artifact_outbound_tx,
        },
    )
    .await;

    // Admission has stopped. Settle accepted filesystem jobs while active agents are cancelled in
    // parallel; queued jobs receive cancellation errors and running jobs get a bounded grace
    // period. The outbound writer remains live for every final response and notification.
    let (
        git_dispatcher_result,
        skill_dispatcher_result,
        skill_acquisition_dispatcher_result,
        image_generation_configuration_dispatcher_result,
        image_generation_execution_shutdown,
        (cancelled_runs, timed_out),
    ) = tokio::join!(
        git_dispatcher.shutdown(),
        skill_dispatcher.shutdown(),
        skill_acquisition_dispatcher.shutdown(),
        image_generation_configuration_dispatcher.shutdown(),
        bootstrap
            .image_generation_execution
            .shutdown(Duration::from_secs(2)),
        bootstrap
            .agent_service
            .shutdown_active_runs(Duration::from_secs(2))
    );

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
    if let Some(error) = outbound_error {
        return Err(error);
    }
    writer_result
}

#[derive(Clone)]
pub(crate) struct SkillServices {
    pub(crate) catalog: Arc<SkillsService>,
    pub(crate) installations: Arc<SkillInstallationService>,
    pub(crate) workflow: Arc<SkillInstallationWorkflow>,
    pub(crate) source_resolution: Arc<SkillSourceResolutionService>,
}

pub(crate) fn database_path() -> PathBuf {
    if let Ok(path) = std::env::var("MYCOPILOT_STORAGE_DB") {
        return PathBuf::from(path);
    }

    if cfg!(target_os = "macos") {
        return home_dir()
            .join("Library")
            .join("Application Support")
            .join("mycopilot-next")
            .join("storage.sqlite");
    }

    if cfg!(target_os = "windows") {
        if let Ok(app_data) = std::env::var("APPDATA") {
            return PathBuf::from(app_data)
                .join("mycopilot-next")
                .join("storage.sqlite");
        }
    }

    std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".local").join("share"))
        .join("mycopilot-next")
        .join("storage.sqlite")
}

pub(crate) fn skill_store_root(database_path: &std::path::Path) -> PathBuf {
    database_path
        .parent()
        .map(|parent| parent.join("skills"))
        .unwrap_or_else(|| PathBuf::from("skills"))
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
fn image_generation_credential_store(database_path: &Path) -> io::Result<Arc<dyn CredentialStore>> {
    #[cfg(target_os = "macos")]
    {
        if macos_core_server_has_stable_signing_identity() {
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
        let _ = database_path;
        Ok(Arc::new(SystemCredentialStore::image_generation()))
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

pub(crate) fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}
