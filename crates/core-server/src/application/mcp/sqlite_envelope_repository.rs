use std::sync::Arc;

use mycopilot_core::storage::models::McpApprovalEnvelopeRecord;
use mycopilot_core::storage::service::StorageService;

use super::approval_payload_store::{
    McpApprovalEnvelopeRepository, McpApprovalInvocationId, McpApprovalPayloadStoreError,
    PersistedMcpApprovalEnvelope,
};

pub(crate) struct SqliteMcpApprovalEnvelopeRepository {
    storage: Arc<StorageService>,
}

impl SqliteMcpApprovalEnvelopeRepository {
    #[allow(dead_code)] // Production key provisioning is intentionally deferred to the settings round.
    pub(crate) fn new(storage: Arc<StorageService>) -> Self {
        Self { storage }
    }

    fn decode_record(
        record: McpApprovalEnvelopeRecord,
    ) -> Result<PersistedMcpApprovalEnvelope, McpApprovalPayloadStoreError> {
        PersistedMcpApprovalEnvelope::from_persisted_parts(
            record.invocation_id,
            record.action_id,
            record.envelope_version,
            record.nonce_base64,
            record.ciphertext_base64,
            record.aad_digest,
            record.created_at,
            record.expires_at,
        )
    }
}

impl McpApprovalEnvelopeRepository for SqliteMcpApprovalEnvelopeRepository {
    fn insert(
        &self,
        invocation_id: &McpApprovalInvocationId,
        envelope: PersistedMcpApprovalEnvelope,
    ) -> Result<(), McpApprovalPayloadStoreError> {
        if envelope.invocation_id() != invocation_id.as_str() {
            return Err(McpApprovalPayloadStoreError::BindingMismatch);
        }
        let record = McpApprovalEnvelopeRecord {
            invocation_id: invocation_id.as_str().to_string(),
            action_id: envelope.action_binding().to_string(),
            envelope_version: envelope.version(),
            nonce_base64: envelope.nonce_base64().to_string(),
            ciphertext_base64: envelope.ciphertext_base64().to_string(),
            aad_digest: envelope.aad_digest().to_string(),
            created_at: envelope.created_at_ms(),
            expires_at: envelope.expires_at_ms(),
        };
        self.storage
            .store_mcp_approval_envelope(record)
            .map(|_| ())
            .map_err(|_| McpApprovalPayloadStoreError::RepositoryUnavailable)
    }

    fn get(
        &self,
        invocation_id: &McpApprovalInvocationId,
    ) -> Result<Option<PersistedMcpApprovalEnvelope>, McpApprovalPayloadStoreError> {
        self.storage
            .load_mcp_approval_envelope(invocation_id.as_str())
            .map_err(|_| McpApprovalPayloadStoreError::RepositoryUnavailable)?
            .map(Self::decode_record)
            .transpose()
    }

    fn take(
        &self,
        invocation_id: &McpApprovalInvocationId,
    ) -> Result<Option<PersistedMcpApprovalEnvelope>, McpApprovalPayloadStoreError> {
        self.storage
            .take_mcp_approval_envelope(invocation_id.as_str())
            .map_err(|_| McpApprovalPayloadStoreError::RepositoryUnavailable)?
            .map(Self::decode_record)
            .transpose()
    }

    fn delete(
        &self,
        invocation_id: &McpApprovalInvocationId,
    ) -> Result<bool, McpApprovalPayloadStoreError> {
        self.storage
            .delete_mcp_approval_envelope(invocation_id.as_str())
            .map_err(|_| McpApprovalPayloadStoreError::RepositoryUnavailable)
    }

