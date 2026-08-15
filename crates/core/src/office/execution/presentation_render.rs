use super::{
    invalid_request, MAX_OFFICE_GRID_COLUMNS, MAX_OFFICE_PAGE_NUMBER, MAX_OFFICE_TOTAL_PAGES,
};
use crate::office::{
    OfficeDocumentKind, OfficeEngineError, OfficeExecutionRequest, OfficeGridLayout,
    OfficeOperation, OfficeOperationParameters, OfficePresentationRenderPlan, OfficeViewMode,
    OfficeViewport,
};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

const PRESENTATION_XML_PATH: &str = "ppt/presentation.xml";
const PRESENTATION_RELS_PATH: &str = "ppt/_rels/presentation.xml.rels";
const PRESENTATIONML_TRANSITIONAL_NAMESPACE: &str =
    "http://schemas.openxmlformats.org/presentationml/2006/main";
const PRESENTATIONML_STRICT_NAMESPACE: &str = "http://purl.oclc.org/ooxml/presentationml/main";
const OFFICE_RELATIONSHIPS_TRANSITIONAL_NAMESPACE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const OFFICE_RELATIONSHIPS_STRICT_NAMESPACE: &str =
    "http://purl.oclc.org/ooxml/officeDocument/relationships";
const PACKAGE_RELATIONSHIPS_NAMESPACE: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships";
const SLIDE_RELATIONSHIP_TRANSITIONAL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide";
const SLIDE_RELATIONSHIP_STRICT_TYPE: &str =
    "http://purl.oclc.org/ooxml/officeDocument/relationships/slide";
const MAX_PRESENTATION_XML_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PRESENTATION_ZIP_ENTRIES: usize = 100_000;
const MIN_SLIDE_NUMERIC_ID: u32 = 256;
const MAX_SLIDE_NUMERIC_ID: u32 = 2_147_483_647;
const DEFAULT_VIEWPORT_WIDTH: u32 = 1600;
const DEFAULT_VIEWPORT_HEIGHT: u32 = 1200;
const PROVIDER_MAX_VIEWPORT_DIMENSION: u32 = 1920;
const GRID_PADDING_PX: f64 = 12.0;
const GRID_GAP_PX: f64 = 12.0;
const GRID_HEIGHT_SAFETY_PX: u32 = 1;

#[derive(Clone, Copy)]
struct PresentationDialect {
    presentation_namespace: &'static str,
    office_relationships_namespace: &'static str,
    slide_relationship_type: &'static str,
}

const TRANSITIONAL_DIALECT: PresentationDialect = PresentationDialect {
    presentation_namespace: PRESENTATIONML_TRANSITIONAL_NAMESPACE,
    office_relationships_namespace: OFFICE_RELATIONSHIPS_TRANSITIONAL_NAMESPACE,
    slide_relationship_type: SLIDE_RELATIONSHIP_TRANSITIONAL_TYPE,
};

const STRICT_DIALECT: PresentationDialect = PresentationDialect {
    presentation_namespace: PRESENTATIONML_STRICT_NAMESPACE,
    office_relationships_namespace: OFFICE_RELATIONSHIPS_STRICT_NAMESPACE,
    slide_relationship_type: SLIDE_RELATIONSHIP_STRICT_TYPE,
};

pub(super) fn resolve_presentation_render_plan(
    request: &OfficeExecutionRequest,
    frozen_document_path: &Path,
) -> Result<Option<OfficePresentationRenderPlan>, OfficeEngineError> {
    let OfficeOperationParameters::View {
        mode,
        start,
        end,
        pages,
        range,
        ..
    } = request.typed_parameters()
    else {
        return Ok(None);
    };
    if request.document_kind != OfficeDocumentKind::Presentation
        || request.operation != OfficeOperation::View
        || *mode != OfficeViewMode::Screenshot
        || range.is_some()
    {
        return Ok(None);
    }

    let (slide_count, slide_width_emu, slide_height_emu) =
        read_presentation_geometry(frozen_document_path)?;
    let requested_pages = requested_pages(slide_count, pages, *start, *end)?;
    let requested_viewport = match request.typed_parameters() {
        OfficeOperationParameters::View { viewport, .. } => viewport.clone(),
        _ => None,
    };
    let (max_width, max_height) = cap_dimensions(
        requested_viewport
            .as_ref()
            .map_or(DEFAULT_VIEWPORT_WIDTH, |viewport| viewport.width),
        requested_viewport
            .as_ref()
            .map_or(DEFAULT_VIEWPORT_HEIGHT, |viewport| viewport.height),
    )?;

    let (viewport, grid) = if requested_pages.len() == 1 {
        (
            single_slide_viewport(max_width, max_height, slide_width_emu, slide_height_emu)?,
            None,
        )
    } else {
        let (columns, viewport_height) = choose_grid(
            requested_pages.len(),
            max_width,
            max_height,
            slide_width_emu,
            slide_height_emu,
        )?;
        (
            OfficeViewport {
                width: max_width,
                height: viewport_height,
            },
            Some(OfficeGridLayout::Columns { columns }),
        )
    };

    Ok(Some(OfficePresentationRenderPlan {
        requested_pages,
        slide_width_emu,
        slide_height_emu,
        viewport,
        grid,
    }))
}

pub(super) fn apply_presentation_render_plan(
    request: &OfficeExecutionRequest,
    plan: Option<&OfficePresentationRenderPlan>,
) -> OfficeExecutionRequest {
    let Some(plan) = plan else {
        return request.clone();
    };
    let mut resolved = request.clone();
    if let OfficeOperationParameters::View {
        viewport,
        grid,
        render_mode,
        ..
    } = &mut resolved.parameters
    {
        *viewport = Some(plan.viewport.clone());
        *grid = plan.grid;
        // A fixed browser viewport is the layout boundary. Native/registry PNG
        // backends may return tight content bounds that cannot be attested against it.
        *render_mode = Some(crate::office::OfficeViewRenderMode::Html);
    }
    resolved
}

