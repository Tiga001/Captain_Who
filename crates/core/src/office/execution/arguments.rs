use super::*;

/// Compiles one provider-neutral Office request into the only OfficeCLI argv accepted by the
/// managed adapter.
///
/// This is a trusted, deterministic boundary: model input is represented by typed fields, maps
/// are sorted, structured output is always enabled, and provider flags are never accepted from
/// the caller. The frozen argv is rebuilt through this function immediately before execution.
pub(crate) fn compile_office_arguments(
    request: &OfficeExecutionRequest,
) -> Result<Vec<String>, OfficeEngineError> {
    let parameters = &request.parameters;
    if parameters.operation() != request.operation {
        return Err(invalid_request(
            "Office operation does not match its typed parameter variant.",
        ));
    }

    let mut arguments = Vec::new();
    match parameters {
        OfficeOperationParameters::Help { verb, element } => {
            arguments.push(document_format_name(request.document_kind).to_string());
            if let Some(verb) = verb {
                let provider_verb = verb.provider_element_cli_name().ok_or_else(|| {
                    invalid_request(format!(
                        "Office `{}` operation help is Host-managed and cannot be forwarded to OfficeCLI.",
                        verb.stable_name()
                    ))
                })?;
                arguments.push(provider_verb.to_string());
            }
            if let Some(element) = optional_non_empty("help element", element.as_deref())? {
                if verb.is_none() {
                    return Err(invalid_request("Office help element requires a help verb."));
                }
                arguments.push(element.to_string());
            }
        }
        OfficeOperationParameters::Create {
            locale,
            minimal,
            overwrite,
        } => {
            if (*minimal || locale.is_some())
                && request.document_kind != OfficeDocumentKind::Document
            {
                return Err(invalid_request(
                    "Office create locale and minimal mode are supported only for documents.",
                ));
            }
            if let Some(locale) = optional_non_empty("locale", locale.as_deref())? {
                push_option(&mut arguments, "--locale", locale);
            }
            if *minimal {
                arguments.push("--minimal".to_string());
            }
            if *overwrite {
                arguments.push("--force".to_string());
            }
        }
        OfficeOperationParameters::View {
            mode,
            start,
            end,
            max_lines,
            issue_type,
            limit,
            columns,
            pages,
            range,
            viewport,
            grid,
            render_mode,
            page_count,
        } => {
            arguments.push(mode.cli_name().to_string());
            if let (Some(start), Some(end)) = (start, end) {
                if start > end {
                    return Err(invalid_request(
                        "Office view start cannot be greater than end.",
                    ));
                }
            }
            push_positive_u32_option(&mut arguments, "view start", "--start", *start)?;
            push_positive_u32_option(&mut arguments, "view end", "--end", *end)?;
            push_positive_u32_option(&mut arguments, "view maxLines", "--max-lines", *max_lines)?;
            if let Some(issue_type) = optional_non_empty("view issueType", issue_type.as_deref())? {
                if *mode != OfficeViewMode::Issues {
                    return Err(invalid_request(
                        "Office view issueType is valid only in issues mode.",
                    ));
                }
                validate_issue_type(request.document_kind, issue_type)?;
                push_option(&mut arguments, "--type", issue_type);
            }
            push_positive_u32_option(&mut arguments, "view limit", "--limit", *limit)?;
            if !columns.is_empty() {
                if request.document_kind != OfficeDocumentKind::Spreadsheet {
                    return Err(invalid_request(
                        "Office view columns are supported only for spreadsheets.",
                    ));
                }
                push_option(
                    &mut arguments,
                    "--cols",
                    &canonical_list("view columns", columns)?,
                );
            }
            if !pages.is_empty() {
                push_option(&mut arguments, "--page", &canonical_page_ranges(pages)?);
            }
            if let Some(range) = optional_non_empty("view range", range.as_deref())? {
                push_option(&mut arguments, "--range", range);
            }
            if let Some(viewport) = viewport {
                if *mode != OfficeViewMode::Screenshot {
                    return Err(invalid_request(
                        "Office viewport is valid only in screenshot mode.",
                    ));
                }
                if viewport.width == 0
                    || viewport.height == 0
                    || viewport.width > MAX_OFFICE_SCREENSHOT_DIMENSION
                    || viewport.height > MAX_OFFICE_SCREENSHOT_DIMENSION
                {
                    return Err(invalid_request(format!(
                        "Office viewport dimensions must be between 1 and {MAX_OFFICE_SCREENSHOT_DIMENSION}."
                    )));
                }
                push_option(
                    &mut arguments,
                    "--screenshot-width",
                    &viewport.width.to_string(),
                );
                push_option(
                    &mut arguments,
                    "--screenshot-height",
                    &viewport.height.to_string(),
                );
            }
            if let Some(grid) = grid {
                if request.document_kind == OfficeDocumentKind::Spreadsheet {
                    return Err(invalid_request(
                        "Office screenshot grid is supported only for documents and presentations.",
                    ));
                }
                if *mode != OfficeViewMode::Screenshot {
                    return Err(invalid_request(
                        "Office grid is valid only in screenshot mode.",
                    ));
                }
                match grid {
                    OfficeGridLayout::Auto => push_option(&mut arguments, "--grid", "auto"),
                    OfficeGridLayout::Columns { columns } => {
                        if *columns == 0 || *columns > MAX_OFFICE_GRID_COLUMNS {
                            return Err(invalid_request(format!(
                                "Office grid columns must be between 1 and {MAX_OFFICE_GRID_COLUMNS}."
                            )));
                        }
                        push_option(&mut arguments, "--grid", &columns.to_string());
                    }
                }
            }
            if let Some(render_mode) = render_mode {
                if request.document_kind == OfficeDocumentKind::Spreadsheet {
                    return Err(invalid_request(
                        "Office renderMode is supported only for documents and presentations.",
                    ));
                }
                if *mode != OfficeViewMode::Screenshot {
                    return Err(invalid_request(
                        "Office renderMode is valid only in screenshot mode.",
                    ));
                }
                push_option(&mut arguments, "--render", render_mode.cli_name());
            }
            if *page_count {
                if request.document_kind != OfficeDocumentKind::Document
                    || *mode != OfficeViewMode::Stats
                {
                    return Err(invalid_request(
                        "Office pageCount is valid only for document stats view.",
                    ));
                }
                arguments.push("--page-count".to_string());
            }
        }
        OfficeOperationParameters::Get { target, depth } => {
            if let Some(target) = optional_non_empty("get target", target.as_deref())? {
                arguments.push(target.to_string());
            }
            push_u32_option(&mut arguments, "--depth", *depth);
        }
        OfficeOperationParameters::Query {
            selector,
            contains,
            compact,
            fields,
        } => {
            if request.document_kind == OfficeDocumentKind::Spreadsheet
                && (*compact || !fields.is_empty())
            {
                return Err(invalid_request(
                    "Office spreadsheet query does not support compact or fields; use a typed view range or columns instead.",
                ));
            }
            let selector = required_non_empty("query selector", selector)?;
            if selector.starts_with('/') {
                return Err(invalid_request(
                    "Office query selector must be an element or CSS-like selector such as `slide` or `shape[text=Hello]`; use get for paths beginning with `/`.",
                ));
            }
            arguments.push(selector.to_string());
            if let Some(contains) = optional_non_empty("query contains", contains.as_deref())? {
                push_option(&mut arguments, "--find", contains);
            }
            if *compact {
                arguments.push("--compact".to_string());
            }
            if !fields.is_empty() {
                push_option(
                    &mut arguments,
                    "--fields",
                    &canonical_list("query fields", fields)?,
                );
            }
        }
        OfficeOperationParameters::Validate => {}
        OfficeOperationParameters::Set {
            target,
            properties,
            replacement,
            force,
        } => {
            arguments.push(required_non_empty("set target", target)?.to_string());
            if properties.is_empty() && replacement.is_none() {
                return Err(invalid_request(
                    "Office set requires at least one property or a text replacement.",
                ));
            }
            push_properties(&mut arguments, properties)?;
            if let Some(replacement) = replacement {
                push_option(
                    &mut arguments,
                    "--find",
                    required_non_empty("replacement find", &replacement.find)?,
                );
                push_option(&mut arguments, "--replace", &replacement.replace);
            }
            if *force {
                arguments.push("--force".to_string());
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
            arguments.push(required_non_empty("add parent", parent)?.to_string());
            let element_type = required_non_empty("add elementType", element_type)?;
            push_option(&mut arguments, "--type", element_type);
            if let Some(copy_from) = optional_non_empty("add copyFrom", copy_from.as_deref())? {
                push_option(&mut arguments, "--from", copy_from);
            }
            push_position(&mut arguments, position.as_ref())?;
            let mut properties = properties.clone();
            if matches!(
                element_type.trim().to_ascii_lowercase().as_str(),
                "diagram" | "flowchart" | "mermaid"
            ) {
                match properties.get("render") {
                    Some(value) if value.as_str() != Some("native") => {
                        return Err(invalid_request(
                            "Office diagrams use the built-in native renderer; omit render or set it to `native`.",
                        ))
                    }
                    None => {
                        properties.insert("render".to_string(), Value::String("native".to_string()));
                    }
                    _ => {}
                }
            }
            push_properties(&mut arguments, &properties)?;
            if *force {
                arguments.push("--force".to_string());
            }
        }
        OfficeOperationParameters::Remove {
            target,
            shift,
            properties,
        } => {
            arguments.push(required_non_empty("remove target", target)?.to_string());
            if let Some(shift) = shift {
                if request.document_kind != OfficeDocumentKind::Spreadsheet {
                    return Err(invalid_request(
                        "Office remove shift is supported only for spreadsheet cells.",
                    ));
                }
                push_option(&mut arguments, "--shift", shift.cli_name());
            }
            push_properties(&mut arguments, properties)?;
        }
        OfficeOperationParameters::Move {
            target,
            new_parent,
            position,
            properties,
        } => {
            arguments.push(required_non_empty("move target", target)?.to_string());
            if let Some(new_parent) = optional_non_empty("move newParent", new_parent.as_deref())? {
                push_option(&mut arguments, "--to", new_parent);
            }
            push_position(&mut arguments, position.as_ref())?;
            push_properties(&mut arguments, properties)?;
        }
        OfficeOperationParameters::Swap {
            first_target,
            second_target,
        } => {
            let first = required_non_empty("swap firstTarget", first_target)?;
            let second = required_non_empty("swap secondTarget", second_target)?;
            if first == second {
                return Err(invalid_request(
                    "Office swap targets must identify two different elements.",
                ));
            }
            arguments.push(first.to_string());
            arguments.push(second.to_string());
        }
    }
    arguments.push("--json".to_string());
    validate_arguments(&arguments)?;
    Ok(arguments)
}

fn document_format_name(kind: OfficeDocumentKind) -> &'static str {
    match kind {
        OfficeDocumentKind::Document => "docx",
        OfficeDocumentKind::Spreadsheet => "xlsx",
        OfficeDocumentKind::Presentation => "pptx",
    }
}

