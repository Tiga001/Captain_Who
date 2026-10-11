use super::*;

/// An account grant is process-only, but its publication must have the same ordering boundary as
/// Turn admission. These callbacks may touch only the short in-memory access registry; invoking a
/// StorageService method from them would recursively acquire the writer and is forbidden.
type ExecutionAccessDenial = (&'static str, &'static str);

impl StorageService {
    /// Publishes a process-owned lifecycle fence at the same serialization boundary as durable
    /// admission. The callback only changes short in-memory state; it must never call storage,
    /// wait for another job, drain effects or perform external work.
    pub fn coordinate_runtime_control<T: Send>(
        &self,
        publish: impl FnOnce() -> Result<T, String> + Send,
    ) -> Result<T, String> {
        self.state
            .write_sync(4096, crate::storage::StorageWriteClass::Control, |_| {
                publish()
            })?
    }

    /// The DTO-selected Conversation is only a lookup hint. Resolve its current project on the
    /// writer before publishing a destructive scope, so a concurrent move cannot register the
    /// mutation against an obsolete project's drain counter. A missing Conversation has no
    /// project, preserving idempotent removal of an already absent ordinary Conversation.
    pub fn coordinate_conversation_control<T: Send>(
        &self,
        conversation_id: &str,
        expected_project_id: Option<&str>,
        publish: impl FnOnce() -> Result<T, String> + Send,
    ) -> Result<T, String> {
        self.state.write_sync(
            conversation_id.len().saturating_add(
                expected_project_id.map_or(4096, |id| id.len().saturating_add(4096)),
            ),
            crate::storage::StorageWriteClass::Control,
            |connection| {
                let current = chat_repository::get_conversation_meta(connection, conversation_id)
                    .map_err(storage_error)?;
                if current.as_ref().and_then(|conversation| conversation.project_id.as_deref())
                    != expected_project_id
                {
                    return Err("Conversation project changed before destructive scope registration; retry the operation.".into());
                }
                publish()
            },
        )?
    }

    /// Capture every current member (including child/archived Conversations) at the same writer
    /// boundary which publishes their runtime fences. The caller first fences the Project and
    /// must compare this member set again at final deletion; metadata movement can still change
    /// membership while the exact captured scopes are being drained.
    pub fn coordinate_project_conversations_control<T: Send>(
        &self,
        project_id: &str,
        publish: impl FnOnce(Vec<String>) -> Result<T, String> + Send,
    ) -> Result<T, String> {
        self.state.write_sync(
            project_id.len().saturating_add(4096),
            crate::storage::StorageWriteClass::Control,
            |connection| {
                let mut statement = connection
                    .prepare("SELECT id FROM conversations WHERE project_id = ?1 ORDER BY id")
                    .map_err(storage_error)?;
                let ids = statement
                    .query_map([project_id], |row| row.get::<_, String>(0))
                    .map_err(storage_error)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(storage_error)?;
                drop(statement);
                publish(ids)
            },
        )?
    }

    pub fn publish_execution_access_boundary<T: Send>(
        &self,
        publish: impl FnOnce() -> Result<(T, Option<ExecutionAccessDenial>), String> + Send,
    ) -> Result<T, String> {
        self.state.write_sync(
            4096,
            crate::storage::StorageWriteClass::Control,
            |connection| {
                // Release the registry lock inside `publish` before doing any SQL. The writer
                // boundary still prevents admission or a newer grant from interleaving here.
                let (output, denial) = publish()?;
                if let Some((code, message)) = denial {
                    // Each repository batch is a complete short transaction. A failed batch must
                    // leave the denial authoritative and propagate the failure, never resurrect
                    // the previous grant or report successful durable settlement.
                    while
                        automation_repository::fail_unadmitted_automation_runs_for_execution_access(
                            connection,
                            None,
                            code,
                            message,
                            now_ms(),
                            100,
                        )
                        .map_err(storage_error)?
                            == 100
                    {}
                }
                Ok(output)
            },
        )?
    }

