use crate::office::{OfficeElementPosition, OfficeOperationParameters, OfficePropertyMap};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub(crate) const PRESENTATION_EDITOR_PLAN_SCHEMA_VERSION: u32 = 1;
pub(crate) const PRESENTATION_EDITOR_PLAN_ENV: &str = "MYCOPILOT_PRESENTATION_EDIT_PLAN";
pub(crate) const PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH: &str =
    "__mycopilot/presentation-editor/editor.mjs";
pub(crate) const PRESENTATION_EDITOR_RESERVED_MOUNT_PREFIX: &str =
    "__mycopilot/presentation-editor";
pub(crate) const MAX_PRESENTATION_EDITOR_PLAN_BYTES: u64 = 256 * 1024;
pub(crate) const MAX_PRESENTATION_EDITOR_OPERATIONS: usize = 256;
const MAX_PRESENTATION_EDITOR_STRING_CHARS: usize = 32 * 1024;
const MAX_PRESENTATION_EDITOR_SCRIPT_BYTES: u64 = 512 * 1024;
const PRESENTATION_EDITOR_TEMPLATE: &str =
    include_str!("../skills/bundled/presentations/templates/editor.mjs");
const EDIT_REGION_BEGIN: &str = "// BEGIN EDIT REGION";
const EDIT_REGION_END: &str = "// END EDIT REGION";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PresentationEditorPlan {
    pub(crate) schema_version: u32,
    pub(crate) source: PresentationEditorInput,
    pub(crate) destination: PresentationEditorOutput,
    pub(crate) mode: PresentationEditorMode,
    pub(crate) operations: Vec<OfficeOperationParameters>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum PresentationEditorInput {
    Input { mount_path: String },
}

impl PresentationEditorInput {
    pub(crate) fn mount_path(&self) -> &str {
        match self {
            Self::Input { mount_path } => mount_path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum PresentationEditorOutput {
    Output { path: String },
}

impl PresentationEditorOutput {
    pub(crate) fn path(&self) -> &str {
        match self {
            Self::Output { path } => path,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PresentationEditorMode {
    SaveAs,
}

pub(crate) fn read_presentation_editor_plan(path: &Path) -> Result<PresentationEditorPlan, String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("Presentation Editor did not produce a readable plan: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("Cannot inspect the Presentation Editor plan: {error}"))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err("Presentation Editor plan must be a non-empty regular file.".to_string());
    }
    if metadata.len() > MAX_PRESENTATION_EDITOR_PLAN_BYTES {
        return Err(format!(
            "Presentation Editor plan exceeds the {MAX_PRESENTATION_EDITOR_PLAN_BYTES}-byte limit."
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("Cannot read the Presentation Editor plan: {error}"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != metadata.len() {
        return Err("Presentation Editor plan changed while it was read.".to_string());
    }
    let plan: PresentationEditorPlan = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Presentation Editor emitted an invalid typed plan: {error}"))?;
    validate_presentation_editor_plan(&plan)?;
    Ok(plan)
}

/// Verifies that an approved editor is the trusted template with only its bounded edit region
/// changed. The OS/Node sandbox remains the authority boundary; this structural gate keeps common
/// accidental or adversarial attempts to replace the fixed wrapper from reaching that boundary.
pub(crate) fn validate_presentation_editor_script(path: &Path) -> Result<(), String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("Cannot read frozen Presentation Editor script: {error}"))?;
    if bytes.is_empty()
        || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_PRESENTATION_EDITOR_SCRIPT_BYTES
    {
        return Err("Frozen Presentation Editor script is empty or too large.".to_string());
    }
    let candidate = std::str::from_utf8(&bytes)
        .map_err(|_| "Frozen Presentation Editor script must be UTF-8.".to_string())?;
    let (template_prefix, template_tail) = split_editor_template(PRESENTATION_EDITOR_TEMPLATE)?;
    let (candidate_prefix, candidate_tail) = split_editor_template(candidate)?;
    if candidate_prefix != template_prefix || candidate_tail.1 != template_tail.1 {
        return Err(
            "Presentation Editor fixed wrapper changed outside the bounded EDIT REGION; rematerialize the trusted template and patch only that region."
                .to_string(),
        );
    }
    let _ = candidate_tail.0;
    Ok(())
}

fn split_editor_template(source: &str) -> Result<(&str, (&str, &str)), String> {
    let (prefix, after_begin) = source.split_once(EDIT_REGION_BEGIN).ok_or_else(|| {
        "Presentation Editor script is missing its BEGIN EDIT REGION marker.".to_string()
    })?;
    let (region, suffix) = after_begin.split_once(EDIT_REGION_END).ok_or_else(|| {
        "Presentation Editor script is missing its END EDIT REGION marker.".to_string()
    })?;
    if suffix.contains(EDIT_REGION_BEGIN)
        || suffix.contains(EDIT_REGION_END)
        || region.contains(EDIT_REGION_BEGIN)
    {
        return Err("Presentation Editor script has duplicate EDIT REGION markers.".to_string());
    }
    Ok((prefix, (region, suffix)))
}

pub(crate) fn validate_presentation_editor_plan(
    plan: &PresentationEditorPlan,
) -> Result<(), String> {
    if plan.schema_version != PRESENTATION_EDITOR_PLAN_SCHEMA_VERSION {
        return Err("Presentation Editor plan uses an unsupported schema version.".to_string());
    }
    validate_safe_relative_path(plan.source.mount_path(), "source input mount", "pptx")?;
    validate_output_path(plan.destination.path())?;
    if plan.source.mount_path() == plan.destination.path() {
        return Err(
            "Presentation Editor save-as destination must differ from its source input mount."
                .to_string(),
        );
    }
    if plan.operations.is_empty() || plan.operations.len() > MAX_PRESENTATION_EDITOR_OPERATIONS {
        return Err(format!(
            "Presentation Editor plan must contain between 1 and {MAX_PRESENTATION_EDITOR_OPERATIONS} operations."
        ));
    }
    for operation in &plan.operations {
        validate_operation(operation)?;
        let encoded = serde_json::to_vec(operation).map_err(|error| {
            format!("Cannot canonicalize Presentation Editor operation: {error}")
        })?;
        if encoded.len() > MAX_PRESENTATION_EDITOR_STRING_CHARS * 2 {
            return Err("One Presentation Editor operation is too large.".to_string());
        }
    }
    Ok(())
}

fn validate_operation(operation: &OfficeOperationParameters) -> Result<(), String> {
    match operation {
        OfficeOperationParameters::Set {
            target,
            properties,
            replacement,
            force,
        } => {
            validate_element_target(target, "set target")?;
            if properties.is_empty() && replacement.is_none() {
                return Err("Presentation set requires properties or replaceText.".to_string());
            }
            validate_properties(properties, Some(target))?;
            if replacement.is_some() && !properties.is_empty() {
                return Err(
                    "Presentation replaceText cannot be combined with generic properties."
                        .to_string(),
                );
            }
            if let Some(replacement) = replacement {
                if !target.contains("/shape[@id=")
                    || replacement.find.is_empty()
                    || replacement.find.len() > MAX_PRESENTATION_EDITOR_STRING_CHARS
                    || replacement.replace.len() > MAX_PRESENTATION_EDITOR_STRING_CHARS
                    || replacement.find.contains('\0')
                    || replacement.replace.contains('\0')
                {
                    return Err(
                        "Presentation replaceText requires one bounded shape find/replace pair."
                            .to_string(),
                    );
                }
            }
            if *force {
                return Err(
                    "Presentation Editor cannot force protected-file mutations.".to_string()
                );
            }
        }
        OfficeOperationParameters::Add {
            parent,
            element_type,
            copy_from,
            position,
            properties,
            force,
        } => {
            if !matches!(
                element_type.as_str(),
                "textbox" | "shape" | "picture" | "table" | "chart" | "connector"
            ) {
                return Err(format!(
                    "Presentation Editor add elementType `{element_type}` is not supported."
                ));
            }
            validate_exact_slide_target(parent, "add parent")?;
            if let Some(copy_from) = copy_from {
                validate_element_target(copy_from, "add copyFrom")?;
            }
            validate_position(position.as_ref())?;
            validate_properties(properties, None)?;
            if *force {
                return Err(
                    "Presentation Editor cannot force protected-file mutations.".to_string()
                );
            }
        }
        OfficeOperationParameters::Remove {
            target,
            shift,
            properties,
        } => {
            validate_element_target(target, "remove target")?;
            validate_properties(properties, Some(target))?;
            if shift.is_some() {
                return Err(
                    "Presentation Editor does not support spreadsheet cell shift.".to_string(),
                );
            }
        }
        OfficeOperationParameters::Move {
            target,
            new_parent,
            position,
            properties,
        } => {
            validate_element_target(target, "move target")?;
            if let Some(parent) = new_parent {
                validate_exact_slide_target(parent, "move newParent")?;
            }
            validate_position(position.as_ref())?;
            validate_properties(properties, Some(target))?;
        }
        OfficeOperationParameters::Swap {
            first_target,
            second_target,
        } => {
            validate_element_target(first_target, "swap firstTarget")?;
            validate_element_target(second_target, "swap secondTarget")?;
        }
        OfficeOperationParameters::Help { .. }
        | OfficeOperationParameters::Create { .. }
        | OfficeOperationParameters::View { .. }
        | OfficeOperationParameters::Get { .. }
        | OfficeOperationParameters::Query { .. }
        | OfficeOperationParameters::Validate => {
            return Err("Presentation Editor plan contains a non-mutation operation.".to_string())
        }
    }
    Ok(())
}

fn validate_properties(properties: &OfficePropertyMap, target: Option<&str>) -> Result<(), String> {
    if properties.len() > 64 {
        return Err("Presentation Editor properties exceed the 64-property limit.".to_string());
    }
    let mut chart_categories = None;
    let mut chart_series = Vec::new();
    for (name, value) in properties {
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(format!(
                "Presentation Editor property `{name}` has an invalid name."
            ));
        }
        let normalized = name.to_ascii_lowercase();
        let file_property = matches!(
            normalized.as_str(),
            "background"
                | "csv"
                | "fallback"
                | "file"
                | "image"
                | "imagefill"
                | "imagepath"
                | "path"
                | "poster"
                | "preview"
                | "src"
                | "template"
        );
        if file_property {
            if normalized != "src" || target.is_none_or(|target| !target.contains("/picture[@id="))
            {
                return Err(format!(
                    "Presentation Editor property `{name}` cannot read an undeclared file resource."
                ));
            }
            let Value::Object(resource) = value else {
                return Err(
                    "Presentation Editor picture src must be a declared input resource."
                        .to_string(),
                );
            };
            let Some(path) = (resource.len() == 1)
                .then(|| resource.get("resourcePath"))
                .flatten()
                .and_then(Value::as_str)
            else {
                return Err(
                    "Presentation Editor picture src must contain only resourcePath.".to_string(),
                );
            };
            if !path.starts_with(crate::office::OFFICE_AGENT_INPUT_PLACEHOLDER_PREFIX)
                || path.len() == crate::office::OFFICE_AGENT_INPUT_PLACEHOLDER_PREFIX.len()
            {
                return Err(
                    "Presentation Editor picture src must reference one declared input mount."
                        .to_string(),
                );
            }
            continue;
        }
        match value {
            Value::String(value)
                if value.len() <= MAX_PRESENTATION_EDITOR_STRING_CHARS
                    && !value.contains('\0')
                    && !value.contains('\r') => {}
            Value::Bool(_) | Value::Number(_) => {}
            _ => {
                return Err(format!(
                    "Presentation Editor property `{name}` must be one bounded scalar value."
                ))
            }
        }
        if normalized == "categories" {
            chart_categories = value.as_str();
        } else if let Some(index) = normalized.strip_prefix("series") {
            let index = index
                .parse::<usize>()
                .ok()
                .filter(|index| *index > 0 && *index <= 32)
                .ok_or_else(|| {
                    "Presentation Editor chart series keys must be series1 through series32."
                        .to_string()
                })?;
            chart_series.push((
                index,
                value.as_str().ok_or_else(|| {
                    "Presentation Editor chart series values must be strings.".to_string()
                })?,
            ));
        }
    }
    if chart_categories.is_some() || !chart_series.is_empty() {
        let target = target.ok_or_else(|| {
            "Presentation Editor chart properties require an inspected chart target.".to_string()
        })?;
        if !target.contains("/chart[@id=") {
            return Err(
                "Presentation Editor chart properties require an inspected chart target."
                    .to_string(),
            );
        }
        validate_chart_properties(chart_categories, &chart_series)?;
    }
    Ok(())
}

fn validate_chart_properties(
    categories: Option<&str>,
    series: &[(usize, &str)],
) -> Result<(), String> {
    let categories = categories.ok_or_else(|| {
        "Presentation Editor chart updates require categories and series1.".to_string()
    })?;
    let category_count = categories.split(',').count();
    if categories.is_empty()
        || categories.split(',').any(str::is_empty)
        || category_count > 256
        || series.is_empty()
    {
        return Err("Presentation Editor chart categories/series are invalid.".to_string());
    }
    let mut ordered = series.to_vec();
    ordered.sort_by_key(|(index, _)| *index);
    for (expected, (index, encoded)) in (1usize..).zip(ordered) {
        if index != expected {
            return Err("Presentation Editor chart series keys must be contiguous.".to_string());
        }
        let Some((name, values)) = encoded.split_once(':') else {
            return Err(
                "Presentation Editor chart series must use Name:v1,v2 encoding.".to_string(),
            );
        };
        if name.is_empty() || name.contains(',') {
            return Err("Presentation Editor chart series name is invalid.".to_string());
        }
        let values = values.split(',').collect::<Vec<_>>();
        if values.len() != category_count
            || values.iter().any(|value| {
                value
                    .parse::<f64>()
                    .ok()
                    .is_none_or(|value| !value.is_finite())
            })
        {
            return Err(
                "Presentation Editor chart series values must be finite and match categories."
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn validate_position(position: Option<&OfficeElementPosition>) -> Result<(), String> {
    match position {
        None | Some(OfficeElementPosition::Index { .. }) => Ok(()),
        Some(OfficeElementPosition::After { target })
        | Some(OfficeElementPosition::Before { target }) => {
            validate_element_target(target, "position target")
        }
    }
}

fn validate_element_target(value: &str, label: &str) -> Result<(), String> {
    validate_stable_target(value, label, false)
}

fn validate_exact_slide_target(value: &str, label: &str) -> Result<(), String> {
    let result = validate_stable_target(value, label, true);
    if result.is_ok()
        && value.strip_prefix("/slide[").is_some_and(|rest| {
            rest.strip_suffix(']')
                .and_then(|slide| slide.parse::<u32>().ok())
                .is_some_and(|slide| slide > 0)
        })
    {
        Ok(())
    } else {
        Err(format!(
            "Presentation Editor {label} must be an exact inspected /slide[N] target."
        ))
    }
}

fn validate_stable_target(value: &str, label: &str, allow_slide: bool) -> Result<(), String> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().count() > MAX_PRESENTATION_EDITOR_STRING_CHARS
        || value.contains('\0')
        || value.contains('\n')
        || value.contains('\r')
    {
        return Err(format!("Presentation Editor {label} is empty or invalid."));
    }
    let Some(rest) = value.strip_prefix("/slide[") else {
        return Err(format!(
            "Presentation Editor {label} must begin with a one-based /slide[N] anchor."
        ));
    };
    let Some((slide, suffix)) = rest.split_once(']') else {
        return Err(format!(
            "Presentation Editor {label} has an invalid slide anchor."
        ));
    };
    if slide
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .is_none()
    {
        return Err(format!(
            "Presentation Editor {label} has an invalid slide number."
        ));
    }
    if suffix.is_empty() {
        return if allow_slide {
            Ok(())
        } else {
            Err(format!(
                "Presentation Editor {label} must identify an element."
            ))
        };
    }
    let Some(element_and_tail) = suffix.strip_prefix('/') else {
        return Err(format!(
            "Presentation Editor {label} must use an inspected stable element id (`[@id=…]`), not a guessed array index."
        ));
    };
    let (element, tail) = element_and_tail
        .split_once('/')
        .map_or((element_and_tail, None), |(element, tail)| {
            (element, Some(tail))
        });
    let Some((kind, id_tail)) = element.split_once("[@id=") else {
        return Err(format!(
            "Presentation Editor {label} has no stable element id."
        ));
    };
    if !matches!(kind, "shape" | "picture" | "table" | "chart" | "connector") {
        return Err(format!(
            "Presentation Editor {label} uses an unsupported element kind."
        ));
    }
    let Some(id) = id_tail.strip_suffix(']') else {
        return Err(format!(
            "Presentation Editor {label} has an invalid stable element id."
        ));
    };
    if id.parse::<u32>().ok().filter(|value| *value > 0).is_none() {
        return Err(format!(
            "Presentation Editor {label} has an invalid stable element id."
        ));
    }
    match tail {
        None => Ok(()),
        Some(tail) if kind == "table" => validate_table_cell_tail(tail, label),
        Some(_) => Err(format!(
            "Presentation Editor {label} contains an unsupported path suffix."
        )),
    }
}

fn validate_table_cell_tail(value: &str, label: &str) -> Result<(), String> {
    let Some(rest) = value.strip_prefix("row[") else {
        return Err(format!(
            "Presentation Editor {label} contains an unsupported table path."
        ));
    };
    let Some((row, rest)) = rest.split_once("]/cell[") else {
        return Err(format!(
            "Presentation Editor {label} contains an unsupported table path."
        ));
    };
    let Some(column) = rest.strip_suffix(']') else {
        return Err(format!(
            "Presentation Editor {label} contains an unsupported table path."
        ));
    };
    if [row, column]
        .into_iter()
        .all(|value| value.parse::<u32>().ok().is_some_and(|value| value > 0))
    {
        Ok(())
    } else {
        Err(format!(
            "Presentation Editor {label} contains an invalid table cell index."
        ))
    }
}

fn validate_safe_relative_path(value: &str, label: &str, extension: &str) -> Result<(), String> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().count() > 1024
        || value.contains('\0')
        || value.contains('\n')
        || value.contains('\r')
        || Path::new(value).is_absolute()
    {
        return Err(format!(
            "Presentation Editor {label} must be a safe relative path."
        ));
    }
    let mut normalized = PathBuf::new();
    for component in Path::new(value).components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "Presentation Editor {label} cannot contain parent or root components."
                ))
            }
        }
    }
    if normalized.as_os_str().is_empty()
        || normalized
            .extension()
            .and_then(|value| value.to_str())
            .is_none_or(|value| !value.eq_ignore_ascii_case(extension))
    {
        return Err(format!(
            "Presentation Editor {label} must end in .{extension}."
        ));
    }
    Ok(())
}

fn validate_output_path(value: &str) -> Result<(), String> {
    validate_safe_relative_path(value, "output", "pptx")
}
