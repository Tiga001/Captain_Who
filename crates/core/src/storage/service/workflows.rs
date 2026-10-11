use super::StorageService;
use crate::storage::workflow_repository::{self, Error};
use crate::storage::{write_size::estimated_json_bytes, StorageWriteClass};
use crate::workflow::{Request, Response};

fn workflow_request_bytes(request: &Request) -> Result<usize, String> {
    let payload = match request {
        Request::List => 0,
        Request::ImportTemplateMarkdown { markdown } => markdown.len().saturating_mul(2),
        Request::ExportTemplateMarkdown { id, language, .. } => {
            id.len().saturating_add(language.len()).saturating_mul(2)
        }
        Request::Manage(request) => estimated_json_bytes(request)?,
        Request::SaveWithDraft { definition, .. }
        | Request::Validate { definition }
        | Request::Save { definition, .. } => estimated_json_bytes(definition)?,
        Request::Delete { id, .. } => id.len().saturating_mul(2),
    };
    Ok(payload.saturating_add(512))
}

impl StorageService {
    pub fn workflow_request(&self, request: Request) -> Result<Response, Error> {
        if matches!(
            request,
            Request::Manage(
                crate::workflow_management::Request::ListInstances {}
                    | crate::workflow_management::Request::GetInstance { .. }
            ) | Request::ExportTemplateMarkdown { .. }
        ) {
            let mut connection = self.state.read_connection().map_err(Error::Storage)?;
            return workflow_repository::request(&mut connection, request, &Default::default());
        }
        // Use the same authoritative availability projection as model selectors. Resolve
        // credentials before taking the workflow transaction's SQLite lock.
        let available_models: std::collections::HashSet<String> = self
            .load_model_projection()
            .map_err(Error::Storage)?
            .into_iter()
            .flat_map(|projection| projection.models)
            .filter(|entry| entry.execution.is_available())
            .map(|entry| entry.model.id)
            .collect();
        if matches!(request, Request::List | Request::Validate { .. }) {
            let mut connection = self.state.read_connection().map_err(Error::Storage)?;
            return workflow_repository::request(&mut connection, request, &available_models);
        }
        let estimated_bytes = workflow_request_bytes(&request)
            .map_err(Error::Storage)?
            .saturating_add(estimated_json_bytes(&available_models).map_err(Error::Storage)?);
        self.state
            .write_sync(
                estimated_bytes,
                StorageWriteClass::Normal,
                move |connection| {
                    workflow_repository::request(connection, request, &available_models)
                },
            )
            .map_err(Error::Storage)?
    }
}
