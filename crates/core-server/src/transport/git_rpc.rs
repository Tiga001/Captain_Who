use super::*;
use mycopilot_core::git_review::GitReviewSource;
use mycopilot_core::storage::models::ProjectRecord;
use mycopilot_protocol_rs::GitReviewSourceRequest;

pub(crate) fn handle_git_request(
    storage: &StorageService,
    git_review_service: &GitReviewService,
    request: JsonRpcRequest,
) -> Value {
    match request.method.as_str() {
        GIT_INSPECT_REPOSITORY_METHOD => {
            let input = match parse_params::<GitRepositoryInspectRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match resolve_git_project(storage, &input.project_id) {
                Ok(project) => {
                    response_success(request.id, git_review_service.inspect_project(&project))
                }
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }

        GIT_GET_REVIEW_SUMMARY_METHOD => {
            let input = match parse_params::<GitReviewSummaryRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            let project = match resolve_git_project(storage, &input.project_id) {
                Ok(project) => project,
                Err(message) => return response_error(Some(request.id), -32000, message),
            };
            let source = input.source.map(|s| match s {
                GitReviewSourceRequest::Folder { folder_id } => {
                    GitReviewSource::Folder { folder_id }
                }
                GitReviewSourceRequest::All {} => GitReviewSource::All,
            });
            let summary = match input.target {
                GitReviewTargetRequest::LastTurn { conversation_id } => (|| {
                    let turn =
                        storage.load_latest_agent_turn_diff(&conversation_id, &input.project_id)?;
                    let workspace = turn
                        .as_ref()
                        .map(|r| {
                            storage.load_run_workspace(
                                &r.identity.assistant_message_id,
                                Some(&input.project_id),
                            )
                        })
                        .transpose()?
                        .flatten();
                    git_review_service.review_project_last_turn_summary(
                        &project,
                        source.as_ref(),
                        &conversation_id,
                        turn.as_ref(),
                        workspace.as_ref(),
                    )
                })(),
                target => match &source {
                    Some(GitReviewSource::All) => {
                        Err("All repositories is available only for last-turn review.".into())
                    }
                    source => git_review_service.review_project_summary(
                        &project,
                        source.as_ref().and_then(|s| match s {
                            GitReviewSource::Folder { folder_id } => Some(folder_id.as_str()),
                            _ => None,
                        }),
                        core_review_target(target),
                    ),
                },
            };
            match summary {
                Ok(summary) => response_success(request.id, summary),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        GIT_GET_REVIEW_REPOSITORY_CONTEXT_METHOD => {
            let input = match parse_params::<GitReviewRepositoryContextRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            let project = match resolve_git_project(storage, &input.project_id) {
                Ok(project) => project,
                Err(message) => return response_error(Some(request.id), -32000, message),
            };
            match git_review_service
                .review_project_repository_context(&project, input.folder_id.as_deref())
            {
                Ok(context) => response_success(request.id, context),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        GIT_LIST_REVIEW_COMMITS_METHOD => {
            let input = match parse_params::<GitReviewCommitListRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            let project = match resolve_git_project(storage, &input.project_id) {
                Ok(project) => project,
                Err(message) => return response_error(Some(request.id), -32000, message),
            };
            match git_review_service.review_project_commits(&project, input.folder_id.as_deref()) {
                Ok(commits) => response_success(request.id, commits),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        GIT_GET_TURN_DIFF_SUMMARIES_METHOD => {
            let input = match parse_params::<GitTurnDiffSummariesRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            let project_path = match resolve_project_path(storage, &input.project_id) {
                Ok(path) => path,
                Err(message) => return response_error(Some(request.id), -32000, message),
            };
            let records = match storage.load_agent_turn_diffs_for_messages(
                &input.conversation_id,
                &input.project_id,
                &input.assistant_message_ids,
            ) {
                Ok(records) => records,
                Err(message) => return response_error(Some(request.id), -32000, message),
            };
            response_success(
                request.id,
                git_review_service.turn_diff_summaries(
                    &input.conversation_id,
                    &project_path,
                    &records,
                ),
            )
        }
        GIT_GET_REVIEW_FILE_DIFF_METHOD => {
            let input = match parse_params::<GitReviewFileDiffRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            if let Err(message) = storage.load_projects().and_then(|projects| {
                git_review_service.revalidate_snapshot_membership(&input.snapshot_id, &projects)
            }) {
                return response_error(Some(request.id), -32000, message);
            }
            match git_review_service.review_file_diff(&input.snapshot_id, &input.file_id) {
                Ok(diff) => response_success(request.id, diff),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        GIT_GET_REVIEW_FILE_CONTENT_METHOD => {
            let input = match parse_params::<GitReviewFileContentRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            if let Err(message) = storage.load_projects().and_then(|projects| {
                git_review_service.revalidate_snapshot_membership(&input.snapshot_id, &projects)
            }) {
                return response_error(Some(request.id), -32000, message);
            }
            match git_review_service.review_file_content(&input.snapshot_id, &input.file_id) {
                Ok(content) => response_success(request.id, content),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        GIT_MUTATE_REVIEW_FILE_METHOD => {
            let input = match parse_params::<GitReviewFileMutationRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            let action = match GitReviewFileMutationAction::parse(&input.action) {
                Ok(action) => action,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            if let Err(message) = storage.load_projects().and_then(|projects| {
                git_review_service.revalidate_snapshot_membership(&input.snapshot_id, &projects)
            }) {
                return response_error(Some(request.id), -32000, message);
            }
            match git_review_service.mutate_review_file(&input.snapshot_id, &input.file_id, action)
            {
                Ok(mutation) => response_success(request.id, mutation),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        _ => response_error(Some(request.id), -32601, "Method not found"),
    }
}

fn core_review_target(target: GitReviewTargetRequest) -> GitReviewTarget {
    match target {
        GitReviewTargetRequest::LastTurn { conversation_id } => {
            GitReviewTarget::LastTurn { conversation_id }
        }
        GitReviewTargetRequest::Uncommitted => GitReviewTarget::Uncommitted,
        GitReviewTargetRequest::Unstaged => GitReviewTarget::Unstaged,
        GitReviewTargetRequest::Staged => GitReviewTarget::Staged,
        GitReviewTargetRequest::Commit { commit_sha } => GitReviewTarget::Commit { commit_sha },
        GitReviewTargetRequest::Branch { base_ref } => GitReviewTarget::Branch { base_ref },
    }
}

pub(crate) fn resolve_project_path(
    storage: &StorageService,
    project_id: &str,
) -> Result<PathBuf, String> {
    let project = storage
        .load_projects()?
        .into_iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| "The selected project no longer exists.".to_string())?;
    let path = project
        .primary_path()
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| "The selected project does not have a local directory.".to_string())?;
    Ok(PathBuf::from(path))
}

fn resolve_git_project(
    storage: &StorageService,
    project_id: &str,
) -> Result<ProjectRecord, String> {
    storage
        .load_projects()?
        .into_iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| "The selected project no longer exists.".into())
}

#[cfg(test)]
mod multi_source_tests {
    use super::*;
    use mycopilot_core::storage::models::{ProjectFolderRecord, ProjectFolderRole};
    use serde_json::json;
    use std::path::Path;

    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn call(
        storage: &StorageService,
        service: &GitReviewService,
        method: &str,
        params: Value,
    ) -> Value {
        handle_git_request(
            storage,
            service,
            serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
                .unwrap(),
        )
    }
    fn fixture() -> (
        tempfile::TempDir,
        StorageService,
        GitReviewService,
        ProjectRecord,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("notes");
        let repo = temp.path().join("code");
        std::fs::create_dir(&main).unwrap();
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.email", "git-review@test.invalid"]);
        git(&repo, &["config", "user.name", "Review Test"]);
        std::fs::write(repo.join("readme.txt"), "before\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "initial"]);
        std::fs::write(repo.join("readme.txt"), "after\n").unwrap();
        let mut project = ProjectRecord::with_primary_folder(
            "project-review-rpc",
            "Review RPC",
            main.canonicalize().unwrap().to_string_lossy(),
            1,
        );
        project.folders.push(ProjectFolderRecord {
            id: "folder-code".into(),
            path: repo.canonicalize().unwrap().to_string_lossy().into_owned(),
            alias: "code".into(),
            role: ProjectFolderRole::Auxiliary,
            sort_order: 1,
            created_at: 1,
        });
        let storage = StorageService::open(&temp.path().join("storage.sqlite")).unwrap();
        storage.save_project(project.clone()).unwrap();
        (temp, storage, GitReviewService::new(), project)
    }

    #[test]
    fn git_multi_source_rpc_catalog_selection_and_all_scope_contract() {
        let (_temp, storage, service, project) = fixture();
        let result = call(
            &storage,
            &service,
            GIT_INSPECT_REPOSITORY_METHOD,
            json!({"projectId":project.id}),
        );
        assert_eq!(result["result"]["state"], "ready");
        assert_eq!(result["result"]["folders"].as_array().unwrap().len(), 2);
        assert_eq!(result["result"]["defaultFolderId"], "folder-code");
        let summary = call(
            &storage,
            &service,
            GIT_GET_REVIEW_SUMMARY_METHOD,
            json!({"projectId":project.id,"source":{"kind":"folder","folderId":"folder-code"},"target":{"kind":"unstaged"}}),
        );
        assert_eq!(summary["result"]["source"]["folderId"], "folder-code");
        assert_eq!(
            summary["result"]["files"][0]["workspacePath"],
            "@workspace/code/readme.txt"
        );
        for target in [
            json!({"kind":"unstaged"}),
            json!({"kind":"staged"}),
            json!({"kind":"uncommitted"}),
            json!({"kind":"branch","baseRef":"refs/heads/main"}),
            json!({"kind":"commit","commitSha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}),
        ] {
            let rejected = call(
                &storage,
                &service,
                GIT_GET_REVIEW_SUMMARY_METHOD,
                json!({"projectId":project.id,"source":{"kind":"all"},"target":target}),
            );
            assert!(rejected["error"]["message"]
                .as_str()
                .unwrap()
                .contains("only for last-turn"));
        }
        let all = call(
            &storage,
            &service,
            GIT_GET_REVIEW_SUMMARY_METHOD,
            json!({"projectId":project.id,"source":{"kind":"all"},"target":{"kind":"lastTurn","conversationId":"empty-conversation"}}),
        );
        assert_eq!(all["result"]["source"]["kind"], "all");
        assert!(all["result"]["message"]
            .as_str()
            .unwrap()
            .contains("no recorded"));
        let context = call(
            &storage,
            &service,
            GIT_GET_REVIEW_REPOSITORY_CONTEXT_METHOD,
            json!({"projectId":project.id,"folderId":"folder-code"}),
        );
        assert!(context["result"]["headSha"].is_string());
        let commits = call(
            &storage,
            &service,
            GIT_LIST_REVIEW_COMMITS_METHOD,
            json!({"projectId":project.id,"folderId":"folder-code"}),
        );
        assert_eq!(commits["result"]["commits"].as_array().unwrap().len(), 1);
        let unknown = call(
            &storage,
            &service,
            GIT_GET_REVIEW_SUMMARY_METHOD,
            json!({"projectId":project.id,"source":{"kind":"folder","folderId":"foreign-source"},"target":{"kind":"unstaged"}}),
        );
        assert!(unknown["error"].is_object());
    }

    #[test]
    fn git_multi_source_rpc_source_removal_expires_cached_content_and_mutation() {
        let (_temp, storage, service, mut project) = fixture();
        let summary = call(
            &storage,
            &service,
            GIT_GET_REVIEW_SUMMARY_METHOD,
            json!({"projectId":project.id,"source":{"kind":"folder","folderId":"folder-code"},"target":{"kind":"unstaged"}}),
        );
        let snapshot = &summary["result"]["snapshotId"];
        let file = &summary["result"]["files"][0]["id"];
        project.folders.retain(|f| f.id != "folder-code");
        storage.save_project(project).unwrap();
        let input = json!({"snapshotId":snapshot,"fileId":file});
        let diff = call(
            &storage,
            &service,
            GIT_GET_REVIEW_FILE_DIFF_METHOD,
            input.clone(),
        );
        assert_eq!(diff["result"]["status"], "snapshotExpired");
        let content = call(
            &storage,
            &service,
            GIT_GET_REVIEW_FILE_CONTENT_METHOD,
            input,
        );
        assert_eq!(content["result"]["status"], "snapshotExpired");
        let mutation = call(
            &storage,
            &service,
            GIT_MUTATE_REVIEW_FILE_METHOD,
            json!({"snapshotId":snapshot,"fileId":file,"action":"stage"}),
        );
        assert_eq!(mutation["result"]["status"], "snapshotExpired");
    }
}
