use super::model::SkillDiagnosticCode;
use serde::Deserialize;
use std::error::Error;
use std::fmt;

pub(super) const MAX_SKILL_NAME_CHARS: usize = 64;
pub(super) const MAX_SKILL_DESCRIPTION_CHARS: usize = 1024;
const MAX_SKILL_FRONTMATTER_BYTES: usize = 16 * 1024;

#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ParsedSkillMetadata {
    pub name: String,
    pub description: String,
    pub name_was_defaulted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SkillParseError {
    MissingFrontmatter,
    InvalidFrontmatter(String),
    MissingDescription,
    InvalidName(String),
    InvalidDescription(String),
}

impl SkillParseError {
    pub fn diagnostic_code(&self) -> SkillDiagnosticCode {
        match self {
            Self::MissingFrontmatter => SkillDiagnosticCode::MissingFrontmatter,
            Self::InvalidFrontmatter(_) => SkillDiagnosticCode::InvalidFrontmatter,
            Self::MissingDescription => SkillDiagnosticCode::MissingDescription,
            Self::InvalidName(_) => SkillDiagnosticCode::InvalidName,
            Self::InvalidDescription(_) => SkillDiagnosticCode::InvalidDescription,
        }
    }
}

impl fmt::Display for SkillParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFrontmatter => {
                formatter.write_str("missing YAML frontmatter delimited by ---")
            }
            Self::InvalidFrontmatter(reason) => {
                write!(formatter, "invalid YAML frontmatter: {reason}")
            }
            Self::MissingDescription => formatter.write_str("missing field `description`"),
            Self::InvalidName(reason) => write!(formatter, "invalid name: {reason}"),
            Self::InvalidDescription(reason) => {
                write!(formatter, "invalid description: {reason}")
            }
        }
    }
}

impl Error for SkillParseError {}

pub(super) fn parse_skill_metadata(
    contents: &str,
    default_name: &str,
) -> Result<ParsedSkillMetadata, SkillParseError> {
    let frontmatter = extract_frontmatter(contents)?;
    let parsed = parse_frontmatter_yaml(&frontmatter)?;

    let explicit_name = match parsed.name.as_deref() {
        Some(value) => {
            reject_unsafe_controls(value, SkillMetadataField::Name)?;
            Some(sanitize_single_line(value)).filter(|value| !value.is_empty())
        }
        None => None,
    };
    let name_was_defaulted = explicit_name.is_none();
    let name = match explicit_name {
        Some(name) => name,
        None => {
            reject_unsafe_controls(default_name, SkillMetadataField::Name)?;
            sanitize_single_line(default_name)
        }
    };
    if name.is_empty() {
        return Err(SkillParseError::InvalidName(
            "the name and directory fallback are both empty".to_string(),
        ));
    }
    let name_chars = name.chars().count();
    if name_chars > MAX_SKILL_NAME_CHARS {
        return Err(SkillParseError::InvalidName(format!(
            "exceeds the maximum length of {MAX_SKILL_NAME_CHARS} characters"
        )));
    }

    let description = match parsed.description.as_deref() {
        Some(value) => {
            reject_unsafe_controls(value, SkillMetadataField::Description)?;
            sanitize_single_line(value)
        }
        None => String::new(),
    };
    if description.is_empty() {
        return Err(SkillParseError::MissingDescription);
    }
    let description_chars = description.chars().count();
    if description_chars > MAX_SKILL_DESCRIPTION_CHARS {
        return Err(SkillParseError::InvalidDescription(format!(
            "exceeds the maximum length of {MAX_SKILL_DESCRIPTION_CHARS} characters"
        )));
    }

    Ok(ParsedSkillMetadata {
        name,
        description,
        name_was_defaulted,
    })
}

fn parse_frontmatter_yaml(frontmatter: &str) -> Result<SkillFrontmatter, SkillParseError> {
    match serde_yaml::from_str(frontmatter) {
        Ok(parsed) => Ok(parsed),
        Err(original_error) => {
            if let Some(repaired) = repair_unquoted_plain_scalars(frontmatter) {
                if let Ok(parsed) = serde_yaml::from_str(&repaired) {
                    return Ok(parsed);
                }
            }

            Err(SkillParseError::InvalidFrontmatter(
                original_error.to_string(),
            ))
        }
    }
}

