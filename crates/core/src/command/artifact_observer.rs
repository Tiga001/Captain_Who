use super::*;
use std::collections::{HashMap as StdHashMap, HashSet};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Seek, SeekFrom};

pub(crate) const MAX_EXPECTED_OUTPUTS: usize = 32;
pub(crate) const MAX_ADDITIONAL_ROOTS: usize = 32;
pub(crate) const MAX_OBSERVATION_PATH_CHARS: usize = 4_096;

const MAX_DIRECTORY_ENTRIES_PER_SNAPSHOT: u64 = 100_000;
const MAX_OFFICE_FILES_PER_SNAPSHOT: u64 = 2_048;
const MAX_HASHED_BYTES_PER_SNAPSHOT: u64 = 512 * 1024 * 1024;
const MAX_SINGLE_FILE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_RECURSION_DEPTH: usize = 64;
const MAX_WARNINGS: usize = 64;
const MAX_OOXML_ENTRIES: usize = 65_535;
const MAX_OOXML_CENTRAL_DIRECTORY_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ZIP64_EOCD_RECORD_BYTES: u64 = 4 * 1024;
const ZIP_EOCD_FIXED_BYTES: usize = 22;
const ZIP_EOCD_MAX_SEARCH_BYTES: usize = ZIP_EOCD_FIXED_BYTES + u16::MAX as usize;
const ZIP64_EOCD_FIXED_BYTES: usize = 56;
const ZIP64_LOCATOR_BYTES: usize = 20;
const ZIP_CENTRAL_DIRECTORY_ENTRY_FIXED_BYTES: u64 = 46;
const MAX_SNAPSHOT_DURATION_MS: u64 = 2_000;
const MAX_REPORTED_CHANGES: usize = 256;
const MAX_REPORTED_CHANGE_BYTES: usize = 128 * 1024;

const EXCLUDED_WORKSPACE_DIRECTORIES: &[&str] =
    &[".git", ".hg", ".svn", "node_modules", "target", ".next"];

#[derive(Debug, Clone)]
pub(super) struct CommandArtifactObserver {
    workspace_root: Option<PathBuf>,
    workspace_included: bool,
    expected_outputs: Vec<ExpectedOutput>,
    additional_root_count: u64,
    roots: Vec<ObservationRoot>,
    setup_warnings: Vec<AgentCommandArtifactObservationWarning>,
    budget: ObservationBudget,
}

