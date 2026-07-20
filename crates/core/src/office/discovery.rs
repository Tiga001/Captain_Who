use super::execution::{prepare_office_cli, probe_engine, run_prepared_office_cli};
use super::types::{
    OfficeEngine, OfficeEngineCapabilities, OfficeEngineError, OfficeEngineErrorCode,
    OfficeEngineRecovery, OfficeEngineSource, OfficeEngineStatus, OfficeExecutionContext,
    OfficeExecutionRequest, OfficeExecutionResult, OfficePreparedExecution, OFFICECLI_PROVIDER_ID,
    OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
};
use crate::AgentCancellationToken;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

const COMPONENT_DIRECTORY: &str = "officecli";
const ENGINE_REVISION_PREFIX: &str = "office-engine-sha256-v1:";
const MAX_OFFICECLI_BINARY_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct OfficeCliDiscoveryOptions {
    configured_executable: Option<PathBuf>,
    application_resources_dir: Option<PathBuf>,
    workspace_root: Option<PathBuf>,
    allow_path_fallback: bool,
}

impl OfficeCliDiscoveryOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_configured_executable(mut self, executable: impl Into<PathBuf>) -> Self {
        self.configured_executable = Some(executable.into());
        self
    }

    pub fn with_application_resources_dir(mut self, directory: impl Into<PathBuf>) -> Self {
        self.application_resources_dir = Some(directory.into());
        self
    }

    pub fn with_workspace_root(mut self, workspace_root: impl Into<PathBuf>) -> Self {
        self.workspace_root = Some(workspace_root.into());
        self
    }

    /// Enables the development-only PATH fallback. Production callers should
    /// prefer a configured or packaged component and leave this disabled.
    pub fn allow_path_fallback(mut self, allow: bool) -> Self {
        self.allow_path_fallback = allow;
        self
    }
}

#[derive(Debug, Clone)]
pub struct OfficeCliEngine {
    executable: PathBuf,
    source: OfficeEngineSource,
    engine_revision: String,
}

impl OfficeCliEngine {
    pub fn discover(options: &OfficeCliDiscoveryOptions) -> Result<Self, OfficeEngineError> {
        discover_with_path(options, std::env::var_os("PATH"))
    }

    pub fn inspect(
        options: &OfficeCliDiscoveryOptions,
        cancellation: AgentCancellationToken,
    ) -> OfficeEngineStatus {
        match Self::discover(options) {
            Ok(engine) => engine.status(cancellation),
            Err(error) => OfficeEngineStatus {
                schema_version: super::types::OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
                provider_id: super::types::OFFICECLI_PROVIDER_ID.to_string(),
                availability: super::types::OfficeEngineAvailability::Unavailable,
                source: None,
                version: None,
                engine_revision: None,
                capabilities: OfficeEngineCapabilities::office_cli(),
                error_code: Some(error.code().stable_name().to_string()),
                message: Some(error.message().to_string()),
            },
        }
    }

    pub fn executable_path(&self) -> &Path {
        &self.executable
    }

    pub fn source(&self) -> OfficeEngineSource {
        self.source
    }

    pub fn engine_revision(&self) -> &str {
        &self.engine_revision
    }

    pub(crate) fn verify_engine_revision(&self) -> Result<(), OfficeEngineError> {
        let current = executable_revision(&self.executable)?;
        if current != self.engine_revision {
            return Err(OfficeEngineError::new(
                OfficeEngineErrorCode::InvalidConfiguration,
                OfficeEngineRecovery::Retry,
                "OfficeCLI changed after discovery; rediscover the engine and prepare the operation again.",
            ));
        }
        Ok(())
    }
}

impl OfficeEngine for OfficeCliEngine {
    fn capabilities(&self) -> OfficeEngineCapabilities {
        OfficeEngineCapabilities::office_cli()
    }

    fn status(&self, cancellation: AgentCancellationToken) -> OfficeEngineStatus {
        probe_engine(self, cancellation)
    }

    fn prepare(
        &self,
        context: &OfficeExecutionContext,
        request: &OfficeExecutionRequest,
    ) -> Result<OfficePreparedExecution, OfficeEngineError> {
        prepare_office_cli(self, context, request)
    }

