/// Retired model-visible tools are diagnosed in untrusted Skill instructions instead of being
/// rewritten or re-registered as aliases. Application-bundled Skills are checked at build time by
/// the FileChange source-boundary test and do not pass through this untrusted-source lint.
const UNSUPPORTED_MODEL_TOOL_REFERENCES: &[&str] = &["write_file"];

pub(super) fn unsupported_model_tool_reference(instructions: &str) -> Option<&'static str> {
    UNSUPPORTED_MODEL_TOOL_REFERENCES
        .iter()
        .copied()
        .find(|candidate| contains_identifier(instructions, candidate))
}

fn contains_identifier(source: &str, candidate: &str) -> bool {
    source.match_indices(candidate).any(|(start, _)| {
        let end = start + candidate.len();
        let before = source[..start].chars().next_back();
        let after = source[end..].chars().next();
        before.is_none_or(|character| !is_identifier_character(character))
            && after.is_none_or(|character| !is_identifier_character(character))
    })
}

fn is_identifier_character(character: char) -> bool {
    character == '_' || character.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_the_exact_retired_model_tool_identifier() {
        for source in [
            "Call write_file with the final content.",
            "Use `write_file` after reading the target.",
            "tool: write_file\n",
        ] {
            assert_eq!(unsupported_model_tool_reference(source), Some("write_file"));
        }

        for source in [
            "Use apply_patch.",
            "Call writeFile on the workbook API.",
            "The ordinary words write file are harmless.",
            "not_write_file",
            "write_file_v2",
        ] {
            assert_eq!(unsupported_model_tool_reference(source), None);
        }
    }
}