#[derive(Debug, Clone)]
struct ExpectedOutput {
    requested_path: String,
    resolved_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct ObservationRoot {
    path: PathBuf,
    mode: ObservationRootMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ObservationRootMode {
    ExpectedTarget,
    Additional,
    Workspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservationPathPurpose {
    ExpectedOutput,
    AdditionalRoot,
}

#[derive(Debug, Clone, Copy)]
struct ObservationBudget {
    max_directory_entries: u64,
    max_office_files: u64,
    max_hashed_bytes: u64,
    max_single_file_bytes: u64,
    max_recursion_depth: usize,
    max_duration: Duration,
}

impl Default for ObservationBudget {
    fn default() -> Self {
        Self {
            max_directory_entries: MAX_DIRECTORY_ENTRIES_PER_SNAPSHOT,
            max_office_files: MAX_OFFICE_FILES_PER_SNAPSHOT,
            max_hashed_bytes: MAX_HASHED_BYTES_PER_SNAPSHOT,
            max_single_file_bytes: MAX_SINGLE_FILE_BYTES,
            max_recursion_depth: MAX_RECURSION_DEPTH,
            max_duration: Duration::from_millis(MAX_SNAPSHOT_DURATION_MS),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct CommandArtifactCapture {
    files: BTreeMap<PathBuf, ObservedArtifact>,
    complete_roots: Vec<PathBuf>,
    /// Explicit subtrees omitted from otherwise complete roots.
    ///
    /// A more-specific complete root (for example an exact expected output or an explicitly
    /// requested additional root) overrides an enclosing exclusion. This lets the workspace scan
    /// exclude `node_modules` without treating the whole workspace as incomplete, while still
    /// allowing an exact expected target inside that directory to establish coverage.
    excluded_roots: Vec<PathBuf>,
    coverage: AgentCommandArtifactSnapshotCoverage,
    warnings: Vec<AgentCommandArtifactObservationWarning>,
}

#[derive(Debug, Clone)]
struct ObservedArtifact {
    kind: AgentCommandArtifactKind,
    metadata: AgentCommandArtifactMetadata,
    identity: Option<FileIdentity>,
    modified_ns: Option<u128>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    first: u64,
    second: u64,
}

#[derive(Debug)]
struct CaptureState {
    files: BTreeMap<PathBuf, ObservedArtifact>,
    complete_roots: Vec<PathBuf>,
    excluded_roots: Vec<PathBuf>,
    coverage: AgentCommandArtifactSnapshotCoverage,
    warnings: Vec<AgentCommandArtifactObservationWarning>,
    phase: AgentCommandArtifactObservationPhase,
    budget: ObservationBudget,
    deadline: Instant,
    cancellation_token: Option<AgentCancellationToken>,
}

impl CommandArtifactObserver {
    pub(super) fn prepare(
        workspace_root: Option<&Path>,
        cwd: &Path,
        request: Option<&AgentCommandArtifactObservationRequest>,
        permissions: AgentPermissions,
    ) -> Option<Self> {
        let request = request?;
        if !request
            .kinds
            .contains(&AgentCommandArtifactObservationKind::Office)
        {
            return None;
        }

        let workspace_root = workspace_root.map(Path::to_path_buf);
        let mut setup_warnings = Vec::new();
        let mut expected_outputs = Vec::new();
        let mut roots = Vec::new();

        for requested_path in request.expected_outputs.iter().take(MAX_EXPECTED_OUTPUTS) {
            let resolved_path = resolve_observation_path(
                requested_path,
                cwd,
                workspace_root.as_deref(),
                permissions,
                ObservationPathPurpose::ExpectedOutput,
            )
            .and_then(|path| {
                if office_artifact_kind(&path).is_none() {
                    Err((
                        "command.artifact.expected_output.unsupported_extension",
                        "预期输出必须使用受支持的 Office 文件扩展名（docx、xlsx、xlsm、csv 或 pptx）。"
                            .to_string(),
                    ))
                } else {
                    Ok(path)
                }
            })
            .map_err(|(code, message)| {
                push_warning(
                    &mut setup_warnings,
                    AgentCommandArtifactObservationWarning {
                        phase: AgentCommandArtifactObservationPhase::Setup,
                        code: code.to_string(),
                        path: Some(requested_path.clone()),
                        message,
                    },
                );
            })
            .ok();

            if let Some(path) = resolved_path.as_ref() {
                roots.push(ObservationRoot {
                    path: path.clone(),
                    mode: ObservationRootMode::ExpectedTarget,
                });
            }
            expected_outputs.push(ExpectedOutput {
                requested_path: requested_path.clone(),
                resolved_path,
            });
        }
        if request.expected_outputs.len() > MAX_EXPECTED_OUTPUTS {
            push_warning(
                &mut setup_warnings,
                setup_limit_warning(
                    "command.artifact.expected_outputs.limit_exceeded",
                    format!("预期输出数量超过上限 {MAX_EXPECTED_OUTPUTS}；超出部分未观察。"),
                ),
            );
        }

        let additional_roots = request
            .additional_roots
            .iter()
            .take(MAX_ADDITIONAL_ROOTS)
            .filter_map(|requested_path| {
                match resolve_observation_path(
                    requested_path,
                    cwd,
                    workspace_root.as_deref(),
                    permissions,
                    ObservationPathPurpose::AdditionalRoot,
                ) {
                    Ok(path) => Some(ObservationRoot {
                        path,
                        mode: ObservationRootMode::Additional,
                    }),
                    Err((code, message)) => {
                        push_warning(
                            &mut setup_warnings,
                            AgentCommandArtifactObservationWarning {
                                phase: AgentCommandArtifactObservationPhase::Setup,
                                code: code.to_string(),
                                path: Some(requested_path.clone()),
                                message,
                            },
                        );
                        None
                    }
                }
            })
            .collect::<Vec<_>>();
        let additional_root_count = additional_roots.len() as u64;
        roots.extend(additional_roots);
        if request.additional_roots.len() > MAX_ADDITIONAL_ROOTS {
            push_warning(
                &mut setup_warnings,
                setup_limit_warning(
                    "command.artifact.additional_roots.limit_exceeded",
                    format!("附加观察根数量超过上限 {MAX_ADDITIONAL_ROOTS}；超出部分未观察。"),
                ),
            );
        }

        if let Some(root) = workspace_root.as_ref() {
            roots.push(ObservationRoot {
                path: root.clone(),
                mode: ObservationRootMode::Workspace,
            });
        }
        deduplicate_roots(&mut roots);

        Some(Self {
            workspace_included: workspace_root.is_some(),
            workspace_root,
            expected_outputs,
            additional_root_count,
            roots,
            setup_warnings,
            budget: ObservationBudget::default(),
        })
    }

    pub(super) fn capture(
        &self,
        phase: AgentCommandArtifactObservationPhase,
        cancellation_token: Option<&AgentCancellationToken>,
    ) -> CommandArtifactCapture {
        debug_assert!(matches!(
            phase,
            AgentCommandArtifactObservationPhase::Before
                | AgentCommandArtifactObservationPhase::After
        ));
        let started = Instant::now();
        let mut state = CaptureState {
            files: BTreeMap::new(),
            complete_roots: Vec::new(),
            excluded_roots: Vec::new(),
            coverage: AgentCommandArtifactSnapshotCoverage::default(),
            warnings: Vec::new(),
            phase,
            budget: self.budget,
            deadline: started + self.budget.max_duration,
            cancellation_token: cancellation_token.cloned(),
        };
        for root in &self.roots {
            if state.coverage.truncated || capture_should_stop(&mut state) {
                break;
            }
            scan_root(root, &mut state);
        }
        state.coverage.duration_ms = started.elapsed().as_millis() as u64;
        CommandArtifactCapture {
            files: state.files,
            complete_roots: state.complete_roots,
            excluded_roots: state.excluded_roots,
            coverage: state.coverage,
            warnings: state.warnings,
        }
    }

    pub(super) fn finish(
        &self,
        before: CommandArtifactCapture,
        after: CommandArtifactCapture,
    ) -> AgentCommandArtifactObservation {
        let all_changes = diff_captures(&before, &after, self.workspace_root.as_deref());
        let mut warnings = self.setup_warnings.clone();
        for warning in before.warnings.iter().chain(after.warnings.iter()) {
            push_warning(&mut warnings, warning.clone());
        }
        // Compute explicit expected-output outcomes from the full in-memory diff before bounding
        // the model/audit-facing list. A noisy command must not hide its declared output contract.
        let expected_outputs = self.expected_outcomes(&before, &after, &all_changes, &mut warnings);
        let (changes, changes_omitted) = limit_reported_changes(
            all_changes,
            &self.expected_outputs,
            self.workspace_root.as_deref(),
        );
        let changes_truncated = changes_omitted > 0;
        if changes_truncated {
            push_warning(
                &mut warnings,
                AgentCommandArtifactObservationWarning {
                    phase: AgentCommandArtifactObservationPhase::After,
                    code: "command.artifact.changes.report_budget".to_string(),
                    path: None,
                    message: format!(
                        "Office 变更报告达到数量或序列化大小上限；省略 {changes_omitted} 项。"
                    ),
                },
            );
        }
        let coverage = AgentCommandArtifactObservationCoverage {
            workspace_included: self.workspace_included,
            expected_output_count: self.expected_outputs.len() as u64,
            additional_root_count: self.additional_root_count,
            before: before.coverage,
            after: after.coverage,
        };
        let observation_impaired = snapshot_coverage_is_partial(&coverage.before)
            || snapshot_coverage_is_partial(&coverage.after)
            || warnings.iter().any(warning_impairs_observation);
        let no_observable_roots = self.roots.is_empty()
            || (coverage.before.roots_scanned == 0 && coverage.after.roots_scanned == 0);
        let status = if no_observable_roots {
            AgentCommandArtifactObservationStatus::Failed
        } else if observation_impaired {
            AgentCommandArtifactObservationStatus::Partial
        } else {
            AgentCommandArtifactObservationStatus::Complete
        };
        let partial = status != AgentCommandArtifactObservationStatus::Complete;
        let mut stop_reasons =
            artifact_observation_stop_reasons(&coverage, &warnings, changes_truncated);
        if no_observable_roots
            && !stop_reasons
                .iter()
                .any(|reason| reason == "no_observable_roots")
        {
            stop_reasons.push("no_observable_roots".to_string());
        }
        let scanned = coverage
            .before
            .office_files_seen
            .saturating_add(coverage.after.office_files_seen);
        let returned = u64::try_from(changes.len()).unwrap_or(u64::MAX);

        AgentCommandArtifactObservation {
            schema_version: AGENT_COMMAND_ARTIFACT_OBSERVATION_SCHEMA_VERSION,
            status,
            partial: Some(partial),
            stop_reasons,
            scanned: Some(scanned),
            returned: Some(returned),
            omitted: Some(changes_omitted),
            coverage,
            changes,
            changes_truncated,
            changes_omitted,
            expected_outputs,
            warnings,
        }
    }

    fn expected_outcomes(
        &self,
        before: &CommandArtifactCapture,
        after: &CommandArtifactCapture,
        changes: &[AgentCommandArtifactChange],
        warnings: &mut Vec<AgentCommandArtifactObservationWarning>,
    ) -> Vec<AgentCommandExpectedArtifactOutcome> {
        self.expected_outputs
            .iter()
            .map(|expected| {
                let Some(path) = expected.resolved_path.as_ref() else {
                    push_warning(
                        warnings,
                        AgentCommandArtifactObservationWarning {
                            phase: AgentCommandArtifactObservationPhase::Setup,
                            code: "command.artifact.expected_output.unobserved".to_string(),
                            path: Some(expected.requested_path.clone()),
                            message: "预期 Office 输出路径未进入已授权观察边界。".to_string(),
                        },
                    );
                    return AgentCommandExpectedArtifactOutcome {
                        requested_path: expected.requested_path.clone(),
                        outcome: AgentCommandExpectedArtifactOutcomeKind::Unobserved,
                        path: None,
                        scope: None,
                        artifact_kind: None,
                        metadata: None,
                    };
                };
                let (display_path, scope) =
                    display_observed_path(path, self.workspace_root.as_deref());
                let direct_change = changes
                    .iter()
                    .find(|change| change.path == display_path && change.scope == scope);
                let renamed_away = changes.iter().find(|change| {
                    change.kind == AgentCommandArtifactChangeKind::Renamed
                        && change.previous_path.as_deref() == Some(display_path.as_str())
                        && change.previous_scope == Some(scope)
                });

                let outcome = if let Some(artifact) = after.files.get(path) {
                    if artifact.metadata.validation.status
                        == AgentCommandArtifactValidationStatus::Invalid
                    {
                        AgentCommandExpectedArtifactOutcomeKind::Invalid
                    } else {
                        match direct_change.map(|change| change.kind) {
                            Some(AgentCommandArtifactChangeKind::Created) => {
                                AgentCommandExpectedArtifactOutcomeKind::Created
                            }
                            Some(AgentCommandArtifactChangeKind::Modified) => {
                                AgentCommandExpectedArtifactOutcomeKind::Modified
                            }
                            Some(AgentCommandArtifactChangeKind::Replaced) => {
                                AgentCommandExpectedArtifactOutcomeKind::Replaced
                            }
                            Some(AgentCommandArtifactChangeKind::Renamed) => {
                                AgentCommandExpectedArtifactOutcomeKind::Renamed
                            }
                            _ if before.files.contains_key(path) => {
                                AgentCommandExpectedArtifactOutcomeKind::Unchanged
                            }
                            // Seeing a target only after the command is not proof that the command
                            // created it unless the before snapshot covered that exact path. A
                            // cancelled, budget-limited, or unstable before scan must remain
                            // epistemically explicit instead of being mislabeled as unchanged.
                            _ => AgentCommandExpectedArtifactOutcomeKind::Unobserved,
                        }
                    }
                } else if renamed_away.is_some() {
                    AgentCommandExpectedArtifactOutcomeKind::Renamed
                } else if path_is_covered(path, &after.complete_roots, &after.excluded_roots) {
                    AgentCommandExpectedArtifactOutcomeKind::Missing
                } else {
                    AgentCommandExpectedArtifactOutcomeKind::Unobserved
                };

                let (actual_path, actual_scope, artifact_kind, metadata) =
                    if let Some(artifact) = after.files.get(path) {
                        (
                            Some(display_path.clone()),
                            Some(scope),
                            Some(artifact.kind),
                            Some(artifact.metadata.clone()),
                        )
                    } else if let Some(change) = renamed_away {
                        (
                            Some(change.path.clone()),
                            Some(change.scope),
                            Some(change.artifact_kind),
                            change.after.clone(),
                        )
                    } else {
                        (
                            Some(display_path.clone()),
                            Some(scope),
                            office_artifact_kind(path),
                            None,
                        )
                    };

                if let Some((code, message)) = expected_outcome_warning(outcome) {
                    push_warning(
                        warnings,
                        AgentCommandArtifactObservationWarning {
                            phase: AgentCommandArtifactObservationPhase::After,
                            code: code.to_string(),
                            path: Some(display_path),
                            message: message.to_string(),
                        },
                    );
                }

                AgentCommandExpectedArtifactOutcome {
                    requested_path: expected.requested_path.clone(),
                    outcome,
                    path: actual_path,
                    scope: actual_scope,
                    artifact_kind,
                    metadata,
                }
            })
            .collect()
    }
}

fn resolve_observation_path(
    raw: &str,
    cwd: &Path,
    workspace_root: Option<&Path>,
    permissions: AgentPermissions,
    purpose: ObservationPathPurpose,
) -> Result<PathBuf, (&'static str, String)> {
    let raw = raw.trim();
    if raw.is_empty() || raw.contains('\0') {
        return Err((
            "command.artifact.path.invalid",
            "观察路径不能为空或包含空字符。".to_string(),
        ));
    }
    if raw.chars().count() > MAX_OBSERVATION_PATH_CHARS {
        return Err((
            "command.artifact.path.too_long",
            format!("观察路径最多允许 {MAX_OBSERVATION_PATH_CHARS} 个字符。"),
        ));
    }
    let expanded = expand_system_path(raw).map_err(|error| {
        (
            "command.artifact.path.invalid",
            format!("观察路径无法解析：{error}"),
        )
    })?;
    let candidate = match expanded {
        Some(path) => path,
        None if Path::new(raw).is_absolute() => PathBuf::from(raw),
        None => cwd.join(raw),
    };
    let normalized = normalize_absolute_path(&candidate).map_err(|message| {
        (
            "command.artifact.path.invalid",
            format!("观察路径无法规范化：{message}"),
        )
    })?;
    let inside_workspace = workspace_root.is_some_and(|root| normalized.starts_with(root));
    if !inside_workspace {
        match purpose {
            ObservationPathPurpose::ExpectedOutput
                if permissions.write != AgentWritePermission::All =>
            {
                return Err((
                    "command.artifact.path.outside_write_scope",
                    "观察 workspace 外的明确预期输出需要 write=all；observe 不会扩大权限。"
                        .to_string(),
                ));
            }
            ObservationPathPurpose::AdditionalRoot
                if permissions.read != AgentReadPermission::All =>
            {
                return Err((
                    "command.artifact.path.outside_read_scope",
                    "递归观察 workspace 外的附加根需要 read=all；observe 不会扩大权限。"
                        .to_string(),
                ));
            }
            _ => {}
        }
    }
    Ok(normalized)
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("路径不是绝对路径。".to_string());
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err("路径尝试越过文件系统根目录。".to_string());
                }
            }
        }
    }
    if normalized.is_absolute() {
        Ok(normalized)
    } else {
        Err("路径规范化后不是绝对路径。".to_string())
    }
}

fn deduplicate_roots(roots: &mut Vec<ObservationRoot>) {
    let mut seen = HashSet::new();
    roots.retain(|root| {
        let key = (root.path.clone(), root.mode);
        seen.insert(key)
    });
}

fn scan_root(root: &ObservationRoot, state: &mut CaptureState) {
    if capture_should_stop(state) {
        return;
    }
    if let Some(symlink) = first_symlink_component(&root.path) {
        state.coverage.symlinks_skipped += 1;
        push_warning(
            &mut state.warnings,
            AgentCommandArtifactObservationWarning {
                phase: state.phase,
                code: "command.artifact.symlink_skipped".to_string(),
                path: Some(symlink.to_string_lossy().to_string()),
                message: "观察器不会跟随符号链接。".to_string(),
            },
        );
        return;
    }

    let mut root_complete = true;
    match fs::symlink_metadata(&root.path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            state.coverage.symlinks_skipped += 1;
            root_complete = false;
            push_scan_warning(
                state,
                "command.artifact.symlink_skipped",
                &root.path,
                "观察器不会跟随符号链接。",
            );
        }
        Ok(metadata) if metadata.is_file() => {
            if !observe_file(&root.path, &metadata, state) {
                root_complete = false;
            }
        }
        Ok(metadata) if metadata.is_dir() && root.mode == ObservationRootMode::ExpectedTarget => {
            root_complete = false;
            push_scan_warning(
                state,
                "command.artifact.expected_output.not_regular_file",
                &root.path,
                "预期 Office 输出路径是目录，不会枚举其内容。",
            );
        }
        Ok(metadata) if metadata.is_dir() => {
            scan_directory(&root.path, 0, true, root.mode, state, &mut root_complete);
        }
        Ok(_) => {
            root_complete = false;
            push_scan_warning(
                state,
                "command.artifact.root.unsupported_file_type",
                &root.path,
                "观察根不是普通文件或目录。",
            );
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            root_complete = false;
            push_scan_warning(
                state,
                "command.artifact.root.unavailable",
                &root.path,
                &format!("无法读取观察根：{error}"),
            );
        }
    }
    state.coverage.roots_scanned += 1;
    if root_complete {
        state.complete_roots.push(root.path.clone());
    }
}

fn scan_directory(
    directory: &Path,
    depth: usize,
    recursive: bool,
    mode: ObservationRootMode,
    state: &mut CaptureState,
    root_complete: &mut bool,
) {
    if state.coverage.truncated || capture_should_stop(state) {
        *root_complete = false;
        return;
    }
    if depth > state.budget.max_recursion_depth {
        state.coverage.truncated = true;
        *root_complete = false;
        push_scan_warning(
            state,
            "command.artifact.scan.depth_limit",
            directory,
            "Office 产物扫描达到最大目录深度。",
        );
        return;
    }
    if let Some(symlink) = first_symlink_component(directory) {
        state.coverage.symlinks_skipped += 1;
        *root_complete = false;
        push_scan_warning(
            state,
            "command.artifact.symlink_skipped",
            &symlink,
            "观察器不会跟随符号链接目录。",
        );
        return;
    }

    let identity_before = match stable_directory_identity(directory) {
        Ok(identity) => identity,
        Err(message) => {
            *root_complete = false;
            push_scan_warning(
                state,
                "command.artifact.directory.identity_unavailable",
                directory,
                &message,
            );
            return;
        }
    };

    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            *root_complete = false;
            push_scan_warning(
                state,
                "command.artifact.directory.unavailable",
                directory,
                &format!("无法枚举观察目录：{error}"),
            );
            return;
        }
    };
    let remaining = state
        .budget
        .max_directory_entries
        .saturating_sub(state.coverage.directory_entries_scanned) as usize;
    if remaining == 0 {
        state.coverage.truncated = true;
        *root_complete = false;
        push_scan_warning(
            state,
            "command.artifact.scan.entry_budget",
            directory,
            "Office 产物扫描达到目录项预算。",
        );
        return;
    }

    let mut paths = Vec::new();
    let mut overflow = false;
    for entry in entries {
        if capture_should_stop(state) {
            *root_complete = false;
            return;
        }
        match entry {
            Ok(entry) if paths.len() < remaining => paths.push(entry.path()),
            Ok(_) => {
                overflow = true;
                break;
            }
            Err(error) => {
                *root_complete = false;
                push_scan_warning(
                    state,
                    "command.artifact.directory.entry_unavailable",
                    directory,
                    &format!("无法读取目录项：{error}"),
                );
            }
        }
    }
    paths.sort();
    if overflow {
        state.coverage.truncated = true;
        *root_complete = false;
        push_scan_warning(
            state,
            "command.artifact.scan.entry_budget",
            directory,
            "Office 产物扫描达到目录项预算。",
        );
    }

    for path in paths {
        if capture_should_stop(state) {
            *root_complete = false;
            break;
        }
        state.coverage.directory_entries_scanned += 1;
        if state.coverage.truncated {
            *root_complete = false;
            break;
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                *root_complete = false;
                push_scan_warning(
                    state,
                    "command.artifact.entry.unavailable",
                    &path,
                    &format!("无法检查目录项：{error}"),
                );
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            state.coverage.symlinks_skipped += 1;
            *root_complete = false;
            push_scan_warning(
                state,
                "command.artifact.symlink_skipped",
                &path,
                "观察器不会跟随符号链接。",
            );
        } else if metadata.is_file() {
            if !observe_file(&path, &metadata, state) {
                *root_complete = false;
            }
        } else if metadata.is_dir() && recursive {
            if mode == ObservationRootMode::Workspace && is_workspace_excluded_directory(&path) {
                state.coverage.excluded_directories += 1;
                // Fixed exclusions are intentionally outside the workspace observation domain.
                // Record the gap precisely instead of making the entire workspace incomplete;
                // created/deleted evidence later consults this boundary before claiming absence.
                state.excluded_roots.push(path);
                continue;
            }
            scan_directory(&path, depth + 1, recursive, mode, state, root_complete);
        }
    }
    verify_directory_identity(directory, identity_before, state, root_complete);
}

