#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JsonStringFieldEvent {
    Delta { field: String, value: String },
    Completed { field: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopLevelState {
    BeforeObject,
    ExpectKey,
    ExpectColon,
    ExpectValue,
    AfterValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StringRole {
    Key,
    Value(String),
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscapeState {
    None,
    Escaped,
    Unicode { digits: u8, value: u16 },
}

pub(crate) struct TopLevelJsonStringStream {
    depth: usize,
    nested_root_field: Option<String>,
    target_active: bool,
    target_depth: usize,
    state: TopLevelState,
    current_key: Option<String>,
    in_string: bool,
    string_role: StringRole,
    key_buffer: String,
    escape: EscapeState,
    pending_high_surrogate: Option<u16>,
}

impl Default for TopLevelJsonStringStream {
    fn default() -> Self {
        Self {
            depth: 0,
            nested_root_field: None,
            target_active: true,
            target_depth: 1,
            state: TopLevelState::BeforeObject,
            current_key: None,
            in_string: false,
            string_role: StringRole::Ignored,
            key_buffer: String::new(),
            escape: EscapeState::None,
            pending_high_surrogate: None,
        }
    }
}

impl TopLevelJsonStringStream {
    pub(crate) fn nested_object(root_field: &str) -> Self {
        Self {
            nested_root_field: Some(root_field.to_string()),
            target_active: false,
            target_depth: 2,
            ..Self::default()
        }
    }

    pub(crate) fn push(&mut self, input: &str) -> Vec<JsonStringFieldEvent> {
        let mut events = Vec::new();
        for ch in input.chars() {
            if self.in_string {
                self.push_string_char(ch, &mut events);
            } else {
                self.push_structural_char(ch);
            }
        }
        events
    }

    fn push_structural_char(&mut self, ch: char) {
        match ch {
            '"' => {
                self.in_string = true;
                self.escape = EscapeState::None;
                self.pending_high_surrogate = None;
                let parsing_depth = if self.target_active {
                    self.target_depth
                } else {
                    1
                };
                self.string_role = if self.depth == parsing_depth {
                    match self.state {
                        TopLevelState::ExpectKey => {
                            self.key_buffer.clear();
                            StringRole::Key
                        }
                        TopLevelState::ExpectValue if self.target_active => {
                            StringRole::Value(self.current_key.clone().unwrap_or_default())
                        }
                        _ => StringRole::Ignored,
                    }
                } else {
                    StringRole::Ignored
                };
            }
            '{' => {
                let entering_nested_target = !self.target_active
                    && self.depth == 1
                    && self.state == TopLevelState::ExpectValue
                    && self.current_key.as_deref() == self.nested_root_field.as_deref();
                self.depth = self.depth.saturating_add(1);
                if self.depth == 1 {
                    self.state = TopLevelState::ExpectKey;
                } else if entering_nested_target {
                    self.target_active = true;
                    self.state = TopLevelState::ExpectKey;
                    self.current_key = None;
                } else if self.target_active
                    && self.depth == self.target_depth.saturating_add(1)
                    && self.state == TopLevelState::ExpectValue
                {
                    self.state = TopLevelState::AfterValue;
                }
            }
            '[' => {
                self.depth = self.depth.saturating_add(1);
                if self.target_active
                    && self.depth == self.target_depth.saturating_add(1)
                    && self.state == TopLevelState::ExpectValue
                {
                    self.state = TopLevelState::AfterValue;
                }
            }
            '}' | ']' => {
                if self.target_active
                    && self.nested_root_field.is_some()
                    && self.depth == self.target_depth
                    && ch == '}'
                {
                    self.target_active = false;
                }
                self.depth = self.depth.saturating_sub(1);
                if self.depth == 0 {
                    self.state = TopLevelState::BeforeObject;
                    self.current_key = None;
                } else if !self.target_active && self.depth == 1 {
                    self.state = TopLevelState::AfterValue;
                    self.current_key = None;
                }
            }
            ':' if self.depth == self.parsing_depth()
                && self.state == TopLevelState::ExpectColon =>
            {
                self.state = TopLevelState::ExpectValue;
            }
            ',' if self.depth == self.parsing_depth() => {
                self.state = TopLevelState::ExpectKey;
                self.current_key = None;
            }
            value
                if self.depth == self.parsing_depth()
                    && self.state == TopLevelState::ExpectValue
                    && !value.is_whitespace() =>
            {
                self.state = TopLevelState::AfterValue;
            }
            _ => {}
        }
    }

    fn parsing_depth(&self) -> usize {
        if self.target_active {
            self.target_depth
        } else {
            1
        }
    }

    fn push_string_char(&mut self, ch: char, events: &mut Vec<JsonStringFieldEvent>) {
        match self.escape {
            EscapeState::None => match ch {
                '"' => self.finish_string(events),
                '\\' => self.escape = EscapeState::Escaped,
                value => {
                    self.flush_pending_surrogate(events);
                    self.emit_char(value, events);
                }
            },
            EscapeState::Escaped => {
                self.escape = if ch == 'u' {
                    EscapeState::Unicode {
                        digits: 0,
                        value: 0,
                    }
                } else {
                    self.flush_pending_surrogate(events);
                    let decoded = match ch {
                        '"' => '"',
                        '\\' => '\\',
                        '/' => '/',
                        'b' => '\u{0008}',
                        'f' => '\u{000c}',
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        _ => '\u{fffd}',
                    };
                    self.emit_char(decoded, events);
                    EscapeState::None
                };
            }
            EscapeState::Unicode { digits, value } => {
                let Some(digit) = ch.to_digit(16) else {
                    self.flush_pending_surrogate(events);
                    self.emit_char('\u{fffd}', events);
                    self.escape = EscapeState::None;
                    return;
                };
                let next_digits = digits.saturating_add(1);
                let next_value = (value << 4) | digit as u16;
                if next_digits < 4 {
                    self.escape = EscapeState::Unicode {
                        digits: next_digits,
                        value: next_value,
                    };
                } else {
                    self.escape = EscapeState::None;
                    self.emit_code_unit(next_value, events);
                }
            }
        }
    }

    fn finish_string(&mut self, events: &mut Vec<JsonStringFieldEvent>) {
        self.flush_pending_surrogate(events);
        self.in_string = false;
        self.escape = EscapeState::None;
        match std::mem::replace(&mut self.string_role, StringRole::Ignored) {
            StringRole::Key => {
                self.current_key = Some(std::mem::take(&mut self.key_buffer));
                self.state = TopLevelState::ExpectColon;
            }
            StringRole::Value(field) => {
                if !field.is_empty() {
                    events.push(JsonStringFieldEvent::Completed { field });
                }
                self.state = TopLevelState::AfterValue;
                self.current_key = None;
            }
            StringRole::Ignored => {}
        }
    }

    fn emit_code_unit(&mut self, value: u16, events: &mut Vec<JsonStringFieldEvent>) {
        if (0xd800..=0xdbff).contains(&value) {
            self.flush_pending_surrogate(events);
            self.pending_high_surrogate = Some(value);
            return;
        }

        if (0xdc00..=0xdfff).contains(&value) {
            let Some(high) = self.pending_high_surrogate.take() else {
                self.emit_char('\u{fffd}', events);
                return;
            };
            let scalar = 0x1_0000 + (((high as u32) - 0xd800) << 10) + ((value as u32) - 0xdc00);
            self.emit_char(char::from_u32(scalar).unwrap_or('\u{fffd}'), events);
            return;
        }

        self.flush_pending_surrogate(events);
        self.emit_char(char::from_u32(value as u32).unwrap_or('\u{fffd}'), events);
    }

    fn flush_pending_surrogate(&mut self, events: &mut Vec<JsonStringFieldEvent>) {
        if self.pending_high_surrogate.take().is_some() {
            self.emit_char('\u{fffd}', events);
        }
    }

    fn emit_char(&mut self, ch: char, events: &mut Vec<JsonStringFieldEvent>) {
        match &self.string_role {
            StringRole::Key => self.key_buffer.push(ch),
            StringRole::Value(field) if !field.is_empty() => {
                if let Some(JsonStringFieldEvent::Delta {
                    field: previous_field,
                    value,
                }) = events.last_mut()
                {
                    if previous_field == field {
                        value.push(ch);
                        return;
                    }
                }
                events.push(JsonStringFieldEvent::Delta {
                    field: field.clone(),
                    value: ch.to_string(),
                });
            }
            StringRole::Value(_) | StringRole::Ignored => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn collect_fields(chunks: &[&str]) -> (BTreeMap<String, String>, Vec<String>) {
        let mut parser = TopLevelJsonStringStream::default();
        let mut values = BTreeMap::<String, String>::new();
        let mut completed = Vec::new();
        for chunk in chunks {
            for event in parser.push(chunk) {
                match event {
                    JsonStringFieldEvent::Delta { field, value } => {
                        values.entry(field).or_default().push_str(&value);
                    }
                    JsonStringFieldEvent::Completed { field } => completed.push(field),
                }
            }
        }
        (values, completed)
    }

    #[test]
    fn decodes_fragmented_top_level_string_fields() {
        let (values, completed) = collect_fields(&[
            r#"{"phase":"app"#,
            r#"end","draftId":"draft-1","content":"a\n中\u6587\ud83d"#,
            r#"\ude80"}"#,
        ]);

        assert_eq!(values["phase"], "append");
        assert_eq!(values["draftId"], "draft-1");
        assert_eq!(values["content"], "a\n中文🚀");
        assert_eq!(completed, vec!["phase", "draftId", "content"]);
    }

    #[test]
    fn ignores_nested_string_fields_and_decodes_escapes() {
        let (values, completed) =
            collect_fields(&[r#"{"edits":[{"text":"ignore"}],"content":"\"quoted\"\\path"}"#]);

        assert_eq!(values.len(), 1);
        assert_eq!(values["content"], "\"quoted\"\\path");
        assert_eq!(completed, vec!["content"]);
    }

    #[test]
    fn nested_object_mode_streams_only_request_level_strings() {
        let mut parser = TopLevelJsonStringStream::nested_object("request");
        let mut values = BTreeMap::<String, String>::new();
        let mut completed = Vec::new();
        for chunk in [
            r#"{"request":{"action":"app"#,
            r#"end","transactionId":"txn-1","edits":[{"text":"PRIVATE_NESTED"}],"content":"中\u6587"#,
            r#"🚀"},"body":"PRIVATE_ROOT"}"#,
        ] {
            for event in parser.push(chunk) {
                match event {
                    JsonStringFieldEvent::Delta { field, value } => {
                        values.entry(field).or_default().push_str(&value);
                    }
                    JsonStringFieldEvent::Completed { field } => completed.push(field),
                }
            }
        }

        assert_eq!(values["action"], "append");
        assert_eq!(values["transactionId"], "txn-1");
        assert_eq!(values["content"], "中文🚀");
        assert!(!values.values().any(|value| value.contains("PRIVATE")));
        assert_eq!(completed, vec!["action", "transactionId", "content"]);
    }
}
