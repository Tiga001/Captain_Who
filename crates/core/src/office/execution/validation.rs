use super::*;

pub(crate) fn validate_office_request(
    request: &OfficeExecutionRequest,
) -> Result<(), OfficeEngineError> {
    validate_request_syntax(request).map(|_| ())
}

pub(super) fn validate_request_syntax(
    request: &OfficeExecutionRequest,
) -> Result<(Vec<String>, Vec<String>), OfficeEngineError> {
    let arguments = compile_office_arguments(request)?;
    let resource_paths = validate_operation_arguments(request.operation, &arguments)?;
    validate_agent_input_contract(request, &resource_paths)?;

    if request
        .timeout_ms
        .is_some_and(|timeout| !(1..=MAX_OFFICE_TIMEOUT_MS).contains(&timeout))
    {
        return Err(invalid_request(format!(
            "Office timeout must be between 1 and {MAX_OFFICE_TIMEOUT_MS} milliseconds."
        )));
    }

    match request.operation {
        OfficeOperation::Help => {
            if request.document_path.is_some()
                || request.output_path.is_some()
                || request.destination_path.is_some()
            {
                return Err(invalid_request(
                    "The Office help operation cannot receive document, output, or destination paths.",
                ));
            }
        }
        OfficeOperation::Create => {
            required_document_path(request)?;
            if request.output_path.is_some() || request.destination_path.is_some() {
                return Err(invalid_request(
                    "The Office create operation cannot receive a separate output or destination path.",
                ));
            }
        }
        OfficeOperation::View => {
            required_document_path(request)?;
            if request.destination_path.is_some() {
                return Err(invalid_request(
                    "The Office view operation cannot receive a mutation destination path.",
                ));
            }
            let mode = request.view_mode();
            let writes_render = mode.is_some_and(OfficeViewMode::writes_output);
            match (request.output_path.is_some(), writes_render) {
                (true, false) => {
                    return Err(invalid_request(
                        "Office view output paths are supported only for html, screenshot, or svg modes.",
                    ))
                }
                (false, true) => {
                    return Err(invalid_request(
                        "Rendering requires an explicit output path.",
                    ))
                }
                _ => {}
            }
        }
        OfficeOperation::Set
        | OfficeOperation::Add
        | OfficeOperation::Remove
        | OfficeOperation::Move
        | OfficeOperation::Swap => {
            required_document_path(request)?;
            if request.output_path.is_some() {
                return Err(invalid_request(
                    "Only the Office view operation accepts an output path.",
                ));
            }
        }
        OfficeOperation::Get | OfficeOperation::Query | OfficeOperation::Validate => {
            required_document_path(request)?;
            if request.output_path.is_some() || request.destination_path.is_some() {
                return Err(invalid_request(
                    "Read-only Office operations cannot receive output or mutation destination paths.",
                ));
            }
        }
    }

    if let Some(document_path) = request.document_path.as_deref() {
        validate_document_extension(request, Path::new(document_path.trim()))?;
    }
    if let Some(destination_path) = request.destination_path.as_deref() {
        validate_document_extension(request, Path::new(destination_path.trim()))?;
    }
    if let Some(output_path) = request.output_path.as_deref() {
        validate_render_output_extension(
            Path::new(output_path.trim()),
            request.view_mode().map(OfficeViewMode::cli_name),
        )?;
    }

    Ok((arguments, resource_paths))
}

fn validate_agent_input_contract(
    request: &OfficeExecutionRequest,
    resource_paths: &[String],
) -> Result<(), OfficeEngineError> {
    let normalized =
        normalize_agent_file_input_specs(&request.inputs).map_err(office_input_prepare_error)?;
    if normalized != request.inputs {
        return Err(invalid_request(
            "Office input specs must use canonical mount paths and source references.",
        ));
    }

    let mut expected = BTreeMap::<String, usize>::new();
    for input in &normalized {
        let placeholder = office_agent_input_placeholder(&input.mount_path);
        expected.insert(placeholder, 1);
    }
    let mut observed = BTreeMap::<String, usize>::new();
    for resource in resource_paths {
        if !resource.starts_with(OFFICE_AGENT_INPUT_PLACEHOLDER_PREFIX) {
            continue;
        }
        if !expected.contains_key(resource) {
            return Err(invalid_request(format!(
                "Office resource `{resource}` does not have a matching declared Agent input.",
            )));
        }
        *observed.entry(resource.clone()).or_default() += 1;
    }
    if observed != expected {
        return Err(invalid_request(
            "Every declared Office Agent input must be referenced exactly once by its trusted placeholder.",
        ));
    }
    Ok(())
}