fn extract_frontmatter(contents: &str) -> Result<String, SkillParseError> {
    let contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);
    let (opening_line, mut offset) = line_at(contents, 0);
    if !is_frontmatter_delimiter(opening_line) {
        return Err(SkillParseError::MissingFrontmatter);
    }

    let frontmatter_start = offset;
    while offset < contents.len() {
        let line_start = offset;
        let (line, next_offset) = line_at(contents, line_start);
        if is_frontmatter_delimiter(line) {
            let frontmatter_bytes = line_start.saturating_sub(frontmatter_start);
            if frontmatter_bytes == 0 {
                return Err(SkillParseError::MissingFrontmatter);
            }
            if frontmatter_bytes > MAX_SKILL_FRONTMATTER_BYTES {
                return Err(frontmatter_too_large());
            }
            return Ok(contents[frontmatter_start..line_start].to_string());
        }

        let frontmatter_bytes = next_offset.saturating_sub(frontmatter_start);
        if frontmatter_bytes > MAX_SKILL_FRONTMATTER_BYTES {
            return Err(frontmatter_too_large());
        }
        offset = next_offset;
    }

    Err(SkillParseError::MissingFrontmatter)
}

fn is_frontmatter_delimiter(line: &str) -> bool {
    line.strip_prefix("---")
        .is_some_and(|suffix| suffix.bytes().all(|byte| matches!(byte, b' ' | b'\t')))
}

fn line_at(contents: &str, start: usize) -> (&str, usize) {
    let remainder = &contents[start..];
    match remainder.find('\n') {
        Some(relative_end) => {
            let raw_end = start + relative_end;
            let content_end = if raw_end > start && contents.as_bytes()[raw_end - 1] == b'\r' {
                raw_end - 1
            } else {
                raw_end
            };
            (&contents[start..content_end], raw_end + 1)
        }
        None => (&contents[start..], contents.len()),
    }
}

fn frontmatter_too_large() -> SkillParseError {
    SkillParseError::InvalidFrontmatter(format!(
        "frontmatter exceeds {MAX_SKILL_FRONTMATTER_BYTES} bytes"
    ))
}

/// Repairs only ambiguous, unquoted mapping scalars. This deliberately avoids
/// interpreting general YAML: quoted/flow values, comments, and block scalar
/// bodies are copied byte-for-byte.
fn repair_unquoted_plain_scalars(frontmatter: &str) -> Option<String> {
    let mut repaired = String::with_capacity(frontmatter.len());
    let mut changed = false;
    let mut block_scalar_parent_indent = None;

    for segment in frontmatter.split_inclusive('\n') {
        let (line, line_ending) = split_line_ending(segment);
        let indent = line.bytes().take_while(|byte| *byte == b' ').count();
        let trimmed = &line[indent..];

        if let Some(parent_indent) = block_scalar_parent_indent {
            if trimmed.is_empty() || indent > parent_indent {
                repaired.push_str(segment);
                continue;
            }
            block_scalar_parent_indent = None;
        }

        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('\t') {
            repaired.push_str(segment);
            continue;
        }

        let Some(separator) = plain_mapping_separator(trimmed) else {
            repaired.push_str(segment);
            continue;
        };
        let value = &trimmed[separator + 1..];
        let value_without_indent = value.trim_start_matches([' ', '\t']);
        if is_block_scalar_header(value_without_indent) {
            block_scalar_parent_indent = Some(indent);
            repaired.push_str(segment);
            continue;
        }

        let leading_whitespace = value.len() - value_without_indent.len();
        let comment_start = inline_comment_start(value_without_indent);
        let scalar_region =
            &value_without_indent[..comment_start.unwrap_or(value_without_indent.len())];
        let scalar = scalar_region.trim_end_matches([' ', '\t']);
        if scalar.is_empty()
            || starts_with_yaml_structure(scalar)
            || !contains_ambiguous_mapping_colon(scalar)
        {
            repaired.push_str(segment);
            continue;
        }

        let scalar_start = indent + separator + 1 + leading_whitespace;
        let scalar_end = scalar_start + scalar.len();
        repaired.push_str(&line[..scalar_start]);
        repaired.push_str(
            &serde_json::to_string(scalar).expect("serializing a string to JSON cannot fail"),
        );
        repaired.push_str(&line[scalar_end..]);
        repaired.push_str(line_ending);
        changed = true;
    }

    changed.then_some(repaired)
}

