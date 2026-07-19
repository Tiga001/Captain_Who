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
            let scope = match GitReviewScope::parse(&input.scope) {
                Ok(scope) => scope,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            let project_path = match resolve_project_path(storage, &input.project_id) {
                Ok(path) => path,
                Err(message) => return response_error(Some(request.id), -32000, message),
            };
            match git_review_service.review_summary(&project_path, scope) {
                Ok(summary) => response_success(request.id, summary),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
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
