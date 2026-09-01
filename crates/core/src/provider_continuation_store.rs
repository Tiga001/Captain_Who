//! Host-private encrypted persistence for provider-native Assistant Turn replay state.
//!
//! Plaintext exists only in non-serializable `LlmAssistantTurn` values and bounded, zeroizing
//! codec buffers. SQLite receives authenticated ciphertext plus safe ownership/ordering metadata.

use crate::image_generation::{
    CredentialReference, CredentialSecret, CredentialStore, CredentialStoreBackend,
};
use crate::llm::{
    LlmAssistantTurn, LlmRuntimeToolCallBinding, LlmToolCall, ProviderContinuation,
    ProviderContinuationAttachment, ProviderContinuationFragment, ProviderContinuationPosition,
    ProviderContinuationReplayScope, ReasoningProjection,
};
use crate::protocol::{AgentAssistantTurnCheckpointIdentity, ProviderContinuationRef};
use crate::provider_profile::ProviderProtocolKey;
pub(crate) use crate::storage::provider_continuation_repository::ProviderContinuationProjection;
use crate::storage::provider_continuation_repository::{
    ProviderContinuationEnvelopeRecord, ProviderContinuationPromotionOutcome,
    ProviderContinuationRecordState, ProviderContinuationReleaseOutcome,
    ProviderContinuationRuntimeToolIdentity, ProviderContinuationStoreOutcome,
    StoredProviderContinuationRecord,
};
use crate::storage::service::StorageService;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::Arc;
use zeroize::Zeroizing;

pub const PROVIDER_CONTINUATION_CREDENTIAL_SERVICE: &str =
    "com.mycopilot.next.provider-continuation.v1";

const PROVIDER_CONTINUATION_MASTER_KEY_OPAQUE_ID: &str = "41b2776289c64fa69fa24d6560c7af18";
const MASTER_KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;
const CODEC_MAGIC: &[u8; 8] = b"MCPCTRN1";
const CODEC_VERSION: u32 = 1;
const MAX_DECODED_BYTES: usize = 8 * 1024 * 1024;
const MAX_CIPHERTEXT_BYTES: usize = 2 * 1024 * 1024;
const CHACHA20_POLY1305_TAG_BYTES: usize = 16;
const MAX_COMPRESSED_BYTES: usize = MAX_CIPHERTEXT_BYTES - CHACHA20_POLY1305_TAG_BYTES;
const MAX_COLLECTION_ITEMS: usize = 4_096;
const ZSTD_LEVEL: i32 = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProviderContinuationStoreError {
    InvalidReference,
    InvalidBinding,
    InvalidTurn,
    PayloadTooLarge,
    PayloadConflict,
    PayloadNotFound,
    CheckpointStateMissing,
    PayloadReleased,
    InvalidEnvelope,
    AuthenticationFailed,
    CredentialUnavailable,
    RepositoryUnavailable,
    RandomnessUnavailable,
    CompressionFailed,
}

impl ProviderContinuationStoreError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidReference => "provider_continuation.invalid_reference",
            Self::InvalidBinding => "provider_continuation.invalid_binding",
            Self::InvalidTurn => "provider_continuation.invalid_turn",
            Self::PayloadTooLarge => "provider_continuation.payload_too_large",
            Self::PayloadConflict => "provider_continuation.payload_conflict",
            Self::PayloadNotFound => "provider_continuation.payload_not_found",
            Self::CheckpointStateMissing => "provider_continuation_missing",
            Self::PayloadReleased => "provider_continuation.payload_released",
            Self::InvalidEnvelope => "provider_continuation.invalid_envelope",
            Self::AuthenticationFailed => "provider_continuation.authentication_failed",
            Self::CredentialUnavailable => "provider_continuation.credential_unavailable",
            Self::RepositoryUnavailable => "provider_continuation.repository_unavailable",
            Self::RandomnessUnavailable => "provider_continuation.randomness_unavailable",
            Self::CompressionFailed => "provider_continuation.compression_failed",
        }
    }
}

impl std::fmt::Debug for ProviderContinuationStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::fmt::Display for ProviderContinuationStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidReference => "provider continuation reference is invalid",
            Self::InvalidBinding => "provider continuation binding is invalid",
            Self::InvalidTurn => "provider continuation turn is invalid",
            Self::PayloadTooLarge => "provider continuation payload exceeds its safety budget",
            Self::PayloadConflict => "provider continuation identity conflicts with stored state",
            Self::PayloadNotFound => "provider continuation payload is unavailable",
            Self::CheckpointStateMissing => {
                "provider continuation required by the checkpoint is unavailable"
            }
            Self::PayloadReleased => "provider continuation payload has been released",
            Self::InvalidEnvelope => "provider continuation envelope is invalid",
            Self::AuthenticationFailed => "provider continuation authentication failed",
            Self::CredentialUnavailable => {
                "provider continuation encryption credential is unavailable"
            }
            Self::RepositoryUnavailable => "provider continuation repository is unavailable",
            Self::RandomnessUnavailable => "secure randomness is unavailable",
            Self::CompressionFailed => "provider continuation compression failed",
        })
    }
}

impl std::error::Error for ProviderContinuationStoreError {}

#[derive(Clone, Copy)]
pub(crate) struct ProviderContinuationBinding<'a> {
    pub(crate) conversation_id: &'a str,
    pub(crate) assistant_message_id: &'a str,
    pub(crate) run_id: &'a str,
    pub(crate) request_index: u64,
    pub(crate) assistant_turn_id: &'a str,
    pub(crate) assistant_turn_digest: &'a str,
    pub(crate) provider_protocol: &'a ProviderProtocolKey,
}

impl std::fmt::Debug for ProviderContinuationBinding<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderContinuationBinding([REDACTED])")
    }
}

#[derive(Clone, Copy)]
#[allow(dead_code)] // Controlled per-owner lifecycle API; transactional GC uses repository paths.
pub(crate) struct ProviderContinuationOwner<'a> {
    pub(crate) conversation_id: &'a str,
    pub(crate) assistant_message_id: &'a str,
    pub(crate) run_id: &'a str,
}

impl std::fmt::Debug for ProviderContinuationOwner<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderContinuationOwner([REDACTED])")
    }
}

#[allow(dead_code)] // Owner metadata is retained for replay selection and future diagnostics.
pub(crate) struct LoadedProviderAssistantTurn {
    pub(crate) continuation_ref: ProviderContinuationRef,
    pub(crate) conversation_id: String,
    pub(crate) assistant_message_id: String,
    pub(crate) run_id: String,
    pub(crate) request_index: u64,
    pub(crate) projection: Option<ProviderContinuationProjection>,
    pub(crate) assistant_turn: LlmAssistantTurn,
}

impl std::fmt::Debug for LoadedProviderAssistantTurn {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LoadedProviderAssistantTurn([REDACTED])")
    }
}

#[derive(Clone)]
pub(crate) struct ProviderContinuationForkMapping {
    pub(crate) source_ref: ProviderContinuationRef,
    pub(crate) source_record: StoredProviderContinuationRecord,
    pub(crate) source_conversation_id: String,
    pub(crate) source_assistant_message_id: String,
    pub(crate) source_run_id: String,
    pub(crate) request_index: u64,
    pub(crate) target_conversation_id: String,
    pub(crate) target_assistant_message_id: String,
    pub(crate) target_run_id: String,
    pub(crate) runtime_tool_call_id_map: Arc<HashMap<String, String>>,
}

impl std::fmt::Debug for ProviderContinuationForkMapping {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderContinuationForkMapping([REDACTED])")
    }
}

pub(crate) struct PreparedProviderContinuationClone {
    pub(crate) source_ref: ProviderContinuationRef,
    pub(crate) target_ref: ProviderContinuationRef,
    pub(crate) record: ProviderContinuationEnvelopeRecord,
}

impl std::fmt::Debug for PreparedProviderContinuationClone {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PreparedProviderContinuationClone([REDACTED])")
    }
}

pub struct ProviderContinuationVault {
    storage: Arc<StorageService>,
    credentials: Arc<dyn CredentialStore>,
    master_key_ref: CredentialReference,
}

impl std::fmt::Debug for ProviderContinuationVault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderContinuationVault([REDACTED])")
    }
}

pub struct ProviderContinuationVaultFactory;