/// Returns whether the relevant file was fully accounted for in this snapshot. Unsupported files
/// are outside the Office observation domain and therefore count as accounted for.
fn observe_file(path: &Path, linked_metadata: &Metadata, state: &mut CaptureState) -> bool {
    if capture_should_stop(state) {
        return false;
    }
    let Some(kind) = office_artifact_kind(path) else {
        return true;
    };
    if state.files.contains_key(path) {
        return true;
    }
    state.coverage.office_files_seen += 1;
    if state.coverage.office_files_seen > state.budget.max_office_files {
        state.coverage.truncated = true;
        push_scan_warning(
            state,
            "command.artifact.scan.file_budget",
            path,
            "Office 产物扫描达到文件数量预算。",
        );
        return false;
    }

    let size = linked_metadata.len();
    let identity = file_identity(linked_metadata);
    let observed_modified_ns = modified_ns(linked_metadata);
    if size > state.budget.max_single_file_bytes {
        state.coverage.files_unhashed += 1;
        state.files.insert(
            path.to_path_buf(),
            ObservedArtifact {
                kind,
                metadata: AgentCommandArtifactMetadata {
                    size_bytes: size,
                    sha256: None,
                    validation: unchecked_validation(
                        "command.artifact.validation.file_too_large",
                        "文件超过单文件观察预算，未计算哈希或校验 OOXML。",
                    ),
                },
                identity,
                modified_ns: observed_modified_ns,
            },
        );
        push_scan_warning(
            state,
            "command.artifact.hash.file_too_large",
            path,
            "Office 文件超过单文件哈希预算。",
        );
        return true;
    }
    if state.coverage.bytes_hashed.saturating_add(size) > state.budget.max_hashed_bytes {
        state.coverage.files_unhashed += 1;
        state.coverage.truncated = true;
        state.files.insert(
            path.to_path_buf(),
            ObservedArtifact {
                kind,
                metadata: AgentCommandArtifactMetadata {
                    size_bytes: size,
                    sha256: None,
                    validation: unchecked_validation(
                        "command.artifact.validation.hash_budget",
                        "总哈希预算已耗尽，未校验此 Office 文件。",
                    ),
                },
                identity,
                modified_ns: observed_modified_ns,
            },
        );
        push_scan_warning(
            state,
            "command.artifact.hash.total_budget",
            path,
            "Office 文件总哈希预算已耗尽。",
        );
        return true;
    }

    let mut file = match open_regular_file_no_follow(path) {
        Ok(file) => file,
        Err(error) => {
            state.coverage.files_unhashed += 1;
            push_scan_warning(
                state,
                "command.artifact.file.open_failed",
                path,
                &format!("无法安全打开 Office 文件：{error}"),
            );
            return false;
        }
    };
    let opened_metadata = match file.metadata() {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => {
            state.coverage.files_unhashed += 1;
            push_scan_warning(
                state,
                "command.artifact.file.not_regular",
                path,
                "Office 观察目标不是普通文件。",
            );
            return false;
        }
        Err(error) => {
            state.coverage.files_unhashed += 1;
            push_scan_warning(
                state,
                "command.artifact.file.metadata_failed",
                path,
                &format!("无法读取 Office 文件元数据：{error}"),
            );
            return false;
        }
    };
    if file_identity(&opened_metadata) != identity || opened_metadata.len() != size {
        state.coverage.files_unhashed += 1;
        push_scan_warning(
            state,
            "command.artifact.file.changed_during_scan",
            path,
            "Office 文件在安全打开前发生变化，未记录不稳定快照。",
        );
        return false;
    }

    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if capture_should_stop(state) {
            state.coverage.files_unhashed += 1;
            return false;
        }
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(error) => {
                state.coverage.files_unhashed += 1;
                push_scan_warning(
                    state,
                    "command.artifact.file.read_failed",
                    path,
                    &format!("读取 Office 文件失败：{error}"),
                );
                return false;
            }
        }
    }
    if let Err(error) = file.seek(SeekFrom::Start(0)) {
        state.coverage.files_unhashed += 1;
        push_scan_warning(
            state,
            "command.artifact.file.seek_failed",
            path,
            &format!("无法复位 Office 文件以执行格式校验：{error}"),
        );
        return false;
    }
    if capture_should_stop(state) {
        state.coverage.files_unhashed += 1;
        return false;
    }
    let validation = validate_office_artifact(path, kind, file);
    if capture_should_stop(state) {
        state.coverage.files_unhashed += 1;
        return false;
    }
    let linked_after = match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => metadata,
        _ => {
            state.coverage.files_unhashed += 1;
            push_scan_warning(
                state,
                "command.artifact.file.changed_during_scan",
                path,
                "Office 文件在哈希过程中被替换或移除，未记录不稳定快照。",
            );
            return false;
        }
    };
    if file_identity(&linked_after) != identity
        || linked_after.len() != size
        || modified_ns(&linked_after) != observed_modified_ns
    {
        state.coverage.files_unhashed += 1;
        push_scan_warning(
            state,
            "command.artifact.file.changed_during_scan",
            path,
            "Office 文件在哈希过程中发生变化，未记录不稳定快照。",
        );
        return false;
    }

    state.coverage.files_hashed += 1;
    state.coverage.bytes_hashed += size;
    state.files.insert(
        path.to_path_buf(),
        ObservedArtifact {
            kind,
            metadata: AgentCommandArtifactMetadata {
                size_bytes: size,
                sha256: Some(format!("{:x}", hasher.finalize())),
                validation,
            },
            identity,
            modified_ns: observed_modified_ns,
        },
    );
    true
}