fn read_presentation_geometry(path: &Path) -> Result<(u32, u32, u32), OfficeEngineError> {
    let file = fs::File::open(path)
        .map_err(|error| invalid_request(format!("Cannot inspect the presentation: {error}")))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| {
        invalid_request(format!(
            "The presentation is not a valid OOXML package: {error}"
        ))
    })?;
    validate_unique_zip_entry_names(path, &mut archive)?;
    let xml = read_bounded_xml(&mut archive, PRESENTATION_XML_PATH)?;
    let document = roxmltree::Document::parse(&xml).map_err(|error| {
        invalid_request(format!("Presentation metadata is invalid XML: {error}"))
    })?;
    let root = document.root_element();
    let dialect = presentation_dialect(root)?;
    let slide_lists = document
        .descendants()
        .filter(|node| has_expanded_name(*node, dialect.presentation_namespace, "sldIdLst"))
        .collect::<Vec<_>>();
    let [slide_list] = slide_lists.as_slice() else {
        return Err(invalid_request(
            "The presentation must contain exactly one namespaced slide list.",
        ));
    };
    if slide_list.parent() != Some(root) {
        return Err(invalid_request(
            "The presentation slide list must be a direct child of the presentation root.",
        ));
    }
    let mut slide_numeric_ids = std::collections::BTreeSet::new();
    let mut slide_relationship_ids = Vec::new();
    for slide in slide_list.children().filter(|node| node.is_element()) {
        if !has_expanded_name(slide, dialect.presentation_namespace, "sldId") {
            return Err(invalid_request(
                "The presentation slide list contains an invalid child element.",
            ));
        }
        let numeric_ids = slide
            .attributes()
            .filter(|attribute| attribute.name() == "id" && attribute.namespace().is_none())
            .collect::<Vec<_>>();
        let [numeric_id] = numeric_ids.as_slice() else {
            return Err(invalid_request(
                "Each presentation slide must have exactly one unnamespaced numeric identity.",
            ));
        };
        let numeric_id = numeric_id
            .value()
            .parse::<u32>()
            .ok()
            .filter(|identity| {
                (MIN_SLIDE_NUMERIC_ID..=MAX_SLIDE_NUMERIC_ID).contains(identity)
            })
            .ok_or_else(|| {
                invalid_request(format!(
                    "Presentation slide numeric identities must be integers between {MIN_SLIDE_NUMERIC_ID} and {MAX_SLIDE_NUMERIC_ID}."
                ))
            })?;
        if !slide_numeric_ids.insert(numeric_id) {
            return Err(invalid_request(
                "Presentation slide numeric identities must be globally unique.",
            ));
        }

        let relationship_ids = slide
            .attributes()
            .filter(|attribute| {
                attribute.name() == "id"
                    && attribute.namespace() == Some(dialect.office_relationships_namespace)
            })
            .collect::<Vec<_>>();
        let [relationship_id] = relationship_ids.as_slice() else {
            return Err(invalid_request(
                "Each presentation slide must have exactly one relationship identity.",
            ));
        };
        if relationship_id.value().is_empty() {
            return Err(invalid_request(
                "A presentation slide is missing its relationship identity.",
            ));
        }
        slide_relationship_ids.push(relationship_id.value().to_string());
    }
    let slide_count = slide_relationship_ids.len();
    if slide_count == 0 || slide_count > MAX_OFFICE_PAGE_NUMBER as usize {
        return Err(invalid_request(format!(
            "Presentation metadata must declare between 1 and {MAX_OFFICE_PAGE_NUMBER} slides."
        )));
    }
    let sizes = document
        .descendants()
        .filter(|node| has_expanded_name(*node, dialect.presentation_namespace, "sldSz"))
        .collect::<Vec<_>>();
    let [size] = sizes.as_slice() else {
        return Err(invalid_request(
            "The presentation must contain exactly one namespaced slide-size element.",
        ));
    };
    if size.parent() != Some(root) {
        return Err(invalid_request(
            "The presentation slide-size element must be a direct child of the presentation root.",
        ));
    }
    let width = parse_emu(size.attribute("cx"), "width")?;
    let height = parse_emu(size.attribute("cy"), "height")?;

    validate_slide_parts(&mut archive, &slide_relationship_ids, dialect)?;
    Ok((slide_count as u32, width, height))
}

fn presentation_dialect(
    root: roxmltree::Node<'_, '_>,
) -> Result<PresentationDialect, OfficeEngineError> {
    if !root.is_element() || root.tag_name().name() != "presentation" {
        return Err(invalid_request(
            "Presentation metadata has an invalid document root.",
        ));
    }
    match root.tag_name().namespace() {
        Some(PRESENTATIONML_TRANSITIONAL_NAMESPACE) => Ok(TRANSITIONAL_DIALECT),
        Some(PRESENTATIONML_STRICT_NAMESPACE) => Ok(STRICT_DIALECT),
        _ => Err(invalid_request(
            "Presentation metadata uses an unsupported presentation namespace.",
        )),
    }
}

fn has_expanded_name(node: roxmltree::Node<'_, '_>, namespace: &str, local_name: &str) -> bool {
    node.is_element()
        && node.tag_name().name() == local_name
        && node.tag_name().namespace() == Some(namespace)
}

fn validate_unique_zip_entry_names(
    path: &Path,
    archive: &mut zip::ZipArchive<fs::File>,
) -> Result<(), OfficeEngineError> {
    let expected_entries = archive.len();
    if expected_entries == 0 || expected_entries > MAX_PRESENTATION_ZIP_ENTRIES {
        return Err(invalid_request(format!(
            "Presentation ZIP must contain between 1 and {MAX_PRESENTATION_ZIP_ENTRIES} entries."
        )));
    }
    let raw_entries = scan_unique_central_directory_names(path, archive.central_directory_start())?;
    validate_central_directory_entry_count(raw_entries, expected_entries)?;

    let mut decoded_names = Vec::with_capacity(expected_entries);
    for index in 0..expected_entries {
        let entry = archive.by_index(index).map_err(|error| {
            invalid_request(format!("Cannot inspect presentation ZIP entry: {error}"))
        })?;
        decoded_names.push(entry.name().to_string());
    }
    validate_unique_decoded_zip_entry_names(&decoded_names)
}

