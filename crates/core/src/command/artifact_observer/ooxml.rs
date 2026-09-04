use super::*;

pub(super) fn validate_office_artifact(
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
pub(super) struct OoxmlZipLayout {
    pub(super) entry_count: u64,
    pub(super) central_directory_size: u64,
    pub(super) central_directory_offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OoxmlZipPreflightError {
    pub(super) code: &'static str,
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
pub(super) fn preflight_ooxml_zip(
    file: &mut File,
) -> Result<OoxmlZipLayout, OoxmlZipPreflightError> {
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

pub(super) fn find_eocd_at_physical_end(tail: &[u8]) -> Option<usize> {
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

pub(super) fn read_u16_le(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

pub(super) fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
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