impl ProviderContinuationVaultFactory {
    pub fn open_or_provision(
        storage: Arc<StorageService>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<ProviderContinuationVault, ProviderContinuationStoreError> {
        let backend = credentials.backend();
        if !matches!(
            backend,
            CredentialStoreBackend::SystemV2
                | CredentialStoreBackend::MacKeychainV2
                | CredentialStoreBackend::DevelopmentFileV1
                | CredentialStoreBackend::InMemoryV1
        ) {
            return Err(ProviderContinuationStoreError::CredentialUnavailable);
        }
        let master_key_ref = CredentialReference::from_stable_opaque_id(
            backend,
            PROVIDER_CONTINUATION_MASTER_KEY_OPAQUE_ID,
        )
        .map_err(|_| ProviderContinuationStoreError::CredentialUnavailable)?;
        if !credentials.supports_reference(&master_key_ref) {
            return Err(ProviderContinuationStoreError::CredentialUnavailable);
        }
        match credentials
            .get(&master_key_ref)
            .map_err(|_| ProviderContinuationStoreError::CredentialUnavailable)?
        {
            Some(_) => {}
            None => {
                let mut key = Zeroizing::new([0_u8; MASTER_KEY_BYTES]);
                SystemRandom::new()
                    .fill(&mut key[..])
                    .map_err(|_| ProviderContinuationStoreError::RandomnessUnavailable)?;
                let secret = CredentialSecret::new(URL_SAFE_NO_PAD.encode(&key[..]))
                    .map_err(|_| ProviderContinuationStoreError::CredentialUnavailable)?;
                credentials
                    .replace(&master_key_ref, secret)
                    .map_err(|_| ProviderContinuationStoreError::CredentialUnavailable)?;
            }
        }
        let vault = ProviderContinuationVault {
            storage,
            credentials,
            master_key_ref,
        };
        vault.with_master_key(|_| Ok(()))?;
        Ok(vault)
    }
}

impl ProviderContinuationVault {
    /// Seals a provider-native Tool-bearing turn without making it replayable. The runtime must
    /// promote the returned reference only after the complete Assistant + ToolCall projection has
    /// been durably published and before any Tool side effect or approval handoff.
    pub(crate) fn persist_staged(
        &self,
        binding: ProviderContinuationBinding<'_>,
        turn: &LlmAssistantTurn,
    ) -> Result<Option<ProviderContinuationRef>, ProviderContinuationStoreError> {
        if !requires_private_provider_replay(turn)? {
            return Ok(None);
        }
        validate_binding(binding, turn)?;
        let runtime_tool_calls = runtime_tool_identities(turn)?;
        if runtime_tool_calls.is_empty() {
            return Err(ProviderContinuationStoreError::InvalidBinding);
        }
        let encoded = encode_turn(turn)?;
        let continuation_ref = ProviderContinuationRef::new();
        let record = self.encrypt_turn(&continuation_ref, binding, runtime_tool_calls, encoded)?;
        match self
            .storage
            .store_staged_provider_continuation(&record)
            .map_err(|_| ProviderContinuationStoreError::RepositoryUnavailable)?
        {
            ProviderContinuationStoreOutcome::Inserted { .. } => Ok(Some(continuation_ref)),
            ProviderContinuationStoreOutcome::Idempotent { continuation_id } => {
                Ok(Some(parse_ref(&continuation_id)?))
            }
            ProviderContinuationStoreOutcome::Conflict => {
                Err(ProviderContinuationStoreError::PayloadConflict)
            }
        }
    }

    /// Seals an ordinary provider-native turn and binds it to one exact Host-private durable
    /// projection. The row remains invisible until the trace observer or terminal-message
    /// transaction commits that exact owner.
    pub(crate) fn persist_staged_with_projection(
        &self,
        binding: ProviderContinuationBinding<'_>,
        projection: ProviderContinuationProjection,
        turn: &LlmAssistantTurn,
    ) -> Result<Option<ProviderContinuationRef>, ProviderContinuationStoreError> {
        if !requires_private_provider_replay(turn)? {
            return Ok(None);
        }
        validate_binding(binding, turn)?;
        let runtime_tool_calls = runtime_tool_identities(turn)?;
        if !runtime_tool_calls.is_empty() {
            return Err(ProviderContinuationStoreError::InvalidBinding);
        }
        let encoded = encode_turn(turn)?;
        let continuation_ref = ProviderContinuationRef::new();
        let record = self.encrypt_turn(&continuation_ref, binding, runtime_tool_calls, encoded)?;
        match self
            .storage
            .store_staged_provider_continuation_with_projection(&record, projection)
            .map_err(|_| ProviderContinuationStoreError::RepositoryUnavailable)?
        {
            ProviderContinuationStoreOutcome::Inserted { .. } => Ok(Some(continuation_ref)),
            ProviderContinuationStoreOutcome::Idempotent { continuation_id } => {
                Ok(Some(parse_ref(&continuation_id)?))
            }
            ProviderContinuationStoreOutcome::Conflict => {
                Err(ProviderContinuationStoreError::PayloadConflict)
            }
        }
    }

    /// Idempotently publishes one exact staged envelope after its durable ToolCall trace commit.
    pub(crate) fn promote_staged(
        &self,
        continuation_ref: &ProviderContinuationRef,
        expected: ProviderContinuationBinding<'_>,
        promoted_at: i64,
    ) -> Result<(), ProviderContinuationStoreError> {
        continuation_ref
            .validate()
            .map_err(|_| ProviderContinuationStoreError::InvalidReference)?;
        let record = self
            .storage
            .load_provider_continuation(&continuation_ref.id)
            .map_err(|_| ProviderContinuationStoreError::RepositoryUnavailable)?
            .ok_or(ProviderContinuationStoreError::PayloadNotFound)?;
        validate_record_binding(&record, expected)?;
        match self
            .storage
            .promote_staged_provider_continuation(&continuation_ref.id, promoted_at)
            .map_err(|_| ProviderContinuationStoreError::RepositoryUnavailable)?
        {
            ProviderContinuationPromotionOutcome::Promoted
            | ProviderContinuationPromotionOutcome::AlreadyPromoted => Ok(()),
            ProviderContinuationPromotionOutcome::Released => {
                Err(ProviderContinuationStoreError::PayloadReleased)
            }
            ProviderContinuationPromotionOutcome::NotFound => {
                Err(ProviderContinuationStoreError::PayloadNotFound)
            }
        }
    }

    #[allow(dead_code)] // Exact-ref restore API; conversation hydration currently batches loads.
    pub(crate) fn load(
        &self,
        continuation_ref: &ProviderContinuationRef,
        expected: ProviderContinuationBinding<'_>,
    ) -> Result<LoadedProviderAssistantTurn, ProviderContinuationStoreError> {
        continuation_ref
            .validate()
            .map_err(|_| ProviderContinuationStoreError::InvalidReference)?;
        let record = self
            .storage
            .load_provider_continuation(&continuation_ref.id)
            .map_err(|_| ProviderContinuationStoreError::RepositoryUnavailable)?
            .ok_or(ProviderContinuationStoreError::PayloadNotFound)?;
        validate_record_binding(&record, expected)?;
        self.decrypt_record(continuation_ref, record, Some(expected.provider_protocol))
    }

    /// Loads every replayable provider-native Tool-bearing turn in durable message order.
    ///
    /// The frozen protocol must match every row exactly. A mixed or stale profile fails the whole
    /// operation; callers must never silently translate encrypted history through another adapter.
    pub(crate) fn list_replayable_for_conversation(
        &self,
        conversation_id: &str,
        expected_protocol: &ProviderProtocolKey,
    ) -> Result<Vec<LoadedProviderAssistantTurn>, ProviderContinuationStoreError> {
        expected_protocol
            .validate()
            .map_err(|_| ProviderContinuationStoreError::InvalidBinding)?;
        let records = self
            .storage
            .list_replayable_provider_continuations_for_conversation(conversation_id)
            .map_err(|_| ProviderContinuationStoreError::RepositoryUnavailable)?;
        self.decrypt_ordered_records(records, expected_protocol)
    }

    /// Reports only whether encrypted provider-native replay state exists for this conversation.
    /// No protocol identity, payload metadata, ciphertext, or plaintext crosses this boundary.
    pub fn has_replayable_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<bool, ProviderContinuationStoreError> {
        self.storage
            .has_replayable_provider_continuations_for_conversation(conversation_id)
            .map_err(|_| ProviderContinuationStoreError::RepositoryUnavailable)
    }

    /// Authenticates every continuation handle required by an approval checkpoint without
    /// returning provider payloads to the Host. This pre-dispatch gate must complete before any
    /// command, filesystem, Office, Skill, or MCP side effect begins.
    pub fn validate_checkpoint_refs(
        &self,
        conversation_id: &str,
        expected_protocol: &ProviderProtocolKey,
        refs: &[ProviderContinuationRef],
    ) -> Result<(), ProviderContinuationStoreError> {
        self.load_checkpoint_refs_exact(conversation_id, expected_protocol, refs)
            .map(|_| ())
    }

    pub fn validate_approval_checkpoint_refs(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        run_id: &str,
        expected_protocol: &ProviderProtocolKey,
        refs: &[ProviderContinuationRef],
        assistant_turn_identity: &AgentAssistantTurnCheckpointIdentity,
    ) -> Result<(), ProviderContinuationStoreError> {
        if assistant_message_id.trim().is_empty() || run_id.trim().is_empty() {
            return Err(ProviderContinuationStoreError::InvalidBinding);
        }
        let loaded = self.load_checkpoint_refs_exact(conversation_id, expected_protocol, refs)?;
        let owns_pending_turn = loaded.iter().any(|loaded| {
            if loaded.assistant_message_id != assistant_message_id || loaded.run_id != run_id {
                return false;
            }
            let turn = &loaded.assistant_turn;
            if turn.stable_id() != assistant_turn_identity.assistant_turn_id
                || turn.stable_digest() != assistant_turn_identity.assistant_turn_digest
            {
                return false;
            }
            let Some(bindings) = turn.runtime_tool_bindings() else {
                return assistant_turn_identity.tool_call_identities.is_empty();
            };
            bindings.len() == assistant_turn_identity.tool_call_identities.len()
                && bindings
                    .iter()
                    .zip(&assistant_turn_identity.tool_call_identities)
                    .all(|(binding, expected)| {
                        u32::try_from(binding.provider_tool_index).ok()
                            == Some(expected.provider_tool_index)
                            && binding.provider_call_id == expected.provider_call_id
                            && binding.runtime_call.id == expected.runtime_call_id
                    })
        });
        if !owns_pending_turn {
            return Err(ProviderContinuationStoreError::CheckpointStateMissing);
        }
        Ok(())
    }

    fn load_checkpoint_refs_exact(
        &self,
        conversation_id: &str,
        expected_protocol: &ProviderProtocolKey,
        refs: &[ProviderContinuationRef],
    ) -> Result<Vec<LoadedProviderAssistantTurn>, ProviderContinuationStoreError> {
        expected_protocol
            .validate()
            .map_err(|_| ProviderContinuationStoreError::InvalidBinding)?;
        if conversation_id.trim().is_empty() || refs.len() > MAX_COLLECTION_ITEMS {
            return Err(ProviderContinuationStoreError::InvalidBinding);
        }
        let records = self
            .storage
            .list_replayable_provider_continuations_for_conversation(conversation_id)
            .map_err(|_| ProviderContinuationStoreError::RepositoryUnavailable)?;
        if records.len() != refs.len() {
            return Err(ProviderContinuationStoreError::CheckpointStateMissing);
        }
        let mut seen = HashSet::with_capacity(refs.len());
        let mut loaded = Vec::with_capacity(refs.len());
        for (continuation_ref, record) in refs.iter().zip(records) {
            continuation_ref
                .validate()
                .map_err(|_| ProviderContinuationStoreError::InvalidReference)?;
            if !seen.insert(continuation_ref.id.as_str()) {
                return Err(ProviderContinuationStoreError::InvalidBinding);
            }
            if record.continuation_id != continuation_ref.id
                || record.conversation_id != conversation_id
            {
                return Err(ProviderContinuationStoreError::CheckpointStateMissing);
            }
            loaded.push(self.decrypt_record(continuation_ref, record, Some(expected_protocol))?);
        }
        Ok(loaded)
    }

