use crate::storage::models::{ChatSearchInput, ChatSearchMatchKind, ChatSearchResult};
use rusqlite::{params, Connection};
use std::collections::HashSet;

const DEFAULT_SEARCH_LIMIT: u32 = 100;
const MAX_SEARCH_LIMIT: u32 = 200;
const SNIPPET_CONTEXT_BEFORE: usize = 16;
const SNIPPET_CONTEXT_AFTER: usize = 54;

#[derive(Debug)]
struct MessageHit {
    conversation_id: String,
    project_id: Option<String>,
    title: String,
    updated_at: i64,
    message_id: String,
    content: String,
    relevance: f64,
    title_match_quality: i64,
    position: i64,
}

#[derive(Debug)]
struct TitleHit {
    conversation_id: String,
    project_id: Option<String>,
    title: String,
    updated_at: i64,
    relevance: f64,
    title_match_quality: i64,
}

#[derive(Debug)]
struct RankedSearchResult {
    result: ChatSearchResult,
    relevance: f64,
    title_match_quality: i64,
    position: i64,
}

pub fn search_chats(
    connection: &Connection,
    input: &ChatSearchInput,
) -> rusqlite::Result<Vec<ChatSearchResult>> {
    let query = input.query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let limit = input
        .limit
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .clamp(1, MAX_SEARCH_LIMIT) as usize;
    let message_hits = query_message_hits(connection, query, limit)?;
    let message_hit_conversation_ids = message_hits
        .iter()
        .map(|hit| hit.conversation_id.clone())
        .collect::<HashSet<_>>();
    let title_hits = query_title_hits(connection, query, limit)?;

    let mut ranked_results = Vec::with_capacity(limit);

    for hit in message_hits {
        ranked_results.push(RankedSearchResult {
            result: ChatSearchResult {
                conversation_id: hit.conversation_id,
                project_id: hit.project_id,
                title: hit.title,
                message_id: Some(hit.message_id),
                snippet: Some(make_snippet(&hit.content, query)),
                match_kind: ChatSearchMatchKind::Message,
                updated_at: hit.updated_at,
            },
            relevance: hit.relevance,
            title_match_quality: hit.title_match_quality,
            position: hit.position,
        });
    }

    for hit in title_hits {
        if message_hit_conversation_ids.contains(&hit.conversation_id) {
            continue;
        }

        ranked_results.push(RankedSearchResult {
            result: ChatSearchResult {
                conversation_id: hit.conversation_id,
                project_id: hit.project_id,
                title: hit.title,
                message_id: None,
                snippet: None,
                match_kind: ChatSearchMatchKind::Title,
                updated_at: hit.updated_at,
            },
            relevance: hit.relevance,
            title_match_quality: hit.title_match_quality,
            position: i64::MIN,
        });
    }

    ranked_results.sort_by(|left, right| {
        left.title_match_quality
            .cmp(&right.title_match_quality)
            .then_with(|| left.relevance.total_cmp(&right.relevance))
            .then_with(|| right.result.updated_at.cmp(&left.result.updated_at))
            .then_with(
                || match (&left.result.message_id, &right.result.message_id) {
                    (None, Some(_)) => std::cmp::Ordering::Less,
                    (Some(_), None) => std::cmp::Ordering::Greater,
                    _ => std::cmp::Ordering::Equal,
                },
            )
            .then_with(|| left.position.cmp(&right.position))
            .then_with(|| {
                left.result
                    .conversation_id
                    .cmp(&right.result.conversation_id)
            })
            .then_with(
                || match (&left.result.message_id, &right.result.message_id) {
                    (Some(left), Some(right)) => left.cmp(right),
                    (None, Some(_)) => std::cmp::Ordering::Less,
                    (Some(_), None) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                },
            )
    });
    ranked_results.truncate(limit);
    Ok(ranked_results
        .into_iter()
        .map(|ranked| ranked.result)
        .collect())
}

fn query_message_hits(
    connection: &Connection,
    query: &str,
    limit: usize,
) -> rusqlite::Result<Vec<MessageHit>> {
    let query_lower = query.to_lowercase();
    let prefix_pattern = like_prefix_pattern(query);
    let contains_pattern = like_contains_pattern(query);
    if should_use_fts(query) {
        query_message_hits_fts(
            connection,
            &fts_phrase(query),
            &query_lower,
            &prefix_pattern,
            &contains_pattern,
            limit,
        )
    } else {
        query_message_hits_like(
            connection,
            &contains_pattern,
            &query_lower,
            &prefix_pattern,
            limit,
        )
    }
}