pub(super) fn validate_frozen_agent_inputs(
    request: &OfficeExecutionRequest,
    bindings: &[AgentFileInputBinding],
) -> Result<(), OfficeEngineError> {
    if bindings.len() != request.inputs.len() {
        return Err(precondition_error(
            "The frozen Office input bindings no longer match the canonical request.",
        ));
    }
    for (spec, binding) in request.inputs.iter().zip(bindings) {
        if spec.mount_path != binding.mount_path || spec.source != binding.source {
            return Err(precondition_error(
                "A frozen Office input binding does not match its declared logical source.",
            ));
        }
    }
    Ok(())
}

pub(super) fn office_input_prepare_error(error: AgentFileInputError) -> OfficeEngineError {
    OfficeEngineError::new(
        if error.code() == "agent.fileInput.authorizationDenied" {
            OfficeEngineErrorCode::WorkspaceViolation
        } else {
            OfficeEngineErrorCode::InvalidRequest
        },
        OfficeEngineRecovery::ChangeRequest,
        format!(
            "Office input preparation failed ({}): {}",
            error.code(),
            error.message()
        ),
    )
}

pub(super) fn office_input_execution_error(error: AgentFileInputError) -> OfficeEngineError {
    OfficeEngineError::new(
        if error.code() == "agent.fileInput.authorizationDenied" {
            OfficeEngineErrorCode::WorkspaceViolation
        } else {
            OfficeEngineErrorCode::PreconditionFailed
        },
        OfficeEngineRecovery::Retry,
        format!(
            "Office input revalidation failed ({}): {}",
            error.code(),
            error.message()
        ),
    )
}

fn validate_render_output_extension(
    path: &Path,
    mode: Option<&str>,
) -> Result<(), OfficeEngineError> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    let matches = match mode {
        Some("html") => matches!(extension.as_deref(), Some("html" | "htm")),
        Some("screenshot") => extension.as_deref() == Some("png"),
        Some("svg") => extension.as_deref() == Some("svg"),
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(invalid_request(
            "Office render output extension must match the requested html, screenshot, or svg mode.",
        ))
    }
}

pub(super) fn required_document_path(
    request: &OfficeExecutionRequest,
) -> Result<&str, OfficeEngineError> {
    request
        .document_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid_request("The Office operation requires a document path."))
}

pub(super) fn validate_document_extension(
    request: &OfficeExecutionRequest,
    path: &Path,
) -> Result<(), OfficeEngineError> {
    if request.document_kind.accepts_path(path) {
        return Ok(());
    }
    Err(invalid_request(format!(
        "Document path `{}` does not match {:?}; expected one of: {}.",
        path.display(),
        request.document_kind,
        request.document_kind.accepted_extensions().join(", ")
    )))
}