    fn execute_prepared(
        &self,
        context: &OfficeExecutionContext,
        prepared: &OfficePreparedExecution,
        cancellation: AgentCancellationToken,
        action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<OfficeExecutionResult, OfficeEngineError> {
        run_prepared_office_cli(self, context, prepared, cancellation, action_cancel_flag)
    }
}

#[derive(Debug, Clone)]
pub struct UnavailableOfficeEngine {
    error: OfficeEngineError,
}

impl UnavailableOfficeEngine {
    pub fn new(error: OfficeEngineError) -> Self {
        Self { error }
    }

    pub fn error(&self) -> &OfficeEngineError {
        &self.error
    }
}

impl OfficeEngine for UnavailableOfficeEngine {
    fn capabilities(&self) -> OfficeEngineCapabilities {
        OfficeEngineCapabilities::office_cli()
    }

    fn status(&self, _cancellation: AgentCancellationToken) -> OfficeEngineStatus {
        OfficeEngineStatus {
            schema_version: OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
            provider_id: OFFICECLI_PROVIDER_ID.to_string(),
            availability: super::types::OfficeEngineAvailability::Unavailable,
            source: None,
            version: None,
            engine_revision: None,
            capabilities: self.capabilities(),
            error_code: Some(self.error.code().stable_name().to_string()),
            message: Some(self.error.message().to_string()),
        }
    }

    fn prepare(
        &self,
        _context: &OfficeExecutionContext,
        _request: &OfficeExecutionRequest,
    ) -> Result<OfficePreparedExecution, OfficeEngineError> {
        Err(self.error.clone())
    }