    fn decrypt_ordered_records(
        &self,
        records: Vec<StoredProviderContinuationRecord>,
        expected_protocol: &ProviderProtocolKey,
    ) -> Result<Vec<LoadedProviderAssistantTurn>, ProviderContinuationStoreError> {
        if records.len() > MAX_COLLECTION_ITEMS {
            return Err(ProviderContinuationStoreError::PayloadTooLarge);
        }
        records
            .into_iter()
            .map(|record| {
                let continuation_ref = parse_ref(&record.continuation_id)?;
                self.decrypt_record(&continuation_ref, record, Some(expected_protocol))
            })
            .collect()
    }

    #[allow(dead_code)] // Exact-ref release API; compaction/edit paths release in their DB txn.
    pub(crate) fn release(
        &self,
        continuation_ref: &ProviderContinuationRef,
        expected: ProviderContinuationOwner<'_>,
        released_at: i64,
    ) -> Result<(), ProviderContinuationStoreError> {
        continuation_ref
            .validate()
            .map_err(|_| ProviderContinuationStoreError::InvalidReference)?;
        let record = self
            .storage
            .load_provider_continuation(&continuation_ref.id)
            .map_err(|_| ProviderContinuationStoreError::RepositoryUnavailable)?
            .ok_or(ProviderContinuationStoreError::PayloadNotFound)?;
        if record.conversation_id != expected.conversation_id
            || record.assistant_message_id != expected.assistant_message_id
            || record.run_id != expected.run_id
        {
            return Err(ProviderContinuationStoreError::InvalidBinding);
        }
        match self
            .storage
            .release_provider_continuation(&continuation_ref.id, released_at)
            .map_err(|_| ProviderContinuationStoreError::RepositoryUnavailable)?
        {
            ProviderContinuationReleaseOutcome::Released
            | ProviderContinuationReleaseOutcome::AlreadyReleased => Ok(()),
            ProviderContinuationReleaseOutcome::NotFound => {
                Err(ProviderContinuationStoreError::PayloadNotFound)
            }
        }
    }

    /// Decrypts source turns and prepares fresh target-bound ciphertext. The returned records are
    /// not visible until the storage fork transaction inserts them after the cloned messages.
    pub(crate) fn prepare_fork_clones(
        &self,
        mappings: &[ProviderContinuationForkMapping],
    ) -> Result<Vec<PreparedProviderContinuationClone>, ProviderContinuationStoreError> {
        if mappings.len() > MAX_COLLECTION_ITEMS {
            return Err(ProviderContinuationStoreError::PayloadTooLarge);
        }
        let mut prepared = Vec::new();
        for mapping in mappings {
            mapping
                .source_ref
                .validate()
                .map_err(|_| ProviderContinuationStoreError::InvalidReference)?;
            let source = mapping.source_record.clone();
            if source.continuation_id != mapping.source_ref.id {
                return Err(ProviderContinuationStoreError::InvalidBinding);
            }
            if source.conversation_id != mapping.source_conversation_id
                || source.assistant_message_id != mapping.source_assistant_message_id
                || source.run_id != mapping.source_run_id
                || source.request_index != mapping.request_index
            {
                return Err(ProviderContinuationStoreError::InvalidBinding);
            }
            let mut loaded = self.decrypt_record(&mapping.source_ref, source, None)?;
            remap_fork_runtime_tool_call_ids(
                &mut loaded.assistant_turn,
                &mapping.runtime_tool_call_id_map,
            )?;
            let protocol = loaded
                .assistant_turn
                .provider_protocol()
                .ok_or(ProviderContinuationStoreError::InvalidTurn)?;
            let target_ref = ProviderContinuationRef::new();
            let assistant_turn_id = loaded.assistant_turn.stable_id();
            let assistant_turn_digest = loaded.assistant_turn.stable_digest();
            let target_binding = ProviderContinuationBinding {
                conversation_id: &mapping.target_conversation_id,
                assistant_message_id: &mapping.target_assistant_message_id,
                run_id: &mapping.target_run_id,
                request_index: loaded.request_index,
                assistant_turn_id: &assistant_turn_id,
                assistant_turn_digest: &assistant_turn_digest,
                provider_protocol: protocol,
            };
            let runtime_tool_calls = runtime_tool_identities(&loaded.assistant_turn)?;
            let encoded = encode_turn(&loaded.assistant_turn)?;
            let record =
                self.encrypt_turn(&target_ref, target_binding, runtime_tool_calls, encoded)?;
            prepared.push(PreparedProviderContinuationClone {
                source_ref: mapping.source_ref.clone(),
                target_ref,
                record,
            });
        }
        Ok(prepared)
    }

