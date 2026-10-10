//! Minimal Markdown framing. Only reserved headings and labels delimit template data.
//! Escaping is reversible and leaves fenced code, inline Markdown and paragraphs untouched.
use super::*;

pub(super) struct Cursor<'a> {
    lines: Vec<&'a str>,
    fence_ends: Vec<Option<usize>>,
    position: usize,
    newline: &'static str,
}

impl<'a> Cursor<'a> {
    pub(super) fn new(markdown: &'a str) -> Self {
        let lines: Vec<_> = markdown.split_inclusive('\n').collect();
        let newline = if lines.first().is_some_and(|line| line.ends_with("\r\n")) {
            "\r\n"
        } else {
            "\n"
        };
        let fence_ends = fence_ends(&lines);
        Self {
            lines,
            fence_ends,
            position: 0,
            newline,
        }
    }

    pub(super) fn next(&mut self) -> Option<&'a str> {
        while let Some(line) = self.lines.get(self.position) {
            self.position += 1;
            let content = line_content(line);
            if !content.trim().is_empty() {
                return Some(content);
            }
        }
        None
    }

    pub(super) fn expect(&mut self, expected: &str) -> Result<(), String> {
        if self.next() == Some(expected) {
            Ok(())
        } else {
            Err(INVALID_FORMAT.into())
        }
    }

    pub(super) fn body(&mut self) -> String {
        let mut body = String::new();
        while let Some(raw) = self.lines.get(self.position).copied() {
            let content = line_content(raw);
            if reserved(content) {
                break;
            }
            if let Some(end) = self.fence_ends[self.position] {
                for line in &self.lines[self.position..=end] {
                    body.push_str(line);
                }
                self.position = end + 1;
            } else {
                body.push_str(
                    if content.starts_with('\\') && escapable(content.trim_start_matches('\\')) {
                        &raw[1..]
                    } else {
                        raw
                    },
                );
                self.position += 1;
            }
        }
        // The writer supplies one blank line after the heading and one after the body.
        // Remove just those separators, retaining additional user-owned whitespace.
        let body = body.strip_prefix(self.newline).unwrap_or(&body);
        let body = body.strip_suffix(self.newline).unwrap_or(body);
        body.strip_suffix(self.newline).unwrap_or(body).to_owned()
    }
}

pub(super) fn write_body(markdown: &mut String, heading: &str, body: &str) {
    writeln!(markdown, "{heading}\n").unwrap();
    if body.is_empty() {
        return;
    }
    let lines: Vec<_> = body.split_inclusive('\n').collect();
    let fence_ends = fence_ends(&lines);
    let mut position = 0;
    while position < lines.len() {
        if let Some(end) = fence_ends[position] {
            for line in &lines[position..=end] {
                markdown.push_str(line);
            }
            position = end + 1;
            continue;
        }
        let raw = lines[position];
        if escapable(line_content(raw).trim_start_matches('\\')) {
            markdown.push('\\');
        }
        markdown.push_str(raw);
        position += 1;
    }
    markdown.push_str("\n\n");
}

fn line_content(raw: &str) -> &str {
    raw.strip_suffix('\n')
        .unwrap_or(raw)
        .strip_suffix('\r')
        .unwrap_or_else(|| raw.strip_suffix('\n').unwrap_or(raw))
}

fn reserved(line: &str) -> bool {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() <= 3 {
        let hashes = trimmed.bytes().take_while(|byte| *byte == b'#').count();
        if (1..=3).contains(&hashes)
            && (trimmed.len() == hashes || trimmed[hashes..].starts_with([' ', '\t']))
        {
            return true;
        }
    }
    // Recognize every supported vocabulary, including when the current document uses
    // another language. This keeps a later locale switch reversible and rejects mixed
    // structural fields instead of silently swallowing them into authored prose.
    language::LANGUAGES.iter().any(|language| {
        [language.receives, language.task, language.delivers].contains(&line)
            || line.starts_with(language.rank)
            || line.starts_with(language.role)
    })
}

fn escapable(line: &str) -> bool {
    reserved(line) || fence_start(line).is_some()
}

fn fence_start(line: &str) -> Option<(u8, usize)> {
    let content = line.trim_start_matches(' ');
    if line.len() - content.len() > 3 {
        return None;
    }
    let marker = *content.as_bytes().first()?;
    if !matches!(marker, b'`' | b'~') {
        return None;
    }
    let count = content.bytes().take_while(|byte| *byte == marker).count();
    if count < 3 || (marker == b'`' && content[count..].contains('`')) {
        return None;
    }
    Some((marker, count))
}

fn fence_ends(lines: &[&str]) -> Vec<Option<usize>> {
    let mut ends = vec![None; lines.len()];
    let mut closers = [
        std::collections::BTreeMap::new(),
        std::collections::BTreeMap::new(),
    ];
    // Keep only closers longer than every nearer fence. Each range lookup then finds
    // the nearest qualifying closer in O(log n), even for many unmatched openers.
    for (index, line) in lines.iter().enumerate().rev() {
        let line = line_content(line);
        let Some((marker, count)) = fence_start(line) else {
            continue;
        };
        let closers = &mut closers[usize::from(marker == b'~')];
        ends[index] = closers.range(count..).next().map(|(_, index)| *index);
        let content = line.trim_start_matches(' ');
        if content[count..].trim_matches([' ', '\t']).is_empty() {
            while closers
                .first_key_value()
                .is_some_and(|(length, _)| *length <= count)
            {
                closers.pop_first();
            }
            closers.insert(count, index);
        }
    }
    ends
}

pub(super) fn encode_name(name: &str) -> String {
    let mut result = String::new();
    for ch in name.chars() {
        if ch == '&' {
            result.push_str("&amp;");
        } else if ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}') {
            write!(result, "&#x{:X};", ch as u32).unwrap();
        } else {
            // Names occur inside headings, so dots, hyphens, parentheses and other
            // ordinary punctuation need no escaping. Protect only inline Markdown
            // syntax, backslashes, HTML delimiters and heading-closing hashes.
            if matches!(ch, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#') {
                result.push('\\');
            }
            result.push(ch);
        }
    }
    result
}

pub(super) fn decode_name(name: &str) -> String {
    let mut result = String::new();
    let mut remaining = name;
    while !remaining.is_empty() {
        if let Some(after) = remaining.strip_prefix('\\') {
            if let Some(ch) = after.chars().next().filter(char::is_ascii_punctuation) {
                result.push(ch);
                remaining = &after[ch.len_utf8()..];
                continue;
            }
        }
        if let Some(after) = remaining.strip_prefix("&amp;") {
            result.push('&');
            remaining = after;
            continue;
        }
        if let Some(after) = remaining.strip_prefix("&#") {
            // Numeric character references are short. Bound the scan so a malformed
            // name containing many "&#" prefixes cannot cause quadratic work.
            if let Some(end) = after.bytes().take(12).position(|byte| byte == b';') {
                let (number, rest) = (&after[..end], &after[end + 1..]);
                let value = if let Some(hex) = number
                    .strip_prefix('x')
                    .or_else(|| number.strip_prefix('X'))
                {
                    u32::from_str_radix(hex, 16).ok()
                } else {
                    number.parse().ok()
                };
                if let Some(ch) = value.and_then(char::from_u32) {
                    result.push(ch);
                    remaining = rest;
                    continue;
                }
            }
        }
        let ch = remaining.chars().next().unwrap();
        result.push(ch);
        remaining = &remaining[ch.len_utf8()..];
    }
    result
}