pub(super) fn validate_arguments(arguments: &[String]) -> Result<(), OfficeEngineError> {
    if arguments.len() > MAX_OFFICE_ARGUMENTS {
        return Err(invalid_request(format!(
            "Office argv exceeds the {MAX_OFFICE_ARGUMENTS}-argument limit."
        )));
    }
    let bytes = arguments
        .iter()
        .try_fold(0usize, |total, value| total.checked_add(value.len()))
        .ok_or_else(|| invalid_request("Office argv size overflowed."))?;
    if bytes > MAX_OFFICE_ARGUMENT_BYTES {
        return Err(invalid_request(format!(
            "Office argv exceeds the {MAX_OFFICE_ARGUMENT_BYTES}-byte limit."
        )));
    }
    if arguments.iter().any(|argument| argument.contains('\0')) {
        return Err(invalid_request("Office argv cannot contain NUL bytes."));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OfficeOptionArity {
    Flag,
    Value,
    OptionalValue,
}

fn validate_operation_arguments(
    operation: OfficeOperation,
    arguments: &[String],
) -> Result<Vec<String>, OfficeEngineError> {
    const RESERVED_OPTIONS: &[&str] = &[
        "-o",
        "--out",
        "--output",
        "--save",
        "--input",
        "--browser",
        "--resident",
        "--watch",
        "--serve",
        "--server",
    ];
    if arguments.iter().any(|argument| {
        let lower = argument.to_ascii_lowercase();
        lower.contains("file://") || lower.contains("http://") || lower.contains("https://")
    }) {
        return Err(OfficeEngineError::new(
            OfficeEngineErrorCode::UnsafeOperation,
            OfficeEngineRecovery::ChangeRequest,
            "Office arguments cannot request file URLs or network resources.",
        ));
    }

    let mut resource_paths = Vec::new();
    let mut positional_arguments = Vec::new();
    let mut add_type = None::<String>;
    let mut diagram_render = None::<String>;
    let mut index = 0usize;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument == "--" {
            return Err(unsafe_argument(
                argument,
                "pass-through argument separators are not allowed",
            ));
        }
        if !argument.starts_with('-') {
            positional_arguments.push(argument.as_str());
            index += 1;
            continue;
        }

        let (raw_name, attached_value) = match argument.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (argument.as_str(), None),
        };
        let normalized_name = raw_name.to_ascii_lowercase();
        if RESERVED_OPTIONS.contains(&normalized_name.as_str())
            || (normalized_name.starts_with("-o") && !normalized_name.starts_with("--"))
        {
            return Err(unsafe_argument(
                argument,
                "the option can write outside the managed Office transaction or changes process behavior",
            ));
        }
        if raw_name != normalized_name {
            return Err(invalid_request(
                "Office option names must use their canonical lowercase spelling.",
            ));
        }

        let Some(arity) = allowed_option(operation, &normalized_name) else {
            return Err(invalid_request(format!(
                "Office option `{raw_name}` is not allowed for `{}` by the managed v1.0.139 command profile.",
                operation.cli_name()
            )));
        };
        let value = match arity {
            OfficeOptionArity::Flag => {
                if attached_value.is_some() {
                    return Err(invalid_request(format!(
                        "Office flag `{raw_name}` does not accept a value."
                    )));
                }
                index += 1;
                None
            }
            OfficeOptionArity::Value => {
                if normalized_name == "--prop" && attached_value.is_some() {
                    return Err(invalid_request(
                        "Office properties must use separate `--prop` and `key=value` arguments; `--prop=...` is rejected.",
                    ));
                }
                let value = if let Some(value) = attached_value {
                    if value.is_empty() {
                        return Err(invalid_request(format!(
                            "Office option `{raw_name}` requires a non-empty value."
                        )));
                    }
                    value
                } else {
                    let value = arguments.get(index + 1).ok_or_else(|| {
                        invalid_request(format!("Office option `{raw_name}` requires a value."))
                    })?;
                    if value.starts_with('-') {
                        return Err(invalid_request(format!(
                            "Office option `{raw_name}` requires a value before `{value}`."
                        )));
                    }
                    index += 1;
                    value.as_str()
                };
                index += 1;
                Some(value)
            }
            OfficeOptionArity::OptionalValue => {
                let value = if let Some(value) = attached_value {
                    if value.is_empty() {
                        return Err(invalid_request(format!(
                            "Office option `{raw_name}` cannot receive an empty value."
                        )));
                    }
                    Some(value)
                } else if arguments
                    .get(index + 1)
                    .is_some_and(|value| !value.starts_with('-'))
                {
                    index += 1;
                    Some(arguments[index].as_str())
                } else {
                    None
                };
                index += 1;
                value
            }
        };

        if operation == OfficeOperation::Add && normalized_name == "--type" {
            add_type = value.map(|value| value.trim().to_ascii_lowercase());
        }
        if normalized_name != "--prop" {
            if operation == OfficeOperation::View
                && normalized_name == "--render"
                && value.is_some_and(|value| !matches!(value, "auto" | "html"))
            {
                return Err(unsafe_argument(
                    argument,
                    "native Office rendering may launch an unmanaged desktop application",
                ));
            }
            continue;
        }
        let property = value.expect("--prop has required value arity");
        let Some((name, value)) = property.split_once('=') else {
            return Err(invalid_request(
                "Office properties must use key=value syntax.",
            ));
        };
        if name.trim().is_empty() {
            return Err(invalid_request(
                "Office properties require a non-empty key.",
            ));
        }
        let normalized_property = name.trim().to_ascii_lowercase();
        if normalized_property == "render" {
            if !value.eq_ignore_ascii_case("native") {
                return Err(unsafe_argument(
                    property,
                    "managed diagram rendering must use the built-in `native` renderer and cannot launch a browser or fetch runtime assets",
                ));
            }
            diagram_render = Some("native".to_string());
        }
        if normalized_property == "data" {
            return Err(unsafe_argument(
                property,
                "the pinned provider treats `data` as either inline content or a FileSource; this ambiguous surface is disabled until it has an element-aware schema",
            ));
        }
        if let Some(resource) = property_resource_reference(name, value)? {
            let path = resource.path();
            if path.to_ascii_lowercase().starts_with("data:") {
                return Err(unsafe_argument(
                    property,
                    "inline resource payloads are not part of the frozen workspace resource set",
                ));
            }
            resource_paths.push(path.to_string());
        }
    }

    if operation == OfficeOperation::Add
        && add_type
            .as_deref()
            .is_some_and(|value| matches!(value, "diagram" | "flowchart" | "mermaid"))
        && diagram_render.as_deref() != Some("native")
    {
        return Err(unsafe_argument(
            "--type diagram",
            "diagram creation must explicitly include `--prop render=native` so the provider cannot start a browser or fetch mermaid.js",
        ));
    }

    let (minimum, maximum) = positional_argument_bounds(operation);
    let positional_count = positional_arguments.len();
    if !(minimum..=maximum).contains(&positional_count) {
        let expected = if minimum == maximum {
            minimum.to_string()
        } else {
            format!("{minimum} to {maximum}")
        };
        return Err(invalid_request(format!(
            "Office operation `{}` requires {expected} positional argument(s) after the document path; received {positional_count}.",
            operation.cli_name()
        )));
    }
    if operation == OfficeOperation::View {
        let mode = positional_arguments[0];
        if !matches!(
            mode,
            "text"
                | "annotated"
                | "outline"
                | "stats"
                | "issues"
                | "html"
                | "svg"
                | "screenshot"
                | "forms"
        ) {
            return Err(invalid_request(format!(
                "Office view mode `{mode}` is outside the managed v1.0.139 mode profile."
            )));
        }
    }
    Ok(resource_paths)
}

