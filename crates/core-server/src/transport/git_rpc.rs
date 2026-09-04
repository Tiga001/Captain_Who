use super::*;

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
            let project_path = match resolve_project_path(storage, &input.project_id) {
                Ok(path) => path,
                Err(message) => return response_error(Some(request.id), -32000, message),
            };
            response_success(
                request.id,
                git_review_service.inspect_repository(&input.project_id, &project_path),
            )
        }
        GIT_GET_REVIEW_SUMMARY_METHOD => {
            let input = match parse_params::<GitReviewSummaryRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            let project_path = match resolve_project_path(storage, &input.project_id) {
                Ok(path) => path,
                Err(message) => return response_error(Some(request.id), -32000, message),
            };
            let summary = match input.target {
                GitReviewTargetRequest::LastTurn { conversation_id } => {
                    match storage.load_latest_agent_turn_diff(&conversation_id, &input.project_id) {
                        Ok(turn) => git_review_service.review_last_turn_summary(
                            &project_path,
                            &conversation_id,
                            turn.as_ref(),
                        ),
                        Err(message) => Err(message),
                    }
                }
                target => {
                    git_review_service.review_summary(&project_path, core_review_target(target))
                }
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
            let project_path = match resolve_project_path(storage, &input.project_id) {
                Ok(path) => path,
                Err(message) => return response_error(Some(request.id), -32000, message),
            };
            match git_review_service.review_repository_context(&project_path) {
                Ok(context) => response_success(request.id, context),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        GIT_LIST_REVIEW_COMMITS_METHOD => {
            let input = match parse_params::<GitReviewCommitListRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            let project_path = match resolve_project_path(storage, &input.project_id) {
                Ok(path) => path,
                Err(message) => return response_error(Some(request.id), -32000, message),
            };
            match git_review_service.review_commits(&project_path) {
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
        .path
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| "The selected project does not have a local directory.".to_string())?;
    Ok(PathBuf::from(path))
}
