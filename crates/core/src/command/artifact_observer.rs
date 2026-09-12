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

mod ooxml;
mod report;

use ooxml::validate_office_artifact;
#[cfg(test)]
use ooxml::{find_eocd_at_physical_end, preflight_ooxml_zip, read_u16_le, read_u32_le};
#[cfg(test)]
use report::display_path;
#[cfg(test)]
use report::limit_reported_changes;
use report::{diff_captures, display_observed_path, limit_projected_changes};

#[derive(Debug, Clone)]
pub(super) struct CommandArtifactObserver {
    workspace_root: Option<PathBuf>,
    workspace: crate::workspace::WorkspaceResolver,
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
    #[cfg(test)]
    pub(super) fn prepare(
        workspace_root: Option<&Path>,
        cwd: &Path,
        request: Option<&AgentCommandArtifactObservationRequest>,
        permissions: AgentPermissions,
    ) -> Option<Self> {
        Self::prepare_with_workspace(
            workspace_root,
            cwd,
            request,
            permissions,
            &crate::workspace::WorkspaceResolver::from_primary(workspace_root),
        )
    }

    pub(super) fn prepare_with_workspace(
        workspace_root: Option<&Path>,
        cwd: &Path,
        request: Option<&AgentCommandArtifactObservationRequest>,
        permissions: AgentPermissions,
        workspace: &crate::workspace::WorkspaceResolver,
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
                workspace,
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
                    workspace,
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
            workspace: workspace.clone(),
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
        let mut all_changes = diff_captures(&before, &after, self.workspace_root.as_deref());
        let mut warnings = self.setup_warnings.clone();
        for warning in before.warnings.iter().chain(after.warnings.iter()) {
            push_warning(&mut warnings, warning.clone());
        }
        // Compute explicit expected-output outcomes from the full in-memory diff before bounding
        // the model/audit-facing list. A noisy command must not hide its declared output contract.
        let expected_outputs = self.expected_outcomes(&before, &after, &all_changes, &mut warnings);
        for change in &mut all_changes {
            self.project_observed_path(&mut change.path, &mut change.scope);
            if let (Some(path), Some(scope)) =
                (&mut change.previous_path, &mut change.previous_scope)
            {
                self.project_observed_path(path, scope);
            }
        }
        let expected_paths = self
            .expected_outputs
            .iter()
            .filter_map(|expected| expected.resolved_path.as_deref())
            .map(|physical| {
                let (mut path, mut scope) =
                    display_observed_path(physical, self.workspace_root.as_deref());
                self.project_observed_path(&mut path, &mut scope);
                (path, scope)
            })
            .collect::<Vec<_>>();
        // Bound the final namespace representation, including aliases and escaped literal paths.
        let (changes, changes_omitted) = limit_projected_changes(all_changes, &expected_paths);
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

        let mut observation = AgentCommandArtifactObservation {
            schema_version: AGENT_COMMAND_ARTIFACT_OBSERVATION_SCHEMA_VERSION,
            status,
            partial,
            stop_reasons,
            scanned,
            returned,
            omitted: changes_omitted,
            coverage,
            changes,
            changes_truncated,
            changes_omitted,
            expected_outputs,
            warnings,
        };
        for expected in &mut observation.expected_outputs {
            if let (Some(path), Some(scope)) = (&mut expected.path, &mut expected.scope) {
                self.project_observed_path(path, scope);
            }
        }
        for warning in &mut observation.warnings {
            if let Some(path) = &mut warning.path {
                if Path::new(path).is_absolute() {
                    *path = self.workspace.display_path(Path::new(path));
                }
            }
        }
        observation
    }

    fn project_observed_path(&self, path: &mut String, scope: &mut AgentCommandArtifactScope) {
        let physical = if *scope == AgentCommandArtifactScope::Workspace {
            self.workspace_root
                .as_deref()
                .map(|root| root.join(&*path))
                .unwrap_or_else(|| PathBuf::from(&*path))
        } else {
            PathBuf::from(&*path)
        };
        *path = self.workspace.display_path(&physical);
        if self
            .workspace
            .containing_root(&physical)
            .ok()
            .flatten()
            .is_some()
        {
            *scope = AgentCommandArtifactScope::Workspace;
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
    workspace: &crate::workspace::WorkspaceResolver,
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
    crate::resource_locator::ResourceLocator::parse(raw)
        .map_err(|error| ("command.artifact.path.invalid", error.to_string()))?;
    let candidate = if crate::workspace::parse_workspace_path(raw)
        .map_err(|error| ("command.artifact.path.invalid", error))?
        .is_some()
    {
        workspace
            .resolve_input(raw)
            .map_err(|error| ("command.artifact.path.invalid", error))?
    } else {
        match expanded {
            Some(path) => path,
            None if Path::new(raw).is_absolute() => PathBuf::from(raw),
            None => cwd.join(raw),
        }
    };
    let normalized = normalize_absolute_path(&candidate).map_err(|message| {
        (
            "command.artifact.path.invalid",
            format!("观察路径无法规范化：{message}"),
        )
    })?;
    let inside_workspace = workspace
        .containing_root(&normalized)
        .map_err(|error| ("command.artifact.path.invalid", error))?
        .is_some();
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
mod tests;