fn scan_unique_central_directory_names(
    path: &Path,
    central_directory_start: u64,
) -> Result<usize, OfficeEngineError> {
    use std::collections::BTreeSet;

    const CENTRAL_DIRECTORY_HEADER: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
    const CENTRAL_DIRECTORY_FIXED_BYTES: usize = 46;
    let mut file = fs::File::open(path)
        .map_err(|error| invalid_request(format!("Cannot inspect presentation ZIP: {error}")))?;
    file.seek(SeekFrom::Start(central_directory_start))
        .map_err(|error| invalid_request(format!("Cannot seek presentation ZIP: {error}")))?;
    let mut names = BTreeSet::new();
    let mut entry_count = 0_usize;
    loop {
        let position = file.stream_position().map_err(|error| {
            invalid_request(format!("Cannot inspect presentation ZIP: {error}"))
        })?;
        let mut signature = [0_u8; 4];
        file.read_exact(&mut signature).map_err(|error| {
            invalid_request(format!("Cannot read presentation ZIP directory: {error}"))
        })?;
        if signature != CENTRAL_DIRECTORY_HEADER {
            file.seek(SeekFrom::Start(position)).map_err(|error| {
                invalid_request(format!("Cannot seek presentation ZIP: {error}"))
            })?;
            break;
        }
        let mut fixed = [0_u8; CENTRAL_DIRECTORY_FIXED_BYTES - 4];
        file.read_exact(&mut fixed).map_err(|error| {
            invalid_request(format!("Cannot read presentation ZIP directory: {error}"))
        })?;
        let name_length = usize::from(u16::from_le_bytes([fixed[24], fixed[25]]));
        let extra_length = u64::from(u16::from_le_bytes([fixed[26], fixed[27]]));
        let comment_length = u64::from(u16::from_le_bytes([fixed[28], fixed[29]]));
        if name_length == 0 {
            return Err(invalid_request(
                "Presentation ZIP contains an invalid entry name.",
            ));
        }
        let mut name = vec![0_u8; name_length];
        file.read_exact(&mut name).map_err(|error| {
            invalid_request(format!("Cannot read presentation ZIP entry name: {error}"))
        })?;
        if !names.insert(name) {
            return Err(invalid_request(
                "Presentation package contains a duplicate ZIP entry name.",
            ));
        }
        entry_count = entry_count
            .checked_add(1)
            .ok_or_else(|| invalid_request("Presentation ZIP entry count overflowed."))?;
        if entry_count > MAX_PRESENTATION_ZIP_ENTRIES {
            return Err(invalid_request(format!(
                "Presentation ZIP exceeds the {MAX_PRESENTATION_ZIP_ENTRIES}-entry inspection limit."
            )));
        }
        let trailing = extra_length
            .checked_add(comment_length)
            .ok_or_else(|| invalid_request("Presentation ZIP directory length overflowed."))?;
        file.seek(SeekFrom::Current(i64::try_from(trailing).map_err(
            |_| invalid_request("Presentation ZIP directory length overflowed."),
        )?))
        .map_err(|error| {
            invalid_request(format!("Cannot seek presentation ZIP directory: {error}"))
        })?;
    }
    if names.is_empty() {
        return Err(invalid_request(
            "Presentation ZIP contains no central-directory entries.",
        ));
    }
    Ok(entry_count)
}

fn validate_central_directory_entry_count(
    raw_entry_count: usize,
    archive_entry_count: usize,
) -> Result<(), OfficeEngineError> {
    if raw_entry_count != archive_entry_count {
        return Err(invalid_request(
            "Presentation ZIP central-directory entry count does not match the parsed archive.",
        ));
    }
    Ok(())
}

fn validate_unique_decoded_zip_entry_names(names: &[String]) -> Result<(), OfficeEngineError> {
    let mut unique_names = std::collections::BTreeSet::new();
    if names.iter().any(|name| !unique_names.insert(name)) {
        return Err(invalid_request(
            "Presentation package contains ZIP entry names that collide after decoding.",
        ));
    }
    Ok(())
}

fn read_bounded_xml(
    archive: &mut zip::ZipArchive<fs::File>,
    name: &str,
) -> Result<String, OfficeEngineError> {
    let mut occurrences = 0_usize;
    for index in 0..archive.len() {
        if archive
            .by_index(index)
            .is_ok_and(|entry| entry.name() == name)
        {
            occurrences += 1;
        }
    }
    if occurrences != 1 {
        return Err(invalid_request(format!(
            "The presentation package must contain exactly one `{name}` part."
        )));
    }
    let mut entry = archive
        .by_name(name)
        .map_err(|_| invalid_request(format!("The presentation is missing `{name}`.")))?;
    if entry.size() == 0 || entry.size() > MAX_PRESENTATION_XML_BYTES {
        return Err(invalid_request(format!(
            "Presentation metadata `{name}` is empty or exceeds the inspection limit."
        )));
    }
    let mut xml = String::new();
    entry
        .by_ref()
        .take(MAX_PRESENTATION_XML_BYTES + 1)
        .read_to_string(&mut xml)
        .map_err(|error| invalid_request(format!("Cannot read `{name}`: {error}")))?;
    if xml.len() as u64 > MAX_PRESENTATION_XML_BYTES {
        return Err(invalid_request(format!(
            "Presentation metadata `{name}` exceeds the inspection limit."
        )));
    }
    Ok(xml)
}