fn validate_office_artifact(
    path: &Path,
    kind: AgentCommandArtifactKind,
    mut file: File,
) -> AgentCommandArtifactValidation {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("csv"))
    {
        return AgentCommandArtifactValidation {
            status: AgentCommandArtifactValidationStatus::NotApplicable,
            code: Some("command.artifact.validation.not_applicable.csv".to_string()),
            message: Some("CSV 不是 OOXML ZIP 包；已保留文件哈希。".to_string()),
        };
    }

    let layout = match preflight_ooxml_zip(&mut file) {
        Ok(layout) => layout,
        Err(error) => {
            return invalid_validation(error.code, &error.message);
        }
    };
    if let Err(error) = file.seek(SeekFrom::Start(0)) {
        return invalid_validation(
            "command.artifact.validation.invalid_ooxml_zip",
            &format!("无法复位 OOXML ZIP 包：{error}"),
        );
    }

    // `ZipArchive::new` is deliberately called only after the untrusted EOCD/ZIP64 declarations
    // have been bounded and checked against the physical file. The zip crate may reserve or loop
    // based on those declarations, so a post-construction limit is too late for hostile inputs.
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(error) => {
            return invalid_validation(
                "command.artifact.validation.invalid_ooxml_zip",
                &format!("文件不是有效的 OOXML ZIP 包：{error}"),
            );
        }
    };
    if archive.len() != layout.entry_count as usize {
        return invalid_validation(
            "command.artifact.validation.inconsistent_ooxml_zip",
            "OOXML ZIP 实际条目数量与中央目录声明不一致。",
        );
    }
    if archive.by_name("[Content_Types].xml").is_err() {
        return invalid_validation(
            "command.artifact.validation.missing_content_types",
            "OOXML 包缺少 [Content_Types].xml。",
        );
    }
    let main_part = match kind {
        AgentCommandArtifactKind::Document => "word/document.xml",
        AgentCommandArtifactKind::Spreadsheet => "xl/workbook.xml",
        AgentCommandArtifactKind::Presentation => "ppt/presentation.xml",
    };
    if archive.by_name(main_part).is_err() {
        return invalid_validation(
            "command.artifact.validation.missing_main_part",
            &format!("OOXML 包缺少主文档部件 {main_part}。"),
        );
    }
    AgentCommandArtifactValidation {
        status: AgentCommandArtifactValidationStatus::Valid,
        code: Some("command.artifact.validation.valid_ooxml".to_string()),
        message: None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OoxmlZipLayout {
    entry_count: u64,
    central_directory_size: u64,
    central_directory_offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OoxmlZipPreflightError {
    code: &'static str,
    message: String,
}

impl OoxmlZipPreflightError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "command.artifact.validation.invalid_ooxml_zip",
            message: message.into(),
        }
    }

    fn limit(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Parses only bounded ZIP tail/fixed records before handing an OOXML package to `zip`.
///
/// This intentionally supports single-disk ZIP and ZIP64 only. OOXML packages do not need
/// split-disk archives, self-extracting prefixes, archive-extra-data records, or central-directory
/// signatures. Rejecting those uncommon layouts gives us an exact physical relationship between
/// the central directory and EOCD records, rather than trusting attacker-controlled offsets.
fn preflight_ooxml_zip(file: &mut File) -> Result<OoxmlZipLayout, OoxmlZipPreflightError> {
    let file_size = file
        .metadata()
        .map_err(|error| {
            OoxmlZipPreflightError::invalid(format!("无法读取 OOXML ZIP 元数据：{error}"))
        })?
        .len();
    if file_size < ZIP_EOCD_FIXED_BYTES as u64 {
        return Err(OoxmlZipPreflightError::invalid(
            "OOXML ZIP 包已截断：缺少 EOCD 记录。",
        ));
    }

    let tail_len = usize::try_from(file_size.min(ZIP_EOCD_MAX_SEARCH_BYTES as u64))
        .expect("bounded ZIP tail length always fits usize");
    let tail_offset = file_size - tail_len as u64;
    let mut tail = vec![0_u8; tail_len];
    read_exact_at(file, tail_offset, &mut tail).map_err(|error| {
        OoxmlZipPreflightError::invalid(format!("无法读取 OOXML ZIP 文件尾：{error}"))
    })?;

    let eocd_tail_offset = find_eocd_at_physical_end(&tail).ok_or_else(|| {
        OoxmlZipPreflightError::invalid("OOXML ZIP 包缺少位于文件末尾的完整 EOCD 记录。")
    })?;
    let eocd_offset = tail_offset
        .checked_add(eocd_tail_offset as u64)
        .ok_or_else(|| OoxmlZipPreflightError::invalid("OOXML ZIP EOCD 偏移溢出。"))?;
    let eocd = &tail[eocd_tail_offset..eocd_tail_offset + ZIP_EOCD_FIXED_BYTES];

    let legacy_disk = read_u16_le(eocd, 4);
    let legacy_directory_disk = read_u16_le(eocd, 6);
    let legacy_entries_on_disk = read_u16_le(eocd, 8);
    let legacy_entry_count = read_u16_le(eocd, 10);
    let legacy_directory_size = read_u32_le(eocd, 12);
    let legacy_directory_offset = read_u32_le(eocd, 16);
    let legacy_directory_end =
        u64::from(legacy_directory_offset).checked_add(u64::from(legacy_directory_size));
    let has_zip64_sentinel = legacy_disk == u16::MAX
        || legacy_directory_disk == u16::MAX
        || legacy_entries_on_disk == u16::MAX
        || legacy_entry_count == u16::MAX
        || legacy_directory_size == u32::MAX
        || legacy_directory_offset == u32::MAX;
    let locator_offset = eocd_offset.checked_sub(ZIP64_LOCATOR_BYTES as u64);
    let locator = locator_offset
        .and_then(|offset| read_fixed_at::<ZIP64_LOCATOR_BYTES>(file, offset).ok())
        .filter(|record| record[..4] == [0x50, 0x4b, 0x06, 0x07]);
    // Some producers emit ZIP64 metadata before it is strictly required. Prefer the classic
    // record whenever it already points exactly to this EOCD; otherwise a structurally present
    // locator is authoritative and must pass all ZIP64 checks.
    let uses_zip64 =
        has_zip64_sentinel || locator.is_some_and(|_| legacy_directory_end != Some(eocd_offset));

    let (layout, directory_boundary) = if uses_zip64 {
        let locator_offset = locator_offset
            .ok_or_else(|| OoxmlZipPreflightError::invalid("ZIP64 EOCD 定位器偏移无效。"))?;
        let locator = locator.ok_or_else(|| {
            OoxmlZipPreflightError::invalid("OOXML ZIP 使用 ZIP64 哨兵值但缺少 ZIP64 EOCD 定位器。")
        })?;
        let locator_disk = read_u32_le(&locator, 4);
        let zip64_eocd_offset = read_u64_le(&locator, 8);
        let locator_disk_count = read_u32_le(&locator, 16);
        if locator_disk != 0 || locator_disk_count != 1 {
            return Err(OoxmlZipPreflightError::limit(
                "command.artifact.validation.unsupported_multidisk_ooxml_zip",
                "OOXML ZIP 不支持分卷 ZIP64 包。",
            ));
        }
        if zip64_eocd_offset >= locator_offset {
            return Err(OoxmlZipPreflightError::invalid(
                "ZIP64 EOCD 偏移超出其定位器边界。",
            ));
        }

        let zip64_eocd =
            read_fixed_at::<ZIP64_EOCD_FIXED_BYTES>(file, zip64_eocd_offset).map_err(|error| {
                OoxmlZipPreflightError::invalid(format!("ZIP64 EOCD 记录已截断：{error}"))
            })?;
        if zip64_eocd[..4] != [0x50, 0x4b, 0x06, 0x06] {
            return Err(OoxmlZipPreflightError::invalid(
                "ZIP64 EOCD 定位器未指向有效记录。",
            ));
        }
        let zip64_record_payload_size = read_u64_le(&zip64_eocd, 4);
        if zip64_record_payload_size < 44 {
            return Err(OoxmlZipPreflightError::invalid(
                "ZIP64 EOCD 记录长度小于规范最小值。",
            ));
        }
        let zip64_record_size = zip64_record_payload_size
            .checked_add(12)
            .ok_or_else(|| OoxmlZipPreflightError::invalid("ZIP64 EOCD 记录长度溢出。"))?;
        if zip64_record_size > MAX_ZIP64_EOCD_RECORD_BYTES {
            return Err(OoxmlZipPreflightError::limit(
                "command.artifact.validation.zip64_eocd_too_large",
                format!("ZIP64 EOCD 记录长度超过安全上限 {MAX_ZIP64_EOCD_RECORD_BYTES} 字节。"),
            ));
        }
        if zip64_eocd_offset.checked_add(zip64_record_size) != Some(locator_offset) {
            return Err(OoxmlZipPreflightError::invalid(
                "ZIP64 EOCD 长度、偏移与定位器位置不一致。",
            ));
        }

        let zip64_disk = read_u32_le(&zip64_eocd, 16);
        let zip64_directory_disk = read_u32_le(&zip64_eocd, 20);
        let entries_on_disk = read_u64_le(&zip64_eocd, 24);
        let entry_count = read_u64_le(&zip64_eocd, 32);
        let central_directory_size = read_u64_le(&zip64_eocd, 40);
        let central_directory_offset = read_u64_le(&zip64_eocd, 48);
        if zip64_disk != 0 || zip64_directory_disk != 0 || entries_on_disk != entry_count {
            return Err(OoxmlZipPreflightError::limit(
                "command.artifact.validation.unsupported_multidisk_ooxml_zip",
                "OOXML ZIP 不支持分卷包，且本卷条目数必须等于总条目数。",
            ));
        }
        if !legacy_u16_matches(legacy_disk, u64::from(zip64_disk))
            || !legacy_u16_matches(legacy_directory_disk, u64::from(zip64_directory_disk))
            || !legacy_u16_matches(legacy_entries_on_disk, entries_on_disk)
            || !legacy_u16_matches(legacy_entry_count, entry_count)
            || !legacy_u32_matches(legacy_directory_size, central_directory_size)
            || !legacy_u32_matches(legacy_directory_offset, central_directory_offset)
        {
            return Err(OoxmlZipPreflightError::invalid(
                "ZIP64 EOCD 与经典 EOCD 中的非哨兵字段不一致。",
            ));
        }

        (
            OoxmlZipLayout {
                entry_count,
                central_directory_size,
                central_directory_offset,
            },
            zip64_eocd_offset,
        )
    } else {
        if legacy_disk != 0
            || legacy_directory_disk != 0
            || legacy_entries_on_disk != legacy_entry_count
        {
            return Err(OoxmlZipPreflightError::limit(
                "command.artifact.validation.unsupported_multidisk_ooxml_zip",
                "OOXML ZIP 不支持分卷包，且本卷条目数必须等于总条目数。",
            ));
        }
        (
            OoxmlZipLayout {
                entry_count: u64::from(legacy_entry_count),
                central_directory_size: u64::from(legacy_directory_size),
                central_directory_offset: u64::from(legacy_directory_offset),
            },
            eocd_offset,
        )
    };

    validate_ooxml_central_directory_layout(layout, directory_boundary, file_size)?;
    Ok(layout)
}

fn validate_ooxml_central_directory_layout(
    layout: OoxmlZipLayout,
    directory_boundary: u64,
    file_size: u64,
) -> Result<(), OoxmlZipPreflightError> {
    if layout.entry_count > MAX_OOXML_ENTRIES as u64 {
        return Err(OoxmlZipPreflightError::limit(
            "command.artifact.validation.too_many_ooxml_entries",
            format!("OOXML ZIP 条目数量超过安全上限 {MAX_OOXML_ENTRIES}。"),
        ));
    }
    if layout.central_directory_size > MAX_OOXML_CENTRAL_DIRECTORY_BYTES {
        return Err(OoxmlZipPreflightError::limit(
            "command.artifact.validation.ooxml_central_directory_too_large",
            format!("OOXML ZIP 中央目录超过安全上限 {MAX_OOXML_CENTRAL_DIRECTORY_BYTES} 字节。"),
        ));
    }
    if directory_boundary > file_size {
        return Err(OoxmlZipPreflightError::invalid(
            "OOXML ZIP 中央目录边界超出文件大小。",
        ));
    }
    let directory_end = layout
        .central_directory_offset
        .checked_add(layout.central_directory_size)
        .ok_or_else(|| OoxmlZipPreflightError::invalid("OOXML ZIP 中央目录范围溢出。"))?;
    if directory_end != directory_boundary {
        return Err(OoxmlZipPreflightError::invalid(
            "OOXML ZIP 中央目录长度、偏移与 EOCD 位置不一致。",
        ));
    }
    let minimum_directory_size = layout
        .entry_count
        .checked_mul(ZIP_CENTRAL_DIRECTORY_ENTRY_FIXED_BYTES)
        .ok_or_else(|| OoxmlZipPreflightError::invalid("OOXML ZIP 中央目录最小长度溢出。"))?;
    if layout.central_directory_size < minimum_directory_size {
        return Err(OoxmlZipPreflightError::invalid(
            "OOXML ZIP 中央目录不足以容纳声明的条目数量。",
        ));
    }
    if layout.entry_count == 0 && layout.central_directory_size != 0 {
        return Err(OoxmlZipPreflightError::invalid(
            "空 OOXML ZIP 不得声明非空中央目录。",
        ));
    }
    Ok(())
}

fn find_eocd_at_physical_end(tail: &[u8]) -> Option<usize> {
    if tail.len() < ZIP_EOCD_FIXED_BYTES {
        return None;
    }
    (0..=tail.len() - ZIP_EOCD_FIXED_BYTES)
        .rev()
        .find(|&offset| {
            tail[offset..offset + 4] == [0x50, 0x4b, 0x05, 0x06]
                && offset
                    .checked_add(ZIP_EOCD_FIXED_BYTES)
                    .and_then(|end| end.checked_add(read_u16_le(tail, offset + 20) as usize))
                    == Some(tail.len())
        })
}

fn read_exact_at(file: &mut File, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buffer)
}

fn read_fixed_at<const N: usize>(file: &mut File, offset: u64) -> io::Result<[u8; N]> {
    let mut buffer = [0_u8; N];
    read_exact_at(file, offset, &mut buffer)?;
    Ok(buffer)
}

fn read_u16_le(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_u64_le(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

fn legacy_u16_matches(legacy: u16, actual: u64) -> bool {
    legacy == u16::MAX || u64::from(legacy) == actual
}

fn legacy_u32_matches(legacy: u32, actual: u64) -> bool {
    legacy == u32::MAX || u64::from(legacy) == actual
}

fn open_regular_file_no_follow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options.open(path)
}

fn office_artifact_kind(path: &Path) -> Option<AgentCommandArtifactKind> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())?
        .to_ascii_lowercase()
        .as_str()
    {
        "docx" => Some(AgentCommandArtifactKind::Document),
        "xlsx" | "xlsm" | "csv" => Some(AgentCommandArtifactKind::Spreadsheet),
        "pptx" => Some(AgentCommandArtifactKind::Presentation),
        _ => None,
    }
}

fn diff_captures(
    before: &CommandArtifactCapture,
    after: &CommandArtifactCapture,
    workspace_root: Option<&Path>,
) -> Vec<AgentCommandArtifactChange> {
    let mut changes = Vec::new();
    let mut deleted = Vec::new();
    let mut created = Vec::new();

    for (path, old) in &before.files {
        if let Some(new) = after.files.get(path) {
            if let Some(kind) = same_path_change_kind(old, new) {
                changes.push(build_change(
                    kind,
                    path,
                    None,
                    old.kind,
                    Some(old.metadata.clone()),
                    Some(new.metadata.clone()),
                    workspace_root,
                ));
            }
        } else if path_is_covered(path, &after.complete_roots, &after.excluded_roots) {
            // Absence is evidence only when the opposite snapshot fully covered this path. A
            // time/entry/hash budget, cancellation, directory race, or read failure must never be
            // converted into a fabricated deletion.
            deleted.push((path, old));
        }
    }
    for (path, new) in &after.files {
        if !before.files.contains_key(path)
            && path_is_covered(path, &before.complete_roots, &before.excluded_roots)
        {
            // Symmetrically, a file seen only after the command is a creation only when the before
            // snapshot proved the path absent. Rename pairing is built solely from these qualified
            // creation/deletion candidates.
            created.push((path, new));
        }
    }

    let mut deleted_by_digest: StdHashMap<(AgentCommandArtifactKind, String, u64), Vec<usize>> =
        StdHashMap::new();
    let mut created_by_digest: StdHashMap<(AgentCommandArtifactKind, String, u64), Vec<usize>> =
        StdHashMap::new();
    for (index, (_, artifact)) in deleted.iter().enumerate() {
        if let Some(digest) = artifact.metadata.sha256.as_ref() {
            deleted_by_digest
                .entry((artifact.kind, digest.clone(), artifact.metadata.size_bytes))
                .or_default()
                .push(index);
        }
    }
    for (index, (_, artifact)) in created.iter().enumerate() {
        if let Some(digest) = artifact.metadata.sha256.as_ref() {
            created_by_digest
                .entry((artifact.kind, digest.clone(), artifact.metadata.size_bytes))
                .or_default()
                .push(index);
        }
    }

    let mut renamed_deleted = HashSet::new();
    let mut renamed_created = HashSet::new();
    for (key, deleted_indexes) in &deleted_by_digest {
        let Some(created_indexes) = created_by_digest.get(key) else {
            continue;
        };
        if deleted_indexes.len() == 1 && created_indexes.len() == 1 {
            let deleted_index = deleted_indexes[0];
            let created_index = created_indexes[0];
            let (old_path, old) = deleted[deleted_index];
            let (new_path, new) = created[created_index];
            changes.push(build_change(
                AgentCommandArtifactChangeKind::Renamed,
                new_path,
                Some(old_path),
                new.kind,
                Some(old.metadata.clone()),
                Some(new.metadata.clone()),
                workspace_root,
            ));
            renamed_deleted.insert(deleted_index);
            renamed_created.insert(created_index);
        }
    }

    for (index, (path, artifact)) in deleted.into_iter().enumerate() {
        if !renamed_deleted.contains(&index) {
            changes.push(build_change(
                AgentCommandArtifactChangeKind::Deleted,
                path,
                None,
                artifact.kind,
                Some(artifact.metadata.clone()),
                None,
                workspace_root,
            ));
        }
    }
    for (index, (path, artifact)) in created.into_iter().enumerate() {
        if !renamed_created.contains(&index) {
            changes.push(build_change(
                AgentCommandArtifactChangeKind::Created,
                path,
                None,
                artifact.kind,
                None,
                Some(artifact.metadata.clone()),
                workspace_root,
            ));
        }
    }
    changes.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| change_kind_order(left.kind).cmp(&change_kind_order(right.kind)))
    });
    changes
}