fn query_message_hits_fts(
    connection: &Connection,
    fts_query: &str,
    query_lower: &str,
    prefix_pattern: &str,
    contains_pattern: &str,
    limit: usize,
) -> rusqlite::Result<Vec<MessageHit>> {
    let mut statement = connection.prepare(
        "
        SELECT
            c.id,
            c.project_id,
            c.title,
            c.updated_at,
            m.id,
            m.content,
            bm25(conversation_history_fts),
            CASE
                WHEN lower(c.title) = ?2 THEN 0
                WHEN lower(c.title) LIKE ?3 ESCAPE '\\' THEN 1
                WHEN lower(c.title) LIKE ?4 ESCAPE '\\' THEN 2
                ELSE 3
            END,
            m.position
        FROM conversation_history_fts
        JOIN messages m ON m.id = conversation_history_fts.message_id
        JOIN conversations c ON c.id = m.conversation_id
        WHERE conversation_history_fts MATCH ?1
          AND conversation_history_fts.record_type = 'message'
          AND c.archived_at IS NULL
          AND NOT EXISTS (
              SELECT 1 FROM agent_nodes hidden_agent
              WHERE hidden_agent.conversation_id = c.id
                AND hidden_agent.parent_agent_id IS NOT NULL
          )
          AND NOT (
              m.input_origin_kind IS 'agent'
              OR (
                  m.input_origin_kind IS 'snapshot'
                  AND m.snapshot_original_origin_kind IS 'agent'
              )
          )
          AND NOT EXISTS (
              SELECT 1
              FROM conversation_turn_rewrites AS rewrite
              WHERE rewrite.conversation_id = m.conversation_id
                AND (
                    rewrite.source_user_message_id = m.id
                    OR rewrite.source_assistant_message_id = m.id
                )
          )
          AND m.content <> ''
          -- FTS narrows the candidate set, while this legacy predicate keeps the pre-existing
          -- Unicode/case matching contract exactly stable for Renderer-visible results.
          AND lower(m.content) LIKE ?4 ESCAPE '\\'
        ORDER BY
            8 ASC,
            bm25(conversation_history_fts) ASC,
            c.updated_at DESC,
            m.position ASC,
            m.created_at ASC
        LIMIT ?5
        ",
    )?;

    let hits = statement
        .query_map(
            params![
                fts_query,
                query_lower,
                prefix_pattern,
                contains_pattern,
                limit as i64
            ],
            message_hit_from_row,
        )?
        .collect();
    hits
}

fn query_message_hits_like(
    connection: &Connection,
    contains_pattern: &str,
    query_lower: &str,
    prefix_pattern: &str,
    limit: usize,
) -> rusqlite::Result<Vec<MessageHit>> {
    let mut statement = connection.prepare(
        "
        SELECT
            c.id,
            c.project_id,
            c.title,
            c.updated_at,
            m.id,
            m.content,
            0.0,
            CASE
                WHEN lower(c.title) = ?2 THEN 0
                WHEN lower(c.title) LIKE ?3 ESCAPE '\\' THEN 1
                WHEN lower(c.title) LIKE ?1 ESCAPE '\\' THEN 2
                ELSE 3
            END,
            m.position
        FROM messages m
        JOIN conversations c ON c.id = m.conversation_id
        WHERE c.archived_at IS NULL
          AND NOT EXISTS (
              SELECT 1 FROM agent_nodes hidden_agent
              WHERE hidden_agent.conversation_id = c.id
                AND hidden_agent.parent_agent_id IS NOT NULL
          )
          AND NOT (
              m.input_origin_kind IS 'agent'
              OR (
                  m.input_origin_kind IS 'snapshot'
                  AND m.snapshot_original_origin_kind IS 'agent'
              )
          )
          AND NOT EXISTS (
              SELECT 1
              FROM conversation_turn_rewrites AS rewrite
              WHERE rewrite.conversation_id = m.conversation_id
                AND (
                    rewrite.source_user_message_id = m.id
                    OR rewrite.source_assistant_message_id = m.id
                )
          )
          AND m.content <> ''
          AND lower(m.content) LIKE ?1 ESCAPE '\\'
        ORDER BY 8 ASC, c.updated_at DESC, m.position ASC, m.created_at ASC
        LIMIT ?4
        ",
    )?;

    let hits = statement
        .query_map(
            params![contains_pattern, query_lower, prefix_pattern, limit as i64],
            message_hit_from_row,
        )?
        .collect();
    hits
}

