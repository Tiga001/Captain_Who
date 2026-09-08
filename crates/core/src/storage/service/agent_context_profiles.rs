use super::StorageService;
use crate::storage::agent_context_profile_repository;
use crate::AgentContextProfile;

impl StorageService {
    /// The admission-time mode, including a trusted wake's inherited mode.
    pub fn load_agent_context_profile_for_run(
        &self,
        run_id: &str,
    ) -> Result<Option<AgentContextProfile>, String> {
        let connection = self.state.connection()?;
        agent_context_profile_repository::load_run(&connection, run_id)
            .map_err(|error| error.to_string())
    }
}