fn limit_reported_changes(
    mut changes: Vec<AgentCommandArtifactChange>,
    expected_outputs: &[ExpectedOutput],
    workspace_root: Option<&Path>,
) -> (Vec<AgentCommandArtifactChange>, u64) {
    let expected_paths = expected_outputs
        .iter()
        .filter_map(|expected| expected.resolved_path.as_deref())
        .map(|path| display_observed_path(path, workspace_root))
        .collect::<Vec<_>>();

    // Stable sorting keeps the existing deterministic path order within both groups while
    // attempting explicit expected-output changes before incidental workspace changes.
    changes.sort_by_key(|change| !change_matches_expected_output(change, &expected_paths));

    let mut reported = Vec::with_capacity(changes.len().min(MAX_REPORTED_CHANGES));
    let mut serialized_bytes = 2_usize; // JSON array brackets.
    let mut omitted = 0_u64;
    for change in changes {
        let encoded_len = serde_json::to_vec(&change)
            .map(|encoded| encoded.len())
            .unwrap_or(MAX_REPORTED_CHANGE_BYTES.saturating_add(1));
        let separator_len = usize::from(!reported.is_empty());
        let fits = reported.len() < MAX_REPORTED_CHANGES
            && serialized_bytes
                .saturating_add(separator_len)
                .saturating_add(encoded_len)
                <= MAX_REPORTED_CHANGE_BYTES;
        if fits {
            serialized_bytes += separator_len + encoded_len;
            reported.push(change);
        } else {
            omitted += 1;
        }
    }
    reported.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| change_kind_order(left.kind).cmp(&change_kind_order(right.kind)))
    });
    (reported, omitted)
}

fn change_matches_expected_output(
    change: &AgentCommandArtifactChange,
    expected_paths: &[(String, AgentCommandArtifactScope)],
) -> bool {
    expected_paths.iter().any(|(path, scope)| {
        (&change.path == path && &change.scope == scope)
            || (change.previous_path.as_ref() == Some(path)
                && change.previous_scope.as_ref() == Some(scope))
    })
}

fn same_path_change_kind(
    before: &ObservedArtifact,
    after: &ObservedArtifact,
) -> Option<AgentCommandArtifactChangeKind> {
    if before.identity.is_some() && after.identity.is_some() && before.identity != after.identity {
        return Some(AgentCommandArtifactChangeKind::Replaced);
    }
    match (
        before.metadata.sha256.as_ref(),
        after.metadata.sha256.as_ref(),
    ) {
        (Some(before_digest), Some(after_digest)) if before_digest != after_digest => {
            Some(AgentCommandArtifactChangeKind::Modified)
        }
        (Some(_), Some(_)) => None,
        _ if before.metadata.size_bytes != after.metadata.size_bytes
            || before.modified_ns != after.modified_ns =>
        {
            Some(AgentCommandArtifactChangeKind::Modified)
        }
        _ => None,
    }
}

fn build_change(
    kind: AgentCommandArtifactChangeKind,
    path: &Path,
    previous_path: Option<&Path>,
    artifact_kind: AgentCommandArtifactKind,
    before: Option<AgentCommandArtifactMetadata>,
    after: Option<AgentCommandArtifactMetadata>,
    workspace_root: Option<&Path>,
) -> AgentCommandArtifactChange {
    let (path, scope) = display_observed_path(path, workspace_root);
    let (previous_path, previous_scope) = previous_path
        .map(|path| display_observed_path(path, workspace_root))
        .map_or((None, None), |(path, scope)| (Some(path), Some(scope)));
    AgentCommandArtifactChange {
        kind,
        artifact_kind,
        path,
        scope,
        previous_path,
        previous_scope,
        before,
        after,
    }
}

fn display_observed_path(
    path: &Path,
    workspace_root: Option<&Path>,
) -> (String, AgentCommandArtifactScope) {
    if let Some(relative) = workspace_root.and_then(|root| path.strip_prefix(root).ok()) {
        (display_path(relative), AgentCommandArtifactScope::Workspace)
    } else {
        (display_path(path), AgentCommandArtifactScope::External)
    }
}

fn display_path(path: &Path) -> String {
    let parts = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if path.is_absolute() {
        format!("/{}", parts.join("/"))
    } else if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

fn first_symlink_component(path: &Path) -> Option<PathBuf> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Some(current),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
            Err(_) => return None,
        }
    }
    None
}

fn stable_directory_identity(path: &Path) -> Result<FileIdentity, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("无法读取目录身份：{error}"))?;
    if metadata.file_type().is_symlink() {
        return Err("目录在枚举前已变为符号链接。".to_string());
    }
    if !metadata.is_dir() {
        return Err("目录在枚举前已不再是目录。".to_string());
    }
    file_identity(&metadata).ok_or_else(|| "当前平台无法建立稳定目录身份。".to_string())
}

fn verify_directory_identity(
    directory: &Path,
    identity_before: FileIdentity,
    state: &mut CaptureState,
    root_complete: &mut bool,
) {
    if stable_directory_identity(directory).is_ok_and(|identity| identity == identity_before) {
        return;
    }

    // A check-then-read_dir race cannot be completely eliminated without a portable no-follow
    // directory-handle API. Conservatively discard every result obtained below the unstable
    // subtree, including completeness evidence, so a replacement directory cannot leak into the
    // reported snapshot as if it had been the one originally authorized.
    state.files.retain(|path, _| !path.starts_with(directory));
    state
        .complete_roots
        .retain(|path| !path.starts_with(directory));
    *root_complete = false;
    push_scan_warning(
        state,
        "command.artifact.directory.identity_changed",
        directory,
        "观察目录在枚举期间被替换或发生身份变化；已丢弃该子树的扫描结果。",
    );
}

fn capture_should_stop(state: &mut CaptureState) -> bool {
    if state.coverage.truncated {
        return true;
    }
    if state.phase == AgentCommandArtifactObservationPhase::Before
        && state
            .cancellation_token
            .as_ref()
            .is_some_and(AgentCancellationToken::is_cancelled)
    {
        state.coverage.cancelled = true;
        state.coverage.truncated = true;
        push_warning(
            &mut state.warnings,
            AgentCommandArtifactObservationWarning {
                phase: state.phase,
                code: "command.artifact.scan.cancelled".to_string(),
                path: None,
                message: "命令在执行前已取消；before 产物快照已尽快停止。".to_string(),
            },
        );
        return true;
    }
    if Instant::now() >= state.deadline {
        state.coverage.time_budget_exceeded = true;
        state.coverage.truncated = true;
        push_warning(
            &mut state.warnings,
            AgentCommandArtifactObservationWarning {
                phase: state.phase,
                code: "command.artifact.scan.time_budget".to_string(),
                path: None,
                message: format!(
                    "Office 产物快照达到 {}ms 墙钟时间预算。",
                    state.budget.max_duration.as_millis()
                ),
            },
        );
        return true;
    }
    false
}

fn is_workspace_excluded_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| EXCLUDED_WORKSPACE_DIRECTORIES.contains(&name))
}

fn path_is_covered(path: &Path, complete_roots: &[PathBuf], excluded_roots: &[PathBuf]) -> bool {
    let most_specific_complete = complete_roots
        .iter()
        .filter(|root| path == root.as_path() || path.starts_with(root))
        .map(|root| root.components().count())
        .max();
    let Some(complete_depth) = most_specific_complete else {
        return false;
    };
    let most_specific_exclusion = excluded_roots
        .iter()
        .filter(|root| path == root.as_path() || path.starts_with(root))
        .map(|root| root.components().count())
        .max();

    // An exact/more-specific explicitly scanned root overrides an enclosing fixed exclusion.
    most_specific_exclusion.is_none_or(|excluded_depth| complete_depth >= excluded_depth)
}

#[cfg(unix)]
fn file_identity(metadata: &Metadata) -> Option<FileIdentity> {
    use std::os::unix::fs::MetadataExt;
    Some(FileIdentity {
        first: metadata.dev(),
        second: metadata.ino(),
    })
}

#[cfg(not(unix))]
fn file_identity(_metadata: &Metadata) -> Option<FileIdentity> {
    None
}

fn modified_ns(metadata: &Metadata) -> Option<u128> {
    metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos())
}

fn unchecked_validation(code: &str, message: &str) -> AgentCommandArtifactValidation {
    AgentCommandArtifactValidation {
        status: AgentCommandArtifactValidationStatus::Unchecked,
        code: Some(code.to_string()),
        message: Some(message.to_string()),
    }
}

fn invalid_validation(code: &str, message: &str) -> AgentCommandArtifactValidation {
    AgentCommandArtifactValidation {
        status: AgentCommandArtifactValidationStatus::Invalid,
        code: Some(code.to_string()),
        message: Some(message.to_string()),
    }
}

fn setup_limit_warning(code: &str, message: String) -> AgentCommandArtifactObservationWarning {
    AgentCommandArtifactObservationWarning {
        phase: AgentCommandArtifactObservationPhase::Setup,
        code: code.to_string(),
        path: None,
        message,
    }
}

fn push_scan_warning(state: &mut CaptureState, code: &str, path: &Path, message: &str) {
    push_warning(
        &mut state.warnings,
        AgentCommandArtifactObservationWarning {
            phase: state.phase,
            code: code.to_string(),
            path: Some(path.to_string_lossy().to_string()),
            message: message.to_string(),
        },
    );
}

fn push_warning(
    warnings: &mut Vec<AgentCommandArtifactObservationWarning>,
    warning: AgentCommandArtifactObservationWarning,
) {
    if warnings.len() < MAX_WARNINGS {
        warnings.push(warning);
    } else if warnings
        .last()
        .is_some_and(|last| last.code != "command.artifact.warning_budget")
    {
        warnings[MAX_WARNINGS - 1] = AgentCommandArtifactObservationWarning {
            phase: warning.phase,
            code: "command.artifact.warning_budget".to_string(),
            path: None,
            message: format!("观察警告超过上限 {MAX_WARNINGS}；其余警告已省略。"),
        };
    }
}

fn snapshot_coverage_is_partial(coverage: &AgentCommandArtifactSnapshotCoverage) -> bool {
    coverage.truncated || coverage.files_unhashed > 0 || coverage.symlinks_skipped > 0
}

