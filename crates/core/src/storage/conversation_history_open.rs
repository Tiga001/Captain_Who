//! Opaque, versioned routes used by the `conversation_history` tool.
//!
//! The encoded value is deliberately not a public persistence format. Keeping the codec beside
//! conversation storage gives archive continuations and conversation forks one authoritative way
//! to create, validate, decode, and remap locations without teaching callers about the wire shape.

use super::conversation_history_repository::ConversationHistoryRecordRef;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub(crate) const HISTORY_OPEN_PREFIX: &str = "hist_v1_";
pub(crate) const HISTORY_OPEN_MAX_BYTES: usize = 8 * 1024;
pub(crate) const HISTORY_QUERY_MAX_CHARS: usize = 500;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HistoryTurnPageDirection {
    Latest,
    Older,
    Newer,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum HistoryOpenRoute {
    TurnPage {
        anchor_turn_id: Option<String>,
        direction: HistoryTurnPageDirection,
    },
    Search {
        query: String,
        after_turn_id: Option<String>,
    },
    Turn {
        turn_id: String,
        after: Option<ConversationHistoryRecordRef>,
    },
    Around {
        reference: ConversationHistoryRecordRef,
    },
    AroundPage {
        reference: ConversationHistoryRecordRef,
        after: Option<ConversationHistoryRecordRef>,
    },
    Record {
        reference: ConversationHistoryRecordRef,
        start_char: u64,
    },
    ToolExchange {
        reference: ConversationHistoryRecordRef,
    },
    ToolExchangePage {
        reference: ConversationHistoryRecordRef,
        after: Option<ConversationHistoryRecordRef>,
    },
    Archive {
        archive_ref: String,
        start_char: u64,
    },
    ArchiveMatch {
        archive_ref: String,
        query: String,
    },
}

pub(crate) fn encode_history_open(route: &HistoryOpenRoute) -> Result<String, String> {
    validate_history_open_route(route)?;
    let bytes = serde_json::to_vec(route).map_err(|error| format!("无法生成历史位置：{error}"))?;
    Ok(format!(
        "{HISTORY_OPEN_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    ))
}

pub(crate) fn encode_archive_history_open(
    archive_ref: impl Into<String>,
    start_char: u64,
) -> Result<String, String> {
    encode_history_open(&HistoryOpenRoute::Archive {
        archive_ref: archive_ref.into(),
        start_char,
    })
}

pub(crate) fn decode_history_open(value: &str) -> Result<HistoryOpenRoute, String> {
    if value.len() > HISTORY_OPEN_MAX_BYTES {
        return Err("conversation_history.open 过长。".to_string());
    }
    let encoded = value
        .strip_prefix(HISTORY_OPEN_PREFIX)
        .ok_or_else(|| "conversation_history.open 不是有效的 hist_v1_ 位置。".to_string())?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| "conversation_history.open 无法解码。".to_string())?;
    let route = serde_json::from_slice::<HistoryOpenRoute>(&bytes)
        .map_err(|_| "conversation_history.open 内容无效。".to_string())?;
    validate_history_open_route(&route)?;
    Ok(route)
}

/// Decodes one exact opaque location, remaps every conversation-bound identity, and re-encodes it.
///
/// `Ok(None)` means the value is not a valid `hist_v1_` route or none of its identities belong to
/// the supplied fork map. A partially mapped route is rejected so a fork cannot persist a location
/// that mixes source and target conversation identities.
pub(crate) fn remap_history_open(
    value: &str,
    replacements: &HashMap<String, String>,
) -> Result<Option<String>, String> {
    if !value.starts_with(HISTORY_OPEN_PREFIX) {
        return Ok(None);
    }
    let Ok(mut route) = decode_history_open(value) else {
        // User-authored text may happen to begin with the reserved prefix. Only routes previously
        // emitted by the codec participate in fork rewriting.
        return Ok(None);
    };
    let mut identities = 0_usize;
    let mut remapped = 0_usize;
    visit_route_identities_mut(&mut route, &mut |identity| {
        identities = identities.saturating_add(1);
        if let Some(replacement) = replacements.get(identity) {
            *identity = replacement.clone();
            remapped = remapped.saturating_add(1);
        }
    });
    if remapped == 0 {
        return Ok(None);
    }
    if remapped != identities {
        return Err(
            "conversation_history.open 在 fork 时只能映射部分历史身份，已拒绝生成悬空位置。"
                .to_string(),
        );
    }
    encode_history_open(&route).map(Some)
}

fn visit_route_identities_mut(route: &mut HistoryOpenRoute, visit: &mut impl FnMut(&mut String)) {
    match route {
        HistoryOpenRoute::TurnPage { anchor_turn_id, .. } => {
            if let Some(anchor_turn_id) = anchor_turn_id {
                visit(anchor_turn_id);
            }
        }
        HistoryOpenRoute::Search { after_turn_id, .. } => {
            if let Some(after_turn_id) = after_turn_id {
                visit(after_turn_id);
            }
        }
        HistoryOpenRoute::Turn { turn_id, after, .. } => {
            visit(turn_id);
            if let Some(after) = after {
                visit_record_ref_identities_mut(after, visit);
            }
        }
        HistoryOpenRoute::Around { reference } => {
            visit_record_ref_identities_mut(reference, visit);
        }
        HistoryOpenRoute::AroundPage { reference, after } => {
            visit_record_ref_identities_mut(reference, visit);
            if let Some(after) = after {
                visit_record_ref_identities_mut(after, visit);
            }
        }
        HistoryOpenRoute::Record { reference, .. } => {
            visit_record_ref_identities_mut(reference, visit);
        }
        HistoryOpenRoute::ToolExchange { reference } => {
            visit_record_ref_identities_mut(reference, visit);
        }
        HistoryOpenRoute::ToolExchangePage { reference, after } => {
            visit_record_ref_identities_mut(reference, visit);
            if let Some(after) = after {
                visit_record_ref_identities_mut(after, visit);
            }
        }
        HistoryOpenRoute::Archive { archive_ref, .. }
        | HistoryOpenRoute::ArchiveMatch { archive_ref, .. } => visit(archive_ref),
    }
}

fn visit_record_ref_identities_mut(
    reference: &mut ConversationHistoryRecordRef,
    visit: &mut impl FnMut(&mut String),
) {
    match reference {
        ConversationHistoryRecordRef::Message { message_id } => visit(message_id),
        ConversationHistoryRecordRef::TraceItem {
            assistant_message_id,
            ..
        } => visit(assistant_message_id),
        ConversationHistoryRecordRef::Archive { archive_ref } => visit(archive_ref),
    }
}

fn validate_history_open_route(route: &HistoryOpenRoute) -> Result<(), String> {
    let valid_identity = |value: &str| !value.trim().is_empty() && value.len() <= 1_024;
    let valid_ref = |reference: &ConversationHistoryRecordRef| match reference {
        ConversationHistoryRecordRef::Message { message_id } => valid_identity(message_id),
        ConversationHistoryRecordRef::TraceItem {
            assistant_message_id,
            ..
        } => valid_identity(assistant_message_id),
        ConversationHistoryRecordRef::Archive { archive_ref } => valid_identity(archive_ref),
    };
    let valid = match route {
        HistoryOpenRoute::TurnPage { anchor_turn_id, .. } => {
            anchor_turn_id.as_deref().is_none_or(valid_identity)
        }
        HistoryOpenRoute::Search {
            query,
            after_turn_id,
        } => {
            !query.trim().is_empty()
                && query.chars().count() <= HISTORY_QUERY_MAX_CHARS
                && after_turn_id.as_deref().is_none_or(valid_identity)
        }
        HistoryOpenRoute::Turn { turn_id, after, .. } => {
            valid_identity(turn_id) && after.as_ref().is_none_or(valid_ref)
        }
        HistoryOpenRoute::Around { reference } => valid_ref(reference),
        HistoryOpenRoute::AroundPage { reference, after } => {
            valid_ref(reference) && after.as_ref().is_none_or(valid_ref)
        }
        HistoryOpenRoute::Record { reference, .. } => valid_ref(reference),
        HistoryOpenRoute::ToolExchange { reference } => valid_ref(reference),
        HistoryOpenRoute::ToolExchangePage { reference, after } => {
            valid_ref(reference) && after.as_ref().is_none_or(valid_ref)
        }
        HistoryOpenRoute::Archive { archive_ref, .. } => valid_identity(archive_ref),
        HistoryOpenRoute::ArchiveMatch { archive_ref, query } => {
            valid_identity(archive_ref)
                && !query.trim().is_empty()
                && query.chars().count() <= HISTORY_QUERY_MAX_CHARS
        }
    };
    if valid {
        Ok(())
    } else {
        Err("conversation_history.open 包含无效或过长的历史身份。".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_continuation_round_trips() {
        let open = encode_archive_history_open("archive-1", 42).unwrap();

        assert_eq!(
            decode_history_open(&open).unwrap(),
            HistoryOpenRoute::Archive {
                archive_ref: "archive-1".to_string(),
                start_char: 42,
            }
        );
    }

    #[test]
    fn fork_remap_rewrites_every_identity_and_rejects_partial_routes() {
        let open = encode_history_open(&HistoryOpenRoute::Turn {
            turn_id: "user-1".to_string(),
            after: Some(ConversationHistoryRecordRef::TraceItem {
                assistant_message_id: "assistant-1".to_string(),
                sequence: 7,
            }),
        })
        .unwrap();
        let complete = HashMap::from([
            ("user-1".to_string(), "forked-user".to_string()),
            ("assistant-1".to_string(), "forked-assistant".to_string()),
        ]);
        let remapped = remap_history_open(&open, &complete).unwrap().unwrap();

        assert_eq!(
            decode_history_open(&remapped).unwrap(),
            HistoryOpenRoute::Turn {
                turn_id: "forked-user".to_string(),
                after: Some(ConversationHistoryRecordRef::TraceItem {
                    assistant_message_id: "forked-assistant".to_string(),
                    sequence: 7,
                }),
            }
        );
        assert!(remap_history_open(
            &open,
            &HashMap::from([("user-1".to_string(), "forked-user".to_string())])
        )
        .is_err());

        let search = encode_history_open(&HistoryOpenRoute::Search {
            query: "needle".to_string(),
            after_turn_id: Some("user-1".to_string()),
        })
        .unwrap();
        let remapped = remap_history_open(&search, &complete).unwrap().unwrap();
        assert_eq!(
            decode_history_open(&remapped).unwrap(),
            HistoryOpenRoute::Search {
                query: "needle".to_string(),
                after_turn_id: Some("forked-user".to_string()),
            }
        );
    }

    #[test]
    fn malformed_user_text_with_reserved_prefix_is_not_rewritten() {
        assert_eq!(
            remap_history_open(
                "hist_v1_not-a-real-route",
                &HashMap::from([("source".to_string(), "target".to_string())]),
            )
            .unwrap(),
            None
        );
    }
}
