use rusqlite::{params, Connection, OptionalExtension};

pub const MANAGED_ARTIFACT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedArtifactKind {
    Image,
    Document,
}

impl ManagedArtifactKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Document => "document",
        }
    }

    fn parse(value: &str) -> rusqlite::Result<Self> {
        match value {
            "image" => Ok(Self::Image),
            "document" => Ok(Self::Document),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedArtifactRecord {
    pub artifact_id: String,
    pub kind: ManagedArtifactKind,
    pub storage_relative_path: String,
    pub format: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedArtifactGrant {
    pub artifact_id: String,
    pub conversation_id: String,
    pub run_id: String,
    pub call_id: String,
    pub created_at: i64,
}

pub fn register(
    connection: &mut Connection,
    artifact: &ManagedArtifactRecord,
) -> rusqlite::Result<ManagedArtifactRecord> {
    let size_bytes = i64::try_from(artifact.size_bytes)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let width = artifact.width.map(i64::from);
    let height = artifact.height.map(i64::from);
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT OR IGNORE INTO managed_artifacts (
            artifact_id, schema_version, kind, storage_relative_path, format, media_type,
            size_bytes, sha256, width, height, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            &artifact.artifact_id,
            MANAGED_ARTIFACT_SCHEMA_VERSION,
            artifact.kind.as_str(),
            &artifact.storage_relative_path,
            &artifact.format,
            &artifact.media_type,
            size_bytes,
            &artifact.sha256,
            width,
            height,
            artifact.created_at,
        ],
    )?;
    let registered =
        query(&transaction, &artifact.artifact_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    if !same_identity(&registered, artifact) {
        transaction.rollback()?;
        return Err(rusqlite::Error::InvalidQuery);
    }
    transaction.commit()?;
    Ok(registered)
}

pub fn find(
    connection: &Connection,
    artifact_id: &str,
) -> rusqlite::Result<Option<ManagedArtifactRecord>> {
    query(connection, artifact_id)
}

pub fn grant(connection: &mut Connection, grant: &ManagedArtifactGrant) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT OR IGNORE INTO managed_artifact_grants (
            artifact_id, conversation_id, run_id, call_id, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            &grant.artifact_id,
            &grant.conversation_id,
            &grant.run_id,
            &grant.call_id,
            grant.created_at,
        ],
    )?;
    Ok(())
}

pub fn find_authorized(
    connection: &Connection,
    artifact_id: &str,
    conversation_id: &str,
) -> rusqlite::Result<Option<ManagedArtifactRecord>> {
    let directly_authorized = connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM managed_artifact_grants
            WHERE artifact_id = ?1 AND conversation_id = ?2
         )",
        params![artifact_id, conversation_id],
        |row| row.get::<_, bool>(0),
    )?;
    if directly_authorized {
        return query(connection, artifact_id);
    }

    let Some(scope) =
        crate::storage::agent_tree_resource_scope::for_conversation(connection, conversation_id)?
    else {
        return Ok(None);
    };
    let tree_authorized = connection.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM managed_artifact_grants AS grant_record
            INNER JOIN agent_nodes AS owner
                ON owner.conversation_id = grant_record.conversation_id
            WHERE grant_record.artifact_id = ?1
              AND owner.root_agent_id = ?2
              AND owner.root_conversation_id = ?3
         )",
        params![
            artifact_id,
            &scope.root_agent_id,
            &scope.root_conversation_id
        ],
        |row| row.get::<_, bool>(0),
    )?;
    if !tree_authorized {
        Ok(None)
    } else {
        query(connection, artifact_id)
    }
}

fn query(
    connection: &Connection,
    artifact_id: &str,
) -> rusqlite::Result<Option<ManagedArtifactRecord>> {
    connection
        .query_row(
            "SELECT artifact_id, kind, storage_relative_path, format, media_type,
                    size_bytes, sha256, width, height, created_at
             FROM managed_artifacts WHERE artifact_id = ?1",
            [artifact_id],
            |row| {
                let size_bytes = u64::try_from(row.get::<_, i64>(5)?).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })?;
                let width = row
                    .get::<_, Option<i64>>(7)?
                    .map(u32::try_from)
                    .transpose()
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            7,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?;
                let height = row
                    .get::<_, Option<i64>>(8)?
                    .map(u32::try_from)
                    .transpose()
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            8,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?;
                Ok(ManagedArtifactRecord {
                    artifact_id: row.get(0)?,
                    kind: ManagedArtifactKind::parse(&row.get::<_, String>(1)?)?,
                    storage_relative_path: row.get(2)?,
                    format: row.get(3)?,
                    media_type: row.get(4)?,
                    size_bytes,
                    sha256: row.get(6)?,
                    width,
                    height,
                    created_at: row.get(9)?,
                })
            },
        )
        .optional()
}