    fn encrypt_turn(
        &self,
        continuation_ref: &ProviderContinuationRef,
        binding: ProviderContinuationBinding<'_>,
        runtime_tool_calls: Vec<ProviderContinuationRuntimeToolIdentity>,
        encoded: Zeroizing<Vec<u8>>,
    ) -> Result<ProviderContinuationEnvelopeRecord, ProviderContinuationStoreError> {
        if encoded.is_empty() || encoded.len() > MAX_DECODED_BYTES {
            return Err(ProviderContinuationStoreError::PayloadTooLarge);
        }
        let payload_digest = sha256_digest(&encoded);
        let provider_protocol_digest = protocol_digest(binding.provider_protocol)?;
        let compressed = zstd::stream::encode_all(encoded.as_slice(), ZSTD_LEVEL)
            .map_err(|_| ProviderContinuationStoreError::CompressionFailed)?;
        let mut ciphertext = Zeroizing::new(compressed);
        if ciphertext.is_empty() || ciphertext.len() > MAX_COMPRESSED_BYTES {
            return Err(ProviderContinuationStoreError::PayloadTooLarge);
        }
        let decoded_bytes = encoded.len() as u64;
        let compressed_bytes = ciphertext.len() as u64;
        let created_at = crate::storage::now_ms();
        let aad = authenticated_metadata(
            continuation_ref,
            binding,
            &provider_protocol_digest,
            &payload_digest,
            decoded_bytes,
            compressed_bytes,
            created_at,
        )?;
        let mut nonce = [0_u8; NONCE_BYTES];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| ProviderContinuationStoreError::RandomnessUnavailable)?;
        self.with_master_key(|key| {
            let key = LessSafeKey::new(
                UnboundKey::new(&aead::CHACHA20_POLY1305, key)
                    .map_err(|_| ProviderContinuationStoreError::CredentialUnavailable)?,
            );
            key.seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad.as_slice()),
                &mut *ciphertext,
            )
            .map_err(|_| ProviderContinuationStoreError::AuthenticationFailed)
        })?;
        Ok(ProviderContinuationEnvelopeRecord {
            continuation_id: continuation_ref.id.clone(),
            conversation_id: binding.conversation_id.to_string(),
            assistant_message_id: binding.assistant_message_id.to_string(),
            run_id: binding.run_id.to_string(),
            request_index: binding.request_index,
            assistant_turn_id: binding.assistant_turn_id.to_string(),
            assistant_turn_digest: binding.assistant_turn_digest.to_string(),
            provider_protocol_digest,
            payload_digest,
            nonce: nonce.to_vec(),
            ciphertext: ciphertext.to_vec(),
            decoded_bytes,
            compressed_bytes,
            created_at,
            runtime_tool_calls,
        })
    }

    fn decrypt_record(
        &self,
        continuation_ref: &ProviderContinuationRef,
        record: StoredProviderContinuationRecord,
        expected_protocol: Option<&ProviderProtocolKey>,
    ) -> Result<LoadedProviderAssistantTurn, ProviderContinuationStoreError> {
        if record.state == ProviderContinuationRecordState::Released {
            return Err(ProviderContinuationStoreError::PayloadReleased);
        }
        if record.activated_at.is_none() {
            return Err(ProviderContinuationStoreError::PayloadNotFound);
        }
        let payload_digest = record
            .payload_digest
            .as_deref()
            .ok_or(ProviderContinuationStoreError::InvalidEnvelope)?;
        let nonce = record
            .nonce
            .as_deref()
            .ok_or(ProviderContinuationStoreError::InvalidEnvelope)?;
        let ciphertext = record
            .ciphertext
            .as_deref()
            .ok_or(ProviderContinuationStoreError::InvalidEnvelope)?;
        let decoded_bytes = record
            .decoded_bytes
            .ok_or(ProviderContinuationStoreError::InvalidEnvelope)?;
        let compressed_bytes = record
            .compressed_bytes
            .ok_or(ProviderContinuationStoreError::InvalidEnvelope)?;
        if nonce.len() != NONCE_BYTES
            || ciphertext.len() <= CHACHA20_POLY1305_TAG_BYTES
            || ciphertext.len() > MAX_COMPRESSED_BYTES + CHACHA20_POLY1305_TAG_BYTES
            || compressed_bytes as usize + CHACHA20_POLY1305_TAG_BYTES != ciphertext.len()
            || decoded_bytes == 0
            || decoded_bytes as usize > MAX_DECODED_BYTES
        {
            return Err(ProviderContinuationStoreError::InvalidEnvelope);
        }
        if let Some(expected_protocol) = expected_protocol {
            if protocol_digest(expected_protocol)? != record.provider_protocol_digest {
                return Err(ProviderContinuationStoreError::InvalidBinding);
            }
        }
        let aad = if let Some(expected_protocol) = expected_protocol {
            let metadata_binding = ProviderContinuationBinding {
                conversation_id: &record.conversation_id,
                assistant_message_id: &record.assistant_message_id,
                run_id: &record.run_id,
                request_index: record.request_index,
                assistant_turn_id: &record.assistant_turn_id,
                assistant_turn_digest: &record.assistant_turn_digest,
                provider_protocol: expected_protocol,
            };
            authenticated_metadata(
                continuation_ref,
                metadata_binding,
                &record.provider_protocol_digest,
                payload_digest,
                decoded_bytes,
                compressed_bytes,
                record.created_at,
            )?
        } else {
            authenticated_metadata_from_record(continuation_ref, &record)?
        };
        let mut plaintext = Zeroizing::new(ciphertext.to_vec());
        let plaintext_len = self.with_master_key(|key| {
            let key = LessSafeKey::new(
                UnboundKey::new(&aead::CHACHA20_POLY1305, key)
                    .map_err(|_| ProviderContinuationStoreError::CredentialUnavailable)?,
            );
            let plaintext = key
                .open_in_place(
                    Nonce::try_assume_unique_for_key(nonce)
                        .map_err(|_| ProviderContinuationStoreError::InvalidEnvelope)?,
                    Aad::from(aad.as_slice()),
                    &mut plaintext,
                )
                .map_err(|_| ProviderContinuationStoreError::AuthenticationFailed)?;
            Ok(plaintext.len())
        })?;
        plaintext.truncate(plaintext_len);
        if plaintext.len() != compressed_bytes as usize {
            return Err(ProviderContinuationStoreError::InvalidEnvelope);
        }
        let decoder = zstd::stream::read::Decoder::new(plaintext.as_slice())
            .map_err(|_| ProviderContinuationStoreError::CompressionFailed)?;
        let mut decoded = Zeroizing::new(Vec::with_capacity(decoded_bytes as usize));
        decoder
            .take((MAX_DECODED_BYTES as u64).saturating_add(1))
            .read_to_end(&mut decoded)
            .map_err(|_| ProviderContinuationStoreError::CompressionFailed)?;
        if decoded.len() != decoded_bytes as usize || decoded.len() > MAX_DECODED_BYTES {
            return Err(ProviderContinuationStoreError::InvalidEnvelope);
        }
        if sha256_digest(&decoded) != payload_digest {
            return Err(ProviderContinuationStoreError::AuthenticationFailed);
        }
        let assistant_turn = decode_turn(&decoded, continuation_ref.clone())?;
        if assistant_turn.stable_id() != record.assistant_turn_id
            || assistant_turn.stable_digest() != record.assistant_turn_digest
        {
            return Err(ProviderContinuationStoreError::AuthenticationFailed);
        }
        let decoded_protocol = assistant_turn
            .provider_protocol()
            .ok_or(ProviderContinuationStoreError::InvalidTurn)?;
        if protocol_digest(decoded_protocol)? != record.provider_protocol_digest {
            return Err(ProviderContinuationStoreError::AuthenticationFailed);
        }
        if expected_protocol.is_some_and(|expected| expected != decoded_protocol) {
            return Err(ProviderContinuationStoreError::InvalidBinding);
        }
        Ok(LoadedProviderAssistantTurn {
            continuation_ref: continuation_ref.clone(),
            conversation_id: record.conversation_id,
            assistant_message_id: record.assistant_message_id,
            run_id: record.run_id,
            request_index: record.request_index,
            projection: record.projection,
            assistant_turn,
        })
    }

    fn with_master_key<T>(
        &self,
        use_key: impl FnOnce(&[u8]) -> Result<T, ProviderContinuationStoreError>,
    ) -> Result<T, ProviderContinuationStoreError> {
        let secret = self
            .credentials
            .get(&self.master_key_ref)
            .map_err(|_| ProviderContinuationStoreError::CredentialUnavailable)?
            .ok_or(ProviderContinuationStoreError::CredentialUnavailable)?;
        let mut key = Zeroizing::new([0_u8; MASTER_KEY_BYTES]);
        let decoded = secret.with_secret_bytes(|encoded| {
            URL_SAFE_NO_PAD
                .decode_slice(encoded, &mut key[..])
                .map_err(|_| ProviderContinuationStoreError::CredentialUnavailable)
        })?;
        if decoded != MASTER_KEY_BYTES {
            return Err(ProviderContinuationStoreError::CredentialUnavailable);
        }
        use_key(&key[..])
    }
}

fn remap_fork_runtime_tool_call_ids(
    turn: &mut LlmAssistantTurn,
    runtime_tool_call_id_map: &HashMap<String, String>,
) -> Result<(), ProviderContinuationStoreError> {
    let Some(bindings) = turn.runtime_tool_bindings() else {
        if turn.provider_tool_calls().is_empty() {
            return Ok(());
        }
        return Err(ProviderContinuationStoreError::InvalidTurn);
    };
    let mut remapped = bindings.to_vec();
    for binding in &mut remapped {
        binding.runtime_call.id = runtime_tool_call_id_map
            .get(&binding.runtime_call.id)
            .cloned()
            .ok_or(ProviderContinuationStoreError::InvalidBinding)?;
    }
    turn.set_runtime_tool_bindings(remapped)
        .map_err(|_| ProviderContinuationStoreError::InvalidTurn)
}

fn validate_binding(
    binding: ProviderContinuationBinding<'_>,
    turn: &LlmAssistantTurn,
) -> Result<(), ProviderContinuationStoreError> {
    binding
        .provider_protocol
        .validate()
        .map_err(|_| ProviderContinuationStoreError::InvalidBinding)?;
    if binding.conversation_id.trim().is_empty()
        || binding.assistant_message_id.trim().is_empty()
        || binding.run_id.trim().is_empty()
        || turn.provider_protocol() != Some(binding.provider_protocol)
        || turn.stable_id() != binding.assistant_turn_id
        || turn.stable_digest() != binding.assistant_turn_digest
    {
        return Err(ProviderContinuationStoreError::InvalidBinding);
    }
    if !requires_private_provider_replay(turn)? {
        return Err(ProviderContinuationStoreError::InvalidBinding);
    }
    Ok(())
}

fn requires_private_provider_replay(
    turn: &LlmAssistantTurn,
) -> Result<bool, ProviderContinuationStoreError> {
    let protocol = turn
        .provider_protocol()
        .ok_or(ProviderContinuationStoreError::InvalidBinding)?;
    protocol
        .validate()
        .map_err(|_| ProviderContinuationStoreError::InvalidBinding)?;
    let capabilities = crate::resolve_provider_runtime_capabilities(protocol)
        .map_err(|_| ProviderContinuationStoreError::InvalidBinding)?;
    Ok(capabilities
        .classify_turn(
            !turn.provider_tool_calls().is_empty(),
            turn.provider_continuation().is_some(),
            crate::ReasoningMode::ProviderDefault,
        )
        .requires_private_replay())
}

fn runtime_tool_identities(
    turn: &LlmAssistantTurn,
) -> Result<Vec<ProviderContinuationRuntimeToolIdentity>, ProviderContinuationStoreError> {
    if turn.provider_tool_calls().is_empty() {
        return Ok(Vec::new());
    }
    let bindings = turn
        .runtime_tool_bindings()
        .ok_or(ProviderContinuationStoreError::InvalidTurn)?;
    if bindings.len() != turn.provider_tool_calls().len() {
        return Err(ProviderContinuationStoreError::InvalidTurn);
    }
    let mut identities = bindings
        .iter()
        .map(|binding| ProviderContinuationRuntimeToolIdentity {
            provider_tool_index: binding.provider_tool_index,
            runtime_call_id: binding.runtime_call.id.clone(),
        })
        .collect::<Vec<_>>();
    identities.sort_by_key(|identity| identity.provider_tool_index);
    Ok(identities)
}

#[allow(dead_code)] // Validation boundary for the exact-ref load API above.
fn validate_record_binding(
    record: &StoredProviderContinuationRecord,
    expected: ProviderContinuationBinding<'_>,
) -> Result<(), ProviderContinuationStoreError> {
    if record.conversation_id != expected.conversation_id
        || record.assistant_message_id != expected.assistant_message_id
        || record.run_id != expected.run_id
        || record.request_index != expected.request_index
        || record.assistant_turn_id != expected.assistant_turn_id
        || record.assistant_turn_digest != expected.assistant_turn_digest
        || record.provider_protocol_digest != protocol_digest(expected.provider_protocol)?
    {
        return Err(ProviderContinuationStoreError::InvalidBinding);
    }
    Ok(())
}

fn parse_ref(value: &str) -> Result<ProviderContinuationRef, ProviderContinuationStoreError> {
    ProviderContinuationRef::parse(1, value.to_string())
        .map_err(|_| ProviderContinuationStoreError::InvalidReference)
}

fn protocol_digest(
    protocol: &ProviderProtocolKey,
) -> Result<String, ProviderContinuationStoreError> {
    protocol
        .validate()
        .map_err(|_| ProviderContinuationStoreError::InvalidBinding)?;
    serde_json::to_vec(protocol)
        .map(|encoded| sha256_digest(&encoded))
        .map_err(|_| ProviderContinuationStoreError::InvalidBinding)
}

