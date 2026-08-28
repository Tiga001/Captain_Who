use unicode_segmentation::UnicodeSegmentation;

pub const PROMPT_EXCERPT_MAX_GRAPHEMES: usize = 48;
pub const NOTIFICATION_SUBJECT_MAX_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationSubject {
    pub kind: &'static str,
    pub text: String,
}

/// Builds the only user-visible identity allowed in a normal HumanRoot notification.
///
/// Exact Composer attachment summaries are transport text rather than user-authored content, so
/// they are stripped. Attachment-only turns use a fixed identity that never exposes file names.
/// The prompt excerpt is bounded by both grapheme count and UTF-8 bytes because one grapheme can
/// contain an arbitrarily large emoji sequence.
pub fn human_root_notification_subject(
    content: &str,
    attachment_names: &[String],
) -> NotificationSubject {
    let visible = strip_composer_attachment_summary(content, attachment_names);
    let normalized = visible.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() && !attachment_names.is_empty() {
        return NotificationSubject {
            kind: "attachment_task",
            text: "附件任务".to_string(),
        };
    }

    NotificationSubject {
        kind: "prompt_excerpt",
        text: truncate_graphemes_and_bytes(if normalized.is_empty() {
            "任务"
        } else {
            &normalized
        }),
    }
}

fn strip_composer_attachment_summary<'a>(content: &'a str, attachment_names: &[String]) -> &'a str {
    if attachment_names.is_empty() {
        return content;
    }

    let summaries = [
        format!("Attachments: {}", attachment_names.join(", ")),
        format!("附件：{}", attachment_names.join("、")),
    ];
    for summary in &summaries {
        if content == summary {
            return "";
        }
        let suffix = format!("\n\n{summary}");
        if let Some(visible) = content.strip_suffix(&suffix) {
            return visible;
        }
    }
    content
}

fn truncate_graphemes_and_bytes(value: &str) -> String {
    const ELLIPSIS: &str = "…";
    let mut prefix = String::new();
    let mut graphemes = value.graphemes(true).peekable();
    let mut count = 0;
    while let Some(grapheme) = graphemes.next() {
        let has_more = graphemes.peek().is_some();
        if has_more && count >= PROMPT_EXCERPT_MAX_GRAPHEMES.saturating_sub(1) {
            prefix.push_str(ELLIPSIS);
            return prefix;
        }
        let byte_budget = if has_more {
            NOTIFICATION_SUBJECT_MAX_BYTES.saturating_sub(ELLIPSIS.len())
        } else {
            NOTIFICATION_SUBJECT_MAX_BYTES
        };
        if count >= PROMPT_EXCERPT_MAX_GRAPHEMES
            || prefix.len().saturating_add(grapheme.len()) > byte_budget
        {
            prefix.push_str(ELLIPSIS);
            return prefix;
        }
        prefix.push_str(grapheme);
        count += 1;
    }
    prefix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_large_emoji_graphemes_by_bytes_and_clusters() {
        let family = "👨‍👩‍👧‍👦";
        let oversized_cluster = family.repeat(80);
        let subject = human_root_notification_subject(&oversized_cluster, &[]);
        assert!(subject.text.ends_with('…'));
        assert!(subject.text.len() <= NOTIFICATION_SUBJECT_MAX_BYTES);
        assert!(subject.text.graphemes(true).count() <= PROMPT_EXCERPT_MAX_GRAPHEMES);
    }
}
