use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GitReviewSummaryRequest {
    pub project_id: String,
    pub source: Option<GitReviewSourceRequest>,
    pub target: GitReviewTargetRequest,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "camelCase")]
pub enum GitReviewSourceRequest {
    Folder {
        #[serde(rename = "folderId")]
        folder_id: String,
    },
    All {},
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind")]
pub enum GitReviewTargetRequest {
    #[serde(rename = "lastTurn")]
    LastTurn {
        #[serde(rename = "conversationId")]
        conversation_id: String,
    },
    #[serde(rename = "uncommitted")]
    Uncommitted,
    #[serde(rename = "unstaged")]
    Unstaged,
    #[serde(rename = "staged")]
    Staged,
    #[serde(rename = "commit")]
    Commit {
        #[serde(rename = "commitSha")]
        commit_sha: String,
    },
    #[serde(rename = "branch")]
    Branch {
        #[serde(rename = "baseRef")]
        base_ref: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GitReviewRepositoryContextRequest {
    pub project_id: String,
    pub folder_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GitReviewCommitListRequest {
    pub project_id: String,
    pub folder_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitTurnDiffSummariesRequest {
    pub conversation_id: String,
    pub project_id: String,
    pub assistant_message_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitReviewFileDiffRequest {
    pub snapshot_id: String,
    pub file_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitReviewFileContentRequest {
    pub snapshot_id: String,
    pub file_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitReviewFileMutationRequest {
    pub snapshot_id: String,
    pub file_id: String,
    pub action: String,
}