    pub fn fail_unadmitted_automation_runs_at_execution_access_boundary(
        &self,
        run_id: Option<&str>,
        limit: usize,
        check: impl FnOnce() -> Result<Option<ExecutionAccessDenial>, String> + Send,
    ) -> Result<usize, String> {
        self.state.write_sync(
            run_id.map_or(4096, |id| id.len().saturating_add(4096)),
            crate::storage::StorageWriteClass::Control,
            |connection| {
                let Some((code, message)) = check()? else {
                    return Ok(0);
                };
                automation_repository::fail_unadmitted_automation_runs_for_execution_access(
                    connection,
                    run_id,
                    code,
                    message,
                    now_ms(),
                    limit,
                )
                .map_err(storage_error)
            },
        )?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn storage(path: &Path) -> StorageService {
        StorageService::open_with_model_credentials(
            path,
            Arc::new(crate::image_generation::InMemoryCredentialStore::default()),
        )
        .unwrap()
    }

    #[test]
    fn conversation_control_rejects_a_project_move_before_scope_publication() {
        let directory = tempfile::tempdir().unwrap();
        let service = storage(&directory.path().join("scope.sqlite"));
        service
            .save_project(ProjectRecord::without_folders("project", "Project", 1))
            .unwrap();
        service
            .save_conversation(ChatConversationRecord {
                id: "conversation".into(),
                title: "Conversation".into(),
                project_id: None,
                model_id: None,
                messages: Vec::new(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let expected = service
            .load_conversation_meta("conversation")
            .unwrap()
            .unwrap();
        let mut moved = expected.clone();
        moved.project_id = Some("project".into());
        moved.updated_at = 2;
        service.save_conversation_meta(moved).unwrap();
        let published = AtomicBool::new(false);
        assert!(service
            .coordinate_conversation_control("conversation", expected.project_id.as_deref(), || {
                published.store(true, Ordering::Release);
                Ok(())
            },)
            .is_err());
        assert!(!published.load(Ordering::Acquire));
        service
            .coordinate_conversation_control("conversation", Some("project"), || {
                published.store(true, Ordering::Release);
                Ok(())
            })
            .unwrap();
        assert!(published.load(Ordering::Acquire));
    }

    #[test]
    fn grant_publication_uses_writer_and_failed_settlement_does_not_undo_denial() {
        let directory = tempfile::tempdir().unwrap();
        let service = storage(&directory.path().join("access.sqlite"));
        let worker = service
            .publish_execution_access_boundary(|| {
                Ok((std::thread::current().name().unwrap().to_string(), None))
            })
            .unwrap();
        assert_eq!(worker, "core-sqlite-writer");
        // Only this isolated fixture is damaged. A repository failure after a grant change must
        // surface to the caller while the in-memory denial remains authoritative.
        service
            .state
            .write_sync(
                0,
                crate::storage::StorageWriteClass::Control,
                |connection| connection.execute_batch("DROP TABLE automation_runs"),
            )
            .unwrap()
            .unwrap();
        let denied = AtomicBool::new(false);
        let result = service.publish_execution_access_boundary(|| {
            denied.store(true, Ordering::Release);
            Ok(((), Some(("ACCOUNT_LOGIN_REQUIRED", "signed out"))))
        });
        assert!(result.is_err());
        assert!(denied.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn grant_publication_after_storage_shutdown_never_runs_callback() {
        let directory = tempfile::tempdir().unwrap();
        let service = storage(&directory.path().join("access.sqlite"));
        service.shutdown_storage().await.unwrap();
        let published = AtomicBool::new(false);
        let result = service.publish_execution_access_boundary(|| {
            published.store(true, Ordering::Release);
            Ok(((), None))
        });
        assert!(result.is_err());
        assert!(!published.load(Ordering::Acquire));
    }
}