    fn execute_prepared(
        &self,
        _context: &OfficeExecutionContext,
        _prepared: &OfficePreparedExecution,
        _cancellation: AgentCancellationToken,
        _action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<OfficeExecutionResult, OfficeEngineError> {
        Err(self.error.clone())
    }
}

pub fn resolve_office_engine(options: &OfficeCliDiscoveryOptions) -> Arc<dyn OfficeEngine> {
    match OfficeCliEngine::discover(options) {
        Ok(engine) => Arc::new(engine),
        Err(error) => Arc::new(UnavailableOfficeEngine::new(error)),
    }
}

pub fn office_cli_component_relative_path() -> PathBuf {
    PathBuf::from(COMPONENT_DIRECTORY).join(executable_basename())
}

pub(super) fn executable_basename() -> &'static str {
    if cfg!(windows) {
        "officecli.exe"
    } else {
        "officecli"
    }
}

fn discover_with_path(
    options: &OfficeCliDiscoveryOptions,
    search_path: Option<OsString>,
) -> Result<OfficeCliEngine, OfficeEngineError> {
    let workspace_root = options
        .workspace_root
        .as_deref()
        .map(canonical_directory)
        .transpose()?;

    if let Some(configured) = options.configured_executable.as_deref() {
        if !configured.is_absolute() {
            return Err(configuration_error(
                "The configured OfficeCLI executable must be an absolute path.",
            ));
        }
        let executable =
            validate_executable(configured, workspace_root.as_deref()).map_err(|error| {
                configuration_error(format!(
                    "The configured OfficeCLI executable is invalid: {}",
                    error.message()
                ))
            })?;
        let engine_revision = executable_revision(&executable)?;
        return Ok(OfficeCliEngine {
            executable,
            source: OfficeEngineSource::Configured,
            engine_revision,
        });
    }

    if let Some(resources) = options.application_resources_dir.as_deref() {
        if !resources.is_absolute() {
            return Err(configuration_error(
                "The application resources directory must be an absolute path.",
            ));
        }
        let candidates = [
            resources.join(office_cli_component_relative_path()),
            resources.join(executable_basename()),
        ];
        for candidate in candidates {
            if !candidate.exists() {
                continue;
            }
            let resources = canonical_directory(resources)?;
            let executable = validate_executable(&candidate, workspace_root.as_deref())?;
            if !executable.starts_with(&resources) {
                return Err(configuration_error(
                    "The packaged OfficeCLI component resolves outside application resources.",
                ));
            }
            let engine_revision = executable_revision(&executable)?;
            return Ok(OfficeCliEngine {
                executable,
                source: OfficeEngineSource::PackagedComponent,
                engine_revision,
            });
        }
    }

    if options.allow_path_fallback {
        if let Some(path) = search_path {
            for directory in std::env::split_paths(&path) {
                // Relative PATH entries are cwd-dependent and therefore not a
                // trustworthy development executable source.
                if !directory.is_absolute() {
                    continue;
                }
                let candidate = directory.join(executable_basename());
                let Ok(executable) = validate_executable(&candidate, workspace_root.as_deref())
                else {
                    continue;
                };
                let engine_revision = executable_revision(&executable)?;
                return Ok(OfficeCliEngine {
                    executable,
                    source: OfficeEngineSource::DevelopmentPath,
                    engine_revision,
                });
            }
        }
    }

    Err(OfficeEngineError::new(
        OfficeEngineErrorCode::Unavailable,
        OfficeEngineRecovery::InstallComponent,
        "OfficeCLI is unavailable. Package the managed component, configure an absolute executable path, or enable the development PATH fallback.",
    ))
}

fn canonical_directory(path: &Path) -> Result<PathBuf, OfficeEngineError> {
    let canonical = path.canonicalize().map_err(|error| {
        configuration_error(format!(
            "Cannot resolve directory `{}`: {error}",
            path.display()
        ))
    })?;
    if !canonical.is_dir() {
        return Err(configuration_error(format!(
            "Configured path `{}` is not a directory.",
            path.display()
        )));
    }
    Ok(canonical)
}

fn validate_executable(
    path: &Path,
    workspace_root: Option<&Path>,
) -> Result<PathBuf, OfficeEngineError> {
    let canonical = path.canonicalize().map_err(|error| {
        OfficeEngineError::new(
            OfficeEngineErrorCode::Unavailable,
            OfficeEngineRecovery::InstallComponent,
            format!(
                "Cannot resolve OfficeCLI executable `{}`: {error}",
                path.display()
            ),
        )
    })?;
    let metadata = fs::metadata(&canonical).map_err(|error| {
        OfficeEngineError::new(
            OfficeEngineErrorCode::Unavailable,
            OfficeEngineRecovery::InstallComponent,
            format!(
                "Cannot inspect OfficeCLI executable `{}`: {error}",
                canonical.display()
            ),
        )
    })?;
    if !metadata.is_file() || !is_executable(&metadata) {
        return Err(OfficeEngineError::new(
            OfficeEngineErrorCode::Unavailable,
            OfficeEngineRecovery::InstallComponent,
            format!(
                "OfficeCLI candidate `{}` is not an executable file.",
                canonical.display()
            ),
        ));
    }
    if workspace_root.is_some_and(|root| canonical.starts_with(root)) {
        return Err(configuration_error(
            "OfficeCLI must not be loaded from the agent-writable workspace.",
        ));
    }
    Ok(canonical)
}

fn executable_revision(path: &Path) -> Result<String, OfficeEngineError> {
    let mut file = fs::File::open(path).map_err(|error| {
        configuration_error(format!(
            "Cannot open OfficeCLI executable `{}` for identity verification: {error}",
            path.display()
        ))
    })?;
    let before = file.metadata().map_err(|error| {
        configuration_error(format!(
            "Cannot inspect OfficeCLI executable `{}` for identity verification: {error}",
            path.display()
        ))
    })?;
    if !before.is_file() || before.len() > MAX_OFFICECLI_BINARY_BYTES {
        return Err(configuration_error(format!(
            "OfficeCLI executable must be a regular file no larger than {MAX_OFFICECLI_BINARY_BYTES} bytes."
        )));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            configuration_error(format!(
                "Cannot read OfficeCLI executable `{}` for identity verification: {error}",
                path.display()
            ))
        })?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > MAX_OFFICECLI_BINARY_BYTES {
            return Err(configuration_error(format!(
                "OfficeCLI executable exceeds the {MAX_OFFICECLI_BINARY_BYTES}-byte identity limit."
            )));
        }
        digest.update(&buffer[..read]);
    }
    let after = file.metadata().map_err(|error| {
        configuration_error(format!(
            "Cannot reinspect OfficeCLI executable `{}`: {error}",
            path.display()
        ))
    })?;
    if total != before.len() || after.len() != before.len() {
        return Err(configuration_error(
            "OfficeCLI executable changed during identity verification.",
        ));
    }
    Ok(format!(
        "{ENGINE_REVISION_PREFIX}{}",
        hex_lower(&digest.finalize())
    ))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(windows)]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    true
}

fn configuration_error(message: impl Into<String>) -> OfficeEngineError {
    OfficeEngineError::new(
        OfficeEngineErrorCode::InvalidConfiguration,
        OfficeEngineRecovery::ChangeConfiguration,
        message,
    )
}

#[cfg(all(test, unix))]
pub(super) fn discover_with_test_path(
    options: &OfficeCliDiscoveryOptions,
    path: Option<OsString>,
) -> Result<OfficeCliEngine, OfficeEngineError> {
    discover_with_path(options, path)
}
