use super::*;

pub(super) fn managed_command_hard_timeout(
    managed_pdf: bool,
    presentation_editor: bool,
    requested_timeout_ms: Option<u64>,
) -> Option<Duration> {
    if managed_pdf {
        Some(Duration::from_millis(MANAGED_PDF_HARD_TIMEOUT_MS))
    } else if presentation_editor {
        Some(Duration::from_millis(
            requested_timeout_ms
                .unwrap_or(PRESENTATION_EDITOR_PLAN_TIMEOUT_MS)
                .clamp(1, PRESENTATION_EDITOR_PLAN_TIMEOUT_MS),
        ))
    } else {
        requested_timeout_ms.map(|timeout| Duration::from_millis(timeout.clamp(1, MAX_TIMEOUT_MS)))
    }
}

pub(super) fn settle_presentation_editor_office_result(
    command: &mut AgentCommandExecutionResult,
    office: OfficePresentationEditResult,
) -> bool {
    let succeeded = office.exit_code == Some(0)
        && !office.timed_out
        && !office.cancelled
        && office.error_code.is_none()
        && office.error.is_none();
    if succeeded {
        command.exit_code = Some(0);
        return true;
    }
    command.exit_code = office.exit_code.or(Some(1));
    command.timed_out |= office.timed_out;
    command.cancelled |= office.cancelled;
    let preserve_missing_exit =
        (office.timed_out || office.cancelled) && office.exit_code.is_none();
    let message = presentation_editor_office_failure_message(&office);
    fail_presentation_editor_result(command, &message);
    if preserve_missing_exit {
        command.exit_code = None;
    }
    false
}

pub(super) fn settle_managed_office_script_result(
    command: &mut AgentCommandExecutionResult,
    office: OfficeManagedScriptOutputResult,
) -> bool {
    let succeeded = office.exit_code == Some(0)
        && !office.timed_out
        && !office.cancelled
        && office.error_code.is_none()
        && office.error.is_none();
    if succeeded {
        command.exit_code = Some(0);
        return true;
    }
    command.exit_code = office.exit_code.or(Some(1));
    command.timed_out |= office.timed_out;
    command.cancelled |= office.cancelled;
    let mut diagnostics = Vec::new();
    if let Some(error) = office.error {
        diagnostics.push(error);
    }
    if let Some(code) = office.error_code {
        diagnostics.push(format!("Host code: {code}"));
    }
    if let Some(provider) = stable_office_json_diagnostic(&office.stdout, 1_024)
        .or_else(|| bounded_diagnostic_text(&office.stderr, 1_024))
    {
        diagnostics.push(provider);
    }
    let message = if diagnostics.is_empty() {
        "Managed Office script output failed strict validation or atomic publication.".to_string()
    } else {
        diagnostics.join("; ")
    };
    fail_presentation_editor_result(command, &message);
    false
}

pub(super) fn presentation_editor_office_failure_message(
    office: &OfficePresentationEditResult,
) -> String {
    const FIELD_LIMIT: usize = 1_024;
    const MESSAGE_LIMIT: usize = 4_096;

    let provider = stable_office_json_diagnostic(&office.stdout, FIELD_LIMIT).or_else(|| {
        bounded_diagnostic_text(&office.stderr, FIELD_LIMIT)
            .map(|diagnostic| format!("OfficeCLI stderr: {diagnostic}"))
    });
    let host_code = office
        .error_code
        .as_deref()
        .and_then(|value| bounded_diagnostic_text(value, FIELD_LIMIT));
    let host_error = office
        .error
        .as_deref()
        .and_then(|value| bounded_diagnostic_text(value, FIELD_LIMIT));

    let mut parts = Vec::new();
    push_unique_diagnostic(&mut parts, provider);
    push_unique_diagnostic(
        &mut parts,
        host_code.map(|code| format!("Host code: {code}")),
    );
    push_unique_diagnostic(
        &mut parts,
        host_error.map(|error| format!("Host error: {error}")),
    );
    if parts.is_empty() {
        parts.push("Presentation Editor Host transaction failed.".to_string());
    }
    parts.join(" | ").chars().take(MESSAGE_LIMIT).collect()
}

pub(super) fn stable_office_json_diagnostic(stdout: &str, field_limit: usize) -> Option<String> {
    let mut fields = Vec::new();
    for value in serde_json::Deserializer::from_str(stdout)
        .into_iter::<serde_json::Value>()
        .take(8)
    {
        let Ok(value) = value else {
            break;
        };
        if value.get("success").and_then(serde_json::Value::as_bool) == Some(true) {
            continue;
        }
        for key in [
            "type",
            "description",
            "path",
            "part",
            "code",
            "error",
            "message",
        ] {
            let Some(value) = find_stable_json_string(&value, key)
                .and_then(|value| bounded_diagnostic_text(value, field_limit))
            else {
                continue;
            };
            let field = format!("{key}={value}");
            if !fields.contains(&field) {
                fields.push(field);
            }
        }
    }
    (!fields.is_empty()).then(|| format!("OfficeCLI: {}", fields.join("; ")))
}