fn validate_issue_type(
    document_kind: OfficeDocumentKind,
    issue_type: &str,
) -> Result<(), OfficeEngineError> {
    let supported = match document_kind {
        OfficeDocumentKind::Document => matches!(
            issue_type,
            "format" | "content" | "structure" | "field_not_evaluated" | "field_cache_stale"
        ),
        OfficeDocumentKind::Spreadsheet => matches!(
            issue_type,
            "format"
                | "content"
                | "structure"
                | "formula_not_evaluated"
                | "formula_cache_stale"
                | "formula_ref_missing_sheet"
                | "formula_eval_error"
                | "chart_series_ref_missing_sheet"
                | "chart_cache_stale"
                | "definedname_broken"
                | "definedname_target_missing"
        ),
        OfficeDocumentKind::Presentation => matches!(
            issue_type,
            "format"
                | "content"
                | "structure"
                | "slide_field_not_evaluated"
                | "notes_unresolved_rid"
                | "broken_part_ref"
                | "low_contrast"
        ),
    };
    if supported {
        Ok(())
    } else {
        Err(invalid_request(format!(
            "Office issueType `{issue_type}` is not supported for {} files.",
            document_format_name(document_kind)
        )))
    }
}

fn required_non_empty<'a>(name: &str, value: &'a str) -> Result<&'a str, OfficeEngineError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(invalid_request(format!("Office {name} cannot be empty.")));
    }
    if trimmed != value {
        return Err(invalid_request(format!(
            "Office {name} cannot contain leading or trailing whitespace."
        )));
    }
    Ok(value)
}