fn validate_slide_parts(
    archive: &mut zip::ZipArchive<fs::File>,
    slide_relationship_ids: &[String],
    dialect: PresentationDialect,
) -> Result<(), OfficeEngineError> {
    use std::collections::{BTreeMap, BTreeSet};

    let relationships_xml = read_bounded_xml(archive, PRESENTATION_RELS_PATH)?;
    let relationships = roxmltree::Document::parse(&relationships_xml).map_err(|error| {
        invalid_request(format!(
            "Presentation relationships are invalid XML: {error}"
        ))
    })?;
    let relationships_root = relationships.root_element();
    if !has_expanded_name(
        relationships_root,
        PACKAGE_RELATIONSHIPS_NAMESPACE,
        "Relationships",
    ) {
        return Err(invalid_request(
            "Presentation relationships have an invalid document root or namespace.",
        ));
    }
    let mut slide_targets = BTreeMap::new();
    let mut all_relationship_ids = BTreeSet::new();
    for relationship in relationships_root
        .children()
        .filter(|node| node.is_element())
    {
        if !has_expanded_name(
            relationship,
            PACKAGE_RELATIONSHIPS_NAMESPACE,
            "Relationship",
        ) || relationship.children().any(|child| child.is_element())
        {
            return Err(invalid_request(
                "Presentation relationships contain an invalid child element.",
            ));
        }
        let id = relationship
            .attribute("Id")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| invalid_request("A presentation relationship is missing its Id."))?;
        if !all_relationship_ids.insert(id) {
            return Err(invalid_request(
                "Presentation relationship identities must be globally unique.",
            ));
        }
        let relationship_type = relationship
            .attribute("Type")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| invalid_request("A presentation relationship is missing its Type."))?;
        let target = relationship
            .attribute("Target")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| invalid_request("A presentation relationship is missing its Target."))?;
        if relationship_type != dialect.slide_relationship_type {
            continue;
        }
        if relationship
            .attribute("TargetMode")
            .is_some_and(|mode| mode.eq_ignore_ascii_case("External"))
        {
            return Err(invalid_request(
                "Presentation slide relationships cannot target external resources.",
            ));
        }
        let target = normalize_slide_target(target)?;
        slide_targets.insert(id.to_string(), target);
    }

    let mut referenced_parts = BTreeSet::new();
    let mut seen_relationship_ids = BTreeSet::new();
    for relationship_id in slide_relationship_ids {
        if !seen_relationship_ids.insert(relationship_id) {
            return Err(invalid_request(
                "Presentation slides cannot reuse the same relationship identity.",
            ));
        }
        let target = slide_targets.get(relationship_id).ok_or_else(|| {
            invalid_request(format!(
                "Presentation slide relationship `{relationship_id}` is missing or is not an internal slide."
            ))
        })?;
        if !referenced_parts.insert(target.clone()) {
            return Err(invalid_request(
                "Presentation slide relationships must resolve to unique slide parts.",
            ));
        }
        let entry = archive.by_name(target).map_err(|_| {
            invalid_request(format!(
                "Presentation slide relationship `{relationship_id}` targets a missing slide part."
            ))
        })?;
        if entry.size() == 0 {
            return Err(invalid_request(
                "Presentation slide relationships cannot target an empty slide part.",
            ));
        }
    }

    let mut packaged_parts = BTreeSet::new();
    for index in 0..archive.len() {
        let name = archive
            .by_index(index)
            .map_err(|error| invalid_request(format!("Cannot inspect slide parts: {error}")))?
            .name()
            .to_string();
        if name.starts_with("ppt/slides/")
            && name.ends_with(".xml")
            && !name["ppt/slides/".len()..].contains('/')
            && !packaged_parts.insert(name)
        {
            return Err(invalid_request(
                "Presentation package contains a duplicate slide part.",
            ));
        }
    }
    if packaged_parts != referenced_parts {
        return Err(invalid_request(
            "Presentation slide metadata and packaged slide parts do not form one closed set.",
        ));
    }
    Ok(())
}

fn normalize_slide_target(target: &str) -> Result<String, OfficeEngineError> {
    if target.contains(['\\', '?', '#', '%']) {
        return Err(invalid_request(
            "Presentation slide relationship target is not a canonical internal package path.",
        ));
    }
    // OPC relationship targets may be relative to `/ppt/presentation.xml` or package-absolute.
    // OfficeCLI emits the latter. Accept only the exact `/ppt/` package prefix; every other
    // absolute form remains closed, and the component walk below still prevents `..` escape.
    let target = if let Some(package_relative) = target.strip_prefix("/ppt/") {
        package_relative
    } else {
        if Path::new(target).is_absolute() {
            return Err(invalid_request(
                "Presentation slide relationship uses an unsupported package-absolute path.",
            ));
        }
        target
    };
    let mut normalized = PathBuf::from("ppt");
    for component in Path::new(target).components() {
        match component {
            Component::Normal(value) => normalized.push(value),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() || normalized.as_os_str().is_empty() {
                    return Err(invalid_request(
                        "Presentation slide relationship escapes the package root.",
                    ));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(invalid_request(
                    "Presentation slide relationship must use a relative package path.",
                ))
            }
        }
    }
    let normalized = normalized.to_string_lossy().replace('\\', "/");
    if !normalized.starts_with("ppt/slides/") || !normalized.ends_with(".xml") {
        return Err(invalid_request(
            "Presentation slide relationship does not target an internal slide part.",
        ));
    }
    Ok(normalized)
}

fn parse_emu(value: Option<&str>, name: &str) -> Result<u32, OfficeEngineError> {
    value
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid_request(format!("Presentation slide {name} is invalid.")))
}

fn requested_pages(
    slide_count: u32,
    ranges: &[crate::office::OfficePageRange],
    start: Option<u32>,
    end: Option<u32>,
) -> Result<Vec<u32>, OfficeEngineError> {
    let (first, last) = if ranges.is_empty() {
        (start.unwrap_or(1), end.unwrap_or(slide_count))
    } else if ranges.len() == 1 {
        let range = &ranges[0];
        (range.start, range.end.unwrap_or(range.start))
    } else {
        return Err(invalid_request(
            "Presentation screenshots require one continuous page range; render separate ranges independently.",
        ));
    };
    if first == 0 || last < first || last > slide_count {
        return Err(invalid_request(format!(
            "Presentation screenshot range {first}-{last} is outside the 1-{slide_count} slide set."
        )));
    }
    Ok((first..=last).collect())
}

fn cap_dimensions(width: u32, height: u32) -> Result<(u32, u32), OfficeEngineError> {
    if width == 0 || height == 0 {
        return Err(invalid_request(
            "Presentation screenshot viewport dimensions must be positive.",
        ));
    }
    let maximum = width.max(height);
    if maximum <= PROVIDER_MAX_VIEWPORT_DIMENSION {
        return Ok((width, height));
    }
    let scale = f64::from(PROVIDER_MAX_VIEWPORT_DIMENSION) / f64::from(maximum);
    Ok((
        (f64::from(width) * scale).floor().max(1.0) as u32,
        (f64::from(height) * scale).floor().max(1.0) as u32,
    ))
}

