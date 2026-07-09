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
}

#[derive(Debug)]
struct TitleHit {
    conversation_id: String,
    project_id: Option<String>,
    title: String,
    updated_at: i64,
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
    let pattern = like_contains_pattern(query);
    let message_hits = query_message_hits(connection, &pattern, limit)?;
    let message_hit_conversation_ids = message_hits
        .iter()
        .map(|hit| hit.conversation_id.clone())
        .collect::<HashSet<_>>();
    let title_hits = query_title_hits(connection, &pattern, limit)?;

    let mut results = Vec::with_capacity(limit);

    for hit in message_hits {
        results.push(ChatSearchResult {
            conversation_id: hit.conversation_id,
            project_id: hit.project_id,
            title: hit.title,
            message_id: Some(hit.message_id),
            snippet: Some(make_snippet(&hit.content, query)),
            match_kind: ChatSearchMatchKind::Message,
            updated_at: hit.updated_at,
        });
    }

    for hit in title_hits {
        if message_hit_conversation_ids.contains(&hit.conversation_id) {
            continue;
        }

        results.push(ChatSearchResult {
            conversation_id: hit.conversation_id,
            project_id: hit.project_id,
            title: hit.title,
            message_id: None,
            snippet: None,
            match_kind: ChatSearchMatchKind::Title,
            updated_at: hit.updated_at,
        });
    }

    results.sort_by(|left, right| {
        right.updated_at.cmp(&left.updated_at).then_with(|| {
            match (&left.message_id, &right.message_id) {
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            }
        })
    });
    results.truncate(limit);
    Ok(results)
}

fn query_message_hits(
    connection: &Connection,
    pattern: &str,
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
            m.content
        FROM messages m
        JOIN conversations c ON c.id = m.conversation_id
        WHERE c.archived_at IS NULL
          AND m.content <> ''
          AND lower(m.content) LIKE ?1 ESCAPE '\\'
        ORDER BY c.updated_at DESC, m.position ASC, m.created_at ASC
        LIMIT ?2
        ",
    )?;

    let hits = statement
        .query_map(params![pattern, limit as i64], |row| {
            Ok(MessageHit {
                conversation_id: row.get(0)?,
                project_id: row.get(1)?,
                title: row.get(2)?,
                updated_at: row.get(3)?,
                message_id: row.get(4)?,
                content: row.get(5)?,
            })
        })?
        .collect();
    hits
}

fn query_title_hits(
    connection: &Connection,
    pattern: &str,
    limit: usize,
) -> rusqlite::Result<Vec<TitleHit>> {
    let mut statement = connection.prepare(
        "
        SELECT id, project_id, title, updated_at
        FROM conversations
        WHERE archived_at IS NULL
          AND lower(title) LIKE ?1 ESCAPE '\\'
          AND NOT EXISTS (
              SELECT 1
              FROM messages
              WHERE messages.conversation_id = conversations.id
                AND messages.content <> ''
                AND lower(messages.content) LIKE ?1 ESCAPE '\\'
          )
        ORDER BY updated_at DESC
        LIMIT ?2
        ",
    )?;

    let hits = statement
        .query_map(params![pattern, limit as i64], |row| {
            Ok(TitleHit {
                conversation_id: row.get(0)?,
                project_id: row.get(1)?,
                title: row.get(2)?,
                updated_at: row.get(3)?,
            })
        })?
        .collect();
    hits
}

fn like_contains_pattern(query: &str) -> String {
    let mut pattern = String::with_capacity(query.len() + 2);
    pattern.push('%');
    for character in query.to_lowercase().chars() {
        match character {
            '%' | '_' | '\\' => {
                pattern.push('\\');
                pattern.push(character);
            }
            _ => pattern.push(character),
        }
    }
    pattern.push('%');
    pattern
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
        assert_eq!(results[0].conversation_id, "conversation-2");
        assert_eq!(results[0].message_id.as_deref(), Some("message-2"));
        assert_eq!(results[0].match_kind, ChatSearchMatchKind::Message);
        assert!(results[0].snippet.as_deref().unwrap().contains("开发"));
        assert_eq!(results[1].conversation_id, "conversation-1");
        assert_eq!(results[1].message_id.as_deref(), Some("message-1"));
        assert_eq!(results[2].conversation_id, "conversation-3");
        assert_eq!(results[2].message_id, None);
        assert_eq!(results[2].snippet, None);
        assert_eq!(results[2].match_kind, ChatSearchMatchKind::Title);
    }

    #[test]
    fn escapes_like_wildcards() {
        let connection = in_memory_connection();
        insert_conversation(&connection, "conversation-1", None, "100% done", 10, None);
        insert_conversation(&connection, "conversation-2", None, "100x done", 20, None);

        let results = search_chats(
            &connection,
            &ChatSearchInput {
                query: "100%".to_string(),
                limit: Some(10),
            },
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].conversation_id, "conversation-1");
    }
}