fn sha256_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity("sha256:".len() + 64);
    encoded.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn authenticated_metadata(
    continuation_ref: &ProviderContinuationRef,
    binding: ProviderContinuationBinding<'_>,
    provider_protocol_digest: &str,
    payload_digest: &str,
    decoded_bytes: u64,
    compressed_bytes: u64,
    created_at: i64,
) -> Result<Vec<u8>, ProviderContinuationStoreError> {
    continuation_ref
        .validate()
        .map_err(|_| ProviderContinuationStoreError::InvalidReference)?;
    let mut encoded = Vec::new();
    push_bytes(&mut encoded, b"mycopilot.provider-continuation-aad.v1")?;
    push_string(&mut encoded, &continuation_ref.id)?;
    push_string(&mut encoded, binding.conversation_id)?;
    push_string(&mut encoded, binding.assistant_message_id)?;
    push_string(&mut encoded, binding.run_id)?;
    encoded.extend_from_slice(&binding.request_index.to_be_bytes());
    push_string(&mut encoded, binding.assistant_turn_id)?;
    push_string(&mut encoded, binding.assistant_turn_digest)?;
    push_string(&mut encoded, provider_protocol_digest)?;
    push_string(&mut encoded, payload_digest)?;
    encoded.extend_from_slice(&decoded_bytes.to_be_bytes());
    encoded.extend_from_slice(&compressed_bytes.to_be_bytes());
    encoded.extend_from_slice(&created_at.to_be_bytes());
    Ok(encoded)
}

fn authenticated_metadata_from_record(
    continuation_ref: &ProviderContinuationRef,
    record: &StoredProviderContinuationRecord,
) -> Result<Vec<u8>, ProviderContinuationStoreError> {
    let mut encoded = Vec::new();
    push_bytes(&mut encoded, b"mycopilot.provider-continuation-aad.v1")?;
    push_string(&mut encoded, &continuation_ref.id)?;
    push_string(&mut encoded, &record.conversation_id)?;
    push_string(&mut encoded, &record.assistant_message_id)?;
    push_string(&mut encoded, &record.run_id)?;
    encoded.extend_from_slice(&record.request_index.to_be_bytes());
    push_string(&mut encoded, &record.assistant_turn_id)?;
    push_string(&mut encoded, &record.assistant_turn_digest)?;
    push_string(&mut encoded, &record.provider_protocol_digest)?;
    push_string(
        &mut encoded,
        record
            .payload_digest
            .as_deref()
            .ok_or(ProviderContinuationStoreError::InvalidEnvelope)?,
    )?;
    encoded.extend_from_slice(
        &record
            .decoded_bytes
            .ok_or(ProviderContinuationStoreError::InvalidEnvelope)?
            .to_be_bytes(),
    );
    encoded.extend_from_slice(
        &record
            .compressed_bytes
            .ok_or(ProviderContinuationStoreError::InvalidEnvelope)?
            .to_be_bytes(),
    );
    encoded.extend_from_slice(&record.created_at.to_be_bytes());
    Ok(encoded)
}

fn encode_turn(
    turn: &LlmAssistantTurn,
) -> Result<Zeroizing<Vec<u8>>, ProviderContinuationStoreError> {
    let protocol = turn
        .provider_protocol()
        .ok_or(ProviderContinuationStoreError::InvalidTurn)?;
    let mut encoded = Zeroizing::new(Vec::new());
    encoded.extend_from_slice(CODEC_MAGIC);
    encoded.extend_from_slice(&CODEC_VERSION.to_be_bytes());
    push_json(&mut encoded, protocol)?;
    push_string(&mut encoded, turn.provider_visible_text())?;
    match turn.runtime_visible_text() {
        Some(value) => {
            encoded.push(1);
            push_string(&mut encoded, value)?;
        }
        None => encoded.push(0),
    }
    push_count(&mut encoded, turn.provider_tool_calls().len())?;
    for call in turn.provider_tool_calls() {
        encode_tool_call(&mut encoded, call)?;
    }
    match turn.runtime_tool_bindings() {
        Some(bindings) => {
            encoded.push(1);
            push_count(&mut encoded, bindings.len())?;
            for binding in bindings {
                push_u64(&mut encoded, binding.provider_tool_index)?;
                push_string(&mut encoded, &binding.provider_call_id)?;
                encode_tool_call(&mut encoded, &binding.runtime_call)?;
            }
        }
        None => encoded.push(0),
    }
    push_count(&mut encoded, turn.reasoning().len())?;
    for reasoning in turn.reasoning() {
        encode_position(&mut encoded, reasoning.position());
        push_string(&mut encoded, reasoning.summary_text())?;
    }
    match turn.provider_continuation() {
        Some(continuation) => {
            encoded.push(1);
            encoded.push(match continuation.replay_scope() {
                ProviderContinuationReplayScope::AssistantTurnV1 => 0,
                ProviderContinuationReplayScope::InteractionV1 => 1,
            });
            push_count(&mut encoded, continuation.fragments().len())?;
            for fragment in continuation.fragments() {
                encode_position(&mut encoded, fragment.position());
                push_bytes(&mut encoded, fragment.opaque())?;
            }
        }
        None => encoded.push(0),
    }
    if encoded.len() > MAX_DECODED_BYTES {
        return Err(ProviderContinuationStoreError::PayloadTooLarge);
    }
    Ok(encoded)
}

fn decode_turn(
    bytes: &[u8],
    continuation_ref: ProviderContinuationRef,
) -> Result<LlmAssistantTurn, ProviderContinuationStoreError> {
    let mut decoder = Decoder::new(bytes);
    if decoder.take(CODEC_MAGIC.len())? != CODEC_MAGIC {
        return Err(ProviderContinuationStoreError::InvalidEnvelope);
    }
    if decoder.u32()? != CODEC_VERSION {
        return Err(ProviderContinuationStoreError::InvalidEnvelope);
    }
    let protocol: ProviderProtocolKey = decoder.json()?;
    protocol
        .validate()
        .map_err(|_| ProviderContinuationStoreError::InvalidTurn)?;
    let provider_visible_text = decoder.string()?;
    let runtime_visible_text = match decoder.u8()? {
        0 => None,
        1 => Some(decoder.string()?),
        _ => return Err(ProviderContinuationStoreError::InvalidEnvelope),
    };
    let provider_tool_calls = decode_tool_calls(&mut decoder)?;
    let runtime_bindings = match decoder.u8()? {
        0 => None,
        1 => {
            let count = decoder.count()?;
            let mut bindings = Vec::with_capacity(count);
            for _ in 0..count {
                let provider_tool_index = decoder.usize()?;
                let provider_call_id = decoder.string()?;
                let runtime_call = decode_tool_call(&mut decoder)?;
                let provider_call = provider_tool_calls
                    .get(provider_tool_index)
                    .ok_or(ProviderContinuationStoreError::InvalidTurn)?;
                if provider_call.id != provider_call_id {
                    return Err(ProviderContinuationStoreError::InvalidTurn);
                }
                bindings.push(LlmRuntimeToolCallBinding::new(
                    provider_tool_index,
                    provider_call,
                    runtime_call,
                ));
            }
            Some(bindings)
        }
        _ => return Err(ProviderContinuationStoreError::InvalidEnvelope),
    };
    let reasoning_count = decoder.count()?;
    let mut reasoning = Vec::with_capacity(reasoning_count);
    for _ in 0..reasoning_count {
        reasoning.push(ReasoningProjection::summary(
            decode_position(&mut decoder)?,
            decoder.string()?,
        ));
    }
    let continuation = match decoder.u8()? {
        0 => None,
        1 => {
            let replay_scope = match decoder.u8()? {
                0 => ProviderContinuationReplayScope::AssistantTurnV1,
                1 => ProviderContinuationReplayScope::InteractionV1,
                _ => return Err(ProviderContinuationStoreError::InvalidEnvelope),
            };
            let fragment_count = decoder.count()?;
            if fragment_count == 0 {
                return Err(ProviderContinuationStoreError::InvalidTurn);
            }
            let mut fragments = Vec::with_capacity(fragment_count);
            for _ in 0..fragment_count {
                fragments.push(ProviderContinuationFragment::new(
                    decode_position(&mut decoder)?,
                    decoder.bytes()?.to_vec(),
                ));
            }
            Some((replay_scope, fragments))
        }
        _ => return Err(ProviderContinuationStoreError::InvalidEnvelope),
    };
    decoder.finish()?;

    let mut turn = LlmAssistantTurn::from_provider(
        protocol.clone(),
        provider_visible_text,
        provider_tool_calls,
    )
    .map_err(|_| ProviderContinuationStoreError::InvalidTurn)?;
    if let Some(runtime_visible_text) = runtime_visible_text {
        turn.set_runtime_visible_text(runtime_visible_text);
    }
    if let Some(bindings) = runtime_bindings {
        turn.set_runtime_tool_bindings(bindings)
            .map_err(|_| ProviderContinuationStoreError::InvalidTurn)?;
    }
    turn = turn
        .with_reasoning(reasoning)
        .map_err(|_| ProviderContinuationStoreError::InvalidTurn)?;
    if let Some((replay_scope, fragments)) = continuation {
        let continuation =
            ProviderContinuation::new(protocol, replay_scope, turn.digest(), fragments)
                .map_err(|_| ProviderContinuationStoreError::InvalidTurn)?;
        turn = turn
            .with_provider_continuation(continuation)
            .map_err(|_| ProviderContinuationStoreError::InvalidTurn)?;
    }
    turn.with_provider_continuation_ref(continuation_ref)
        .map_err(|_| ProviderContinuationStoreError::InvalidTurn)
}

fn encode_tool_call(
    encoded: &mut Vec<u8>,
    call: &LlmToolCall,
) -> Result<(), ProviderContinuationStoreError> {
    push_string(encoded, &call.id)?;
    push_string(encoded, &call.name)?;
    push_json(encoded, &call.args)
}

fn decode_tool_calls(
    decoder: &mut Decoder<'_>,
) -> Result<Vec<LlmToolCall>, ProviderContinuationStoreError> {
    let count = decoder.count()?;
    let mut calls = Vec::with_capacity(count);
    for _ in 0..count {
        calls.push(decode_tool_call(decoder)?);
    }
    Ok(calls)
}