fn single_slide_viewport(
    max_width: u32,
    max_height: u32,
    slide_width: u32,
    slide_height: u32,
) -> Result<OfficeViewport, OfficeEngineError> {
    if max_width == 0 || max_height == 0 || slide_width == 0 || slide_height == 0 {
        return Err(invalid_request(
            "Presentation slide geometry requires positive dimensions.",
        ));
    }
    let ratio = f64::from(slide_height) / f64::from(slide_width);
    let mut width = max_width;
    let mut height = checked_round_dimension(f64::from(width) * ratio)?;
    if height > max_height {
        height = max_height;
        width = checked_round_dimension(f64::from(height) / ratio)?;
    }
    if width == 0 || height == 0 {
        return Err(invalid_request(
            "Presentation slide geometry cannot produce a valid screenshot viewport.",
        ));
    }
    Ok(OfficeViewport { width, height })
}

fn choose_grid(
    page_count: usize,
    viewport_width: u32,
    max_height: u32,
    slide_width: u32,
    slide_height: u32,
) -> Result<(u16, u32), OfficeEngineError> {
    if !(2..=MAX_OFFICE_TOTAL_PAGES as usize).contains(&page_count) {
        return Err(invalid_request(format!(
            "Presentation contact sheets require between 2 and {MAX_OFFICE_TOTAL_PAGES} slides."
        )));
    }
    if viewport_width == 0 || max_height == 0 || slide_width == 0 || slide_height == 0 {
        return Err(invalid_request(
            "Presentation contact-sheet geometry requires positive dimensions.",
        ));
    }
    let max_columns = usize::from(MAX_OFFICE_GRID_COLUMNS).min(page_count);
    for columns in 1..=max_columns {
        let columns = u16::try_from(columns)
            .map_err(|_| invalid_request("Presentation grid column count overflowed."))?;
        let rows = page_count.div_ceil(usize::from(columns));
        let tile_width = rounded_css_px(grid_tile_width(viewport_width, columns)?)?;
        let tile_height =
            rounded_css_px(tile_width * f64::from(slide_height) / f64::from(slide_width))?;
        let content_height = 2.0 * GRID_PADDING_PX
            + rows as f64 * tile_height
            + rows.saturating_sub(1) as f64 * GRID_GAP_PX;
        let Some(viewport_height) = checked_ceil_dimension(content_height)
            .and_then(|height| height.checked_add(GRID_HEIGHT_SAFETY_PX))
        else {
            continue;
        };
        if viewport_height <= max_height {
            return Ok((columns, viewport_height));
        }
    }
    Err(invalid_request(
        "The requested presentation screenshot viewport is too small to contain every slide; increase the viewport or render bounded slide ranges.",
    ))
}

fn grid_tile_width(viewport_width: u32, columns: u16) -> Result<f64, OfficeEngineError> {
    let gaps = f64::from(columns.saturating_sub(1)) * GRID_GAP_PX;
    let available = f64::from(viewport_width) - 2.0 * GRID_PADDING_PX - gaps;
    let width = available / f64::from(columns);
    if !width.is_finite() || width < 1.0 {
        return Err(invalid_request(
            "The presentation screenshot viewport is too narrow for the resolved grid.",
        ));
    }
    Ok(width)
}

fn rounded_css_px(value: f64) -> Result<f64, OfficeEngineError> {
    let scaled = value * 100.0;
    if !value.is_finite() || value <= 0.0 || !scaled.is_finite() {
        return Err(invalid_request(
            "Presentation contact-sheet geometry is outside its supported range.",
        ));
    }
    Ok(scaled.round() / 100.0)
}

fn checked_ceil_dimension(value: f64) -> Option<u32> {
    let value = value.ceil();
    (value.is_finite() && value > 0.0 && value <= f64::from(u32::MAX)).then_some(value as u32)
}