fn split_line_ending(segment: &str) -> (&str, &str) {
    if let Some(line) = segment.strip_suffix("\r\n") {
        (line, "\r\n")
    } else if let Some(line) = segment.strip_suffix('\n') {
        (line, "\n")
    } else {
        (segment, "")
    }
}

fn plain_mapping_separator(line: &str) -> Option<usize> {
    let separator = line.find(':')?;
    let key = &line[..separator];
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }

    let value = &line[separator + 1..];
    (value.is_empty() || value.starts_with([' ', '\t'])).then_some(separator)
}

fn inline_comment_start(value: &str) -> Option<usize> {
    value.char_indices().find_map(|(index, character)| {
        if character != '#' {
            return None;
        }
        if index == 0
            || value[..index]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
        {
            Some(index)
        } else {
            None
        }
    })
}

fn is_block_scalar_header(value: &str) -> bool {
    let Some(indicator) = value.chars().next() else {
        return false;
    };
    if !matches!(indicator, '|' | '>') {
        return false;
    }

    let without_comment = inline_comment_start(value)
        .map(|index| &value[..index])
        .unwrap_or(value);
    without_comment[indicator.len_utf8()..]
        .trim()
        .chars()
        .all(|character| character.is_ascii_digit() || matches!(character, '+' | '-'))
}