pub(super) fn find_stable_json_string<'a>(
    value: &'a serde_json::Value,
    key: &str,
) -> Option<&'a str> {
    match value {
        serde_json::Value::Object(values) => values
            .get(key)
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                values
                    .values()
                    .find_map(|value| find_stable_json_string(value, key))
            }),
        serde_json::Value::Array(values) => values
            .iter()
            .find_map(|value| find_stable_json_string(value, key)),
        _ => None,
    }
}

pub(super) fn bounded_diagnostic_text(value: &str, limit: usize) -> Option<String> {
    let normalized = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .take(limit)
        .collect::<String>();
    (!normalized.is_empty()).then_some(normalized)
}

pub(super) fn push_unique_diagnostic(parts: &mut Vec<String>, candidate: Option<String>) {
    let Some(candidate) = candidate else {
        return;
    };
    if !parts
        .iter()
        .any(|part| part == &candidate || part.ends_with(&candidate) || candidate.ends_with(part))
    {
        parts.push(candidate);
    }
}

pub(super) fn fail_presentation_editor_result(
    result: &mut AgentCommandExecutionResult,
    message: &str,
) {
    result.exit_code = result.exit_code.filter(|code| *code != 0).or(Some(1));
    result.error = Some(message.chars().take(4_096).collect());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ManagedBuilderSyntaxLanguage {
    Node,
    Python,
}

impl ManagedBuilderSyntaxLanguage {
    fn display_name(self) -> &'static str {
        match self {
            Self::Node => "Node",
            Self::Python => "Python",
        }
    }

    fn launch_name(self) -> &'static str {
        match self {
            Self::Node => "managed-node-syntax-check",
            Self::Python => "managed-python-syntax-check",
        }
    }

    fn executable_projection(self) -> &'static str {
        match self {
            Self::Node => "<managed-node>",
            Self::Python => "<managed-python>",
        }
    }
}