    fn list_expired(
        &self,
        cutoff_ms: i64,
        limit: usize,
    ) -> Result<Vec<McpApprovalInvocationId>, McpApprovalPayloadStoreError> {
        self.storage
            .list_expired_mcp_approval_envelopes(cutoff_ms, limit)
            .map_err(|_| McpApprovalPayloadStoreError::RepositoryUnavailable)?
            .into_iter()
            .map(|record| McpApprovalInvocationId::parse(record.invocation_id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::{
        image_generation::InMemoryCredentialStore, AgentMcpApprovalPayloadPersistence,
    };
    use tempfile::tempdir;

    use crate::application::mcp::approval_payload_store::{
        McpApprovalPayload, McpApprovalPayloadAad, McpApprovalPayloadStore,
        McpApprovalPayloadStoreFactory,
    };

    const CANARY: &str = "MCP_SQLITE_PLAINTEXT_CANARY";

    fn aad() -> McpApprovalPayloadAad {
        McpApprovalPayloadAad {
            run_id: "run-sqlite-envelope".to_string(),
            action_id: uuid::Uuid::new_v4().to_string(),
            call_id: "provider-call-sqlite-envelope".to_string(),
            server_id: uuid::Uuid::new_v4().to_string(),
            config_epoch: uuid::Uuid::new_v4().to_string(),
            registry_revision: 1,
            config_digest: "1".repeat(64),
            catalog_digest: "2".repeat(64),
            catalog_generation: 1,
            raw_tool_name_digest: "3".repeat(64),
            schema_digest: "4".repeat(64),
            arguments_digest: mycopilot_core::mcp_tool_arguments_digest(
                &serde_json::json!({"value": CANARY}),
            )
            .unwrap(),
            payload_persistence: AgentMcpApprovalPayloadPersistence::DurableAuthenticatedEnvelope,
            created_at_ms: mycopilot_core::storage::now_ms(),
            expires_at_ms: mycopilot_core::storage::now_ms() + 60_000,
        }
    }

    #[test]
    fn durable_store_round_trips_via_sqlite_without_plaintext() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("mcp-approval.sqlite");
        let storage = Arc::new(StorageService::open(&database_path).unwrap());
        let repository: Arc<dyn McpApprovalEnvelopeRepository> = Arc::new(
            SqliteMcpApprovalEnvelopeRepository::new(Arc::clone(&storage)),
        );
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let first_factory =
            McpApprovalPayloadStoreFactory::new(repository.clone(), credentials.clone()).unwrap();
        let key_ref = first_factory
            .master_key_handle()
            .as_persisted_reference()
            .to_string();
        let store = first_factory.open_or_provision().unwrap();
        let invocation_id =
            McpApprovalInvocationId::parse(uuid::Uuid::new_v4().to_string()).unwrap();
        let aad = aad();
        let payload = McpApprovalPayload::from_json(&serde_json::json!({"value": CANARY})).unwrap();
        store.seal(&invocation_id, aad.clone(), payload).unwrap();

        let persisted = storage
            .load_mcp_approval_envelope(invocation_id.as_str())
            .unwrap()
            .unwrap();
        let inspection = rusqlite::Connection::open(&database_path).unwrap();
        let raw_envelope = inspection
            .query_row(
                "SELECT envelope_json FROM mcp_approval_payload_envelopes \
                 WHERE invocation_id = ?1",
                [invocation_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        let raw_value = serde_json::from_str::<serde_json::Value>(&raw_envelope).unwrap();
        let keys = raw_value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            keys,
            std::collections::BTreeSet::from([
                "ciphertextBase64".to_string(),
                "nonceBase64".to_string(),
            ])
        );
        assert!(!raw_envelope.contains(CANARY));
        assert!(!raw_envelope.contains("aad"));
        assert!(!raw_envelope.contains("argumentsDigest"));
        assert!(!raw_envelope.contains("credentialRef"));
        assert!(!raw_envelope.contains("invocationId"));
        assert_eq!(persisted.aad_digest.len(), 64);
        assert_ne!(persisted.aad_digest, aad.arguments_digest);
        let database = std::fs::read(&database_path).unwrap();
        let database = String::from_utf8_lossy(&database);
        for forbidden in [
            CANARY,
            aad.arguments_digest.as_str(),
            key_ref.as_str(),
            aad.call_id.as_str(),
            aad.server_id.as_str(),
            aad.config_digest.as_str(),
            aad.catalog_digest.as_str(),
            aad.raw_tool_name_digest.as_str(),
            aad.schema_digest.as_str(),
        ] {
            assert!(
                !database.contains(forbidden),
                "database must not persist forbidden MCP approval metadata"
            );
        }
        drop(store);
        drop(first_factory);
        let second_factory = McpApprovalPayloadStoreFactory::new(repository, credentials).unwrap();
        assert_eq!(
            second_factory.master_key_handle().as_persisted_reference(),
            key_ref
        );
        let reopened = second_factory.open_or_provision().unwrap();
        let recovered = reopened.consume(&invocation_id, &aad).unwrap();
        recovered
            .with_json(|value| assert_eq!(value["value"], CANARY))
            .unwrap();
    }
}