fn artifact_observation_stop_reasons(
    coverage: &AgentCommandArtifactObservationCoverage,
    warnings: &[AgentCommandArtifactObservationWarning],
    changes_truncated: bool,
) -> Vec<String> {
    let mut reasons = Vec::new();
    let mut push = |reason: &str| {
        if !reasons.iter().any(|existing| existing == reason) {
            reasons.push(reason.to_string());
        }
    };

    for (phase, snapshot) in [("before", &coverage.before), ("after", &coverage.after)] {
        if snapshot.cancelled {
            push(&format!("{phase}_cancelled"));
        }
        if snapshot.time_budget_exceeded {
            push(&format!("{phase}_time_budget"));
        }
        if snapshot.truncated && !snapshot.cancelled && !snapshot.time_budget_exceeded {
            push(&format!("{phase}_scan_limit"));
        }
        if snapshot.files_unhashed > 0 {
            push(&format!("{phase}_unhashed_files"));
        }
        if snapshot.symlinks_skipped > 0 {
            push(&format!("{phase}_symlinks_skipped"));
        }
    }
    if changes_truncated {
        push("change_report_limit");
    }
    for warning in warnings
        .iter()
        .filter(|warning| warning_impairs_observation(warning))
    {
        push(&warning.code);
    }
    reasons
}

fn warning_impairs_observation(warning: &AgentCommandArtifactObservationWarning) -> bool {
    !matches!(
        warning.code.as_str(),
        "command.artifact.expected_output.missing"
            | "command.artifact.expected_output.unchanged"
            | "command.artifact.expected_output.invalid"
    )
}

fn expected_outcome_warning(
    outcome: AgentCommandExpectedArtifactOutcomeKind,
) -> Option<(&'static str, &'static str)> {
    match outcome {
        AgentCommandExpectedArtifactOutcomeKind::Missing => Some((
            "command.artifact.expected_output.missing",
            "命令结束后未发现预期 Office 输出。",
        )),
        AgentCommandExpectedArtifactOutcomeKind::Unchanged => Some((
            "command.artifact.expected_output.unchanged",
            "命令结束后预期 Office 输出未发生变化。",
        )),
        AgentCommandExpectedArtifactOutcomeKind::Unobserved => Some((
            "command.artifact.expected_output.unobserved",
            "观察边界或预算不足，无法确认预期 Office 输出。",
        )),
        AgentCommandExpectedArtifactOutcomeKind::Invalid => Some((
            "command.artifact.expected_output.invalid",
            "预期输出存在，但未通过基础 Office/OOXML 校验。",
        )),
        _ => None,
    }
}