fn optional_non_empty<'a>(
    name: &str,
    value: Option<&'a str>,
) -> Result<Option<&'a str>, OfficeEngineError> {
    value
        .map(|value| required_non_empty(name, value))
        .transpose()
}

fn push_option(arguments: &mut Vec<String>, name: &str, value: &str) {
    arguments.push(name.to_string());
    arguments.push(value.to_string());
}

fn push_u32_option(arguments: &mut Vec<String>, name: &str, value: Option<u32>) {
    if let Some(value) = value {
        push_option(arguments, name, &value.to_string());
    }
}

fn push_positive_u32_option(
    arguments: &mut Vec<String>,
    field_name: &str,
    option_name: &str,
    value: Option<u32>,
) -> Result<(), OfficeEngineError> {
    if let Some(value) = value {
        if value == 0 {
            return Err(invalid_request(format!(
                "Office {field_name} must be greater than zero."
            )));
        }
        push_option(arguments, option_name, &value.to_string());
    }
    Ok(())
}

fn canonical_list(name: &str, values: &[String]) -> Result<String, OfficeEngineError> {
    if values.len() > MAX_OFFICE_LIST_VALUES {
        return Err(invalid_request(format!(
            "Office {name} exceeds the {MAX_OFFICE_LIST_VALUES}-item limit."
        )));
    }
    values
        .iter()
        .map(|value| required_non_empty(name, value))
        .collect::<Result<Vec<_>, _>>()
        .map(|values| values.join(","))
}