fn message_hit_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageHit> {
    Ok(MessageHit {
        conversation_id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        updated_at: row.get(3)?,
        message_id: row.get(4)?,
        content: row.get(5)?,
        relevance: row.get(6)?,
        title_match_quality: row.get(7)?,
        position: row.get(8)?,
    })
}

fn query_title_hits(
    connection: &Connection,
    query: &str,
    limit: usize,
) -> rusqlite::Result<Vec<TitleHit>> {
    let query_lower = query.to_lowercase();
    let prefix_pattern = like_prefix_pattern(query);
    let contains_pattern = like_contains_pattern(query);
    let mut statement = connection.prepare(
        "
        SELECT
            c.id,
            c.project_id,
            c.title,
            c.updated_at,
            0.0,
            CASE
                WHEN lower(c.title) = ?2 THEN 0
                WHEN lower(c.title) LIKE ?3 ESCAPE '\\' THEN 1
                ELSE 2
            END
        FROM conversations c
        WHERE c.archived_at IS NULL
          AND NOT EXISTS (
              SELECT 1 FROM agent_nodes hidden_agent
              WHERE hidden_agent.conversation_id = c.id
                AND hidden_agent.parent_agent_id IS NOT NULL
          )
          AND lower(c.title) LIKE ?1 ESCAPE '\\'
        ORDER BY 6 ASC, c.updated_at DESC
        LIMIT ?4
        ",
    )?;

    let hits = statement
        .query_map(
            params![contains_pattern, query_lower, prefix_pattern, limit as i64],
            title_hit_from_row,
        )?
        .collect();
    hits
}

fn title_hit_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TitleHit> {
    Ok(TitleHit {
        conversation_id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        updated_at: row.get(3)?,
        relevance: row.get(4)?,
        title_match_quality: row.get(5)?,
    })
}

fn should_use_fts(query: &str) -> bool {
    query.chars().count() >= 3
}

fn fts_phrase(query: &str) -> String {
    format!("\"{}\"", query.replace('"', "\"\""))
}

fn like_prefix_pattern(query: &str) -> String {
    let mut pattern = escape_like_fragment(query);
    pattern.push('%');
    pattern
}

fn like_contains_pattern(query: &str) -> String {
    let escaped = escape_like_fragment(query);
    let mut pattern = String::with_capacity(escaped.len() + 2);
    pattern.push('%');
    pattern.push_str(&escaped);
    pattern.push('%');
    pattern
}

fn escape_like_fragment(query: &str) -> String {
    let mut escaped = String::with_capacity(query.len());
    for character in query.to_lowercase().chars() {
        match character {
            '%' | '_' | '\\' => {
                escaped.push('\\');
                escaped.push(character);
            }
            _ => escaped.push(character),
        }
    }
    escaped
}

fn make_snippet(content: &str, query: &str) -> String {
    let normalized = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return String::new();
    }

    let text = normalized.chars().collect::<Vec<_>>();
    let text_lower = normalized.to_lowercase().chars().collect::<Vec<_>>();
    let query_lower = query.to_lowercase().chars().collect::<Vec<_>>();
    let match_start = find_char_match(&text_lower, &query_lower).unwrap_or(0);
    let match_end = (match_start + query_lower.len()).min(text.len());
    let snippet_start = match_start.saturating_sub(SNIPPET_CONTEXT_BEFORE);
    let snippet_end = (match_end + SNIPPET_CONTEXT_AFTER).min(text.len());

    let mut snippet = String::new();
    if snippet_start > 0 {
        snippet.push('…');
    }
    snippet.extend(text[snippet_start..snippet_end].iter());
    if snippet_end < text.len() {
        snippet.push('…');
    }
    snippet
}