fn checked_round_dimension(value: f64) -> Result<u32, OfficeEngineError> {
    let value = value.round();
    if !value.is_finite() || value <= 0.0 || value > f64::from(u32::MAX) {
        return Err(invalid_request(
            "Presentation slide geometry is outside its supported range.",
        ));
    }
    Ok(value as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_raw_presentation(
        path: &Path,
        presentation_xml: &str,
        relationships_xml: &str,
        packaged_slides: u32,
    ) {
        let file = fs::File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        archive.start_file(PRESENTATION_XML_PATH, options).unwrap();
        archive.write_all(presentation_xml.as_bytes()).unwrap();
        archive.start_file(PRESENTATION_RELS_PATH, options).unwrap();
        archive.write_all(relationships_xml.as_bytes()).unwrap();
        for slide in 1..=packaged_slides {
            archive
                .start_file(format!("ppt/slides/slide{slide}.xml"), options)
                .unwrap();
            archive.write_all(b"<p:sld xmlns:p=\"urn:test\"/>").unwrap();
        }
        archive.finish().unwrap();
    }

    fn write_presentation(
        path: &Path,
        declared_slides: u32,
        packaged_slides: u32,
        duplicate_slide: Option<u32>,
    ) {
        let slide_ids = (1..=declared_slides)
            .map(|slide| format!("<p:sldId id=\"{}\" r:id=\"rId{}\"/>", 255 + slide, slide))
            .collect::<String>();
        let presentation_xml = format!(
            "<p:presentation xmlns:p=\"{PRESENTATIONML_TRANSITIONAL_NAMESPACE}\" xmlns:r=\"{OFFICE_RELATIONSHIPS_TRANSITIONAL_NAMESPACE}\"><p:sldIdLst>{slide_ids}</p:sldIdLst><p:sldSz cx=\"12192000\" cy=\"6858000\"/></p:presentation>"
        );
        let relationships = (1..=declared_slides)
            .map(|slide| format!("<Relationship Id=\"rId{slide}\" Type=\"{SLIDE_RELATIONSHIP_TRANSITIONAL_TYPE}\" Target=\"slides/slide{slide}.xml\"/>"))
            .collect::<String>();
        let relationships_xml = format!(
            "<Relationships xmlns=\"{PACKAGE_RELATIONSHIPS_NAMESPACE}\">{relationships}</Relationships>"
        );
        write_raw_presentation(path, &presentation_xml, &relationships_xml, packaged_slides);
        if let Some(slide) = duplicate_slide {
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .unwrap();
            let mut archive = zip::ZipWriter::new_append(file).unwrap();
            let options = zip::write::SimpleFileOptions::default();
            archive
                .start_file("ppt/slides/slide0.xml", options)
                .unwrap();
            archive
                .write_all(b"<p:sld xmlns:p=\"urn:duplicate\"/>")
                .unwrap();
            archive.finish().unwrap();
            assert!(slide < 10, "test duplicate placeholder expects one digit");
        }
        if let Some(slide) = duplicate_slide {
            let placeholder = b"ppt/slides/slide0.xml";
            let replacement = format!("ppt/slides/slide{slide}.xml").into_bytes();
            let mut bytes = fs::read(path).unwrap();
            let mut replacements = 0;
            for offset in 0..=bytes.len() - placeholder.len() {
                if &bytes[offset..offset + placeholder.len()] == placeholder {
                    bytes[offset..offset + placeholder.len()].copy_from_slice(&replacement);
                    replacements += 1;
                }
            }
            assert!(
                replacements >= 2,
                "ZIP local and central names must be patched"
            );
            fs::write(path, bytes).unwrap();
        }
    }

    #[test]
    fn nine_widescreen_slides_resolve_to_a_complete_three_by_three_sheet() {
        let (columns, height) = choose_grid(9, 1600, 1200, 12_192_000, 6_858_000).unwrap();
        assert_eq!(columns, 3);
        assert_eq!(height, 922);
    }

    #[test]
    fn one_widescreen_slide_uses_a_full_width_aspect_matched_viewport() {
        assert_eq!(
            single_slide_viewport(1600, 1200, 12_192_000, 6_858_000).unwrap(),
            OfficeViewport {
                width: 1600,
                height: 900
            }
        );
    }

    #[test]
    fn slide_relationship_targets_accept_only_safe_relative_or_ppt_package_absolute_paths() {
        for target in ["slides/slide1.xml", "/ppt/slides/slide1.xml"] {
            assert_eq!(
                normalize_slide_target(target).unwrap(),
                "ppt/slides/slide1.xml"
            );
        }
        for target in [
            "/slides/slide1.xml",
            "/word/slides/slide1.xml",
            "/ppt/../slides/slide1.xml",
            "/ppt/slides/../../slide1.xml",
            "https://example.test/ppt/slides/slide1.xml",
            "/ppt/slides/slide1.xml?query",
            "/ppt/slides/slide1.xml#fragment",
            "/ppt/slides/slide%31.xml",
            "\\ppt\\slides\\slide1.xml",
            "/ppt/media/image1.xml",
        ] {
            assert!(
                normalize_slide_target(target).is_err(),
                "unsafe slide target `{target}` was accepted"
            );
        }
    }

    #[test]
    fn presentation_geometry_requires_relationships_and_parts_to_close() {
        let directory = tempfile::tempdir().unwrap();
        let valid = directory.path().join("nine-slides.pptx");
        write_presentation(&valid, 9, 9, None);
        assert_eq!(
            read_presentation_geometry(&valid).unwrap(),
            (9, 12_192_000, 6_858_000)
        );

        let missing = directory.path().join("missing-slide-seven.pptx");
        write_presentation(&missing, 9, 6, None);
        let error = read_presentation_geometry(&missing).unwrap_err();
        assert!(error.message().contains("missing slide part"));

        let duplicate = directory.path().join("duplicate-slide-part.pptx");
        write_presentation(&duplicate, 9, 9, Some(7));
        let error = read_presentation_geometry(&duplicate).unwrap_err();
        assert!(error.message().contains("duplicate ZIP entry"));
    }

    #[test]
    fn presentation_metadata_accepts_only_supported_ooxml_namespaces_and_structure() {
        let directory = tempfile::tempdir().unwrap();
        let strict = directory.path().join("strict.pptx");
        write_raw_presentation(
            &strict,
            &format!(
                "<p:presentation xmlns:p=\"{PRESENTATIONML_STRICT_NAMESPACE}\" xmlns:r=\"{OFFICE_RELATIONSHIPS_STRICT_NAMESPACE}\"><p:sldIdLst><p:sldId id=\"256\" r:id=\"rId1\"/></p:sldIdLst><p:sldSz cx=\"4\" cy=\"3\"/></p:presentation>"
            ),
            &format!(
                "<Relationships xmlns=\"{PACKAGE_RELATIONSHIPS_NAMESPACE}\"><Relationship Id=\"rId1\" Type=\"{SLIDE_RELATIONSHIP_STRICT_TYPE}\" Target=\"slides/slide1.xml\"/></Relationships>"
            ),
            1,
        );
        assert_eq!(read_presentation_geometry(&strict).unwrap(), (1, 4, 3));

        let wrong_root = directory.path().join("wrong-root.pptx");
        write_raw_presentation(
            &wrong_root,
            &format!(
                "<p:presentation xmlns:p=\"urn:decoy\" xmlns:r=\"{OFFICE_RELATIONSHIPS_TRANSITIONAL_NAMESPACE}\"><p:sldIdLst><p:sldId id=\"256\" r:id=\"rId1\"/></p:sldIdLst><p:sldSz cx=\"4\" cy=\"3\"/></p:presentation>"
            ),
            &format!(
                "<Relationships xmlns=\"{PACKAGE_RELATIONSHIPS_NAMESPACE}\"><Relationship Id=\"rId1\" Type=\"{SLIDE_RELATIONSHIP_TRANSITIONAL_TYPE}\" Target=\"slides/slide1.xml\"/></Relationships>"
            ),
            1,
        );
        assert!(read_presentation_geometry(&wrong_root)
            .unwrap_err()
            .message()
            .contains("unsupported presentation namespace"));

        let nested_decoy = directory.path().join("nested-slide-list.pptx");
        write_raw_presentation(
            &nested_decoy,
            &format!(
                "<p:presentation xmlns:p=\"{PRESENTATIONML_TRANSITIONAL_NAMESPACE}\" xmlns:r=\"{OFFICE_RELATIONSHIPS_TRANSITIONAL_NAMESPACE}\"><p:extLst><p:sldIdLst><p:sldId id=\"256\" r:id=\"rId1\"/></p:sldIdLst></p:extLst><p:sldSz cx=\"4\" cy=\"3\"/></p:presentation>"
            ),
            &format!(
                "<Relationships xmlns=\"{PACKAGE_RELATIONSHIPS_NAMESPACE}\"><Relationship Id=\"rId1\" Type=\"{SLIDE_RELATIONSHIP_TRANSITIONAL_TYPE}\" Target=\"slides/slide1.xml\"/></Relationships>"
            ),
            1,
        );
        assert!(read_presentation_geometry(&nested_decoy)
            .unwrap_err()
            .message()
            .contains("direct child"));

        let duplicate_size = directory.path().join("duplicate-slide-size.pptx");
        write_raw_presentation(
            &duplicate_size,
            &format!(
                "<p:presentation xmlns:p=\"{PRESENTATIONML_TRANSITIONAL_NAMESPACE}\" xmlns:r=\"{OFFICE_RELATIONSHIPS_TRANSITIONAL_NAMESPACE}\"><p:sldIdLst><p:sldId id=\"256\" r:id=\"rId1\"/></p:sldIdLst><p:sldSz cx=\"4\" cy=\"3\"/><p:sldSz cx=\"16\" cy=\"9\"/></p:presentation>"
            ),
            &format!(
                "<Relationships xmlns=\"{PACKAGE_RELATIONSHIPS_NAMESPACE}\"><Relationship Id=\"rId1\" Type=\"{SLIDE_RELATIONSHIP_TRANSITIONAL_TYPE}\" Target=\"slides/slide1.xml\"/></Relationships>"
            ),
            1,
        );
        assert!(read_presentation_geometry(&duplicate_size)
            .unwrap_err()
            .message()
            .contains("exactly one namespaced slide-size"));
    }

    #[test]
    fn presentation_slide_numeric_id_is_required_bounded_and_unique() {
        let directory = tempfile::tempdir().unwrap();
        let relationships = |count| {
            let relationships = (1..=count)
                .map(|slide| format!("<Relationship Id=\"rId{slide}\" Type=\"{SLIDE_RELATIONSHIP_TRANSITIONAL_TYPE}\" Target=\"slides/slide{slide}.xml\"/>"))
                .collect::<String>();
            format!(
                "<Relationships xmlns=\"{PACKAGE_RELATIONSHIPS_NAMESPACE}\">{relationships}</Relationships>"
            )
        };
        let write_case = |name: &str, slide_ids: &str, count: u32| {
            let path = directory.path().join(name);
            write_raw_presentation(
                &path,
                &format!(
                    "<p:presentation xmlns:p=\"{PRESENTATIONML_TRANSITIONAL_NAMESPACE}\" xmlns:r=\"{OFFICE_RELATIONSHIPS_TRANSITIONAL_NAMESPACE}\" xmlns:x=\"urn:decoy\"><p:sldIdLst>{slide_ids}</p:sldIdLst><p:sldSz cx=\"4\" cy=\"3\"/></p:presentation>"
                ),
                &relationships(count),
                count,
            );
            path
        };

        let maximum = write_case(
            "maximum-id.pptx",
            "<p:sldId id=\"2147483647\" r:id=\"rId1\"/>",
            1,
        );
        assert_eq!(read_presentation_geometry(&maximum).unwrap(), (1, 4, 3));

        for (name, slide_ids, expected) in [
            (
                "missing-id.pptx",
                "<p:sldId r:id=\"rId1\"/>",
                "exactly one unnamespaced numeric identity",
            ),
            (
                "namespaced-decoy-id.pptx",
                "<p:sldId x:id=\"256\" r:id=\"rId1\"/>",
                "exactly one unnamespaced numeric identity",
            ),
            (
                "non-numeric-id.pptx",
                "<p:sldId id=\"slide-256\" r:id=\"rId1\"/>",
                "integers between 256 and 2147483647",
            ),
            (
                "too-small-id.pptx",
                "<p:sldId id=\"255\" r:id=\"rId1\"/>",
                "integers between 256 and 2147483647",
            ),
            (
                "too-large-id.pptx",
                "<p:sldId id=\"2147483648\" r:id=\"rId1\"/>",
                "integers between 256 and 2147483647",
            ),
        ] {
            let path = write_case(name, slide_ids, 1);
            assert!(
                read_presentation_geometry(&path)
                    .unwrap_err()
                    .message()
                    .contains(expected),
                "unexpected error for {name}"
            );
        }

        let duplicate = write_case(
            "duplicate-id.pptx",
            "<p:sldId id=\"256\" r:id=\"rId1\"/><p:sldId id=\"256\" r:id=\"rId2\"/>",
            2,
        );
        assert!(read_presentation_geometry(&duplicate)
            .unwrap_err()
            .message()
            .contains("numeric identities must be globally unique"));
    }

    #[test]
    fn zip_entry_accounting_rejects_early_stop_and_decoded_name_collisions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("valid.pptx");
        write_presentation(&path, 2, 2, None);
        let file = fs::File::open(&path).unwrap();
        let archive = zip::ZipArchive::new(file).unwrap();
        let raw_count =
            scan_unique_central_directory_names(&path, archive.central_directory_start()).unwrap();
        assert_eq!(raw_count, archive.len());
        assert!(
            validate_central_directory_entry_count(raw_count, archive.len() + 1)
                .unwrap_err()
                .message()
                .contains("entry count does not match")
        );

        // Raw central-directory names have already been proven unique before
        // this pass. Equal decoded names therefore model two different raw
        // names resolved to the same Unicode Path extra-field value.
        assert!(validate_unique_decoded_zip_entry_names(&[
            "ppt/slides/幻灯片.xml".to_string(),
            "ppt/slides/幻灯片.xml".to_string(),
        ])
        .unwrap_err()
        .message()
        .contains("collide after decoding"));
    }

    #[test]
    fn presentation_relationships_reject_namespace_type_and_identity_decoys() {
        let directory = tempfile::tempdir().unwrap();
        let presentation_xml = format!(
            "<p:presentation xmlns:p=\"{PRESENTATIONML_TRANSITIONAL_NAMESPACE}\" xmlns:r=\"{OFFICE_RELATIONSHIPS_TRANSITIONAL_NAMESPACE}\"><p:sldIdLst><p:sldId id=\"256\" r:id=\"rId1\"/></p:sldIdLst><p:sldSz cx=\"4\" cy=\"3\"/></p:presentation>"
        );

        let wrong_root = directory.path().join("wrong-rels-root.pptx");
        write_raw_presentation(
            &wrong_root,
            &presentation_xml,
            &format!(
                "<Relationships xmlns=\"urn:decoy\"><Relationship Id=\"rId1\" Type=\"{SLIDE_RELATIONSHIP_TRANSITIONAL_TYPE}\" Target=\"slides/slide1.xml\"/></Relationships>"
            ),
            1,
        );
        assert!(read_presentation_geometry(&wrong_root)
            .unwrap_err()
            .message()
            .contains("invalid document root or namespace"));

        let suffix_decoy = directory.path().join("suffix-slide-type.pptx");
        write_raw_presentation(
            &suffix_decoy,
            &presentation_xml,
            &format!(
                "<Relationships xmlns=\"{PACKAGE_RELATIONSHIPS_NAMESPACE}\"><Relationship Id=\"rId1\" Type=\"urn:decoy/slide\" Target=\"slides/slide1.xml\"/></Relationships>"
            ),
            1,
        );
        assert!(read_presentation_geometry(&suffix_decoy)
            .unwrap_err()
            .message()
            .contains("missing or is not an internal slide"));

        let duplicate_id = directory.path().join("duplicate-relationship-id.pptx");
        write_raw_presentation(
            &duplicate_id,
            &presentation_xml,
            &format!(
                "<Relationships xmlns=\"{PACKAGE_RELATIONSHIPS_NAMESPACE}\"><Relationship Id=\"rId1\" Type=\"{SLIDE_RELATIONSHIP_TRANSITIONAL_TYPE}\" Target=\"slides/slide1.xml\"/><Relationship Id=\"rId1\" Type=\"urn:example:theme\" Target=\"theme/theme1.xml\"/></Relationships>"
            ),
            1,
        );
        assert!(read_presentation_geometry(&duplicate_id)
            .unwrap_err()
            .message()
            .contains("globally unique"));
    }

    #[test]
    fn large_source_decks_allow_single_slide_but_not_unbounded_contact_sheets() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("129-slides.pptx");
        write_presentation(&path, 129, 129, None);
        assert_eq!(read_presentation_geometry(&path).unwrap().0, 129);

        let request = |pages| OfficeExecutionRequest {
            document_kind: OfficeDocumentKind::Presentation,
            operation: OfficeOperation::View,
            document_path: Some("129-slides.pptx".to_string()),
            parameters: OfficeOperationParameters::View {
                mode: OfficeViewMode::Screenshot,
                start: None,
                end: None,
                max_lines: None,
                issue_type: None,
                limit: None,
                columns: Vec::new(),
                pages,
                range: None,
                viewport: None,
                grid: None,
                render_mode: None,
                page_count: false,
            },
            output_path: Some("preview.png".to_string()),
            destination_path: None,
            inputs: Vec::new(),
            timeout_ms: None,
        };
        let one_page = resolve_presentation_render_plan(
            &request(vec![crate::office::OfficePageRange {
                start: 129,
                end: None,
            }]),
            &path,
        )
        .unwrap()
        .unwrap();
        assert_eq!(one_page.requested_pages, vec![129]);
        assert!(one_page.grid.is_none());

        let error = resolve_presentation_render_plan(&request(Vec::new()), &path).unwrap_err();
        assert!(error.message().contains("between 2 and 128 slides"));
    }

    #[test]
    fn deterministic_grid_matrix_keeps_every_tile_inside_the_viewport() {
        for page_count in [5_usize, 6, 7, 9, 12, 20, 128] {
            for (slide_width, slide_height) in [(16_u32, 9_u32), (4, 3), (3, 4)] {
                let (columns, viewport_height) =
                    choose_grid(page_count, 1600, 1200, slide_width, slide_height).unwrap();
                let rows = page_count.div_ceil(usize::from(columns));
                let tile_width = rounded_css_px(grid_tile_width(1600, columns).unwrap()).unwrap();
                let tile_height =
                    rounded_css_px(tile_width * f64::from(slide_height) / f64::from(slide_width))
                        .unwrap();
                let content_height = 2.0 * GRID_PADDING_PX
                    + rows as f64 * tile_height
                    + rows.saturating_sub(1) as f64 * GRID_GAP_PX;
                assert!(columns <= MAX_OFFICE_GRID_COLUMNS);
                assert_eq!(rows, page_count.div_ceil(usize::from(columns)));
                assert!(content_height <= f64::from(viewport_height));

                let plan = OfficePresentationRenderPlan {
                    requested_pages: (1..=u32::try_from(page_count).unwrap()).collect(),
                    slide_width_emu: slide_width,
                    slide_height_emu: slide_height,
                    viewport: OfficeViewport {
                        width: 1600,
                        height: viewport_height,
                    },
                    grid: Some(OfficeGridLayout::Columns { columns }),
                };
                let receipt = super::super::verify_render_layout_coverage(
                    &plan,
                    plan.viewport.width,
                    plan.viewport.height,
                )
                .unwrap();
                assert_eq!(receipt.requested_pages.len(), page_count);
                assert_eq!(
                    receipt.evidence,
                    crate::office::OfficeRenderLayoutEvidence::TrustedRendererGeometry
                );
            }
        }
    }

    #[test]
    fn oversized_or_impossibly_small_contact_sheets_fail_closed() {
        assert!(choose_grid(129, 1600, 1200, 16, 9).is_err());
        assert!(choose_grid(9, 20, 20, 16, 9).is_err());
    }

    #[test]
    fn extreme_slide_geometry_fails_closed_without_overflow() {
        assert!(choose_grid(2, u32::MAX, u32::MAX, 1, u32::MAX).is_err());
        assert!(choose_grid(2, u32::MAX, u32::MAX, u32::MAX, 1).is_ok());
        assert!(choose_grid(2, 1600, 1200, 0, 1).is_err());
        assert!(choose_grid(2, 1600, 1200, 1, 0).is_err());
        assert!(choose_grid(2, 0, 1200, 16, 9).is_err());
        assert!(single_slide_viewport(u32::MAX, u32::MAX, 1, u32::MAX).is_err());
    }
}