pub(super) fn automatic_managed_builder_syntax_check(
    profile: AgentCommandRuntimeProfile,
    kind: AgentCommandRuntimeKind,
    parsed: &ManagedArtifactCommand,
    builder: Option<&ManagedArtifactBuilderCommand>,
) -> Option<ManagedBuilderSyntaxLanguage> {
    let has_declared_output = builder.is_some_and(|builder| !builder.output_paths.is_empty());
    if !has_declared_output {
        return None;
    }
    match (profile, kind) {
        (AgentCommandRuntimeProfile::Presentations, AgentCommandRuntimeKind::Node)
            if !parsed.node_syntax_check =>
        {
            Some(ManagedBuilderSyntaxLanguage::Node)
        }
        (
            AgentCommandRuntimeProfile::Documents | AgentCommandRuntimeProfile::Spreadsheets,
            AgentCommandRuntimeKind::Python,
        ) => Some(ManagedBuilderSyntaxLanguage::Python),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub(super) struct PresentationEditorContract {
    pub(super) source_mount_path: String,
    pub(super) source_path: String,
    pub(super) source_binding: crate::AgentFileInputBinding,
    pub(super) destination_path: String,
    pub(super) plan_destination_path: String,
    pub(super) destination_binding: crate::office::OfficeManagedScriptBinding,
    pub(super) asset_specs: Vec<AgentFileInputSpec>,
    pub(super) asset_bindings: Vec<crate::AgentFileInputBinding>,
}

pub(super) struct PreparedPresentationEditor {
    pub(super) contract: PresentationEditorContract,
    _plan_directory: tempfile::TempDir,
    pub(super) plan_path: PathBuf,
    pub(super) frozen_script_path: PathBuf,
    pub(super) runtime_root: PathBuf,
    private_home: PathBuf,
    private_tmp: PathBuf,
}

pub(super) struct PreparedManagedOfficeScript {
    pub(super) binding: crate::office::OfficeManagedScriptBinding,
    pub(super) frozen_script_path: PathBuf,
    pub(super) staging: OfficeManagedScriptStaging,
}

pub(super) fn prepare_managed_office_script_execution(
    context: &OfficeExecutionContext,
    binding: &crate::office::OfficeManagedScriptBinding,
    prepared_inputs: Option<&PreparedAgentFileInputs>,
) -> Result<PreparedManagedOfficeScript, String> {
    let inputs = prepared_inputs.ok_or_else(|| {
        "Managed Office Script is missing its frozen private input snapshot.".to_string()
    })?;
    let frozen_script_path = inputs.root().join(&binding.script_mount_path);
    let metadata = fs::symlink_metadata(&frozen_script_path)
        .map_err(|error| format!("Cannot inspect frozen Managed Office script: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("Frozen Managed Office script must be a regular file.".to_string());
    }
    let frozen_script_path = frozen_script_path
        .canonicalize()
        .map_err(|error| format!("Cannot canonicalize frozen Managed Office script: {error}"))?;
    let staging = prepare_managed_script_staging(context, binding)
        .map_err(|error| format!("{}: {}", error.code().stable_name(), error.message()))?;
    Ok(PreparedManagedOfficeScript {
        binding: binding.clone(),
        frozen_script_path,
        staging,
    })
}

pub(super) fn rewrite_managed_office_script_arguments(
    arguments: &[OsString],
    script: &PreparedManagedOfficeScript,
    prepared_inputs: Option<&PreparedAgentFileInputs>,
) -> Result<Vec<OsString>, CommandExecutionError> {
    let approved_source_mount = script
        .binding
        .source_mount_path
        .as_deref()
        .map(|mount| {
            let inputs = prepared_inputs.ok_or_else(|| {
                CommandExecutionError::from(
                    "Managed Office Editor source snapshot is unavailable.".to_string(),
                )
            })?;
            let root = inputs.root().canonicalize().map_err(|error| {
                CommandExecutionError::from(format!(
                    "Cannot canonicalize Managed Office input root: {error}"
                ))
            })?;
            let path = root.join(mount);
            let canonical = path.canonicalize().map_err(|error| {
                CommandExecutionError::from(format!(
                    "Cannot canonicalize Managed Office Editor source snapshot: {error}"
                ))
            })?;
            if canonical == root || !canonical.starts_with(&root) {
                return Err(CommandExecutionError::from(
                    "Managed Office Editor source snapshot escaped the private input root."
                        .to_string(),
                ));
            }
            Ok(mount.to_string())
        })
        .transpose()?;
    let mut rewritten = Vec::with_capacity(arguments.len());
    let mut index = 0usize;
    let mut saw_output = false;
    let mut saw_source = false;
    while index < arguments.len() {
        let token = arguments[index].to_string_lossy();
        if token == "--output" {
            let _ = arguments.get(index + 1).ok_or_else(|| {
                CommandExecutionError::from(
                    "Managed Office Script --output is missing its value.".to_string(),
                )
            })?;
            rewritten.push(OsString::from("--output"));
            rewritten.push(script.staging.candidate_path().as_os_str().to_os_string());
            saw_output = true;
            index += 2;
            continue;
        }
        if token.starts_with("--output=") {
            rewritten.push(OsString::from(format!(
                "--output={}",
                script.staging.candidate_path().display()
            )));
            saw_output = true;
            index += 1;
            continue;
        }
        if token == "--source" {
            let requested = arguments.get(index + 1).ok_or_else(|| {
                CommandExecutionError::from(
                    "Managed Office Script --source is missing its value.".to_string(),
                )
            })?;
            let source = approved_source_mount.as_ref().ok_or_else(|| {
                CommandExecutionError::from(
                    "Managed Office Script declared --source without a frozen input.".to_string(),
                )
            })?;
            if requested.to_string_lossy() != source.as_str() {
                return Err(CommandExecutionError::from(
                    "Managed Office Script --source differs from the approved logical mountPath."
                        .to_string(),
                ));
            }
            rewritten.push(OsString::from("--source"));
            // Keep the approved logical mountPath in argv. MYCOPILOT_INPUT_ROOT points at the
            // immutable private snapshot, and the fixed Word/Excel wrappers resolve beneath it.
            rewritten.push(OsString::from(source));
            saw_source = true;
            index += 2;
            continue;
        }
        if token.starts_with("--source=") {
            let source = approved_source_mount.as_ref().ok_or_else(|| {
                CommandExecutionError::from(
                    "Managed Office Script declared --source without a frozen input.".to_string(),
                )
            })?;
            if token.strip_prefix("--source=") != Some(source.as_str()) {
                return Err(CommandExecutionError::from(
                    "Managed Office Script --source differs from the approved logical mountPath."
                        .to_string(),
                ));
            }
            rewritten.push(OsString::from(format!("--source={source}")));
            saw_source = true;
            index += 1;
            continue;
        }
        rewritten.push(arguments[index].clone());
        index += 1;
    }
    if !saw_output {
        return Err(CommandExecutionError::from(
            "Managed Office Script is missing its approved --output.".to_string(),
        ));
    }
    if approved_source_mount.is_some() != saw_source {
        return Err(CommandExecutionError::from(
            "Managed Office Script source arguments differ from the approved binding.".to_string(),
        ));
    }
    Ok(rewritten)
}

pub(super) fn presentation_editor_contract(
    workspace_root: Option<&Path>,
    cwd: &Path,
    request: &AgentCommandRequest,
    binding: &crate::AgentCommandRuntimeBinding,
    parsed: &ManagedArtifactCommand,
    builder: Option<&ManagedArtifactBuilderCommand>,
) -> Result<Option<PresentationEditorContract>, String> {
    let editor_bindings = request
        .inputs
        .iter()
        .filter(|input| input.mount_path == super::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH)
        .collect::<Vec<_>>();
    if editor_bindings.is_empty() {
        if request
            .managed_office_script
            .as_deref()
            .is_some_and(|binding| {
                binding.purpose == OfficeManagedScriptPurpose::EditPresentationPlan
            })
        {
            return Err(
                "Presentation Editor transaction is missing its frozen script input.".to_string(),
            );
        }
        return Ok(None);
    }
    if editor_bindings.len() != 1
        || binding.profile != AgentCommandRuntimeProfile::Presentations
        || binding.kind != AgentCommandRuntimeKind::Node
        || parsed.node_syntax_check
    {
        return Err(
            "Presentation Editor reserved script binding has an invalid runtime identity."
                .to_string(),
        );
    }
    let builder = builder.ok_or_else(|| {
        "Presentation Editor requires one direct, statically inspectable Builder command."
            .to_string()
    })?;
    if !is_presentation_editor_direct_command(&request.command) {
        return Err(
            "Presentation Editor accepts only `node <editor.mjs> --source <mount.pptx> --output <destination.pptx>` (flags may be swapped)."
                .to_string(),
        );
    }
    let workspace_root = workspace_root.ok_or_else(|| {
        "Presentation Editor requires the workspace which owns its materialized script.".to_string()
    })?;
    let script_binding = editor_bindings[0];
    let AgentFileInputRef::Workspace {
        path: frozen_script_source,
    } = &script_binding.source
    else {
        return Err("Presentation Editor script binding must be workspace-owned.".to_string());
    };
    let requested_script = Path::new(
        parsed
            .script
            .as_deref()
            .ok_or_else(|| "Presentation Editor command is missing its script.".to_string())?,
    );
    let requested_script = if requested_script.is_absolute() {
        requested_script.to_path_buf()
    } else {
        cwd.join(requested_script)
    };
    let frozen_source = workspace_root.join(frozen_script_source);
    if requested_script.canonicalize().ok() != frozen_source.canonicalize().ok() {
        return Err(
            "Presentation Editor command script does not match its frozen materialization input."
                .to_string(),
        );
    }

    let source_mount_path = builder.source_paths[0].clone();
    if source_mount_path == super::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH
        || Path::new(&source_mount_path)
            .extension()
            .and_then(OsStr::to_str)
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("pptx"))
    {
        return Err("Presentation Editor --source must name one mounted .pptx input.".to_string());
    }
    let source_matches = request
        .inputs
        .iter()
        .filter(|input| input.mount_path == source_mount_path)
        .collect::<Vec<_>>();
    if source_matches.len() != 1 {
        return Err(
            "Presentation Editor --source must match exactly one frozen input binding.".to_string(),
        );
    }
    let source_binding = source_matches[0].clone();
    let source_path = presentation_editor_source_path(&source_binding.source)?;
    let destination_path = builder.output_paths[0].clone();
    if source_mount_path == destination_path {
        return Err(
            "Presentation Editor save-as destination must differ from its source mount."
                .to_string(),
        );
    }
    let destination_binding = request
        .managed_office_script
        .as_deref()
        .filter(|binding| {
            binding.purpose == OfficeManagedScriptPurpose::EditPresentationPlan
                && binding.document_kind == crate::office::OfficeDocumentKind::Presentation
                && binding.script_mount_path == super::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH
                && binding.source_mount_path.as_deref() == Some(source_mount_path.as_str())
        })
        .ok_or_else(|| {
            "Presentation Editor is missing its approval-time destination binding.".to_string()
        })?
        .clone();

    let mut asset_specs = Vec::new();
    let mut asset_bindings = Vec::new();
    for input in &request.inputs {
        if input.mount_path == super::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH
            || input.mount_path == source_mount_path
        {
            continue;
        }
        asset_specs.push(AgentFileInputSpec {
            mount_path: input.mount_path.clone(),
            source: input.source.clone(),
        });
        asset_bindings.push(input.clone());
    }
    Ok(Some(PresentationEditorContract {
        source_mount_path,
        source_path,
        source_binding,
        destination_path: destination_binding.destination.logical_path.clone(),
        plan_destination_path: destination_path,
        destination_binding,
        asset_specs,
        asset_bindings,
    }))
}

pub(crate) fn is_presentation_editor_direct_command(command: &str) -> bool {
    let Ok(Some(builder)) = infer_managed_artifact_builder_command(command) else {
        return false;
    };
    if builder.kind != AgentCommandRuntimeKind::Node
        || builder.node_syntax_check
        || builder.source_paths.len() != 1
        || builder.output_paths.len() != 1
    {
        return false;
    }
    let Ok(tokens) = managed_artifact_command_tokens(command) else {
        return false;
    };
    tokens.len() == 6
        && tokens[0] == "node"
        && tokens[1].ends_with(".mjs")
        && matches!(tokens[2].as_str(), "--source" | "--output")
        && matches!(tokens[4].as_str(), "--source" | "--output")
        && tokens[2] != tokens[4]
}

pub(super) fn presentation_editor_source_path(
    source: &AgentFileInputRef,
) -> Result<String, String> {
    match source {
        AgentFileInputRef::Workspace { path } | AgentFileInputRef::External { path } => {
            Ok(path.clone())
        }
        AgentFileInputRef::Attachment { read_path } => Ok(read_path.clone()),
        AgentFileInputRef::GeneratedArtifact { path, .. } => Ok(path.clone()),
        AgentFileInputRef::BrowserDownload { reference, .. } => Ok(reference.clone()),
        AgentFileInputRef::SkillResource { .. } => Err(
            "Presentation Editor source must be a workspace, attachment, Artifact, browser download, or authorized external .pptx."
                .to_string(),
        ),
    }
}

pub(super) fn prepare_presentation_editor_execution(
    contract: PresentationEditorContract,
    prepared_inputs: Option<&PreparedAgentFileInputs>,
    runtime_root: &Path,
) -> Result<PreparedPresentationEditor, String> {
    let inputs = prepared_inputs.ok_or_else(|| {
        "Presentation Editor is missing its frozen private input snapshot.".to_string()
    })?;
    let frozen_script_path = inputs
        .root()
        .join(super::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH);
    let metadata = fs::symlink_metadata(&frozen_script_path)
        .map_err(|error| format!("Cannot inspect frozen Presentation Editor script: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("Frozen Presentation Editor script must be a regular file.".to_string());
    }
    let frozen_script_path = frozen_script_path.canonicalize().map_err(|error| {
        format!("Cannot canonicalize frozen Presentation Editor script: {error}")
    })?;
    super::validate_presentation_editor_script(&frozen_script_path)?;
    let runtime_root = runtime_root
        .canonicalize()
        .map_err(|error| format!("Cannot canonicalize Managed Runtime root: {error}"))?;
    let plan_directory = tempfile::Builder::new()
        .prefix("mycopilot-presentation-editor-")
        .tempdir()
        .map_err(|error| {
            format!("Cannot create private Presentation Editor plan directory: {error}")
        })?;
    let plan_root = plan_directory.path().canonicalize().map_err(|error| {
        format!("Cannot canonicalize private Presentation Editor plan directory: {error}")
    })?;
    let private_home = plan_root.join("home");
    let private_tmp = plan_root.join("tmp");
    fs::create_dir(&private_home)
        .and_then(|_| fs::create_dir(&private_tmp))
        .map_err(|error| {
            format!("Cannot prepare private Presentation Editor environment: {error}")
        })?;
    let private_home = private_home.canonicalize().map_err(|error| {
        format!("Cannot canonicalize private Presentation Editor HOME: {error}")
    })?;
    let private_tmp = private_tmp.canonicalize().map_err(|error| {
        format!("Cannot canonicalize private Presentation Editor temporary directory: {error}")
    })?;
    let plan_path = plan_root.join("edit-plan.json");
    Ok(PreparedPresentationEditor {
        contract,
        _plan_directory: plan_directory,
        plan_path,
        frozen_script_path,
        runtime_root,
        private_home,
        private_tmp,
    })
}

pub(super) fn canonical_editor_arguments_prefix(
    invocation: &ArtifactRuntimeInvocation,
) -> Result<Vec<OsString>, CommandExecutionError> {
    let mut arguments = invocation.arguments_prefix().to_vec();
    let mut index = 0usize;
    while index < arguments.len() {
        if arguments[index] == OsStr::new("--import") {
            let bootstrap = arguments.get(index + 1).ok_or_else(|| {
                CommandExecutionError::from(
                    "Managed Node runtime is missing its fixed bootstrap path.".to_string(),
                )
            })?;
            let bootstrap = Path::new(bootstrap).canonicalize().map_err(|error| {
                CommandExecutionError::from(format!(
                    "Cannot canonicalize Managed Node bootstrap: {error}"
                ))
            })?;
            arguments[index + 1] = bootstrap.into_os_string();
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok(arguments)
}

pub(super) fn presentation_editor_environment(
    invocation: &ArtifactRuntimeInvocation,
    editor: &PreparedPresentationEditor,
) -> Vec<(OsString, OsString)> {
    let mut environment = invocation
        .environment()
        .iter()
        .map(|(key, value)| {
            if key == OsStr::new("MYCOPILOT_ARTIFACT_NODE_MODULES") {
                let canonical = Path::new(value)
                    .canonicalize()
                    .unwrap_or_else(|_| PathBuf::from(value));
                (key.clone(), canonical.into_os_string())
            } else {
                (key.clone(), value.clone())
            }
        })
        .collect::<Vec<_>>();
    environment.extend([
        (OsString::from("LANG"), OsString::from("C.UTF-8")),
        (OsString::from("LC_ALL"), OsString::from("C")),
        (OsString::from("TZ"), OsString::from("UTC")),
        (OsString::from("TERM"), OsString::from("dumb")),
        (OsString::from("CI"), OsString::from("1")),
        (
            OsString::from("HOME"),
            editor.private_home.clone().into_os_string(),
        ),
        (
            OsString::from("USERPROFILE"),
            editor.private_home.clone().into_os_string(),
        ),
        (
            OsString::from("TMPDIR"),
            editor.private_tmp.clone().into_os_string(),
        ),
        (
            OsString::from("TMP"),
            editor.private_tmp.clone().into_os_string(),
        ),
        (
            OsString::from("TEMP"),
            editor.private_tmp.clone().into_os_string(),
        ),
    ]);
    environment
}

pub(super) fn presentation_editor_output_redactions(
    editor: &PreparedPresentationEditor,
    prepared_inputs: Option<&PreparedAgentFileInputs>,
    invocation: &ArtifactRuntimeInvocation,
    cwd: &Path,
) -> super::output_capture::ProcessOutputRedactionSet {
    let mut replacements = Vec::new();
    append_private_path_spellings(
        &mut replacements,
        editor
            .plan_path
            .parent()
            .expect("private editor plan has a parent"),
        "<presentation-editor-private>",
    );
    append_private_path_spellings(
        &mut replacements,
        &editor.frozen_script_path,
        "<presentation-editor.mjs>",
    );
    if let Some(inputs) = prepared_inputs {
        append_private_path_spellings(
            &mut replacements,
            inputs.root(),
            "<presentation-editor-inputs>",
        );
    }
    append_private_path_spellings(&mut replacements, &editor.runtime_root, "<managed-runtime>");
    append_private_path_spellings(&mut replacements, invocation.executable(), "<managed-node>");
    append_private_path_spellings(&mut replacements, cwd, ".");
    super::output_capture::ProcessOutputRedactionSet::new(replacements)
}

pub(super) fn managed_office_script_output_redactions(
    script: &PreparedManagedOfficeScript,
    prepared_inputs: Option<&PreparedAgentFileInputs>,
    runtime_root: &Path,
    invocation: &ArtifactRuntimeInvocation,
    cwd: &Path,
) -> super::output_capture::ProcessOutputRedactionSet {
    let mut replacements = Vec::new();
    append_private_path_spellings(
        &mut replacements,
        script.staging.private_directory(),
        "<office-candidate>",
    );
    append_private_path_spellings(
        &mut replacements,
        &script.frozen_script_path,
        "<managed-office-script>",
    );
    if let Some(inputs) = prepared_inputs {
        append_private_path_spellings(&mut replacements, inputs.root(), "$MYCOPILOT_INPUT_ROOT");
    }
    append_private_path_spellings(&mut replacements, runtime_root, "<managed-runtime>");
    append_private_path_spellings(
        &mut replacements,
        invocation.executable(),
        "<managed-runtime>",
    );
    append_private_path_spellings(&mut replacements, cwd, ".");
    super::output_capture::ProcessOutputRedactionSet::new(replacements)
}

#[derive(Debug)]
pub(super) struct ManagedBuilderSyntaxCheckExecution {
    pub(super) exit_code: Option<i32>,
    stdout: Option<CapturedProcessOutput>,
    stderr: Option<CapturedProcessOutput>,
    timed_out: bool,
    cancelled: bool,
    duration_ms: u64,
    error: Option<String>,
}

impl ManagedBuilderSyntaxCheckExecution {
    fn failed(started: Instant, error: impl Into<String>) -> Self {
        Self {
            exit_code: None,
            stdout: None,
            stderr: None,
            timed_out: false,
            cancelled: false,
            duration_ms: elapsed_millis_saturating(started),
            error: Some(error.into()),
        }
    }

    pub(super) fn succeeded(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out && !self.cancelled && self.error.is_none()
    }
}

pub(super) fn run_managed_builder_syntax_check(
    invocation: &ArtifactRuntimeInvocation,
    script: &str,
    cwd: &Path,
    environment: &[(OsString, OsString)],
    cancellation: &AgentCancellationToken,
    redactions: super::output_capture::ProcessOutputRedactionSet,
    language: ManagedBuilderSyntaxLanguage,
) -> ManagedBuilderSyntaxCheckExecution {
    let started = Instant::now();
    if cancellation.is_cancelled() {
        return ManagedBuilderSyntaxCheckExecution {
            cancelled: true,
            ..ManagedBuilderSyntaxCheckExecution::failed(
                started,
                format!(
                    "Managed Builder {} 语法预检在启动前被取消。",
                    language.display_name()
                ),
            )
        };
    }

    let mut arguments = invocation.arguments_prefix().to_vec();
    match language {
        ManagedBuilderSyntaxLanguage::Node => {
            arguments.push(OsString::from("--check"));
            arguments.push(OsString::from(script));
        }
        ManagedBuilderSyntaxLanguage::Python => {
            arguments.push(OsString::from("-c"));
            arguments.push(OsString::from(
                "import sys; path = sys.argv[1]; compile(open(path, 'rb').read(), path, 'exec')",
            ));
            arguments.push(OsString::from(script));
        }
    }
    let launch = CommandDirectLaunchPlan::isolated(
        invocation.executable().to_path_buf(),
        arguments,
        environment.to_vec(),
    );
    let plan = CommandSpawnPlan::direct(
        language.launch_name().to_string(),
        cwd.to_path_buf(),
        None,
        Some(Duration::from_millis(
            MANAGED_BUILDER_SYNTAX_CHECK_TIMEOUT_MS,
        )),
        launch,
    );
    let mut command = plan.build();
    let mut child = match command.spawn() {
        Ok(child) => ManagedCommandChild::new(child),
        Err(_) => {
            return ManagedBuilderSyntaxCheckExecution::failed(
                started,
                format!(
                    "无法启动固定受管 {} 进行 Builder 语法预检。",
                    language.display_name()
                ),
            )
        }
    };
    let Some(stdout) = child.child_mut().stdout.take() else {
        return ManagedBuilderSyntaxCheckExecution::failed(
            started,
            format!(
                "无法捕获受管 {} 语法预检的 stdout。",
                language.display_name()
            ),
        );
    };
    let Some(stderr) = child.child_mut().stderr.take() else {
        return ManagedBuilderSyntaxCheckExecution::failed(
            started,
            format!(
                "无法捕获受管 {} 语法预检的 stderr。",
                language.display_name()
            ),
        );
    };
    let capture_policy = ProcessOutputCapturePolicy::process_default();
    let capture_budget = ProcessOutputCaptureBudget::new(capture_policy.max_capture_bytes());
    let stdout_reader = super::output_capture::spawn_process_output_capture_with_observers(
        stdout,
        capture_budget.clone(),
        capture_policy,
        Some(crate::AgentCommandOutputStream::Stdout),
        None,
        None,
        redactions.clone(),
    );
    let stderr_reader = super::output_capture::spawn_process_output_capture_with_observers(
        stderr,
        capture_budget,
        capture_policy,
        Some(crate::AgentCommandOutputStream::Stderr),
        None,
        None,
        redactions,
    );

    let mut timed_out = false;
    let mut cancelled = false;
    let mut error = None;
    let exit_status = loop {
        match try_wait_command_process_group(child.child_mut()) {
            Ok(Some(status)) => {
                child.mark_reaped();
                break Some(status);
            }
            Ok(None) => {}
            Err(_) => {
                error = Some(format!(
                    "等待受管 {} 语法预检失败。",
                    language.display_name()
                ));
                force_terminate_command_process_group(child.child_mut());
                let status = child.child_mut().wait().ok();
                if status.is_some() {
                    child.mark_reaped();
                }
                break status;
            }
        }
        if cancellation.is_cancelled() {
            cancelled = true;
            force_terminate_command_process_group(child.child_mut());
            let status = child.child_mut().wait().ok();
            if status.is_some() {
                child.mark_reaped();
            }
            break status;
        }
        if started.elapsed() >= Duration::from_millis(MANAGED_BUILDER_SYNTAX_CHECK_TIMEOUT_MS) {
            timed_out = true;
            force_terminate_command_process_group(child.child_mut());
            let status = child.child_mut().wait().ok();
            if status.is_some() {
                child.mark_reaped();
            }
            break status;
        }
        thread::sleep(Duration::from_millis(10));
    };
    drop(child);

    let stdout_label = format!("受管 {} 语法预检 stdout", language.display_name());
    let stdout = match join_process_output_capture(stdout_reader, &stdout_label) {
        Ok(capture) => Some(capture),
        Err(_) => {
            error.get_or_insert_with(|| {
                format!(
                    "读取受管 {} 语法预检 stdout 失败。",
                    language.display_name()
                )
            });
            None
        }
    };
    let stderr_label = format!("受管 {} 语法预检 stderr", language.display_name());
    let stderr = match join_process_output_capture(stderr_reader, &stderr_label) {
        Ok(capture) => Some(capture),
        Err(_) => {
            error.get_or_insert_with(|| {
                format!(
                    "读取受管 {} 语法预检 stderr 失败。",
                    language.display_name()
                )
            });
            None
        }
    };
    ManagedBuilderSyntaxCheckExecution {
        exit_code: exit_status
            .as_ref()
            .and_then(std::process::ExitStatus::code),
        stdout,
        stderr,
        timed_out,
        cancelled,
        duration_ms: elapsed_millis_saturating(started),
        error,
    }
}

pub(super) fn managed_builder_syntax_check_failure_result(
    root: Option<&Path>,
    cwd: &Path,
    request: &AgentCommandRequest,
    resolution: AgentCommandRuntimeResolution,
    execution: ManagedBuilderSyntaxCheckExecution,
    language: ManagedBuilderSyntaxLanguage,
) -> AgentCommandExecutionResult {
    let language_name = language.display_name();
    let (code, recovery, message) = if execution.cancelled {
        (
            ERROR_BUILDER_SYNTAX_CHECK_FAILED,
            "retry",
            format!("Managed Builder {language_name} 语法预检已取消；Builder 未启动。"),
        )
    } else if execution.timed_out {
        (
            ERROR_BUILDER_SYNTAX_CHECK_FAILED,
            "retry",
            format!("Managed Builder {language_name} 语法预检超时；Builder 未启动。"),
        )
    } else if execution.error.is_some() || execution.exit_code.is_none() {
        (
            ERROR_BUILDER_SYNTAX_CHECK_FAILED,
            ArtifactRuntimeRecovery::RepairComponent.stable_name(),
            format!("Managed Builder {language_name} 语法预检无法完成；Builder 未启动。"),
        )
    } else {
        (
            ERROR_BUILDER_SYNTAX_INVALID,
            "changeBuilder",
            format!(
                "Managed Builder 未通过 {language_name} 语法校验；Builder 未启动。请修复脚本后重新执行。"
            ),
        )
    };
    let runtime = with_resolution_error(resolution, code, recovery, &message);
    let mut result = if execution.cancelled {
        runtime_cancelled_result(root, cwd, request, runtime)
    } else {
        runtime_failure_result(root, cwd, request, runtime, execution.duration_ms)
    };
    result.exit_code = execution.exit_code;
    result.timed_out = execution.timed_out;
    result.cancelled = execution.cancelled;
    result.duration_ms = execution.duration_ms;
    result.output_capture = ProcessOutputCaptureMetadata::from_optional_streams(
        execution.stdout.as_ref(),
        execution.stderr.as_ref(),
        execution.stdout.is_none() || execution.stderr.is_none(),
    );
    if let Some(stdout) = execution.stdout.as_ref() {
        result.stdout = stdout.preview().to_string();
        result.stdout_truncated = stdout.preview_truncated();
        result.stdout_spool = stdout.spool();
    } else {
        result.stdout_truncated = true;
    }
    if let Some(stderr) = execution.stderr.as_ref() {
        result.stderr = stderr.preview().to_string();
        result.stderr_truncated = stderr.preview_truncated();
        result.stderr_spool = stderr.spool();
    } else {
        result.stderr_truncated = true;
    }
    result
}

pub(super) fn managed_builder_syntax_check_redactions(
    runtime_root: &Path,
    invocation: &ArtifactRuntimeInvocation,
    cwd: &Path,
    script: &str,
    language: ManagedBuilderSyntaxLanguage,
) -> super::output_capture::ProcessOutputRedactionSet {
    let mut replacements = Vec::new();
    let requested_script = Path::new(script);
    let script_path = if requested_script.is_absolute() {
        requested_script.to_path_buf()
    } else {
        cwd.join(requested_script)
    };
    let script_projection = if requested_script.is_absolute() {
        "<managed-builder>"
    } else {
        script
    };
    append_private_path_spellings(&mut replacements, &script_path, script_projection);
    append_private_path_spellings(&mut replacements, cwd, ".");
    append_private_path_spellings(&mut replacements, runtime_root, "<managed-runtime>");
    append_private_path_spellings(
        &mut replacements,
        invocation.executable(),
        language.executable_projection(),
    );
    super::output_capture::ProcessOutputRedactionSet::new(replacements)
}

pub(super) fn elapsed_millis_saturating(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}