pub(super) fn canonical_page_ranges(
    ranges: &[crate::office::types::OfficePageRange],
) -> Result<String, OfficeEngineError> {
    if ranges.len() > MAX_OFFICE_LIST_VALUES {
        return Err(invalid_request(format!(
            "Office page ranges exceed the {MAX_OFFICE_LIST_VALUES}-item limit."
        )));
    }
    let mut total_pages = 0_u32;
    let mut canonical = Vec::with_capacity(ranges.len());
    for range in ranges {
        let end = range.end.unwrap_or(range.start);
        if range.start == 0 || end < range.start {
            return Err(invalid_request(
                "Office page ranges are one-based and each end must be greater than or equal to its start.",
            ));
        }
        if range.start > MAX_OFFICE_PAGE_NUMBER || end > MAX_OFFICE_PAGE_NUMBER {
            return Err(invalid_request(format!(
                "Office page ranges cannot reference a page greater than {MAX_OFFICE_PAGE_NUMBER}."
            )));
        }
        let page_count = end
            .checked_sub(range.start)
            .and_then(|span| span.checked_add(1))
            .ok_or_else(|| invalid_request("Office page range size overflowed."))?;
        total_pages = total_pages
            .checked_add(page_count)
            .ok_or_else(|| invalid_request("Office page range total overflowed."))?;
        if total_pages > MAX_OFFICE_TOTAL_PAGES {
            return Err(invalid_request(format!(
                "Office page ranges cannot contain more than {MAX_OFFICE_TOTAL_PAGES} pages in total."
            )));
        }
        canonical.push(if end == range.start {
            range.start.to_string()
        } else {
            format!("{}-{end}", range.start)
        });
    }
    Ok(canonical.join(","))
}

fn push_position(
    arguments: &mut Vec<String>,
    position: Option<&OfficeElementPosition>,
) -> Result<(), OfficeEngineError> {
    match position {
        None => Ok(()),
        Some(OfficeElementPosition::Index { index }) => {
            push_option(arguments, "--index", &index.to_string());
            Ok(())
        }
        Some(OfficeElementPosition::After { target }) => {
            push_option(
                arguments,
                "--after",
                required_non_empty("position target", target)?,
            );
            Ok(())
        }
        Some(OfficeElementPosition::Before { target }) => {
            push_option(
                arguments,
                "--before",
                required_non_empty("position target", target)?,
            );
            Ok(())
        }
    }
}

fn push_properties(
    arguments: &mut Vec<String>,
    properties: &OfficePropertyMap,
) -> Result<(), OfficeEngineError> {
    if properties.len() > MAX_OFFICE_PROPERTIES {
        return Err(invalid_request(format!(
            "Office properties exceed the {MAX_OFFICE_PROPERTIES}-property limit."
        )));
    }
    for (name, value) in properties {
        let name = required_non_empty("property name", name)?;
        if name.contains('=') || name.starts_with('-') {
            return Err(invalid_request(
                "Office property names cannot contain `=` or begin with `-`.",
            ));
        }
        let value = match value {
            Value::String(value) => value.clone(),
            Value::Bool(value) => value.to_string(),
            Value::Number(value) => value.to_string(),
            Value::Object(resource) if resource.len() == 1 => {
                if !property_accepts_resource_object(name) {
                    return Err(invalid_request(format!(
                        "Office property `{name}` does not declare a managed file-resource surface. Use resourcePath only with a documented file-bearing property."
                    )));
                }
                let path = resource
                    .get("resourcePath")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        invalid_request(format!(
                            "Office property `{name}` resource objects must contain only a string resourcePath."
                        ))
                    })?;
                let path = required_non_empty("property resourcePath", path)?;
                if name.eq_ignore_ascii_case("background") {
                    format!("image:{path}")
                } else {
                    path.to_string()
                }
            }
            Value::Null | Value::Array(_) | Value::Object(_) => {
                return Err(invalid_request(format!(
                    "Office property `{name}` must be a string, number, boolean, or a single resourcePath object."
                )))
            }
        };
        arguments.push("--prop".to_string());
        arguments.push(format!("{name}={value}"));
    }
    Ok(())
}

fn property_accepts_resource_object(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
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
    )
}