fn allowed_option(operation: OfficeOperation, name: &str) -> Option<OfficeOptionArity> {
    use OfficeOperation as Operation;
    use OfficeOptionArity::{Flag, OptionalValue, Value};

    if matches!(name, "-?" | "-h" | "--help") {
        return Some(Flag);
    }
    match (operation, name) {
        (Operation::Help, "--json" | "--jsonl") => Some(Flag),
        (Operation::Create, "--type" | "--locale") => Some(Value),
        (Operation::Create, "--force" | "--minimal" | "--json") => Some(Flag),
        (
            Operation::View,
            "--start"
            | "--end"
            | "--max-lines"
            | "--type"
            | "--limit"
            | "--cols"
            | "--page"
            | "--range"
            | "--screenshot-width"
            | "--screenshot-height"
            | "--render",
        ) => Some(Value),
        (Operation::View, "--grid") => Some(OptionalValue),
        (Operation::View, "--page-count" | "--json") => Some(Flag),
        (Operation::Get, "--depth") => Some(Value),
        (Operation::Get, "--json") => Some(Flag),
        (Operation::Query, "--find" | "--fields") => Some(Value),
        (Operation::Query, "--compact" | "--json") => Some(Flag),
        (Operation::Validate, "--json") => Some(Flag),
        (Operation::Set, "--prop" | "--find" | "--replace") => Some(Value),
        (Operation::Set, "--json" | "--force") => Some(Flag),
        (Operation::Add, "--type" | "--from" | "--index" | "--after" | "--before" | "--prop") => {
            Some(Value)
        }
        (Operation::Add, "--json" | "--force") => Some(Flag),
        (Operation::Remove, "--shift" | "--prop") => Some(Value),
        (Operation::Remove, "--json") => Some(Flag),
        (Operation::Move, "--to" | "--index" | "--after" | "--before" | "--prop") => Some(Value),
        (Operation::Move, "--json") => Some(Flag),
        (Operation::Swap, "--json") => Some(Flag),
        _ => None,
    }
}