fn same_identity(left: &ManagedArtifactRecord, right: &ManagedArtifactRecord) -> bool {
    left.artifact_id == right.artifact_id
        && left.kind == right.kind
        && left.storage_relative_path == right.storage_relative_path
        && left.format == right.format
        && left.media_type == right.media_type
        && left.size_bytes == right.size_bytes
        && left.sha256 == right.sha256
        && left.width == right.width
        && left.height == right.height
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations::run_migrations;

    fn record() -> ManagedArtifactRecord {
        let digest = "a".repeat(64);
        ManagedArtifactRecord {
            artifact_id: format!("sha256:{digest}"),
            kind: ManagedArtifactKind::Image,
            storage_relative_path: format!("objects/{digest}.png"),
            format: "png".to_string(),
            media_type: "image/png".to_string(),
            size_bytes: 42,
            sha256: digest,
            width: Some(2),
            height: Some(3),
            created_at: 1,
        }
    }

    fn insert_conversation(connection: &Connection, conversation_id: &str) {
        connection
            .execute(
                "INSERT INTO conversations (id, model_id, title, created_at, updated_at)
                 VALUES (?1, 'model-1', ?1, 1, 1)",
                [conversation_id],
            )
            .unwrap();
    }

    fn insert_root(
        connection: &Connection,
        agent_id: &str,
        conversation_id: &str,
        request_id: &str,
    ) {
        connection
            .execute(
                "INSERT INTO agent_nodes (
                    agent_id, schema_version, root_agent_id, root_conversation_id,
                    parent_agent_id, conversation_id, project_id, creation_request_id,
                    task_name, task_path, lifecycle, revision, created_at, updated_at
                 ) VALUES (?1, 1, ?1, ?2, NULL, ?2, NULL, ?3, ?1, '/root',
                           'active', 1, 1, 1)",
                params![agent_id, conversation_id, request_id],
            )
            .unwrap();
    }

    fn insert_child(
        connection: &Connection,
        agent_id: &str,
        conversation_id: &str,
        root_agent_id: &str,
        root_conversation_id: &str,
        task_name: &str,
    ) {
        connection
            .execute(
                "INSERT INTO agent_nodes (
                    agent_id, schema_version, root_agent_id, root_conversation_id,
                    parent_agent_id, conversation_id, project_id, creation_request_id,
                    task_name, task_path,
                    model_config_id_snapshot, model_display_name_snapshot,
                    model_supports_image_snapshot, model_context_window_tokens_snapshot,
                    model_settings_revision_snapshot, provider_connection_revision_snapshot,
                    provider_protocol_revision_snapshot, model_selection_source_snapshot,
                    lifecycle, revision, created_at, updated_at
                 ) VALUES (?1, 1, ?3, ?4, ?3, ?2, NULL, ?1, ?5, '/root/' || ?5,
                           'model-1', 'Model 1', 0, 32000,
                           'settings-1', 'connection-1', 'provider-protocol-v1', 'explicit',
                           'active', 1, 2, 2)",
                params![
                    agent_id,
                    conversation_id,
                    root_agent_id,
                    root_conversation_id,
                    task_name
                ],
            )
            .unwrap();
    }

    #[test]
    fn registration_is_idempotent_but_rejects_conflicting_identity() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO conversations (id, title, created_at, updated_at)
             VALUES ('conversation-1', 'test', 1, 1)",
                [],
            )
            .unwrap();
        let record = record();
        assert_eq!(register(&mut connection, &record).unwrap(), record);
        assert_eq!(register(&mut connection, &record).unwrap(), record);

        let mut conflicting = record.clone();
        conflicting.size_bytes += 1;
        assert!(register(&mut connection, &conflicting).is_err());

        let grant_record = ManagedArtifactGrant {
            artifact_id: record.artifact_id.clone(),
            conversation_id: "conversation-1".to_string(),
            run_id: "run-1".to_string(),
            call_id: "call-1".to_string(),
            created_at: 2,
        };
        grant(&mut connection, &grant_record).unwrap();
        assert!(
            find_authorized(&connection, &record.artifact_id, "conversation-1")
                .unwrap()
                .is_some()
        );
        assert!(
            find_authorized(&connection, &record.artifact_id, "conversation-2")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn authorization_expands_bidirectionally_only_inside_one_agent_tree() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        for conversation_id in [
            "conversation-root",
            "conversation-child",
            "conversation-sibling",
            "conversation-other-root",
            "conversation-ordinary",
        ] {
            insert_conversation(&connection, conversation_id);
        }
        insert_root(
            &connection,
            "agent-root",
            "conversation-root",
            "request-root",
        );
        insert_child(
            &connection,
            "agent-child",
            "conversation-child",
            "agent-root",
            "conversation-root",
            "child",
        );
        insert_child(
            &connection,
            "agent-sibling",
            "conversation-sibling",
            "agent-root",
            "conversation-root",
            "sibling",
        );
        insert_root(
            &connection,
            "agent-other-root",
            "conversation-other-root",
            "request-other-root",
        );

        let artifact = record();
        register(&mut connection, &artifact).unwrap();
        grant(
            &mut connection,
            &ManagedArtifactGrant {
                artifact_id: artifact.artifact_id.clone(),
                conversation_id: "conversation-root".to_string(),
                run_id: "run-root".to_string(),
                call_id: "call-root".to_string(),
                created_at: 2,
            },
        )
        .unwrap();

        for conversation_id in [
            "conversation-root",
            "conversation-child",
            "conversation-sibling",
        ] {
            assert!(
                find_authorized(&connection, &artifact.artifact_id, conversation_id)
                    .unwrap()
                    .is_some(),
                "same-tree consumer {conversation_id} should be authorized"
            );
        }
        for conversation_id in ["conversation-other-root", "conversation-ordinary"] {
            assert!(
                find_authorized(&connection, &artifact.artifact_id, conversation_id)
                    .unwrap()
                    .is_none(),
                "unrelated consumer {conversation_id} must remain isolated"
            );
        }

        connection
            .execute("DELETE FROM managed_artifact_grants", [])
            .unwrap();
        grant(
            &mut connection,
            &ManagedArtifactGrant {
                artifact_id: artifact.artifact_id.clone(),
                conversation_id: "conversation-child".to_string(),
                run_id: "run-child".to_string(),
                call_id: "call-child".to_string(),
                created_at: 3,
            },
        )
        .unwrap();
        assert!(
            find_authorized(&connection, &artifact.artifact_id, "conversation-root")
                .unwrap()
                .is_some(),
            "a child publication must also be readable by its trusted root"
        );
    }
}