fn find_char_match(haystack: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }

    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;

    fn in_memory_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        connection
    }

    fn insert_conversation(
        connection: &Connection,
        id: &str,
        project_id: Option<&str>,
        title: &str,
        updated_at: i64,
        archived_at: Option<i64>,
    ) {
        connection
            .execute(
                "
                INSERT INTO conversations (
                    id,
                    project_id,
                    model_id,
                    title,
                    created_at,
                    updated_at,
                    archived_at
                )
                VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6)
                ",
                params![id, project_id, title, updated_at, updated_at, archived_at],
            )
            .unwrap();
    }

    fn insert_message(
        connection: &Connection,
        conversation_id: &str,
        id: &str,
        content: &str,
        position: i64,
    ) {
        connection
            .execute(
                "
                INSERT INTO messages (
                    id,
                    conversation_id,
                    role,
                    content,
                    created_at,
                    position
                )
                VALUES (?1, ?2, 'user', ?3, ?4, ?5)
                ",
                params![id, conversation_id, content, position, position],
            )
            .unwrap();
    }

    #[test]
    fn searches_message_content_and_title_only_results() {
        let connection = in_memory_connection();
        insert_conversation(
            &connection,
            "conversation-1",
            Some("project-1"),
            "开发计划",
            20,
            None,
        );
        insert_conversation(
            &connection,
            "conversation-2",
            Some("project-1"),
            "普通标题",
            30,
            None,
        );
        insert_conversation(&connection, "conversation-3", None, "开发标题", 10, None);
        insert_conversation(
            &connection,
            "conversation-4",
            None,
            "开发归档",
            40,
            Some(50),
        );
        insert_message(
            &connection,
            "conversation-1",
            "message-1",
            "这里有开发内容，需要显示片段",
            0,
        );
        insert_message(
            &connection,
            "conversation-2",
            "message-2",
            "这条开发内容来自最近更新的对话",
            0,
        );
        insert_message(
            &connection,
            "conversation-4",
            "message-4",
            "开发归档内容",
            0,
        );

        let results = search_chats(
            &connection,
            &ChatSearchInput {
                query: "开发".to_string(),
                limit: Some(10),
            },
        )
        .unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].conversation_id, "conversation-1");
        assert_eq!(results[0].message_id.as_deref(), Some("message-1"));
        assert_eq!(results[0].match_kind, ChatSearchMatchKind::Message);
        assert!(results[0].snippet.as_deref().unwrap().contains("开发"));
        assert_eq!(results[1].conversation_id, "conversation-3");
        assert_eq!(results[1].message_id, None);
        assert_eq!(results[1].snippet, None);
        assert_eq!(results[1].match_kind, ChatSearchMatchKind::Title);
        assert_eq!(results[2].conversation_id, "conversation-2");
        assert_eq!(results[2].message_id.as_deref(), Some("message-2"));
    }

    #[test]
    fn escapes_like_wildcards_in_short_query_fallback() {
        let connection = in_memory_connection();
        insert_conversation(
            &connection,
            "conversation-1",
            None,
            "ordinary one",
            10,
            None,
        );
        insert_conversation(
            &connection,
            "conversation-2",
            None,
            "ordinary two",
            20,
            None,
        );
        insert_message(
            &connection,
            "conversation-1",
            "message-1",
            "literal 100% marker",
            0,
        );
        insert_message(
            &connection,
            "conversation-2",
            "message-2",
            "literal 100x marker",
            0,
        );

        let results = search_chats(
            &connection,
            &ChatSearchInput {
                query: "%".to_string(),
                limit: Some(10),
            },
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].conversation_id, "conversation-1");
    }

    #[test]
    fn uses_fts_for_long_queries_and_like_for_one_or_two_characters() {
        assert!(!should_use_fts("中"));
        assert!(!should_use_fts("中文"));
        assert!(should_use_fts("中文搜"));

        let connection = in_memory_connection();
        insert_conversation(&connection, "conversation-1", None, "普通标题", 10, None);
        insert_message(
            &connection,
            "conversation-1",
            "message-1",
            "这里包含中文搜索内容",
            0,
        );

        for query in ["中", "中文", "中文搜索"] {
            let results = search_chats(
                &connection,
                &ChatSearchInput {
                    query: query.to_string(),
                    limit: Some(10),
                },
            )
            .unwrap();
            assert_eq!(results.len(), 1, "query {query:?}");
            assert_eq!(results[0].message_id.as_deref(), Some("message-1"));
        }
    }

    #[test]
    fn fts_search_is_case_insensitive_and_quotes_special_characters() {
        let connection = in_memory_connection();
        insert_conversation(
            &connection,
            "conversation-1",
            None,
            "Quoted \"Needle\"_\\Title",
            10,
            None,
        );
        insert_message(
            &connection,
            "conversation-1",
            "message-1",
            "Body contains MixedCase and the literal 100% marker",
            0,
        );

        for query in ["MIXEDCASE", "\"NEEDLE\"", "100%", "_\\T"] {
            let results = search_chats(
                &connection,
                &ChatSearchInput {
                    query: query.to_string(),
                    limit: Some(10),
                },
            )
            .unwrap();
            assert_eq!(results.len(), 1, "query {query:?}");
            assert_eq!(results[0].conversation_id, "conversation-1");
        }
    }

    #[test]
    fn fts_candidate_filter_preserves_legacy_non_ascii_case_matching() {
        let connection = in_memory_connection();
        insert_conversation(
            &connection,
            "conversation-1",
            None,
            "ordinary title",
            10,
            None,
        );
        insert_message(
            &connection,
            "conversation-1",
            "message-1",
            "Body contains ΑΘΗΝΑ",
            0,
        );

        let results = search_chats(
            &connection,
            &ChatSearchInput {
                query: "αθηνα".to_string(),
                limit: Some(10),
            },
        )
        .unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn title_search_tracks_updates_without_stale_hits() {
        let connection = in_memory_connection();
        insert_conversation(
            &connection,
            "conversation-1",
            None,
            "Original searchable title",
            10,
            None,
        );

        connection
            .execute(
                "UPDATE conversations SET title = 'Replacement indexed title' WHERE id = ?1",
                ["conversation-1"],
            )
            .unwrap();

        let old_results = search_chats(
            &connection,
            &ChatSearchInput {
                query: "Original searchable".to_string(),
                limit: Some(10),
            },
        )
        .unwrap();
        assert!(old_results.is_empty());

        let new_results = search_chats(
            &connection,
            &ChatSearchInput {
                query: "replacement indexed".to_string(),
                limit: Some(10),
            },
        )
        .unwrap();
        assert_eq!(new_results.len(), 1);
        assert_eq!(new_results[0].match_kind, ChatSearchMatchKind::Title);
    }

    #[test]
    fn relevance_ranking_prefers_exact_prefix_and_title_matches_before_messages() {
        let connection = in_memory_connection();
        insert_conversation(&connection, "exact", None, "needle", 1, None);
        insert_conversation(&connection, "prefix", None, "needle planning", 2, None);
        insert_conversation(
            &connection,
            "contains",
            None,
            "planning needle notes",
            3,
            None,
        );
        insert_conversation(
            &connection,
            "message",
            None,
            "newest conversation",
            100,
            None,
        );
        insert_message(
            &connection,
            "message",
            "message-1",
            "needle appears only in this message",
            0,
        );

        let results = search_chats(
            &connection,
            &ChatSearchInput {
                query: "needle".to_string(),
                limit: Some(10),
            },
        )
        .unwrap();

        assert_eq!(
            results
                .iter()
                .map(|result| result.conversation_id.as_str())
                .collect::<Vec<_>>(),
            vec!["exact", "prefix", "contains", "message"]
        );
    }

    #[test]
    fn archived_conversations_remain_hidden_from_fts_results() {
        let connection = in_memory_connection();
        insert_conversation(
            &connection,
            "visible",
            None,
            "ordinary visible title",
            10,
            None,
        );
        insert_conversation(
            &connection,
            "archived",
            None,
            "archived needle title",
            20,
            Some(30),
        );
        insert_message(
            &connection,
            "visible",
            "visible-message",
            "visible needle content",
            0,
        );
        insert_message(
            &connection,
            "archived",
            "archived-message",
            "archived needle content",
            0,
        );

        let results = search_chats(
            &connection,
            &ChatSearchInput {
                query: "needle".to_string(),
                limit: Some(10),
            },
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].conversation_id, "visible");
    }
}