fn positional_argument_bounds(operation: OfficeOperation) -> (usize, usize) {
    match operation {
        OfficeOperation::Help => (0, 3),
        OfficeOperation::Create | OfficeOperation::Validate => (0, 0),
        OfficeOperation::Get => (0, 1),
        OfficeOperation::View
        | OfficeOperation::Query
        | OfficeOperation::Set
        | OfficeOperation::Add
        | OfficeOperation::Remove
        | OfficeOperation::Move => (1, 1),
        OfficeOperation::Swap => (2, 2),
    }
}

fn unsafe_argument(argument: &str, reason: &str) -> OfficeEngineError {
    OfficeEngineError::new(
        OfficeEngineErrorCode::UnsafeOperation,
        OfficeEngineRecovery::ChangeRequest,
        format!("Office argument `{argument}` is unsafe in the managed engine because {reason}."),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PropertyResourceReference<'a> {
    Direct(&'a str),
    ImagePrefixed(&'a str),
}

impl<'a> PropertyResourceReference<'a> {
    pub(super) fn path(self) -> &'a str {
        match self {
            Self::Direct(path) | Self::ImagePrefixed(path) => path,
        }
    }

    pub(super) fn rewrite(self, property_name: &str, snapshot: &Path) -> String {
        match self {
            Self::Direct(_) => format!("{property_name}={}", snapshot.to_string_lossy()),
            Self::ImagePrefixed(_) => {
                format!("{property_name}=image:{}", snapshot.to_string_lossy())
            }
        }
    }
}

pub(super) fn property_resource_reference<'a>(
    name: &str,
    value: &'a str,
) -> Result<Option<PropertyResourceReference<'a>>, OfficeEngineError> {
    let name = name.trim().to_ascii_lowercase();
    let direct = matches!(
        name.as_str(),
        "csv"
            | "file"
            | "image"
            | "imagefill"
            | "imagepath"
            | "src"
            | "template"
            | "fallback"
            | "preview"
    );
    if direct {
        return Ok(Some(PropertyResourceReference::Direct(value)));
    }
    if name == "poster" {
        return if matches!(value.trim().to_ascii_lowercase().as_str(), "true" | "false") {
            Ok(None)
        } else {
            Ok(Some(PropertyResourceReference::Direct(value)))
        };
    }
    if name == "path" {
        return if matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "line" | "arc" | "circle" | "diamond" | "triangle" | "square" | "custom"
        ) {
            Ok(None)
        } else {
            Ok(Some(PropertyResourceReference::Direct(value)))
        };
    }
    if name == "background" {
        let lower = value.to_ascii_lowercase();
        if lower.starts_with("image:") {
            let path = &value["image:".len()..];
            if path.trim().is_empty() {
                return Err(invalid_request(
                    "Office image background requires a non-empty workspace resource path.",
                ));
            }
            return Ok(Some(PropertyResourceReference::ImagePrefixed(path)));
        }
    }
    Ok(None)
}
