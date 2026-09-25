use super::StorageService;
use crate::storage::workflow_repository::{self, Error};
use crate::workflow::{Request, Response};

impl StorageService {
    pub fn workflow_request(&self, request: Request) -> Result<Response, Error> {
        // Use the same authoritative availability projection as model selectors. Resolve
        // credentials before taking the workflow transaction's SQLite lock.
        let available_models = self
            .load_model_projection()
            .map_err(Error::Storage)?
            .into_iter()
            .flat_map(|projection| projection.models)
            .filter(|entry| entry.execution.is_available())
            .map(|entry| entry.model.id)
            .collect();
        let mut connection = self.state.connection().map_err(Error::Storage)?;
        workflow_repository::request(&mut connection, request, &available_models)
    }
}