fn change_kind_order(kind: AgentCommandArtifactChangeKind) -> u8 {
    match kind {
        AgentCommandArtifactChangeKind::Created => 0,
        AgentCommandArtifactChangeKind::Modified => 1,
        AgentCommandArtifactChangeKind::Replaced => 2,
        AgentCommandArtifactChangeKind::Deleted => 3,
        AgentCommandArtifactChangeKind::Renamed => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;

    fn permissions_with(
        read: AgentReadPermission,
        write: AgentWritePermission,
    ) -> AgentPermissions {
        AgentPermissions {
            read,
            write,
            ..AgentPermissions::default()
        }
    }

    fn permissions(write: AgentWritePermission) -> AgentPermissions {
        permissions_with(AgentReadPermission::WorkspaceOnly, write)
    }

    fn observe(expected_outputs: &[&str]) -> AgentCommandArtifactObservationRequest {
        AgentCommandArtifactObservationRequest {
            kinds: vec![AgentCommandArtifactObservationKind::Office],
            expected_outputs: expected_outputs
                .iter()
                .map(|path| (*path).to_string())
                .collect(),
            additional_roots: Vec::new(),
        }
    }

    fn write_ooxml(path: &Path, main_part: &str, marker: &str) {
        let file = File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        archive.start_file("[Content_Types].xml", options).unwrap();
        archive
            .write_all(
                b"<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"/>",
            )
            .unwrap();
        archive.start_file(main_part, options).unwrap();
        archive
            .write_all(format!("<main>{marker}</main>").as_bytes())
            .unwrap();
        archive.finish().unwrap();
    }

    fn write_workbook(path: &Path, marker: &str) {
        write_ooxml(path, "xl/workbook.xml", marker);
    }

    fn append_u16(bytes: &mut Vec<u8>, value: u16) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn append_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn append_u64(bytes: &mut Vec<u8>, value: u64) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn append_classic_eocd(
        bytes: &mut Vec<u8>,
        entry_count: u16,
        directory_size: u32,
        directory_offset: u32,
        comment_len: u16,
    ) {
        bytes.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
        append_u16(bytes, 0);
        append_u16(bytes, 0);
        append_u16(bytes, entry_count);
        append_u16(bytes, entry_count);
        append_u32(bytes, directory_size);
        append_u32(bytes, directory_offset);
        append_u16(bytes, comment_len);
    }

    fn zip64_metadata_stub(
        entry_count: u64,
        directory_size: u64,
        directory_offset: u64,
        zip64_payload_size: u64,
        locator_record_offset: u64,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0x50, 0x4b, 0x06, 0x06]);
        append_u64(&mut bytes, zip64_payload_size);
        append_u16(&mut bytes, 45);
        append_u16(&mut bytes, 45);
        append_u32(&mut bytes, 0);
        append_u32(&mut bytes, 0);
        append_u64(&mut bytes, entry_count);
        append_u64(&mut bytes, entry_count);
        append_u64(&mut bytes, directory_size);
        append_u64(&mut bytes, directory_offset);
        debug_assert_eq!(bytes.len(), ZIP64_EOCD_FIXED_BYTES);

        bytes.extend_from_slice(&[0x50, 0x4b, 0x06, 0x07]);
        append_u32(&mut bytes, 0);
        append_u64(&mut bytes, locator_record_offset);
        append_u32(&mut bytes, 1);
        append_classic_eocd(&mut bytes, u16::MAX, u32::MAX, u32::MAX, 0);
        bytes
    }

    fn rewrite_with_zip64_eocd(mut bytes: Vec<u8>) -> Vec<u8> {
        let classic_offset = find_eocd_at_physical_end(&bytes).unwrap();
        assert_eq!(classic_offset + ZIP_EOCD_FIXED_BYTES, bytes.len());
        let classic = bytes[classic_offset..].to_vec();
        let entry_count = read_u16_le(&classic, 10) as u64;
        let directory_size = read_u32_le(&classic, 12) as u64;
        let directory_offset = read_u32_le(&classic, 16) as u64;
        bytes.truncate(classic_offset);
        let zip64_eocd_offset = bytes.len() as u64;

        bytes.extend_from_slice(&[0x50, 0x4b, 0x06, 0x06]);
        append_u64(&mut bytes, 44);
        append_u16(&mut bytes, 45);
        append_u16(&mut bytes, 45);
        append_u32(&mut bytes, 0);
        append_u32(&mut bytes, 0);
        append_u64(&mut bytes, entry_count);
        append_u64(&mut bytes, entry_count);
        append_u64(&mut bytes, directory_size);
        append_u64(&mut bytes, directory_offset);
        bytes.extend_from_slice(&[0x50, 0x4b, 0x06, 0x07]);
        append_u32(&mut bytes, 0);
        append_u64(&mut bytes, zip64_eocd_offset);
        append_u32(&mut bytes, 1);
        append_classic_eocd(&mut bytes, u16::MAX, u32::MAX, u32::MAX, 0);
        bytes
    }

    #[test]
    fn bounded_zip_preflight_preserves_normal_ooxml_kinds() {
        let fixture = tempdir().unwrap();
        let cases = [
            (
                "document.docx",
                "word/document.xml",
                AgentCommandArtifactKind::Document,
            ),
            (
                "workbook.xlsx",
                "xl/workbook.xml",
                AgentCommandArtifactKind::Spreadsheet,
            ),
            (
                "slides.pptx",
                "ppt/presentation.xml",
                AgentCommandArtifactKind::Presentation,
            ),
        ];

        for (name, main_part, kind) in cases {
            let path = fixture.path().join(name);
            write_ooxml(&path, main_part, name);
            let mut preflight_file = File::open(&path).unwrap();
            let layout = preflight_ooxml_zip(&mut preflight_file).unwrap();
            assert_eq!(layout.entry_count, 2);
            let validation = validate_office_artifact(&path, kind, File::open(&path).unwrap());
            assert_eq!(
                validation.status,
                AgentCommandArtifactValidationStatus::Valid
            );
        }
    }

    #[test]
    fn bounded_zip_preflight_accepts_valid_small_zip64_ooxml() {
        let fixture = tempdir().unwrap();
        let path = fixture.path().join("zip64.xlsx");
        write_workbook(&path, "zip64");
        let classic = fs::read(&path).unwrap();
        fs::write(&path, rewrite_with_zip64_eocd(classic)).unwrap();

        let mut file = File::open(&path).unwrap();
        let layout = preflight_ooxml_zip(&mut file).unwrap();
        assert_eq!(layout.entry_count, 2);
        let validation = validate_office_artifact(
            &path,
            AgentCommandArtifactKind::Spreadsheet,
            File::open(&path).unwrap(),
        );
        assert_eq!(
            validation.status,
            AgentCommandArtifactValidationStatus::Valid
        );
    }

    #[test]
    fn bounded_zip_preflight_rejects_huge_zip64_declarations_before_zip_parser() {
        let fixture = tempdir().unwrap();

        let too_many_path = fixture.path().join("too-many.xlsx");
        fs::write(
            &too_many_path,
            zip64_metadata_stub(MAX_OOXML_ENTRIES as u64 + 1, 0, 0, 44, 0),
        )
        .unwrap();
        let error = preflight_ooxml_zip(&mut File::open(&too_many_path).unwrap()).unwrap_err();
        assert_eq!(
            error.code,
            "command.artifact.validation.too_many_ooxml_entries"
        );

        let huge_directory_path = fixture.path().join("huge-directory.xlsx");
        fs::write(
            &huge_directory_path,
            zip64_metadata_stub(0, MAX_OOXML_CENTRAL_DIRECTORY_BYTES + 1, 0, 44, 0),
        )
        .unwrap();
        let error =
            preflight_ooxml_zip(&mut File::open(&huge_directory_path).unwrap()).unwrap_err();
        assert_eq!(
            error.code,
            "command.artifact.validation.ooxml_central_directory_too_large"
        );

        let huge_record_path = fixture.path().join("huge-record.xlsx");
        fs::write(
            &huge_record_path,
            zip64_metadata_stub(0, 0, 0, MAX_ZIP64_EOCD_RECORD_BYTES, 0),
        )
        .unwrap();
        let error = preflight_ooxml_zip(&mut File::open(&huge_record_path).unwrap()).unwrap_err();
        assert_eq!(
            error.code,
            "command.artifact.validation.zip64_eocd_too_large"
        );
    }

    #[test]
    fn bounded_zip_preflight_rejects_truncated_and_inconsistent_layouts() {
        let fixture = tempdir().unwrap();

        let truncated_path = fixture.path().join("truncated.xlsx");
        let mut truncated = Vec::new();
        append_classic_eocd(&mut truncated, 0, 0, 0, 1);
        fs::write(&truncated_path, truncated).unwrap();
        assert!(preflight_ooxml_zip(&mut File::open(&truncated_path).unwrap()).is_err());

        let inconsistent_path = fixture.path().join("inconsistent.xlsx");
        let mut inconsistent = Vec::new();
        append_classic_eocd(&mut inconsistent, 1, 46, 0, 0);
        fs::write(&inconsistent_path, inconsistent).unwrap();
        let error = preflight_ooxml_zip(&mut File::open(&inconsistent_path).unwrap()).unwrap_err();
        assert_eq!(error.code, "command.artifact.validation.invalid_ooxml_zip");

        let bad_locator_path = fixture.path().join("bad-locator.xlsx");
        fs::write(
            &bad_locator_path,
            zip64_metadata_stub(0, 0, 0, 44, u64::MAX),
        )
        .unwrap();
        assert!(preflight_ooxml_zip(&mut File::open(&bad_locator_path).unwrap()).is_err());
    }

    #[test]
    fn bounded_zip_preflight_handles_maximum_classic_comment_without_overread() {
        let fixture = tempdir().unwrap();
        let path = fixture.path().join("max-comment.xlsx");
        let mut bytes = Vec::with_capacity(ZIP_EOCD_MAX_SEARCH_BYTES);
        append_classic_eocd(&mut bytes, 0, 0, 0, u16::MAX);
        bytes.resize(ZIP_EOCD_MAX_SEARCH_BYTES, b'x');
        fs::write(&path, bytes).unwrap();

        let layout = preflight_ooxml_zip(&mut File::open(&path).unwrap()).unwrap();
        assert_eq!(layout.entry_count, 0);
        assert_eq!(layout.central_directory_size, 0);
        assert_eq!(layout.central_directory_offset, 0);
    }

    fn capture_with_coverage(
        files: BTreeMap<PathBuf, ObservedArtifact>,
        complete_roots: Vec<PathBuf>,
        excluded_roots: Vec<PathBuf>,
        truncated: bool,
    ) -> CommandArtifactCapture {
        CommandArtifactCapture {
            files,
            complete_roots,
            excluded_roots,
            coverage: AgentCommandArtifactSnapshotCoverage {
                truncated,
                ..AgentCommandArtifactSnapshotCoverage::default()
            },
            warnings: Vec::new(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn reports_created_modified_replaced_deleted_and_unambiguous_rename() {
        let fixture = tempdir().unwrap();
        let workspace = fixture.path().canonicalize().unwrap();
        write_workbook(&workspace.join("modified.xlsx"), "modified-before");
        write_workbook(&workspace.join("replaced.xlsx"), "replaced-before");
        write_workbook(&workspace.join("deleted.xlsx"), "deleted-before");
        write_workbook(&workspace.join("rename-old.xlsx"), "rename-marker");
        write_workbook(&workspace.join("unchanged.xlsx"), "unchanged-marker");

        let observer = CommandArtifactObserver::prepare(
            Some(&workspace),
            &workspace,
            Some(&observe(&[
                "created.xlsx",
                "unchanged.xlsx",
                "missing.xlsx",
                "invalid.xlsx",
                "rename-old.xlsx",
            ])),
            permissions(AgentWritePermission::WorkspaceOnly),
        )
        .unwrap();
        let before = observer.capture(AgentCommandArtifactObservationPhase::Before, None);

        write_workbook(&workspace.join("created.xlsx"), "created-after");
        write_workbook(&workspace.join("modified.xlsx"), "modified-after");
        write_workbook(&workspace.join("replacement.tmp"), "replaced-after");
        fs::rename(
            workspace.join("replacement.tmp"),
            workspace.join("replaced.xlsx"),
        )
        .unwrap();
        fs::remove_file(workspace.join("deleted.xlsx")).unwrap();
        fs::rename(
            workspace.join("rename-old.xlsx"),
            workspace.join("rename-new.xlsx"),
        )
        .unwrap();
        fs::write(workspace.join("invalid.xlsx"), b"not an OOXML package").unwrap();

        let after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
        let result = observer.finish(before, after);
        let changes = result
            .changes
            .iter()
            .map(|change| (change.path.as_str(), change.kind))
            .collect::<StdHashMap<_, _>>();

        assert_eq!(
            changes.get("created.xlsx"),
            Some(&AgentCommandArtifactChangeKind::Created)
        );
        assert_eq!(
            changes.get("modified.xlsx"),
            Some(&AgentCommandArtifactChangeKind::Modified)
        );
        assert_eq!(
            changes.get("replaced.xlsx"),
            Some(&AgentCommandArtifactChangeKind::Replaced)
        );
        assert_eq!(
            changes.get("deleted.xlsx"),
            Some(&AgentCommandArtifactChangeKind::Deleted)
        );
        assert_eq!(
            changes.get("rename-new.xlsx"),
            Some(&AgentCommandArtifactChangeKind::Renamed)
        );
        let renamed = result
            .changes
            .iter()
            .find(|change| change.kind == AgentCommandArtifactChangeKind::Renamed)
            .unwrap();
        assert_eq!(renamed.previous_path.as_deref(), Some("rename-old.xlsx"));

        let outcomes = result
            .expected_outputs
            .iter()
            .map(|outcome| (outcome.requested_path.as_str(), outcome.outcome))
            .collect::<StdHashMap<_, _>>();
        assert_eq!(
            outcomes.get("created.xlsx"),
            Some(&AgentCommandExpectedArtifactOutcomeKind::Created)
        );
        assert_eq!(
            outcomes.get("unchanged.xlsx"),
            Some(&AgentCommandExpectedArtifactOutcomeKind::Unchanged)
        );
        assert_eq!(
            outcomes.get("missing.xlsx"),
            Some(&AgentCommandExpectedArtifactOutcomeKind::Missing)
        );
        assert_eq!(
            outcomes.get("invalid.xlsx"),
            Some(&AgentCommandExpectedArtifactOutcomeKind::Invalid)
        );
        assert_eq!(
            outcomes.get("rename-old.xlsx"),
            Some(&AgentCommandExpectedArtifactOutcomeKind::Renamed)
        );
        assert!(result
            .warnings
            .iter()
            .any(|warning| { warning.code == "command.artifact.expected_output.missing" }));
        assert!(result
            .warnings
            .iter()
            .any(|warning| { warning.code == "command.artifact.expected_output.unchanged" }));
        assert!(result
            .warnings
            .iter()
            .any(|warning| { warning.code == "command.artifact.expected_output.invalid" }));
    }

    #[test]
    fn external_hint_does_not_expand_workspace_only_write_scope() {
        let workspace_fixture = tempdir().unwrap();
        let outside_fixture = tempdir().unwrap();
        let workspace = workspace_fixture.path().canonicalize().unwrap();
        let outside = outside_fixture
            .path()
            .join("external.xlsx")
            .to_string_lossy()
            .to_string();
        let observer = CommandArtifactObserver::prepare(
            Some(&workspace),
            &workspace,
            Some(&observe(&[&outside])),
            permissions(AgentWritePermission::WorkspaceOnly),
        )
        .unwrap();
        let before = observer.capture(AgentCommandArtifactObservationPhase::Before, None);
        let after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
        let result = observer.finish(before, after);

        assert_eq!(
            result.expected_outputs[0].outcome,
            AgentCommandExpectedArtifactOutcomeKind::Unobserved
        );
        assert!(result
            .warnings
            .iter()
            .any(|warning| { warning.code == "command.artifact.path.outside_write_scope" }));
        assert_eq!(
            result.status,
            AgentCommandArtifactObservationStatus::Partial
        );
    }

    #[test]
    fn external_additional_root_requires_read_all() {
        let workspace_fixture = tempdir().unwrap();
        let outside_fixture = tempdir().unwrap();
        let workspace = workspace_fixture.path().canonicalize().unwrap();
        let outside = outside_fixture.path().canonicalize().unwrap();
        let outside_file = outside.join("secret.xlsx");
        write_workbook(&outside_file, "outside");
        let request = AgentCommandArtifactObservationRequest {
            kinds: vec![AgentCommandArtifactObservationKind::Office],
            expected_outputs: Vec::new(),
            additional_roots: vec![outside.to_string_lossy().to_string()],
        };

        let restricted = CommandArtifactObserver::prepare(
            Some(&workspace),
            &workspace,
            Some(&request),
            permissions_with(
                AgentReadPermission::WorkspaceOnly,
                AgentWritePermission::All,
            ),
        )
        .unwrap();
        let before = restricted.capture(AgentCommandArtifactObservationPhase::Before, None);
        let after = restricted.capture(AgentCommandArtifactObservationPhase::After, None);
        let result = restricted.finish(before, after);
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.code == "command.artifact.path.outside_read_scope"));
        assert_eq!(result.coverage.additional_root_count, 0);

        let allowed = CommandArtifactObserver::prepare(
            Some(&workspace),
            &workspace,
            Some(&request),
            permissions_with(
                AgentReadPermission::All,
                AgentWritePermission::WorkspaceOnly,
            ),
        )
        .unwrap();
        let snapshot = allowed.capture(AgentCommandArtifactObservationPhase::Before, None);
        assert!(snapshot.files.contains_key(&outside_file));
    }

    #[test]
    fn external_expected_output_observes_only_the_exact_target() {
        let workspace_fixture = tempdir().unwrap();
        let outside_fixture = tempdir().unwrap();
        let workspace = workspace_fixture.path().canonicalize().unwrap();
        let outside = outside_fixture.path().canonicalize().unwrap();
        let expected = outside.join("expected.xlsx");
        let sibling = outside.join("private-sibling.xlsx");
        write_workbook(&expected, "expected");
        write_workbook(&sibling, "sibling");
        let expected_text = expected.to_string_lossy().to_string();
        let observer = CommandArtifactObserver::prepare(
            Some(&workspace),
            &workspace,
            Some(&observe(&[&expected_text])),
            permissions_with(
                AgentReadPermission::WorkspaceOnly,
                AgentWritePermission::All,
            ),
        )
        .unwrap();

        let snapshot = observer.capture(AgentCommandArtifactObservationPhase::Before, None);
        assert!(snapshot.files.contains_key(&expected));
        assert!(!snapshot.files.contains_key(&sibling));
        assert_eq!(snapshot.coverage.office_files_seen, 1);
    }

    #[cfg(unix)]
    #[test]
    fn workspace_scan_never_follows_symlinks() {
        use std::os::unix::fs::symlink;

        let workspace_fixture = tempdir().unwrap();
        let outside_fixture = tempdir().unwrap();
        let workspace = workspace_fixture.path().canonicalize().unwrap();
        write_workbook(&outside_fixture.path().join("secret.xlsx"), "outside");
        symlink(outside_fixture.path(), workspace.join("linked-output")).unwrap();
        let observer = CommandArtifactObserver::prepare(
            Some(&workspace),
            &workspace,
            Some(&observe(&[])),
            permissions(AgentWritePermission::WorkspaceOnly),
        )
        .unwrap();
        let before = observer.capture(AgentCommandArtifactObservationPhase::Before, None);

        assert!(before.files.is_empty());
        assert_eq!(before.coverage.symlinks_skipped, 1);
        assert!(before
            .warnings
            .iter()
            .any(|warning| warning.code == "command.artifact.symlink_skipped"));
    }

    #[test]
    fn file_and_hash_budgets_produce_explicit_partial_coverage() {
        let fixture = tempdir().unwrap();
        let workspace = fixture.path().canonicalize().unwrap();
        fs::write(workspace.join("one.csv"), b"one").unwrap();
        fs::write(workspace.join("two.csv"), b"two").unwrap();
        let mut observer = CommandArtifactObserver::prepare(
            Some(&workspace),
            &workspace,
            Some(&observe(&[])),
            permissions(AgentWritePermission::WorkspaceOnly),
        )
        .unwrap();
        observer.budget.max_office_files = 1;
        let before = observer.capture(AgentCommandArtifactObservationPhase::Before, None);
        let after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
        let result = observer.finish(before, after);

        assert!(result.coverage.before.truncated);
        assert_eq!(result.coverage.before.office_files_seen, 2);
        assert_eq!(
            result.status,
            AgentCommandArtifactObservationStatus::Partial
        );
        assert_eq!(result.partial, Some(true));
        assert_eq!(result.scanned, Some(4));
        assert_eq!(result.returned, Some(result.changes.len() as u64));
        assert_eq!(result.omitted, Some(result.changes_omitted));
        assert!(result
            .stop_reasons
            .iter()
            .any(|reason| reason == "before_scan_limit"));
        assert!(result
            .stop_reasons
            .iter()
            .any(|reason| reason == "after_scan_limit"));

        let mut hash_limited = CommandArtifactObserver::prepare(
            Some(&workspace),
            &workspace,
            Some(&observe(&[])),
            permissions(AgentWritePermission::WorkspaceOnly),
        )
        .unwrap();
        hash_limited.budget.max_hashed_bytes = 1;
        let snapshot = hash_limited.capture(AgentCommandArtifactObservationPhase::Before, None);
        assert!(snapshot.coverage.truncated);
        assert!(snapshot.coverage.files_unhashed >= 1);
        assert!(snapshot
            .warnings
            .iter()
            .any(|warning| warning.code == "command.artifact.hash.total_budget"));
    }

    #[test]
    fn snapshots_report_time_budget_and_before_cancellation() {
        let fixture = tempdir().unwrap();
        let workspace = fixture.path().canonicalize().unwrap();
        fs::write(workspace.join("artifact.csv"), b"value").unwrap();

        let mut timed = CommandArtifactObserver::prepare(
            Some(&workspace),
            &workspace,
            Some(&observe(&[])),
            permissions(AgentWritePermission::WorkspaceOnly),
        )
        .unwrap();
        timed.budget.max_duration = Duration::ZERO;
        let timed_snapshot = timed.capture(AgentCommandArtifactObservationPhase::Before, None);
        assert!(timed_snapshot.coverage.truncated);
        assert!(timed_snapshot.coverage.time_budget_exceeded);
        assert!(timed_snapshot
            .warnings
            .iter()
            .any(|warning| warning.code == "command.artifact.scan.time_budget"));

        let observer = CommandArtifactObserver::prepare(
            Some(&workspace),
            &workspace,
            Some(&observe(&[])),
            permissions(AgentWritePermission::WorkspaceOnly),
        )
        .unwrap();
        let cancellation = AgentCancellationToken::new();
        cancellation.cancel();
        let before = observer.capture(
            AgentCommandArtifactObservationPhase::Before,
            Some(&cancellation),
        );
        assert!(before.coverage.cancelled);
        assert!(before.coverage.truncated);
        assert!(before.files.is_empty());

        // An after snapshot deliberately does not inherit command cancellation: a cancelled
        // process may already have created or modified an artifact.
        let after = observer.capture(
            AgentCommandArtifactObservationPhase::After,
            Some(&cancellation),
        );
        assert!(!after.coverage.cancelled);
        assert!(after.files.contains_key(&workspace.join("artifact.csv")));
    }

    #[cfg(unix)]
    #[test]
    fn unstable_directory_identity_discards_subtree_results() {
        let fixture = tempdir().unwrap();
        let directory = fixture.path().join("observed");
        fs::create_dir(&directory).unwrap();
        let identity_before = stable_directory_identity(&directory).unwrap();
        let observed_file = directory.join("artifact.csv");
        let mut state = CaptureState {
            files: BTreeMap::from([(
                observed_file,
                ObservedArtifact {
                    kind: AgentCommandArtifactKind::Spreadsheet,
                    metadata: AgentCommandArtifactMetadata {
                        size_bytes: 1,
                        sha256: Some("digest".to_string()),
                        validation: AgentCommandArtifactValidation {
                            status: AgentCommandArtifactValidationStatus::NotApplicable,
                            code: None,
                            message: None,
                        },
                    },
                    identity: None,
                    modified_ns: None,
                },
            )]),
            complete_roots: vec![directory.clone()],
            excluded_roots: Vec::new(),
            coverage: AgentCommandArtifactSnapshotCoverage::default(),
            warnings: Vec::new(),
            phase: AgentCommandArtifactObservationPhase::Before,
            budget: ObservationBudget::default(),
            deadline: Instant::now() + Duration::from_secs(1),
            cancellation_token: None,
        };
        let before = capture_with_coverage(
            state.files.clone(),
            state.complete_roots.clone(),
            Vec::new(),
            false,
        );
        fs::rename(&directory, fixture.path().join("original")).unwrap();
        fs::create_dir(&directory).unwrap();
        let mut root_complete = true;

        verify_directory_identity(&directory, identity_before, &mut state, &mut root_complete);

        assert!(!root_complete);
        assert!(state.files.is_empty());
        assert!(state.complete_roots.is_empty());
        assert!(state
            .warnings
            .iter()
            .any(|warning| { warning.code == "command.artifact.directory.identity_changed" }));
        let after = capture_with_coverage(
            state.files,
            state.complete_roots,
            state.excluded_roots,
            true,
        );
        assert!(diff_captures(&before, &after, None).is_empty());
    }

    #[test]
    fn change_report_is_bounded_and_prioritizes_expected_outputs() {
        let expected_path = PathBuf::from("/outside/zz-expected.xlsx");
        let mut changes = (0..300)
            .map(|index| AgentCommandArtifactChange {
                kind: AgentCommandArtifactChangeKind::Created,
                artifact_kind: AgentCommandArtifactKind::Spreadsheet,
                path: format!("/outside/{index:03}.xlsx"),
                scope: AgentCommandArtifactScope::External,
                previous_path: None,
                previous_scope: None,
                before: None,
                after: None,
            })
            .collect::<Vec<_>>();
        changes.push(AgentCommandArtifactChange {
            kind: AgentCommandArtifactChangeKind::Created,
            artifact_kind: AgentCommandArtifactKind::Spreadsheet,
            path: display_path(&expected_path),
            scope: AgentCommandArtifactScope::External,
            previous_path: None,
            previous_scope: None,
            before: None,
            after: None,
        });
        let expected_outputs = vec![ExpectedOutput {
            requested_path: expected_path.to_string_lossy().to_string(),
            resolved_path: Some(expected_path.clone()),
        }];

        let (reported, omitted) = limit_reported_changes(changes, &expected_outputs, None);

        assert_eq!(reported.len(), MAX_REPORTED_CHANGES);
        assert_eq!(omitted, 45);
        assert!(reported
            .iter()
            .any(|change| change.path == display_path(&expected_path)));
        assert!(serde_json::to_vec(&reported).unwrap().len() <= MAX_REPORTED_CHANGE_BYTES);
    }

    #[test]
    fn fixed_workspace_exclusions_are_reported_without_downgrading_coverage() {
        let fixture = tempdir().unwrap();
        let workspace = fixture.path().canonicalize().unwrap();
        fs::create_dir(workspace.join(".git")).unwrap();
        fs::write(workspace.join(".git").join("ignored.xlsx"), b"not observed").unwrap();
        let observer = CommandArtifactObserver::prepare(
            Some(&workspace),
            &workspace,
            Some(&observe(&[])),
            permissions(AgentWritePermission::WorkspaceOnly),
        )
        .unwrap();
        let before = observer.capture(AgentCommandArtifactObservationPhase::Before, None);
        let after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
        let result = observer.finish(before, after);

        assert_eq!(result.coverage.before.excluded_directories, 1);
        assert_eq!(result.coverage.after.excluded_directories, 1);
        assert_eq!(
            result.status,
            AgentCommandArtifactObservationStatus::Complete
        );
        assert!(result.changes.is_empty());
        assert!(!path_is_covered(
            &workspace.join("node_modules").join("hidden.xlsx"),
            std::slice::from_ref(&workspace),
            &[workspace.join("node_modules")],
        ));
        assert!(path_is_covered(
            &workspace.join("node_modules").join("expected.xlsx"),
            &[
                workspace.clone(),
                workspace.join("node_modules").join("expected.xlsx"),
            ],
            &[workspace.join("node_modules")],
        ));
    }

    #[test]
    fn partial_snapshot_absence_never_fabricates_create_delete_or_rename() {
        let artifact = |digest: &str| ObservedArtifact {
            kind: AgentCommandArtifactKind::Spreadsheet,
            metadata: AgentCommandArtifactMetadata {
                size_bytes: 10,
                sha256: Some(digest.to_string()),
                validation: AgentCommandArtifactValidation {
                    status: AgentCommandArtifactValidationStatus::Valid,
                    code: None,
                    message: None,
                },
            },
            identity: None,
            modified_ns: None,
        };
        let before = capture_with_coverage(
            BTreeMap::from([
                (PathBuf::from("/tmp/deleted.xlsx"), artifact("deleted")),
                (PathBuf::from("/tmp/modified.xlsx"), artifact("before")),
                (PathBuf::from("/tmp/rename-old.xlsx"), artifact("rename")),
            ]),
            Vec::new(),
            Vec::new(),
            true,
        );
        let after = capture_with_coverage(
            BTreeMap::from([
                (PathBuf::from("/tmp/created.xlsx"), artifact("created")),
                (PathBuf::from("/tmp/modified.xlsx"), artifact("after")),
                (PathBuf::from("/tmp/rename-new.xlsx"), artifact("rename")),
            ]),
            Vec::new(),
            Vec::new(),
            true,
        );

        let changes = diff_captures(&before, &after, None);

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "/tmp/modified.xlsx");
        assert_eq!(changes[0].kind, AgentCommandArtifactChangeKind::Modified);
    }

    #[test]
    fn asymmetric_file_budget_does_not_turn_unscanned_files_into_changes() {
        let fixture = tempdir().unwrap();
        let workspace = fixture.path().canonicalize().unwrap();
        fs::write(workspace.join("a.csv"), b"a").unwrap();
        fs::write(workspace.join("b.csv"), b"b").unwrap();
        let mut observer = CommandArtifactObserver::prepare(
            Some(&workspace),
            &workspace,
            Some(&observe(&[])),
            permissions(AgentWritePermission::WorkspaceOnly),
        )
        .unwrap();

        observer.budget.max_office_files = 1;
        let partial_before = observer.capture(AgentCommandArtifactObservationPhase::Before, None);
        observer.budget.max_office_files = 10;
        let complete_after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
        let result = observer.finish(partial_before, complete_after);
        assert_eq!(
            result.status,
            AgentCommandArtifactObservationStatus::Partial
        );
        assert!(result.changes.is_empty());

        let mut observer = CommandArtifactObserver::prepare(
            Some(&workspace),
            &workspace,
            Some(&observe(&[])),
            permissions(AgentWritePermission::WorkspaceOnly),
        )
        .unwrap();
        observer.budget.max_office_files = 10;
        let complete_before = observer.capture(AgentCommandArtifactObservationPhase::Before, None);
        observer.budget.max_office_files = 1;
        let partial_after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
        let result = observer.finish(complete_before, partial_after);
        assert_eq!(
            result.status,
            AgentCommandArtifactObservationStatus::Partial
        );
        assert!(result.changes.is_empty());
    }

    #[test]
    fn expected_output_seen_only_after_an_uncovered_before_snapshot_is_unobserved() {
        let fixture = tempdir().unwrap();
        let workspace = fixture.path().canonicalize().unwrap();
        let mut observer = CommandArtifactObserver::prepare(
            Some(&workspace),
            &workspace,
            Some(&observe(&["created-during-gap.xlsx"])),
            permissions(AgentWritePermission::WorkspaceOnly),
        )
        .unwrap();

        observer.budget.max_duration = Duration::ZERO;
        let before = observer.capture(AgentCommandArtifactObservationPhase::Before, None);
        assert!(before.coverage.truncated);
        assert!(!path_is_covered(
            &workspace.join("created-during-gap.xlsx"),
            &before.complete_roots,
            &before.excluded_roots,
        ));

        write_workbook(&workspace.join("created-during-gap.xlsx"), "after");
        observer.budget.max_duration = Duration::from_secs(1);
        let after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
        let result = observer.finish(before, after);

        assert!(result.changes.is_empty());
        assert_eq!(
            result.expected_outputs[0].outcome,
            AgentCommandExpectedArtifactOutcomeKind::Unobserved
        );
        assert!(result.warnings.iter().any(|warning| {
            warning.code == "command.artifact.expected_output.unobserved"
                && warning.path.as_deref() == Some("created-during-gap.xlsx")
        }));
    }

    #[test]
    fn ambiguous_equal_digests_are_not_misreported_as_renames() {
        let metadata = AgentCommandArtifactMetadata {
            size_bytes: 10,
            sha256: Some("same".to_string()),
            validation: AgentCommandArtifactValidation {
                status: AgentCommandArtifactValidationStatus::Valid,
                code: None,
                message: None,
            },
        };
        let artifact = ObservedArtifact {
            kind: AgentCommandArtifactKind::Spreadsheet,
            metadata,
            identity: None,
            modified_ns: None,
        };
        let before = capture_with_coverage(
            BTreeMap::from([
                (PathBuf::from("/tmp/a.xlsx"), artifact.clone()),
                (PathBuf::from("/tmp/b.xlsx"), artifact.clone()),
            ]),
            vec![PathBuf::from("/tmp")],
            Vec::new(),
            false,
        );
        let after = capture_with_coverage(
            BTreeMap::from([
                (PathBuf::from("/tmp/c.xlsx"), artifact.clone()),
                (PathBuf::from("/tmp/d.xlsx"), artifact),
            ]),
            vec![PathBuf::from("/tmp")],
            Vec::new(),
            false,
        );
        let changes = diff_captures(&before, &after, None);

        assert_eq!(
            changes
                .iter()
                .filter(|change| change.kind == AgentCommandArtifactChangeKind::Deleted)
                .count(),
            2
        );
        assert_eq!(
            changes
                .iter()
                .filter(|change| change.kind == AgentCommandArtifactChangeKind::Created)
                .count(),
            2
        );
        assert!(!changes
            .iter()
            .any(|change| change.kind == AgentCommandArtifactChangeKind::Renamed));
    }
}
