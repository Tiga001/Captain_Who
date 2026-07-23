use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitReviewSummaryRequest {
    pub conversation_id: Option<String>,
    pub project_id: String,
    pub scope: String,
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