fn decode_tool_call(
    decoder: &mut Decoder<'_>,
) -> Result<LlmToolCall, ProviderContinuationStoreError> {
    Ok(LlmToolCall {
        id: decoder.string()?,
        name: decoder.string()?,
        args: decoder.json()?,
    })
}

fn encode_position(encoded: &mut Vec<u8>, position: ProviderContinuationPosition) {
    encoded.extend_from_slice(&position.sequence.to_be_bytes());
    match position.attachment {
        ProviderContinuationAttachment::AssistantTurn => encoded.push(0),
        ProviderContinuationAttachment::ContentBlock(index) => {
            encoded.push(1);
            encoded.extend_from_slice(&index.to_be_bytes());
        }
        ProviderContinuationAttachment::ToolCall(index) => {
            encoded.push(2);
            encoded.extend_from_slice(&index.to_be_bytes());
        }
        ProviderContinuationAttachment::InteractionStep(index) => {
            encoded.push(3);
            encoded.extend_from_slice(&index.to_be_bytes());
        }
    }
}

fn decode_position(
    decoder: &mut Decoder<'_>,
) -> Result<ProviderContinuationPosition, ProviderContinuationStoreError> {
    let sequence = decoder.u32()?;
    let attachment = match decoder.u8()? {
        0 => ProviderContinuationAttachment::AssistantTurn,
        1 => ProviderContinuationAttachment::ContentBlock(decoder.u32()?),
        2 => ProviderContinuationAttachment::ToolCall(decoder.u32()?),
        3 => ProviderContinuationAttachment::InteractionStep(decoder.u32()?),
        _ => return Err(ProviderContinuationStoreError::InvalidEnvelope),
    };
    Ok(ProviderContinuationPosition::new(sequence, attachment))
}

fn push_json<T: serde::Serialize>(
    encoded: &mut Vec<u8>,
    value: &T,
) -> Result<(), ProviderContinuationStoreError> {
    let value =
        serde_json::to_vec(value).map_err(|_| ProviderContinuationStoreError::InvalidTurn)?;
    push_bytes(encoded, &value)
}

fn push_string(encoded: &mut Vec<u8>, value: &str) -> Result<(), ProviderContinuationStoreError> {
    push_bytes(encoded, value.as_bytes())
}

fn push_bytes(encoded: &mut Vec<u8>, value: &[u8]) -> Result<(), ProviderContinuationStoreError> {
    let length =
        u32::try_from(value.len()).map_err(|_| ProviderContinuationStoreError::PayloadTooLarge)?;
    encoded.extend_from_slice(&length.to_be_bytes());
    encoded.extend_from_slice(value);
    if encoded.len() > MAX_DECODED_BYTES {
        return Err(ProviderContinuationStoreError::PayloadTooLarge);
    }
    Ok(())
}

fn push_count(encoded: &mut Vec<u8>, count: usize) -> Result<(), ProviderContinuationStoreError> {
    if count > MAX_COLLECTION_ITEMS {
        return Err(ProviderContinuationStoreError::PayloadTooLarge);
    }
    let count =
        u32::try_from(count).map_err(|_| ProviderContinuationStoreError::PayloadTooLarge)?;
    encoded.extend_from_slice(&count.to_be_bytes());
    Ok(())
}

fn push_u64(encoded: &mut Vec<u8>, value: usize) -> Result<(), ProviderContinuationStoreError> {
    let value =
        u64::try_from(value).map_err(|_| ProviderContinuationStoreError::PayloadTooLarge)?;
    encoded.extend_from_slice(&value.to_be_bytes());
    Ok(())
}

struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], ProviderContinuationStoreError> {
        let end = self
            .offset
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(ProviderContinuationStoreError::InvalidEnvelope)?;
        let value = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, ProviderContinuationStoreError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, ProviderContinuationStoreError> {
        let bytes: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| ProviderContinuationStoreError::InvalidEnvelope)?;
        Ok(u32::from_be_bytes(bytes))
    }

    fn usize(&mut self) -> Result<usize, ProviderContinuationStoreError> {
        let bytes: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| ProviderContinuationStoreError::InvalidEnvelope)?;
        usize::try_from(u64::from_be_bytes(bytes))
            .map_err(|_| ProviderContinuationStoreError::InvalidEnvelope)
    }

    fn count(&mut self) -> Result<usize, ProviderContinuationStoreError> {
        let count = self.u32()? as usize;
        if count > MAX_COLLECTION_ITEMS {
            return Err(ProviderContinuationStoreError::PayloadTooLarge);
        }
        Ok(count)
    }

    fn bytes(&mut self) -> Result<&'a [u8], ProviderContinuationStoreError> {
        let length = self.u32()? as usize;
        self.take(length)
    }

    fn string(&mut self) -> Result<String, ProviderContinuationStoreError> {
        std::str::from_utf8(self.bytes()?)
            .map(str::to_string)
            .map_err(|_| ProviderContinuationStoreError::InvalidEnvelope)
    }

    fn json<T: serde::de::DeserializeOwned>(
        &mut self,
    ) -> Result<T, ProviderContinuationStoreError> {
        serde_json::from_slice(self.bytes()?)
            .map_err(|_| ProviderContinuationStoreError::InvalidEnvelope)
    }

    fn finish(self) -> Result<(), ProviderContinuationStoreError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(ProviderContinuationStoreError::InvalidEnvelope)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_generation::InMemoryCredentialStore;
    use crate::provider_profile::{ProviderProfileConfig, ProviderProtocolDialect};
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::{
        AgentApprovalStatus, ConversationTurnTrace, ConversationTurnTraceItem,
        ConversationTurnTraceTerminalStatus, CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };
    use rusqlite::Connection;
    use serde_json::json;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    const CONVERSATION_ID: &str = "provider-vault-conversation";
    const ASSISTANT_MESSAGE_ID: &str = "provider-vault-assistant";
    const RUN_ID: &str = "provider-vault-run";
    const OPAQUE_CANARY: &str = "RAW_PROVIDER_REASONING_CANARY_MUST_STAY_ENCRYPTED";

    struct VaultFixture {
        _root: TempDir,
        database_path: PathBuf,
        storage: Arc<StorageService>,
        credentials: Arc<InMemoryCredentialStore>,
        vault: ProviderContinuationVault,
        protocol: ProviderProtocolKey,
        turn: LlmAssistantTurn,
        assistant_turn_id: String,
        assistant_turn_digest: String,
    }

    fn build_fixture() -> VaultFixture {
        let root = tempfile::tempdir().unwrap();
        let database_path = root.path().join("provider-continuation.sqlite");
        let storage = Arc::new(StorageService::open(&database_path).unwrap());
        storage
            .save_conversation(ChatConversationRecord {
                id: CONVERSATION_ID.to_string(),
                project_id: None,
                model_id: Some("deepseek-v4-flash".to_string()),
                title: "Provider vault fixture".to_string(),
                messages: vec![ChatMessageRecord {
                    id: ASSISTANT_MESSAGE_ID.to_string(),
                    role: "assistant".to_string(),
                    content: "visible durable projection".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: Some(
                        json!({"runId": RUN_ID, "status": "completed"}).to_string(),
                    ),
                    ui_state_json: None,
                }],
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let vault = ProviderContinuationVaultFactory::open_or_provision(
            Arc::clone(&storage),
            credentials.clone(),
        )
        .unwrap();
        let profile = ProviderProfileConfig::deepseek_v4_default();
        let protocol = ProviderProtocolKey::new(
            ProviderProtocolDialect::OpenAiChatCompletions,
            &profile,
            "deepseek-v4-flash",
            Some("provider-configuration-revision-1".to_string()),
        )
        .unwrap();
        let provider_call = LlmToolCall {
            id: "provider-call-1".to_string(),
            name: "read_file".to_string(),
            args: json!({"path": "/workspace/src/lib.rs"}),
        };
        let runtime_call = LlmToolCall {
            id: "runtime-call-1".to_string(),
            name: "read_file".to_string(),
            args: json!({"path": "src/lib.rs"}),
        };
        let turn = LlmAssistantTurn::from_provider(
            protocol.clone(),
            "provider-visible text",
            vec![provider_call.clone()],
        )
        .unwrap()
        .with_runtime_tool_bindings(vec![LlmRuntimeToolCallBinding::new(
            0,
            &provider_call,
            runtime_call,
        )])
        .unwrap()
        .with_reasoning(vec![ReasoningProjection::summary(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::ContentBlock(0)),
            "safe reasoning projection",
        )])
        .unwrap();
        let continuation = ProviderContinuation::new(
            protocol.clone(),
            ProviderContinuationReplayScope::InteractionV1,
            turn.digest(),
            vec![ProviderContinuationFragment::new(
                ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn),
                [vec![1], OPAQUE_CANARY.as_bytes().to_vec()].concat(),
            )],
        )
        .unwrap();
        let turn = turn.with_provider_continuation(continuation).unwrap();
        let assistant_turn_id = turn.stable_id();
        let assistant_turn_digest = turn.stable_digest();
        VaultFixture {
            _root: root,
            database_path,
            storage,
            credentials,
            vault,
            protocol,
            turn,
            assistant_turn_id,
            assistant_turn_digest,
        }
    }

    impl VaultFixture {
        fn binding(&self) -> ProviderContinuationBinding<'_> {
            ProviderContinuationBinding {
                conversation_id: CONVERSATION_ID,
                assistant_message_id: ASSISTANT_MESSAGE_ID,
                run_id: RUN_ID,
                request_index: 0,
                assistant_turn_id: &self.assistant_turn_id,
                assistant_turn_digest: &self.assistant_turn_digest,
                provider_protocol: &self.protocol,
            }
        }

        fn persist_active(&self) -> ProviderContinuationRef {
            let continuation_ref = self
                .vault
                .persist_staged(self.binding(), &self.turn)
                .unwrap()
                .unwrap();
            self.vault
                .promote_staged(&continuation_ref, self.binding(), 2)
                .unwrap();
            continuation_ref
        }

        fn replace_with_two_call_turn(&mut self) {
            let provider_calls = vec![
                LlmToolCall {
                    id: "provider-call-1".to_string(),
                    name: "read_file".to_string(),
                    args: json!({"path": "/workspace/src/lib.rs"}),
                },
                LlmToolCall {
                    id: "provider-call-2".to_string(),
                    name: "read_file".to_string(),
                    args: json!({"path": "/workspace/src/main.rs"}),
                },
            ];
            let runtime_calls = vec![
                LlmToolCall {
                    id: "runtime-call-1".to_string(),
                    name: "read_file".to_string(),
                    args: json!({"path": "src/lib.rs"}),
                },
                LlmToolCall {
                    id: "runtime-call-2".to_string(),
                    name: "read_file".to_string(),
                    args: json!({"path": "src/main.rs"}),
                },
            ];
            let turn = LlmAssistantTurn::from_provider(
                self.protocol.clone(),
                "provider-visible text",
                provider_calls.clone(),
            )
            .unwrap()
            .with_runtime_tool_bindings(
                provider_calls
                    .iter()
                    .zip(runtime_calls)
                    .enumerate()
                    .map(|(provider_tool_index, (provider_call, runtime_call))| {
                        LlmRuntimeToolCallBinding::new(
                            provider_tool_index,
                            provider_call,
                            runtime_call,
                        )
                    })
                    .collect(),
            )
            .unwrap()
            .with_reasoning(vec![ReasoningProjection::summary(
                ProviderContinuationPosition::new(
                    0,
                    ProviderContinuationAttachment::ContentBlock(0),
                ),
                "safe reasoning projection",
            )])
            .unwrap();
            let continuation = ProviderContinuation::new(
                self.protocol.clone(),
                ProviderContinuationReplayScope::InteractionV1,
                turn.digest(),
                vec![ProviderContinuationFragment::new(
                    ProviderContinuationPosition::new(
                        0,
                        ProviderContinuationAttachment::AssistantTurn,
                    ),
                    [vec![1], OPAQUE_CANARY.as_bytes().to_vec()].concat(),
                )],
            )
            .unwrap();
            self.turn = turn.with_provider_continuation(continuation).unwrap();
            self.assistant_turn_id = self.turn.stable_id();
            self.assistant_turn_digest = self.turn.stable_digest();
        }
    }

    fn database_files(database_path: &Path) -> Vec<PathBuf> {
        [
            database_path.to_path_buf(),
            PathBuf::from(format!("{}-wal", database_path.display())),
            PathBuf::from(format!("{}-journal", database_path.display())),
        ]
        .into_iter()
        .filter(|path| path.exists())
        .collect()
    }

    #[test]
    fn encrypted_round_trip_is_restart_stable_and_never_persists_raw_provider_state() {
        let fixture = build_fixture();
        let continuation_ref = fixture.persist_active();
        let idempotent_ref = fixture.persist_active();
        assert_eq!(continuation_ref, idempotent_ref);

        let reopened = ProviderContinuationVaultFactory::open_or_provision(
            Arc::clone(&fixture.storage),
            fixture.credentials.clone(),
        )
        .unwrap();
        let loaded = reopened.load(&continuation_ref, fixture.binding()).unwrap();
        assert_eq!(loaded.conversation_id, CONVERSATION_ID);
        assert_eq!(loaded.assistant_message_id, ASSISTANT_MESSAGE_ID);
        assert_eq!(loaded.run_id, RUN_ID);
        assert_eq!(loaded.request_index, 0);
        assert_eq!(
            loaded.assistant_turn.provider_visible_text(),
            "provider-visible text"
        );
        assert_eq!(loaded.assistant_turn.provider_tool_calls().len(), 1);
        assert_eq!(
            loaded
                .assistant_turn
                .provider_continuation()
                .unwrap()
                .fragments()[0]
                .opaque(),
            [vec![1], OPAQUE_CANARY.as_bytes().to_vec()].concat()
        );
        assert!(!format!("{:?}{:?}", reopened, loaded).contains(OPAQUE_CANARY));
        for path in database_files(&fixture.database_path) {
            let bytes = std::fs::read(path).unwrap();
            assert!(
                !bytes
                    .windows(OPAQUE_CANARY.len())
                    .any(|window| window == OPAQUE_CANARY.as_bytes()),
                "raw Provider continuation leaked into SQLite"
            );
        }
    }

    #[test]
    fn staged_orphan_is_invisible_but_any_durable_bound_call_recovers_the_provider_boundary() {
        let mut fixture = build_fixture();
        fixture.replace_with_two_call_turn();
        let continuation_ref = fixture
            .vault
            .persist_staged(fixture.binding(), &fixture.turn)
            .unwrap()
            .unwrap();

        assert!(!fixture
            .vault
            .has_replayable_for_conversation(CONVERSATION_ID)
            .unwrap());
        assert!(fixture
            .vault
            .list_replayable_for_conversation(CONVERSATION_ID, &fixture.protocol)
            .unwrap()
            .is_empty());
        assert_eq!(
            fixture
                .vault
                .load(&continuation_ref, fixture.binding())
                .unwrap_err(),
            ProviderContinuationStoreError::PayloadNotFound
        );

        fixture
            .storage
            .append_in_progress_conversation_turn_trace(
                &ConversationTurnTrace {
                    schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                    run_id: RUN_ID.to_string(),
                    conversation_id: CONVERSATION_ID.to_string(),
                    assistant_message_id: ASSISTANT_MESSAGE_ID.to_string(),
                    terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
                    terminal_error: None,
                    truncated: false,
                    items: vec![ConversationTurnTraceItem::ToolCall {
                        sequence: 0,
                        call_id: "runtime-call-1".to_string(),
                        tool: "read_file".to_string(),
                        provenance: crate::AgentToolIdentity::Builtin {
                            tool_name: "read_file".to_string(),
                        },
                        operation: json!({"path": "src/lib.rs"}),
                        approval_status: AgentApprovalStatus::NotRequired,
                        truncated: false,
                    }],
                },
                2,
                2,
            )
            .unwrap();

        let reopened = ProviderContinuationVaultFactory::open_or_provision(
            Arc::clone(&fixture.storage),
            fixture.credentials.clone(),
        )
        .unwrap();
        assert!(reopened
            .has_replayable_for_conversation(CONVERSATION_ID)
            .unwrap());
        let record = fixture
            .storage
            .load_provider_continuation(&continuation_ref.id)
            .unwrap()
            .unwrap();
        assert!(record.activated_at.is_some());
        assert_eq!(
            reopened
                .load(&continuation_ref, fixture.binding())
                .unwrap()
                .assistant_turn
                .provider_tool_calls()
                .len(),
            2
        );
    }

    #[test]
    fn checkpoint_validation_is_exact_and_protocol_mismatch_is_invalid_binding() {
        let fixture = build_fixture();
        let continuation_ref = fixture.persist_active();
        let identity = fixture.turn.checkpoint_identity().unwrap();
        fixture
            .vault
            .validate_approval_checkpoint_refs(
                CONVERSATION_ID,
                ASSISTANT_MESSAGE_ID,
                RUN_ID,
                &fixture.protocol,
                std::slice::from_ref(&continuation_ref),
                &identity,
            )
            .unwrap();
        for (assistant_message_id, run_id) in [
            ("assistant-from-another-turn", RUN_ID),
            (ASSISTANT_MESSAGE_ID, "run-from-another-turn"),
        ] {
            assert_eq!(
                fixture.vault.validate_approval_checkpoint_refs(
                    CONVERSATION_ID,
                    assistant_message_id,
                    run_id,
                    &fixture.protocol,
                    std::slice::from_ref(&continuation_ref),
                    &identity,
                ),
                Err(ProviderContinuationStoreError::CheckpointStateMissing)
            );
        }
        assert_eq!(
            fixture
                .vault
                .validate_checkpoint_refs(CONVERSATION_ID, &fixture.protocol, &[]),
            Err(ProviderContinuationStoreError::CheckpointStateMissing)
        );

        let profile = ProviderProfileConfig::deepseek_v4_default();
        for mismatched_protocol in [
            ProviderProtocolKey::new(
                ProviderProtocolDialect::OpenAiChatCompletions,
                &profile,
                "deepseek-v4-pro",
                Some("provider-configuration-revision-1".to_string()),
            )
            .unwrap(),
            ProviderProtocolKey::new(
                ProviderProtocolDialect::OpenAiChatCompletions,
                &profile,
                "deepseek-v4-flash",
                Some("provider-configuration-revision-2".to_string()),
            )
            .unwrap(),
        ] {
            let error = fixture
                .vault
                .load(
                    &continuation_ref,
                    ProviderContinuationBinding {
                        provider_protocol: &mismatched_protocol,
                        ..fixture.binding()
                    },
                )
                .unwrap_err();
            assert_eq!(error, ProviderContinuationStoreError::InvalidBinding);
        }
    }

    #[test]
    fn deepseek_tool_turn_without_reasoning_fragment_still_gets_a_durable_replay_ref() {
        let mut fixture = build_fixture();
        fixture.turn = fixture
            .turn
            .without_raw_continuation_for_checkpoint()
            .with_reasoning(Vec::new())
            .unwrap();
        fixture.assistant_turn_id = fixture.turn.stable_id();
        fixture.assistant_turn_digest = fixture.turn.stable_digest();
        assert!(fixture.turn.provider_continuation().is_none());

        let continuation_ref = fixture.persist_active();
        let loaded = fixture
            .vault
            .load(&continuation_ref, fixture.binding())
            .unwrap();
        assert!(loaded.assistant_turn.provider_continuation().is_none());
        assert_eq!(loaded.assistant_turn.provider_tool_calls().len(), 1);
        assert_eq!(
            loaded.assistant_turn.provider_tool_calls()[0].id,
            "provider-call-1"
        );
        assert_eq!(
            loaded.assistant_turn.runtime_tool_bindings().unwrap()[0]
                .runtime_call
                .id,
            "runtime-call-1"
        );
        let next_round_history = fixture
            .vault
            .list_replayable_for_conversation(CONVERSATION_ID, &fixture.protocol)
            .unwrap();
        assert_eq!(next_round_history.len(), 1);
        assert_eq!(next_round_history[0].continuation_ref, continuation_ref);
        fixture
            .vault
            .validate_approval_checkpoint_refs(
                CONVERSATION_ID,
                ASSISTANT_MESSAGE_ID,
                RUN_ID,
                &fixture.protocol,
                std::slice::from_ref(&continuation_ref),
                &fixture.turn.checkpoint_identity().unwrap(),
            )
            .unwrap();
    }

    #[test]
    fn generic_tool_turn_does_not_allocate_private_provider_replay_state() {
        let fixture = build_fixture();
        let profile = ProviderProfileConfig::generic_for_dialect(
            ProviderProtocolDialect::OpenAiChatCompletions,
        );
        let protocol = ProviderProtocolKey::new(
            ProviderProtocolDialect::OpenAiChatCompletions,
            &profile,
            "generic-openai-model",
            Some("generic-protocol-revision-1".to_string()),
        )
        .unwrap();
        let turn = LlmAssistantTurn::from_provider(
            protocol.clone(),
            fixture.turn.provider_visible_text(),
            fixture.turn.provider_tool_calls().to_vec(),
        )
        .unwrap()
        .with_runtime_tool_bindings(fixture.turn.runtime_tool_bindings().unwrap().to_vec())
        .unwrap();
        let assistant_turn_id = turn.stable_id();
        let assistant_turn_digest = turn.stable_digest();

        let continuation_ref = fixture
            .vault
            .persist_staged(
                ProviderContinuationBinding {
                    conversation_id: CONVERSATION_ID,
                    assistant_message_id: ASSISTANT_MESSAGE_ID,
                    run_id: RUN_ID,
                    request_index: 0,
                    assistant_turn_id: &assistant_turn_id,
                    assistant_turn_digest: &assistant_turn_digest,
                    provider_protocol: &protocol,
                },
                &turn,
            )
            .unwrap();

        assert!(continuation_ref.is_none());
        assert!(!fixture
            .vault
            .has_replayable_for_conversation(CONVERSATION_ID)
            .unwrap());
    }

    #[test]
    fn wrong_key_tamper_release_and_fork_reseal_all_fail_or_bind_safely() {
        let fixture = build_fixture();
        let continuation_ref = fixture.persist_active();
        let source = fixture
            .storage
            .load_provider_continuation(&continuation_ref.id)
            .unwrap()
            .unwrap();

        let wrong_key_vault = ProviderContinuationVaultFactory::open_or_provision(
            Arc::clone(&fixture.storage),
            Arc::new(InMemoryCredentialStore::default()),
        )
        .unwrap();
        assert_eq!(
            wrong_key_vault
                .load(&continuation_ref, fixture.binding(),)
                .unwrap_err(),
            ProviderContinuationStoreError::AuthenticationFailed
        );

        fixture
            .storage
            .replace_conversation_turn_trace(
                &ConversationTurnTrace {
                    schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                    run_id: RUN_ID.to_string(),
                    conversation_id: CONVERSATION_ID.to_string(),
                    assistant_message_id: ASSISTANT_MESSAGE_ID.to_string(),
                    terminal_status: ConversationTurnTraceTerminalStatus::Completed,
                    terminal_error: None,
                    truncated: false,
                    items: vec![
                        ConversationTurnTraceItem::ToolCall {
                            sequence: 0,
                            call_id: "runtime-call-1".to_string(),
                            tool: "read_file".to_string(),
                            provenance: crate::AgentToolIdentity::Builtin {
                                tool_name: "read_file".to_string(),
                            },
                            operation: json!({"path": "src/lib.rs"}),
                            approval_status: AgentApprovalStatus::NotRequired,
                            truncated: false,
                        },
                        ConversationTurnTraceItem::ToolResult {
                            sequence: 1,
                            call_id: "runtime-call-1".to_string(),
                            tool: "read_file".to_string(),
                            status: crate::ConversationTraceToolResultStatus::Succeeded,
                            success: true,
                            observation: json!({"path": "src/lib.rs"}),
                            approval_status: AgentApprovalStatus::NotRequired,
                            error: None,
                            truncated: false,
                            archive: Default::default(),
                        },
                    ],
                },
                2,
                3,
            )
            .unwrap();

        let forked = fixture
            .storage
            .fork_conversation_request_view_with_provider_continuation_vault(
                crate::storage::models::ForkConversationRequest {
                    request_id: "provider-vault-fork-request".to_string(),
                    source_conversation_id: CONVERSATION_ID.to_string(),
                    fork_point: crate::storage::models::ConversationForkPoint::AssistantReply {
                        assistant_message_id: ASSISTANT_MESSAGE_ID.to_string(),
                    },
                },
                &fixture.vault,
            )
            .unwrap();
        let forked_turns = fixture
            .vault
            .list_replayable_for_conversation(&forked.conversation.id, &fixture.protocol)
            .unwrap();
        assert_eq!(forked_turns.len(), 1);
        assert_ne!(forked_turns[0].continuation_ref, continuation_ref);
        assert_eq!(forked_turns[0].conversation_id, forked.conversation.id);
        let forked_assistant = forked.conversation.messages.last().unwrap();
        let forked_trace = fixture
            .storage
            .get_conversation_turn_trace(&forked_assistant.id)
            .unwrap()
            .unwrap();
        let forked_trace_call_id = match &forked_trace.items[0] {
            ConversationTurnTraceItem::ToolCall { call_id, .. } => call_id,
            item => panic!("expected cloned tool call, got {item:?}"),
        };
        let forked_runtime_call_id = &forked_turns[0]
            .assistant_turn
            .runtime_tool_bindings()
            .unwrap()[0]
            .runtime_call
            .id;
        assert_ne!(forked_runtime_call_id, "runtime-call-1");
        assert_eq!(forked_runtime_call_id, forked_trace_call_id);
        let forked_record = fixture
            .storage
            .load_provider_continuation(&forked_turns[0].continuation_ref.id)
            .unwrap()
            .unwrap();
        assert_ne!(forked_record.nonce, source.nonce);
        assert_ne!(forked_record.ciphertext, source.ciphertext);

        let recursive = fixture
            .storage
            .fork_conversation_request_view_with_provider_continuation_vault(
                crate::storage::models::ForkConversationRequest {
                    request_id: "provider-vault-recursive-fork-request".to_string(),
                    source_conversation_id: forked.conversation.id.clone(),
                    fork_point: crate::storage::models::ConversationForkPoint::AssistantReply {
                        assistant_message_id: forked_assistant.id.clone(),
                    },
                },
                &fixture.vault,
            )
            .unwrap();
        let recursive_turns = fixture
            .vault
            .list_replayable_for_conversation(&recursive.conversation.id, &fixture.protocol)
            .unwrap();
        let recursive_assistant = recursive.conversation.messages.last().unwrap();
        let recursive_trace = fixture
            .storage
            .get_conversation_turn_trace(&recursive_assistant.id)
            .unwrap()
            .unwrap();
        let recursive_trace_call_id = match &recursive_trace.items[0] {
            ConversationTurnTraceItem::ToolCall { call_id, .. } => call_id,
            item => panic!("expected recursively cloned tool call, got {item:?}"),
        };
        let recursive_runtime_call_id = &recursive_turns[0]
            .assistant_turn
            .runtime_tool_bindings()
            .unwrap()[0]
            .runtime_call
            .id;
        assert_ne!(recursive_runtime_call_id, forked_runtime_call_id);
        assert_eq!(recursive_runtime_call_id, recursive_trace_call_id);

        fixture
            .vault
            .release(
                &continuation_ref,
                ProviderContinuationOwner {
                    conversation_id: CONVERSATION_ID,
                    assistant_message_id: ASSISTANT_MESSAGE_ID,
                    run_id: RUN_ID,
                },
                20,
            )
            .unwrap();
        assert_eq!(
            fixture
                .vault
                .load(&continuation_ref, fixture.binding(),)
                .unwrap_err(),
            ProviderContinuationStoreError::PayloadReleased
        );

        let tamper_fixture = build_fixture();
        let continuation_ref = tamper_fixture.persist_active();
        let connection = Connection::open(&tamper_fixture.database_path).unwrap();
        let mut ciphertext = connection
            .query_row(
                "SELECT ciphertext FROM provider_continuations WHERE continuation_id = ?1",
                [&continuation_ref.id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap();
        let last = ciphertext.last_mut().unwrap();
        *last ^= 0x80;
        connection
            .execute(
                "UPDATE provider_continuations SET ciphertext = ?1 WHERE continuation_id = ?2",
                rusqlite::params![ciphertext, &continuation_ref.id],
            )
            .unwrap();
        let fork_error = tamper_fixture
            .storage
            .fork_conversation_request_view_with_provider_continuation_vault(
                crate::storage::models::ForkConversationRequest {
                    request_id: "tampered-provider-vault-fork-request".to_string(),
                    source_conversation_id: CONVERSATION_ID.to_string(),
                    fork_point: crate::storage::models::ConversationForkPoint::AssistantReply {
                        assistant_message_id: ASSISTANT_MESSAGE_ID.to_string(),
                    },
                },
                &tamper_fixture.vault,
            )
            .unwrap_err();
        assert!(fork_error.message().contains("Provider continuation"));
        assert_eq!(
            tamper_fixture.storage.load_conversations().unwrap().len(),
            1,
            "decrypt/reseal failure must leave no visible fork"
        );
        assert_eq!(
            tamper_fixture
                .vault
                .load(&continuation_ref, tamper_fixture.binding(),)
                .unwrap_err(),
            ProviderContinuationStoreError::AuthenticationFailed
        );
    }
}