fn starts_with_yaml_structure(value: &str) -> bool {
    let Some(first) = value.chars().next() else {
        return false;
    };
    matches!(
        first,
        '\'' | '"' | '[' | '{' | '|' | '>' | '&' | '*' | '!' | '%'
    ) || matches!(first, '-' | '?' | ':')
        && value[first.len_utf8()..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
}

fn contains_ambiguous_mapping_colon(value: &str) -> bool {
    let mut characters = value.char_indices().peekable();
    while let Some((_, character)) = characters.next() {
        if character == ':'
            && characters
                .peek()
                .map(|(_, next)| next.is_whitespace())
                .unwrap_or(true)
        {
            return true;
        }
    }
    false
}

#[derive(Clone, Copy)]
enum SkillMetadataField {
    Name,
    Description,
}

fn reject_unsafe_controls(value: &str, field: SkillMetadataField) -> Result<(), SkillParseError> {
    // Newlines and tabs are intentional YAML presentation whitespace and are
    // folded by `sanitize_single_line`; all other C0/C1 controls are rejected.
    let Some(character) = value
        .chars()
        .find(|character| character.is_control() && !matches!(character, '\t' | '\n' | '\r'))
    else {
        return Ok(());
    };
    let reason = format!(
        "contains prohibited control character U+{:04X}",
        character as u32
    );
    match field {
        SkillMetadataField::Name => Err(SkillParseError::InvalidName(reason)),
        SkillMetadataField::Description => Err(SkillParseError::InvalidDescription(reason)),
    }
}

fn sanitize_single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bom_crlf_block_scalar_and_unknown_fields() {
        let contents = concat!(
            "\u{feff}---\r\n",
            "name: repository-auditor\r\n",
            "description: >\r\n",
            "  Inspect repositories and\r\n",
            "  cite source evidence.\r\n",
            "metadata:\r\n",
            "  owner: platform\r\n",
            "---\r\n",
            "# Instructions\r\n"
        );

        let metadata = parse_skill_metadata(contents, "fallback").unwrap();

        assert_eq!(metadata.name, "repository-auditor");
        assert_eq!(
            metadata.description,
            "Inspect repositories and cite source evidence."
        );
        assert!(!metadata.name_was_defaulted);
    }

    #[test]
    fn does_not_treat_an_indented_block_scalar_line_as_the_closing_delimiter() {
        let metadata = parse_skill_metadata(
            "---\nname: delimiter-auditor\ndescription: |\n  ---\n  instructions\n---\n",
            "fallback",
        )
        .unwrap();

        assert_eq!(metadata.description, "--- instructions");
    }

    #[test]
    fn defaults_missing_name_to_directory_name() {
        let metadata = parse_skill_metadata(
            "---\ndescription: Inspect repositories.\n---\n",
            "repository-auditor",
        )
        .unwrap();

        assert_eq!(metadata.name, "repository-auditor");
        assert!(metadata.name_was_defaulted);
    }

    #[test]
    fn rejects_missing_or_unclosed_frontmatter() {
        assert_eq!(
            parse_skill_metadata("description: missing delimiters", "fallback").unwrap_err(),
            SkillParseError::MissingFrontmatter
        );
        assert_eq!(
            parse_skill_metadata("---\ndescription: missing close", "fallback").unwrap_err(),
            SkillParseError::MissingFrontmatter
        );
    }

    #[test]
    fn rejects_invalid_yaml_and_missing_description() {
        assert!(matches!(
            parse_skill_metadata("---\ndescription: [\n---\n", "fallback"),
            Err(SkillParseError::InvalidFrontmatter(_))
        ));
        assert_eq!(
            parse_skill_metadata("---\nname: valid\n---\n", "fallback").unwrap_err(),
            SkillParseError::MissingDescription
        );
    }

    #[test]
    fn repairs_only_ambiguous_unquoted_plain_scalars() {
        let metadata = parse_skill_metadata(
            concat!(
                "---\r\n",
                "name: aws-auditor\r\n",
                "description: Build for AWS: ECS # preserved comment\r\n",
                "notes: >\r\n",
                "  This body: remains untouched.\r\n",
                "---\r\n"
            ),
            "fallback",
        )
        .unwrap();

        assert_eq!(metadata.description, "Build for AWS: ECS");
        assert_eq!(
            repair_unquoted_plain_scalars(
                "description: \"Build for AWS: ECS\"\nnotes: >\n  Body: unchanged.\n# note: unchanged\n"
            ),
            None
        );
    }

    #[test]
    fn reports_original_yaml_error_when_repair_does_not_make_document_valid() {
        let frontmatter = "description: Build for AWS: ECS\nbroken: [\n";
        let original_error = serde_yaml::from_str::<SkillFrontmatter>(frontmatter)
            .unwrap_err()
            .to_string();

        assert_eq!(
            parse_frontmatter_yaml(frontmatter).unwrap_err(),
            SkillParseError::InvalidFrontmatter(original_error)
        );
    }

    #[test]
    fn rejects_escaped_control_characters_and_unsafe_fallbacks() {
        assert!(matches!(
            parse_skill_metadata(
                "---\nname: \"unsafe\\0name\"\ndescription: Valid.\n---\n",
                "fallback"
            ),
            Err(SkillParseError::InvalidName(reason)) if reason.contains("U+0000")
        ));
        assert!(matches!(
            parse_skill_metadata(
                "---\nname: valid\ndescription: \"unsafe\\u001bdescription\"\n---\n",
                "fallback"
            ),
            Err(SkillParseError::InvalidDescription(reason)) if reason.contains("U+001B")
        ));
        assert!(matches!(
            parse_skill_metadata(
                "---\ndescription: Valid.\n---\n",
                "unsafe\u{0085}fallback"
            ),
            Err(SkillParseError::InvalidName(reason)) if reason.contains("U+0085")
        ));
    }

    #[test]
    fn enforces_frontmatter_limit_using_exact_crlf_byte_offsets() {
        let prefix = "description: Valid.\r\n";
        let padding_length = MAX_SKILL_FRONTMATTER_BYTES - prefix.len() - "#\r\n".len();
        let exact_frontmatter = format!("{prefix}#{}\r\n", "x".repeat(padding_length));
        assert_eq!(exact_frontmatter.len(), MAX_SKILL_FRONTMATTER_BYTES);

        let exact_document = format!("---\r\n{exact_frontmatter}---\r\n");
        assert!(parse_skill_metadata(&exact_document, "fallback").is_ok());

        let oversized_frontmatter = format!("{prefix}#{}\r\n", "x".repeat(padding_length + 1));
        let oversized_document = format!("---\r\n{oversized_frontmatter}---\r\n");
        assert!(matches!(
            parse_skill_metadata(&oversized_document, "fallback"),
            Err(SkillParseError::InvalidFrontmatter(reason))
                if reason.contains("exceeds 16384 bytes")
        ));
    }

    #[test]
    fn enforces_metadata_length_limits_in_characters() {
        let long_name = "名".repeat(MAX_SKILL_NAME_CHARS + 1);
        let name_document =
            format!("---\nname: {long_name}\ndescription: Valid description.\n---\n");
        assert!(matches!(
            parse_skill_metadata(&name_document, "fallback"),
            Err(SkillParseError::InvalidName(_))
        ));

        let long_description = "字".repeat(MAX_SKILL_DESCRIPTION_CHARS + 1);
        let description_document =
            format!("---\nname: valid\ndescription: {long_description}\n---\n");
        assert!(matches!(
            parse_skill_metadata(&description_document, "fallback"),
            Err(SkillParseError::InvalidDescription(_))
        ));
    }
}
