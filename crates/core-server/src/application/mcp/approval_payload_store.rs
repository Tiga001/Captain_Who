use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
#[cfg(test)]
use mycopilot_core::image_generation::CredentialDeleteOutcome;
use mycopilot_core::image_generation::{
    CredentialReference, CredentialSecret, CredentialStore, CredentialStoreBackend,
};
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::{Uuid, Version};
use zeroize::Zeroizing;

use mycopilot_core::{AgentMcpApprovalPayloadPersistence, AgentMcpToolApproval};

use crate::pending_action_identity::pending_action_storage_id;

const APPROVAL_ENVELOPE_VERSION: i64 = 3;
const APPROVAL_AAD_SCHEMA: &str = "mycopilot.mcp-approval-aad.v3";
const NONCE_BYTES: usize = 12;
const MASTER_KEY_BYTES: usize = 32;
const MAX_APPROVAL_PAYLOAD_BYTES: usize = 64 * 1024;
const MAX_APPROVAL_PAYLOAD_DEPTH: usize = 32;
const MAX_APPROVAL_PAYLOAD_NODES: usize = 4_096;
const MAX_APPROVAL_OBJECT_PROPERTIES: usize = 256;
const MAX_PERSISTED_ENVELOPE_BYTES: usize = 128 * 1024;
const MAX_METADATA_FIELD_BYTES: usize = 512;
const MAX_EXPIRY_SCAN: usize = 1_000;
const MCP_APPROVAL_MASTER_KEY_OPAQUE_ID: &str = "9e5289f6297448d1b7e8895e5eb0d033";
pub(crate) const MCP_APPROVAL_CREDENTIAL_SERVICE: &str =
    "com.mycopilot.next.mcp-approval-payload.v1";

/// Host-only view of whether a prepared approval can survive startup.
///
/// It intentionally says nothing about current Server/catalog readiness. An `approved` action may
/// remain recoverable while the optional Server is still connecting; complete policy/catalog
/// revalidation remains mandatory at explicit dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpApprovalStartupPayloadState {
    DurableAvailable,
    Expired,
    Unavailable,
}

pub(crate) trait McpApprovalStartupInspector: Send + Sync {
    fn inspect_startup_payload(
        &self,
        approval: &AgentMcpToolApproval,
    ) -> McpApprovalStartupPayloadState;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpApprovalPayloadPersistence {
    Unavailable,
    ProcessOnly,
    DurableAuthenticatedEnvelope,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct McpApprovalInvocationId(String);

impl McpApprovalInvocationId {
    pub(crate) fn generate() -> Result<Self, McpApprovalPayloadStoreError> {
        Ok(Self(Uuid::new_v4().to_string()))
    }

    pub(crate) fn parse(value: impl Into<String>) -> Result<Self, McpApprovalPayloadStoreError> {
        let value = value.into();
        let parsed = Uuid::parse_str(&value)
            .map_err(|_| McpApprovalPayloadStoreError::InvalidInvocationId)?;
        if parsed.is_nil()
            || parsed.get_version() != Some(Version::Random)
            || parsed.to_string() != value
        {
            return Err(McpApprovalPayloadStoreError::InvalidInvocationId);
        }
        Ok(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for McpApprovalInvocationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("McpApprovalInvocationId([OPAQUE])")
    }
}

/// The model-authored MCP argument object.
///
/// This type intentionally has no `Serialize`, `Display`, or value-bearing `Debug`
/// implementation. Callers receive a narrowly scoped borrow and must not copy it into
/// checkpoints, traces, logs, or protocol errors.
pub(crate) struct McpApprovalPayload(Zeroizing<Vec<u8>>);

impl McpApprovalPayload {
    pub(crate) fn from_json(value: &Value) -> Result<Self, McpApprovalPayloadStoreError> {
        validate_payload_budget(value)?;
        let encoded =
            serde_json::to_vec(value).map_err(|_| McpApprovalPayloadStoreError::InvalidPayload)?;
        if encoded.len() > MAX_APPROVAL_PAYLOAD_BYTES {
            return Err(McpApprovalPayloadStoreError::InvalidPayload);
        }
        let canonical = canonical_json(value);
        let bytes = serde_json::to_vec(&canonical)
            .map_err(|_| McpApprovalPayloadStoreError::InvalidPayload)?;
        Self::from_bytes(bytes)
    }

    fn from_bytes(bytes: Vec<u8>) -> Result<Self, McpApprovalPayloadStoreError> {
        if bytes.is_empty() || bytes.len() > MAX_APPROVAL_PAYLOAD_BYTES {
            return Err(McpApprovalPayloadStoreError::InvalidPayload);
        }
        let value = serde_json::from_slice::<Value>(&bytes)
            .map_err(|_| McpApprovalPayloadStoreError::InvalidPayload)?;
        validate_payload_budget(&value)?;
        let canonical = Zeroizing::new(
            serde_json::to_vec(&canonical_json(&value))
                .map_err(|_| McpApprovalPayloadStoreError::InvalidPayload)?,
        );
        if !value.is_object() || canonical.as_slice() != bytes {
            return Err(McpApprovalPayloadStoreError::InvalidPayload);
        }
        Ok(Self(Zeroizing::new(bytes)))
    }

    pub(crate) fn with_json<T>(
        &self,
        expose: impl FnOnce(&Value) -> T,
    ) -> Result<T, McpApprovalPayloadStoreError> {
        let value = serde_json::from_slice::<Value>(&self.0)
            .map_err(|_| McpApprovalPayloadStoreError::InvalidPayload)?;
        Ok(expose(&value))
    }

    fn duplicate(&self) -> Self {
        Self(Zeroizing::new(self.0.to_vec()))
    }

    fn arguments_digest(&self) -> String {
        sha256_hex(self.0.as_slice())
    }
}

/// Iterative preflight which runs before recursive canonicalization or serialization.
///
/// A model-controlled `serde_json::Value` can otherwise be constructed deeply enough to overflow
/// the stack inside the canonicalizer. Once this pass succeeds, subsequent recursion is bounded to
/// a shallow, finite tree.
fn validate_payload_budget(value: &Value) -> Result<(), McpApprovalPayloadStoreError> {
    if !value.is_object() {
        return Err(McpApprovalPayloadStoreError::InvalidPayload);
    }
    let mut stack = vec![(value, 1_usize)];
    let mut nodes = 0_usize;
    while let Some((value, depth)) = stack.pop() {
        nodes = nodes
            .checked_add(1)
            .ok_or(McpApprovalPayloadStoreError::InvalidPayload)?;
        if nodes > MAX_APPROVAL_PAYLOAD_NODES || depth > MAX_APPROVAL_PAYLOAD_DEPTH {
            return Err(McpApprovalPayloadStoreError::InvalidPayload);
        }
        match value {
            Value::Object(object) => {
                if object.len() > MAX_APPROVAL_OBJECT_PROPERTIES {
                    return Err(McpApprovalPayloadStoreError::InvalidPayload);
                }
                stack.extend(
                    object
                        .values()
                        .map(|child| (child, depth.saturating_add(1))),
                );
            }
            Value::Array(array) => {
                stack.extend(array.iter().map(|child| (child, depth.saturating_add(1))));
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    Ok(())
}

impl fmt::Debug for McpApprovalPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("McpApprovalPayload([REDACTED])")
    }
}

/// Safe, bounded metadata authenticated together with one MCP approval payload.
///
/// Raw tool arguments, tool names, descriptions, credentials, headers, and server
/// configuration are deliberately absent. Callers should pass a digest of the raw tool
/// name/schema rather than the server-authored value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpApprovalPayloadAad {
    pub(crate) run_id: String,
    pub(crate) action_id: String,
    pub(crate) call_id: String,
    pub(crate) server_id: String,
    pub(crate) config_epoch: String,
    pub(crate) registry_revision: u64,
    pub(crate) config_digest: String,
    pub(crate) catalog_digest: String,
    pub(crate) catalog_generation: u64,
    pub(crate) raw_tool_name_digest: String,
    pub(crate) schema_digest: String,
    pub(crate) arguments_digest: String,
    pub(crate) payload_persistence: AgentMcpApprovalPayloadPersistence,
    pub(crate) created_at_ms: i64,
    pub(crate) expires_at_ms: i64,
}

impl McpApprovalPayloadAad {
    fn validate(&self) -> Result<(), McpApprovalPayloadStoreError> {
        for value in [
            self.run_id.as_str(),
            self.action_id.as_str(),
            self.call_id.as_str(),
            self.server_id.as_str(),
            self.config_epoch.as_str(),
            self.config_digest.as_str(),
            self.catalog_digest.as_str(),
            self.raw_tool_name_digest.as_str(),
            self.schema_digest.as_str(),
            self.arguments_digest.as_str(),
        ] {
            if value.trim().is_empty()
                || value.len() > MAX_METADATA_FIELD_BYTES
                || value.chars().any(char::is_control)
            {
                return Err(McpApprovalPayloadStoreError::InvalidMetadata);
            }
        }
        let config_epoch = Uuid::parse_str(&self.config_epoch)
            .map_err(|_| McpApprovalPayloadStoreError::InvalidMetadata)?;
        if config_epoch.get_version() != Some(Version::Random)
            || config_epoch.to_string() != self.config_epoch
            || self.registry_revision == 0
            || self.catalog_generation == 0
            || self.created_at_ms <= 0
            || self.expires_at_ms <= self.created_at_ms
        {
            return Err(McpApprovalPayloadStoreError::InvalidMetadata);
        }
        for digest in [
            self.config_digest.as_str(),
            self.catalog_digest.as_str(),
            self.raw_tool_name_digest.as_str(),
            self.schema_digest.as_str(),
            self.arguments_digest.as_str(),
        ] {
            if !is_sha256_digest(digest) {
                return Err(McpApprovalPayloadStoreError::InvalidMetadata);
            }
        }
        Ok(())
    }

    fn ensure_not_expired(&self) -> Result<(), McpApprovalPayloadStoreError> {
        if self.expires_at_ms <= now_ms() {
            return Err(McpApprovalPayloadStoreError::PayloadExpired);
        }
        Ok(())
    }
}

/// Minimal authenticated ciphertext record accepted by envelope repositories.
///
/// The complete AAD and the credential reference deliberately do not have fields here. The Host
/// must reconstruct AAD from the safe pending approval supplied to `load`/`consume`; the durable
/// store instance owns the credential reference out of band.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PersistedMcpApprovalEnvelope {
    invocation_id: String,
    action_binding: String,
    envelope_version: i64,
    aad_digest: String,
    created_at_ms: i64,
    expires_at_ms: i64,
    nonce_base64: String,
    ciphertext_base64: String,
}

impl PersistedMcpApprovalEnvelope {
    pub(crate) fn invocation_id(&self) -> &str {
        &self.invocation_id
    }

    pub(crate) fn action_binding(&self) -> &str {
        &self.action_binding
    }

    pub(crate) const fn version(&self) -> i64 {
        self.envelope_version
    }

    pub(crate) fn nonce_base64(&self) -> &str {
        &self.nonce_base64
    }

    pub(crate) fn ciphertext_base64(&self) -> &str {
        &self.ciphertext_base64
    }

    pub(crate) fn aad_digest(&self) -> &str {
        &self.aad_digest
    }

    pub(crate) fn created_at_ms(&self) -> i64 {
        self.created_at_ms
    }

    pub(crate) fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_persisted_parts(
        invocation_id: String,
        action_binding: String,
        envelope_version: i64,
        nonce_base64: String,
        ciphertext_base64: String,
        aad_digest: String,
        created_at_ms: i64,
        expires_at_ms: i64,
    ) -> Result<Self, McpApprovalPayloadStoreError> {
        let envelope = Self {
            invocation_id,
            action_binding,
            envelope_version,
            aad_digest,
            created_at_ms,
            expires_at_ms,
            nonce_base64,
            ciphertext_base64,
        };
        envelope.validate_stored_shape()?;
        Ok(envelope)
    }

    fn validate_stored_shape(&self) -> Result<(), McpApprovalPayloadStoreError> {
        McpApprovalInvocationId::parse(self.invocation_id.clone())?;
        if self.envelope_version != APPROVAL_ENVELOPE_VERSION
            || self.action_binding.trim().is_empty()
            || self.action_binding.len() > 2_048
            || self.action_binding.chars().any(char::is_control)
            || !is_sha256_digest(&self.aad_digest)
            || self.created_at_ms <= 0
            || self.expires_at_ms <= self.created_at_ms
            || self.nonce_base64.is_empty()
            || self.nonce_base64.len() > MAX_PERSISTED_ENVELOPE_BYTES
            || self.ciphertext_base64.is_empty()
            || self.ciphertext_base64.len() > MAX_PERSISTED_ENVELOPE_BYTES
        {
            return Err(McpApprovalPayloadStoreError::InvalidEnvelope);
        }
        decode_nonce(&self.nonce_base64)?;
        let ciphertext = URL_SAFE_NO_PAD
            .decode(&self.ciphertext_base64)
            .map_err(|_| McpApprovalPayloadStoreError::InvalidEnvelope)?;
        if ciphertext.len() <= aead::CHACHA20_POLY1305.tag_len()
            || ciphertext.len() > MAX_APPROVAL_PAYLOAD_BYTES + aead::CHACHA20_POLY1305.tag_len()
            || URL_SAFE_NO_PAD.encode(&ciphertext) != self.ciphertext_base64
        {
            return Err(McpApprovalPayloadStoreError::InvalidEnvelope);
        }
        Ok(())
    }

    fn validate(
        &self,
        invocation_id: &McpApprovalInvocationId,
        expected_aad: &McpApprovalPayloadAad,
    ) -> Result<Vec<u8>, McpApprovalPayloadStoreError> {
        self.validate_stored_shape()?;
        expected_aad.validate()?;
        let expected_action_binding =
            pending_action_storage_id(&expected_aad.run_id, &expected_aad.action_id);
        if self.invocation_id != invocation_id.as_str()
            || self.action_binding != expected_action_binding
            || self.created_at_ms != expected_aad.created_at_ms
            || self.expires_at_ms != expected_aad.expires_at_ms
        {
            return Err(McpApprovalPayloadStoreError::BindingMismatch);
        }
        let authenticated_metadata = authenticated_metadata(invocation_id, expected_aad)?;
        let expected_aad_digest = sha256_hex(&authenticated_metadata);
        if self.aad_digest != expected_aad_digest {
            return Err(McpApprovalPayloadStoreError::BindingMismatch);
        }
        Ok(authenticated_metadata)
    }
}

impl fmt::Debug for PersistedMcpApprovalEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersistedMcpApprovalEnvelope")
            .field("envelope_version", &self.envelope_version)
            .field("invocation_id", &"[OPAQUE]")
            .field("action_binding", &"[OPAQUE]")
            .field("aad_digest", &"[DIGEST]")
            .field("created_at_ms", &self.created_at_ms)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("nonce", &"[OPAQUE]")
            .field("ciphertext", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpApprovalPayloadStoreError {
    Unavailable,
    InvalidInvocationId,
    InvalidMetadata,
    InvalidPayload,
    PayloadAlreadyExists,
    PayloadNotFound,
    PayloadExpired,
    BindingMismatch,
    InvalidEnvelope,
    AuthenticationFailed,
    CredentialUnavailable,
    RepositoryUnavailable,
    RandomnessUnavailable,
}

impl fmt::Display for McpApprovalPayloadStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "MCP approval payload storage is unavailable",
            Self::InvalidInvocationId => "MCP approval invocation identity is invalid",
            Self::InvalidMetadata => "MCP approval payload metadata is invalid",
            Self::InvalidPayload => "MCP approval payload is invalid",
            Self::PayloadAlreadyExists => "MCP approval payload identity already exists",
            Self::PayloadNotFound => "MCP approval payload is unavailable",
            Self::PayloadExpired => "MCP approval payload has expired",
            Self::BindingMismatch => "MCP approval payload binding does not match",
            Self::InvalidEnvelope => "MCP approval payload envelope is invalid",
            Self::AuthenticationFailed => "MCP approval payload authentication failed",
            Self::CredentialUnavailable => "MCP approval encryption credential is unavailable",
            Self::RepositoryUnavailable => "MCP approval envelope repository is unavailable",
            Self::RandomnessUnavailable => "secure randomness is unavailable",
        })
    }
}

impl std::error::Error for McpApprovalPayloadStoreError {}

pub(crate) trait McpApprovalPayloadStore: Send + Sync {
    fn persistence(&self) -> McpApprovalPayloadPersistence;

    fn seal(
        &self,
        invocation_id: &McpApprovalInvocationId,
        aad: McpApprovalPayloadAad,
        payload: McpApprovalPayload,
    ) -> Result<(), McpApprovalPayloadStoreError>;

    fn load(
        &self,
        invocation_id: &McpApprovalInvocationId,
        expected_aad: &McpApprovalPayloadAad,
    ) -> Result<McpApprovalPayload, McpApprovalPayloadStoreError>;

    fn consume(
        &self,
        invocation_id: &McpApprovalInvocationId,
        expected_aad: &McpApprovalPayloadAad,
    ) -> Result<McpApprovalPayload, McpApprovalPayloadStoreError>;

    fn delete(
        &self,
        invocation_id: &McpApprovalInvocationId,
    ) -> Result<bool, McpApprovalPayloadStoreError>;

    /// Deletes at most one bounded batch of payloads whose persisted expiry is at or before the
    /// supplied wall-clock cutoff.
    ///
    /// Reconciliation is fail-closed garbage collection only. It never loads plaintext, grants an
    /// approval, or authorizes invocation.
    fn reconcile_expired(&self, cutoff_ms: i64) -> Result<usize, McpApprovalPayloadStoreError>;
}

struct InMemoryApprovalPayloadEntry {
    aad: McpApprovalPayloadAad,
    payload: McpApprovalPayload,
}

#[derive(Default)]
pub(crate) struct InMemoryMcpApprovalPayloadStore {
    payloads: Mutex<HashMap<McpApprovalInvocationId, InMemoryApprovalPayloadEntry>>,
}

impl fmt::Debug for InMemoryMcpApprovalPayloadStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InMemoryMcpApprovalPayloadStore([REDACTED])")
    }
}

impl McpApprovalPayloadStore for InMemoryMcpApprovalPayloadStore {
    fn persistence(&self) -> McpApprovalPayloadPersistence {
        McpApprovalPayloadPersistence::ProcessOnly
    }

    fn seal(
        &self,
        invocation_id: &McpApprovalInvocationId,
        aad: McpApprovalPayloadAad,
        payload: McpApprovalPayload,
    ) -> Result<(), McpApprovalPayloadStoreError> {
        aad.validate()?;
        aad.ensure_not_expired()?;
        if payload.arguments_digest() != aad.arguments_digest {
            return Err(McpApprovalPayloadStoreError::BindingMismatch);
        }
        let mut payloads = self
            .payloads
            .lock()
            .map_err(|_| McpApprovalPayloadStoreError::RepositoryUnavailable)?;
        if payloads.contains_key(invocation_id) {
            return Err(McpApprovalPayloadStoreError::PayloadAlreadyExists);
        }
        payloads.insert(
            invocation_id.clone(),
            InMemoryApprovalPayloadEntry { aad, payload },
        );
        Ok(())
    }

    fn load(
        &self,
        invocation_id: &McpApprovalInvocationId,
        expected_aad: &McpApprovalPayloadAad,
    ) -> Result<McpApprovalPayload, McpApprovalPayloadStoreError> {
        expected_aad.validate()?;
        expected_aad.ensure_not_expired()?;
        let payloads = self
            .payloads
            .lock()
            .map_err(|_| McpApprovalPayloadStoreError::RepositoryUnavailable)?;
        let entry = payloads
            .get(invocation_id)
            .ok_or(McpApprovalPayloadStoreError::PayloadNotFound)?;
        if &entry.aad != expected_aad {
            return Err(McpApprovalPayloadStoreError::BindingMismatch);
        }
        Ok(entry.payload.duplicate())
    }

    fn consume(
        &self,
        invocation_id: &McpApprovalInvocationId,
        expected_aad: &McpApprovalPayloadAad,
    ) -> Result<McpApprovalPayload, McpApprovalPayloadStoreError> {
        expected_aad.validate()?;
        expected_aad.ensure_not_expired()?;
        let mut payloads = self
            .payloads
            .lock()
            .map_err(|_| McpApprovalPayloadStoreError::RepositoryUnavailable)?;
        let matches = payloads
            .get(invocation_id)
            .is_some_and(|entry| &entry.aad == expected_aad);
        if !matches {
            return if payloads.contains_key(invocation_id) {
                Err(McpApprovalPayloadStoreError::BindingMismatch)
            } else {
                Err(McpApprovalPayloadStoreError::PayloadNotFound)
            };
        }
        Ok(payloads
            .remove(invocation_id)
            .expect("entry was checked while holding the same lock")
            .payload)
    }

    fn delete(
        &self,
        invocation_id: &McpApprovalInvocationId,
    ) -> Result<bool, McpApprovalPayloadStoreError> {
        Ok(self
            .payloads
            .lock()
            .map_err(|_| McpApprovalPayloadStoreError::RepositoryUnavailable)?
            .remove(invocation_id)
            .is_some())
    }

    fn reconcile_expired(&self, cutoff_ms: i64) -> Result<usize, McpApprovalPayloadStoreError> {
        if cutoff_ms < 0 {
            return Err(McpApprovalPayloadStoreError::InvalidMetadata);
        }
        let mut payloads = self
            .payloads
            .lock()
            .map_err(|_| McpApprovalPayloadStoreError::RepositoryUnavailable)?;
        let previous_len = payloads.len();
        payloads.retain(|_, entry| entry.aad.expires_at_ms > cutoff_ms);
        Ok(previous_len.saturating_sub(payloads.len()))
    }
}

/// Persistence boundary deliberately restricted to authenticated ciphertext envelopes.
///
/// A SQLite adapter may store only the envelope's version, opaque invocation/action bindings,
/// nonce, authenticated ciphertext, AAD digest, and lifecycle timestamps. It cannot receive or
/// serialize `McpApprovalPayload`, full AAD, argument digests, or credential references.
pub(crate) trait McpApprovalEnvelopeRepository: Send + Sync {
    fn insert(
        &self,
        invocation_id: &McpApprovalInvocationId,
        envelope: PersistedMcpApprovalEnvelope,
    ) -> Result<(), McpApprovalPayloadStoreError>;

    fn get(
        &self,
        invocation_id: &McpApprovalInvocationId,
    ) -> Result<Option<PersistedMcpApprovalEnvelope>, McpApprovalPayloadStoreError>;

    fn take(
        &self,
        invocation_id: &McpApprovalInvocationId,
    ) -> Result<Option<PersistedMcpApprovalEnvelope>, McpApprovalPayloadStoreError>;

    fn delete(
        &self,
        invocation_id: &McpApprovalInvocationId,
    ) -> Result<bool, McpApprovalPayloadStoreError>;

    /// Returns bounded opaque identities whose authenticated expiry is at or before `cutoff_ms`.
    ///
    /// Adapters must re-read and authenticate each envelope before consuming it. This listing is
    /// only a garbage-collection hint and never authorizes invocation.
    fn list_expired(
        &self,
        cutoff_ms: i64,
        limit: usize,
    ) -> Result<Vec<McpApprovalInvocationId>, McpApprovalPayloadStoreError>;
}

#[derive(Default)]
pub(crate) struct InMemoryMcpApprovalEnvelopeRepository {
    envelopes: Mutex<HashMap<McpApprovalInvocationId, PersistedMcpApprovalEnvelope>>,
}

impl fmt::Debug for InMemoryMcpApprovalEnvelopeRepository {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InMemoryMcpApprovalEnvelopeRepository([CIPHERTEXT])")
    }
}

impl McpApprovalEnvelopeRepository for InMemoryMcpApprovalEnvelopeRepository {
    fn insert(
        &self,
        invocation_id: &McpApprovalInvocationId,
        envelope: PersistedMcpApprovalEnvelope,
    ) -> Result<(), McpApprovalPayloadStoreError> {
        let mut envelopes = self
            .envelopes
            .lock()
            .map_err(|_| McpApprovalPayloadStoreError::RepositoryUnavailable)?;
        if envelopes.contains_key(invocation_id) {
            return Err(McpApprovalPayloadStoreError::PayloadAlreadyExists);
        }
        envelopes.insert(invocation_id.clone(), envelope);
        Ok(())
    }

    fn get(
        &self,
        invocation_id: &McpApprovalInvocationId,
    ) -> Result<Option<PersistedMcpApprovalEnvelope>, McpApprovalPayloadStoreError> {
        Ok(self
            .envelopes
            .lock()
            .map_err(|_| McpApprovalPayloadStoreError::RepositoryUnavailable)?
            .get(invocation_id)
            .cloned())
    }

    fn take(
        &self,
        invocation_id: &McpApprovalInvocationId,
    ) -> Result<Option<PersistedMcpApprovalEnvelope>, McpApprovalPayloadStoreError> {
        Ok(self
            .envelopes
            .lock()
            .map_err(|_| McpApprovalPayloadStoreError::RepositoryUnavailable)?
            .remove(invocation_id))
    }

    fn delete(
        &self,
        invocation_id: &McpApprovalInvocationId,
    ) -> Result<bool, McpApprovalPayloadStoreError> {
        Ok(self
            .envelopes
            .lock()
            .map_err(|_| McpApprovalPayloadStoreError::RepositoryUnavailable)?
            .remove(invocation_id)
            .is_some())
    }

    fn list_expired(
        &self,
        cutoff_ms: i64,
        limit: usize,
    ) -> Result<Vec<McpApprovalInvocationId>, McpApprovalPayloadStoreError> {
        if cutoff_ms < 0 || limit == 0 || limit > MAX_EXPIRY_SCAN {
            return Err(McpApprovalPayloadStoreError::InvalidMetadata);
        }
        let envelopes = self
            .envelopes
            .lock()
            .map_err(|_| McpApprovalPayloadStoreError::RepositoryUnavailable)?;
        let mut expired = envelopes
            .iter()
            .filter(|(_, envelope)| envelope.expires_at_ms <= cutoff_ms)
            .map(|(invocation_id, _)| invocation_id.clone())
            .collect::<Vec<_>>();
        expired.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        expired.truncate(limit);
        Ok(expired)
    }
}

pub(crate) struct DurableMcpApprovalPayloadStore {
    repository: Arc<dyn McpApprovalEnvelopeRepository>,
    credentials: Arc<dyn CredentialStore>,
    master_key_ref: CredentialReference,
}

/// Out-of-band handle for the durable payload master key.
///
/// This handle is intentionally non-serializable and has a redacted `Debug`. The production
/// factory derives it again on every startup, so no reference needs to enter SQLite, pending
/// actions, checkpoints, events, or Renderer DTOs.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct McpApprovalMasterKeyHandle(CredentialReference);

impl McpApprovalMasterKeyHandle {
    fn for_backend(backend: CredentialStoreBackend) -> Result<Self, McpApprovalPayloadStoreError> {
        CredentialReference::from_stable_opaque_id(backend, MCP_APPROVAL_MASTER_KEY_OPAQUE_ID)
            .map(Self)
            .map_err(|_| McpApprovalPayloadStoreError::CredentialUnavailable)
    }

    #[cfg(test)]
    fn parse(value: impl Into<String>) -> Result<Self, McpApprovalPayloadStoreError> {
        CredentialReference::parse(value)
            .map(Self)
            .map_err(|_| McpApprovalPayloadStoreError::CredentialUnavailable)
    }

    #[cfg(test)]
    pub(crate) fn as_persisted_reference(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for McpApprovalMasterKeyHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("McpApprovalMasterKeyHandle([OPAQUE])")
    }
}

/// Host factory for the singleton durable MCP approval payload key.
///
/// The key handle is derived deterministically from the credential backend and never stored beside
/// an envelope. Only native system backends are accepted in production; `InMemoryV1` remains
/// available solely so tests can model a restart without touching a real credential store.
pub(crate) struct McpApprovalPayloadStoreFactory {
    repository: Arc<dyn McpApprovalEnvelopeRepository>,
    credentials: Arc<dyn CredentialStore>,
    master_key: McpApprovalMasterKeyHandle,
}

impl fmt::Debug for McpApprovalPayloadStoreFactory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpApprovalPayloadStoreFactory")
            .field("repository", &"[CIPHERTEXT]")
            .field("credentials", &"[REDACTED]")
            .field("master_key", &self.master_key)
            .finish()
    }
}

impl McpApprovalPayloadStoreFactory {
    pub(crate) fn new(
        repository: Arc<dyn McpApprovalEnvelopeRepository>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Self, McpApprovalPayloadStoreError> {
        let backend = credentials.backend();
        if !matches!(
            backend,
            CredentialStoreBackend::SystemV2
                | CredentialStoreBackend::MacKeychainV2
                | CredentialStoreBackend::InMemoryV1
        ) {
            return Err(McpApprovalPayloadStoreError::CredentialUnavailable);
        }
        let master_key = McpApprovalMasterKeyHandle::for_backend(backend)?;
        if !credentials.supports_reference(&master_key.0) {
            return Err(McpApprovalPayloadStoreError::CredentialUnavailable);
        }
        Ok(Self {
            repository,
            credentials,
            master_key,
        })
    }

    /// Opens the stable key or provisions it once with 32 bytes of secure randomness.
    ///
    /// A malformed existing key is never overwritten: doing so would silently orphan all
    /// ciphertext. The caller must degrade to a process-only store on every returned error.
    pub(crate) fn open_or_provision(
        &self,
    ) -> Result<DurableMcpApprovalPayloadStore, McpApprovalPayloadStoreError> {
        match self
            .credentials
            .get(&self.master_key.0)
            .map_err(|_| McpApprovalPayloadStoreError::CredentialUnavailable)?
        {
            Some(_) => {}
            None => {
                let mut key = Zeroizing::new([0_u8; MASTER_KEY_BYTES]);
                SystemRandom::new()
                    .fill(&mut key[..])
                    .map_err(|_| McpApprovalPayloadStoreError::RandomnessUnavailable)?;
                let secret = CredentialSecret::new(URL_SAFE_NO_PAD.encode(&key[..]))
                    .map_err(|_| McpApprovalPayloadStoreError::CredentialUnavailable)?;
                self.credentials
                    .replace(&self.master_key.0, secret)
                    .map_err(|_| McpApprovalPayloadStoreError::CredentialUnavailable)?;
            }
        }
        let store = DurableMcpApprovalPayloadStore::open(
            Arc::clone(&self.repository),
            Arc::clone(&self.credentials),
            self.master_key.clone(),
        )?;
        store.with_master_key(|_| Ok(()))?;
        Ok(store)
    }

    #[cfg(test)]
    pub(crate) fn master_key_handle(&self) -> &McpApprovalMasterKeyHandle {
        &self.master_key
    }
}

/// Builds the strongest available store without exposing credential errors or references.
///
/// This is the sole production downgrade boundary: absent, unsupported, locked, or malformed
/// credential backends all become a fresh process-only store. The fallback never uses the
/// development file credential backend.
pub(crate) fn durable_mcp_payload_store_or_process_only(
    repository: Arc<dyn McpApprovalEnvelopeRepository>,
    credentials: Option<Arc<dyn CredentialStore>>,
) -> Arc<dyn McpApprovalPayloadStore> {
    let Some(credentials) = credentials else {
        return Arc::new(InMemoryMcpApprovalPayloadStore::default());
    };
    McpApprovalPayloadStoreFactory::new(repository, credentials)
        .and_then(|factory| factory.open_or_provision())
        .map(|store| Arc::new(store) as Arc<dyn McpApprovalPayloadStore>)
        .unwrap_or_else(|_| Arc::new(InMemoryMcpApprovalPayloadStore::default()))
}

impl fmt::Debug for DurableMcpApprovalPayloadStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DurableMcpApprovalPayloadStore")
            .field("repository", &"[CIPHERTEXT]")
            .field("credentials", &"[REDACTED]")
            .field("master_key_ref", &"[OPAQUE]")
            .finish()
    }
}

impl DurableMcpApprovalPayloadStore {
    /// Test-only low-level constructor for exercising random-reference key lifecycle behavior.
    ///
    /// Production uses [`McpApprovalPayloadStoreFactory`], whose deterministic backend-owned
    /// reference survives restarts without being persisted beside an envelope.
    #[cfg(test)]
    pub(crate) fn provision(
        repository: Arc<dyn McpApprovalEnvelopeRepository>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<(Self, McpApprovalMasterKeyHandle), McpApprovalPayloadStoreError> {
        let master_key_ref = credentials.new_reference();
        let mut key = Zeroizing::new([0_u8; MASTER_KEY_BYTES]);
        SystemRandom::new()
            .fill(&mut key[..])
            .map_err(|_| McpApprovalPayloadStoreError::RandomnessUnavailable)?;
        let encoded = URL_SAFE_NO_PAD.encode(&key[..]);
        let secret = CredentialSecret::new(encoded)
            .map_err(|_| McpApprovalPayloadStoreError::CredentialUnavailable)?;
        credentials
            .replace(&master_key_ref, secret)
            .map_err(|_| McpApprovalPayloadStoreError::CredentialUnavailable)?;
        let store = Self::open(
            repository,
            credentials,
            McpApprovalMasterKeyHandle(master_key_ref.clone()),
        )?;
        Ok((store, McpApprovalMasterKeyHandle(master_key_ref)))
    }

    /// Opens a durable store with an out-of-band key handle loaded by the production Host.
    ///
    /// The repository is intentionally unable to supply or override this handle.
    pub(crate) fn open(
        repository: Arc<dyn McpApprovalEnvelopeRepository>,
        credentials: Arc<dyn CredentialStore>,
        master_key: McpApprovalMasterKeyHandle,
    ) -> Result<Self, McpApprovalPayloadStoreError> {
        let master_key_ref = master_key.0;
        if !credentials.supports_reference(&master_key_ref) {
            return Err(McpApprovalPayloadStoreError::CredentialUnavailable);
        }
        Ok(Self {
            repository,
            credentials,
            master_key_ref,
        })
    }

    fn encrypt(
        &self,
        invocation_id: &McpApprovalInvocationId,
        aad: McpApprovalPayloadAad,
        payload: &McpApprovalPayload,
    ) -> Result<PersistedMcpApprovalEnvelope, McpApprovalPayloadStoreError> {
        aad.validate()?;
        aad.ensure_not_expired()?;
        if payload.arguments_digest() != aad.arguments_digest {
            return Err(McpApprovalPayloadStoreError::BindingMismatch);
        }
        let authenticated_metadata = authenticated_metadata(invocation_id, &aad)?;
        let aad_digest = sha256_hex(&authenticated_metadata);
        let action_binding = pending_action_storage_id(&aad.run_id, &aad.action_id);
        let mut nonce_bytes = [0_u8; NONCE_BYTES];
        SystemRandom::new()
            .fill(&mut nonce_bytes)
            .map_err(|_| McpApprovalPayloadStoreError::RandomnessUnavailable)?;
        let mut ciphertext = Zeroizing::new(payload.0.to_vec());
        self.with_master_key(|key| {
            let key = LessSafeKey::new(
                UnboundKey::new(&aead::CHACHA20_POLY1305, key)
                    .map_err(|_| McpApprovalPayloadStoreError::CredentialUnavailable)?,
            );
            key.seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(authenticated_metadata.as_slice()),
                &mut *ciphertext,
            )
            .map_err(|_| McpApprovalPayloadStoreError::AuthenticationFailed)
        })?;

        Ok(PersistedMcpApprovalEnvelope {
            invocation_id: invocation_id.as_str().to_string(),
            action_binding,
            envelope_version: APPROVAL_ENVELOPE_VERSION,
            aad_digest,
            created_at_ms: aad.created_at_ms,
            expires_at_ms: aad.expires_at_ms,
            nonce_base64: URL_SAFE_NO_PAD.encode(nonce_bytes),
            ciphertext_base64: URL_SAFE_NO_PAD.encode(ciphertext.as_slice()),
        })
    }

    fn decrypt(
        &self,
        invocation_id: &McpApprovalInvocationId,
        expected_aad: &McpApprovalPayloadAad,
        envelope: &PersistedMcpApprovalEnvelope,
    ) -> Result<McpApprovalPayload, McpApprovalPayloadStoreError> {
        expected_aad.ensure_not_expired()?;
        let authenticated_metadata = envelope.validate(invocation_id, expected_aad)?;
        let nonce = decode_nonce(&envelope.nonce_base64)?;
        let mut ciphertext = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(&envelope.ciphertext_base64)
                .map_err(|_| McpApprovalPayloadStoreError::InvalidEnvelope)?,
        );
        if ciphertext.len() <= aead::CHACHA20_POLY1305.tag_len()
            || ciphertext.len() > MAX_APPROVAL_PAYLOAD_BYTES + aead::CHACHA20_POLY1305.tag_len()
        {
            return Err(McpApprovalPayloadStoreError::InvalidEnvelope);
        }
        let plaintext_len = self.with_master_key(|key| {
            let key = LessSafeKey::new(
                UnboundKey::new(&aead::CHACHA20_POLY1305, key)
                    .map_err(|_| McpApprovalPayloadStoreError::CredentialUnavailable)?,
            );
            let plaintext = key
                .open_in_place(
                    Nonce::assume_unique_for_key(nonce),
                    Aad::from(authenticated_metadata.as_slice()),
                    ciphertext.as_mut_slice(),
                )
                .map_err(|_| McpApprovalPayloadStoreError::AuthenticationFailed)?;
            Ok(plaintext.len())
        })?;
        ciphertext.truncate(plaintext_len);
        let payload = McpApprovalPayload::from_bytes(ciphertext.to_vec())?;
        if payload.arguments_digest() != expected_aad.arguments_digest {
            return Err(McpApprovalPayloadStoreError::AuthenticationFailed);
        }
        Ok(payload)
    }

    fn with_master_key<T>(
        &self,
        use_key: impl FnOnce(&[u8]) -> Result<T, McpApprovalPayloadStoreError>,
    ) -> Result<T, McpApprovalPayloadStoreError> {
        let secret = self
            .credentials
            .get(&self.master_key_ref)
            .map_err(|_| McpApprovalPayloadStoreError::CredentialUnavailable)?
            .ok_or(McpApprovalPayloadStoreError::CredentialUnavailable)?;
        let mut key = Zeroizing::new([0_u8; MASTER_KEY_BYTES]);
        let decoded = secret.with_secret_bytes(|encoded| {
            URL_SAFE_NO_PAD
                .decode_slice(encoded, &mut key[..])
                .map_err(|_| McpApprovalPayloadStoreError::CredentialUnavailable)
        })?;
        if decoded != MASTER_KEY_BYTES {
            return Err(McpApprovalPayloadStoreError::CredentialUnavailable);
        }
        use_key(&key[..])
    }

    #[cfg(test)]
    fn delete_master_key_for_test(
        &self,
    ) -> Result<CredentialDeleteOutcome, McpApprovalPayloadStoreError> {
        self.credentials
            .delete(&self.master_key_ref)
            .map_err(|_| McpApprovalPayloadStoreError::CredentialUnavailable)
    }
}

impl McpApprovalPayloadStore for DurableMcpApprovalPayloadStore {
    fn persistence(&self) -> McpApprovalPayloadPersistence {
        McpApprovalPayloadPersistence::DurableAuthenticatedEnvelope
    }

    fn seal(
        &self,
        invocation_id: &McpApprovalInvocationId,
        aad: McpApprovalPayloadAad,
        payload: McpApprovalPayload,
    ) -> Result<(), McpApprovalPayloadStoreError> {
        let envelope = self.encrypt(invocation_id, aad, &payload)?;
        self.repository.insert(invocation_id, envelope)
    }

    fn load(
        &self,
        invocation_id: &McpApprovalInvocationId,
        expected_aad: &McpApprovalPayloadAad,
    ) -> Result<McpApprovalPayload, McpApprovalPayloadStoreError> {
        let envelope = self
            .repository
            .get(invocation_id)?
            .ok_or(McpApprovalPayloadStoreError::PayloadNotFound)?;
        self.decrypt(invocation_id, expected_aad, &envelope)
    }

    fn consume(
        &self,
        invocation_id: &McpApprovalInvocationId,
        expected_aad: &McpApprovalPayloadAad,
    ) -> Result<McpApprovalPayload, McpApprovalPayloadStoreError> {
        let envelope = self
            .repository
            .take(invocation_id)?
            .ok_or(McpApprovalPayloadStoreError::PayloadNotFound)?;
        self.decrypt(invocation_id, expected_aad, &envelope)
    }

    fn delete(
        &self,
        invocation_id: &McpApprovalInvocationId,
    ) -> Result<bool, McpApprovalPayloadStoreError> {
        self.repository.delete(invocation_id)
    }

    fn reconcile_expired(&self, cutoff_ms: i64) -> Result<usize, McpApprovalPayloadStoreError> {
        if cutoff_ms < 0 {
            return Err(McpApprovalPayloadStoreError::InvalidMetadata);
        }
        let expired = self.repository.list_expired(cutoff_ms, MAX_EXPIRY_SCAN)?;
        let mut deleted = 0_usize;
        for invocation_id in expired {
            if self.repository.delete(&invocation_id)? {
                deleted = deleted.saturating_add(1);
            }
        }
        Ok(deleted)
    }
}

#[derive(Debug, Default)]
pub(crate) struct UnavailableMcpApprovalPayloadStore;

impl McpApprovalPayloadStore for UnavailableMcpApprovalPayloadStore {
    fn persistence(&self) -> McpApprovalPayloadPersistence {
        McpApprovalPayloadPersistence::Unavailable
    }

    fn seal(
        &self,
        _: &McpApprovalInvocationId,
        _: McpApprovalPayloadAad,
        _: McpApprovalPayload,
    ) -> Result<(), McpApprovalPayloadStoreError> {
        Err(McpApprovalPayloadStoreError::Unavailable)
    }

    fn load(
        &self,
        _: &McpApprovalInvocationId,
        _: &McpApprovalPayloadAad,
    ) -> Result<McpApprovalPayload, McpApprovalPayloadStoreError> {
        Err(McpApprovalPayloadStoreError::Unavailable)
    }

    fn consume(
        &self,
        _: &McpApprovalInvocationId,
        _: &McpApprovalPayloadAad,
    ) -> Result<McpApprovalPayload, McpApprovalPayloadStoreError> {
        Err(McpApprovalPayloadStoreError::Unavailable)
    }

    fn delete(&self, _: &McpApprovalInvocationId) -> Result<bool, McpApprovalPayloadStoreError> {
        Err(McpApprovalPayloadStoreError::Unavailable)
    }

    fn reconcile_expired(&self, _: i64) -> Result<usize, McpApprovalPayloadStoreError> {
        Err(McpApprovalPayloadStoreError::Unavailable)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthenticatedMcpApprovalMetadata<'a> {
    schema: &'static str,
    invocation_id: &'a str,
    aad: &'a McpApprovalPayloadAad,
}

fn authenticated_metadata(
    invocation_id: &McpApprovalInvocationId,
    aad: &McpApprovalPayloadAad,
) -> Result<Vec<u8>, McpApprovalPayloadStoreError> {
    aad.validate()?;
    serde_json::to_vec(&AuthenticatedMcpApprovalMetadata {
        schema: APPROVAL_AAD_SCHEMA,
        invocation_id: invocation_id.as_str(),
        aad,
    })
    .map_err(|_| McpApprovalPayloadStoreError::InvalidMetadata)
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonical_json).collect()),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut canonical = serde_json::Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonical_json(&values[key]));
            }
            Value::Object(canonical)
        }
        _ => value.clone(),
    }
}

fn is_sha256_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_hex(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    encoded
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn decode_nonce(value: &str) -> Result<[u8; NONCE_BYTES], McpApprovalPayloadStoreError> {
    let mut nonce = [0_u8; NONCE_BYTES];
    let decoded = URL_SAFE_NO_PAD
        .decode_slice(value.as_bytes(), &mut nonce)
        .map_err(|_| McpApprovalPayloadStoreError::InvalidEnvelope)?;
    if decoded != NONCE_BYTES || URL_SAFE_NO_PAD.encode(nonce) != value {
        return Err(McpApprovalPayloadStoreError::InvalidEnvelope);
    }
    Ok(nonce)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::image_generation::{
        CredentialStoreError, CredentialStoreOperation, InMemoryCredentialStore,
    };
    use serde_json::json;

    const CANARY: &str = "MCP_APPROVAL_SECRET_CANARY_DO_NOT_PERSIST";

    fn aad() -> McpApprovalPayloadAad {
        McpApprovalPayloadAad {
            run_id: "run-approval-1".to_string(),
            action_id: "action-approval-1".to_string(),
            call_id: "call-approval-1".to_string(),
            server_id: "018f2ee6-e5fd-7bd0-bf93-55d598670001".to_string(),
            config_epoch: "bf616f04-d3ec-4bd7-825f-731a9f0892f4".to_string(),
            registry_revision: 11,
            config_digest: sha256_hex(b"config-safe-digest"),
            catalog_digest: sha256_hex(b"catalog-safe-digest"),
            catalog_generation: 7,
            raw_tool_name_digest: sha256_hex(b"raw-tool-safe-digest"),
            schema_digest: sha256_hex(b"schema-safe-digest"),
            arguments_digest: payload().arguments_digest(),
            payload_persistence: AgentMcpApprovalPayloadPersistence::DurableAuthenticatedEnvelope,
            created_at_ms: 1_700_000_000_000,
            expires_at_ms: 4_102_444_800_000,
        }
    }

    fn payload() -> McpApprovalPayload {
        McpApprovalPayload::from_json(&json!({
            "query": "safe",
            "password": CANARY,
        }))
        .unwrap()
    }

    fn assert_payload_canary(payload: &McpApprovalPayload) {
        payload
            .with_json(|value| assert_eq!(value["password"], CANARY))
            .unwrap();
    }

    struct UnavailableTestCredentialStore {
        backend: CredentialStoreBackend,
    }

    impl CredentialStore for UnavailableTestCredentialStore {
        fn backend(&self) -> CredentialStoreBackend {
            self.backend
        }

        fn replace(
            &self,
            _: &CredentialReference,
            _: CredentialSecret,
        ) -> Result<(), CredentialStoreError> {
            Err(CredentialStoreError::BackendUnavailable {
                operation: CredentialStoreOperation::Replace,
            })
        }

        fn get(
            &self,
            _: &CredentialReference,
        ) -> Result<Option<CredentialSecret>, CredentialStoreError> {
            Err(CredentialStoreError::BackendUnavailable {
                operation: CredentialStoreOperation::Get,
            })
        }

        fn delete(
            &self,
            _: &CredentialReference,
        ) -> Result<CredentialDeleteOutcome, CredentialStoreError> {
            Err(CredentialStoreError::BackendUnavailable {
                operation: CredentialStoreOperation::Delete,
            })
        }
    }

    #[test]
    fn payload_budget_rejects_excessive_depth_nodes_and_object_properties() {
        let mut deep = Value::String("leaf".to_string());
        for _ in 0..MAX_APPROVAL_PAYLOAD_DEPTH {
            deep = json!({"next": deep});
        }
        assert_eq!(
            McpApprovalPayload::from_json(&deep).unwrap_err(),
            McpApprovalPayloadStoreError::InvalidPayload
        );

        let too_many_nodes = json!({
            "items": (0..MAX_APPROVAL_PAYLOAD_NODES).collect::<Vec<_>>()
        });
        assert_eq!(
            McpApprovalPayload::from_json(&too_many_nodes).unwrap_err(),
            McpApprovalPayloadStoreError::InvalidPayload
        );

        let mut properties = serde_json::Map::new();
        for index in 0..=MAX_APPROVAL_OBJECT_PROPERTIES {
            properties.insert(format!("property_{index}"), Value::Null);
        }
        assert_eq!(
            McpApprovalPayload::from_json(&Value::Object(properties)).unwrap_err(),
            McpApprovalPayloadStoreError::InvalidPayload
        );
    }

    #[test]
    fn process_only_store_is_bound_and_does_not_survive_a_new_instance() {
        let invocation_id = McpApprovalInvocationId::generate().unwrap();
        let store = InMemoryMcpApprovalPayloadStore::default();
        assert_eq!(
            store.persistence(),
            McpApprovalPayloadPersistence::ProcessOnly
        );
        store
            .seal(&invocation_id, aad(), payload())
            .expect("process-only payload should seal");
        assert_payload_canary(&store.load(&invocation_id, &aad()).unwrap());

        let mut wrong_aad = aad();
        wrong_aad.catalog_generation += 1;
        assert_eq!(
            store.load(&invocation_id, &wrong_aad).unwrap_err(),
            McpApprovalPayloadStoreError::BindingMismatch
        );
        assert_eq!(
            InMemoryMcpApprovalPayloadStore::default()
                .load(&invocation_id, &aad())
                .unwrap_err(),
            McpApprovalPayloadStoreError::PayloadNotFound
        );
    }

    #[test]
    fn process_only_consume_is_atomic_and_one_time() {
        let invocation_id = McpApprovalInvocationId::generate().unwrap();
        let store = InMemoryMcpApprovalPayloadStore::default();
        store.seal(&invocation_id, aad(), payload()).unwrap();

        assert_payload_canary(&store.consume(&invocation_id, &aad()).unwrap());
        assert_eq!(
            store.consume(&invocation_id, &aad()).unwrap_err(),
            McpApprovalPayloadStoreError::PayloadNotFound
        );
    }

    #[test]
    fn factory_uses_one_stable_out_of_band_key_across_restarts() {
        let invocation_id = McpApprovalInvocationId::generate().unwrap();
        let repository = Arc::new(InMemoryMcpApprovalEnvelopeRepository::default());
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let first_factory =
            McpApprovalPayloadStoreFactory::new(repository.clone(), credentials.clone()).unwrap();
        let expected_handle = first_factory
            .master_key_handle()
            .as_persisted_reference()
            .to_string();
        let first = first_factory.open_or_provision().unwrap();
        first.seal(&invocation_id, aad(), payload()).unwrap();
        drop(first);
        drop(first_factory);

        let second_factory = McpApprovalPayloadStoreFactory::new(repository, credentials).unwrap();
        assert_eq!(
            second_factory.master_key_handle().as_persisted_reference(),
            expected_handle
        );
        let second = second_factory.open_or_provision().unwrap();
        assert_payload_canary(&second.load(&invocation_id, &aad()).unwrap());
    }

    #[test]
    fn factory_falls_back_to_process_only_without_exposing_backend_failure() {
        assert_ne!(
            MCP_APPROVAL_CREDENTIAL_SERVICE,
            mycopilot_core::image_generation::IMAGE_GENERATION_CREDENTIAL_SERVICE
        );
        let no_backend = durable_mcp_payload_store_or_process_only(
            Arc::new(InMemoryMcpApprovalEnvelopeRepository::default()),
            None,
        );
        assert_eq!(
            no_backend.persistence(),
            McpApprovalPayloadPersistence::ProcessOnly
        );

        let unavailable: Arc<dyn CredentialStore> = Arc::new(UnavailableTestCredentialStore {
            backend: CredentialStoreBackend::InMemoryV1,
        });
        let failed_backend = durable_mcp_payload_store_or_process_only(
            Arc::new(InMemoryMcpApprovalEnvelopeRepository::default()),
            Some(unavailable),
        );
        assert_eq!(
            failed_backend.persistence(),
            McpApprovalPayloadPersistence::ProcessOnly
        );

        let development_backend: Arc<dyn CredentialStore> =
            Arc::new(UnavailableTestCredentialStore {
                backend: CredentialStoreBackend::DevelopmentFileV1,
            });
        assert_eq!(
            durable_mcp_payload_store_or_process_only(
                Arc::new(InMemoryMcpApprovalEnvelopeRepository::default()),
                Some(development_backend),
            )
            .persistence(),
            McpApprovalPayloadPersistence::ProcessOnly
        );
    }

    #[test]
    fn reconciliation_is_bounded_fail_closed_garbage_collection() {
        let process_id = McpApprovalInvocationId::generate().unwrap();
        let process_only = InMemoryMcpApprovalPayloadStore::default();
        process_only.seal(&process_id, aad(), payload()).unwrap();
        assert_eq!(
            process_only
                .reconcile_expired(aad().expires_at_ms - 1)
                .unwrap(),
            0
        );
        assert_eq!(
            process_only.reconcile_expired(aad().expires_at_ms).unwrap(),
            1
        );
        assert_eq!(
            process_only.load(&process_id, &aad()).unwrap_err(),
            McpApprovalPayloadStoreError::PayloadNotFound
        );

        let durable_id = McpApprovalInvocationId::generate().unwrap();
        let repository = Arc::new(InMemoryMcpApprovalEnvelopeRepository::default());
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let factory = McpApprovalPayloadStoreFactory::new(repository.clone(), credentials).unwrap();
        let durable = factory.open_or_provision().unwrap();
        durable.seal(&durable_id, aad(), payload()).unwrap();
        assert_eq!(durable.reconcile_expired(aad().expires_at_ms).unwrap(), 1);
        assert!(repository.get(&durable_id).unwrap().is_none());
        assert_eq!(
            UnavailableMcpApprovalPayloadStore
                .reconcile_expired(aad().expires_at_ms)
                .unwrap_err(),
            McpApprovalPayloadStoreError::Unavailable
        );
    }

    #[test]
    fn durable_envelope_round_trips_across_store_instances_without_plaintext() {
        let invocation_id = McpApprovalInvocationId::generate().unwrap();
        let repository = Arc::new(InMemoryMcpApprovalEnvelopeRepository::default());
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let (store, key_ref) =
            DurableMcpApprovalPayloadStore::provision(repository.clone(), credentials.clone())
                .unwrap();
        assert_eq!(
            store.persistence(),
            McpApprovalPayloadPersistence::DurableAuthenticatedEnvelope
        );
        store.seal(&invocation_id, aad(), payload()).unwrap();

        let envelope = repository.get(&invocation_id).unwrap().unwrap();
        assert_eq!(envelope.invocation_id(), invocation_id.as_str());
        assert_eq!(
            envelope.action_binding(),
            pending_action_storage_id("run-approval-1", "action-approval-1")
        );
        assert_eq!(envelope.version(), APPROVAL_ENVELOPE_VERSION);
        assert_eq!(envelope.created_at_ms(), aad().created_at_ms);
        assert_eq!(envelope.expires_at_ms(), aad().expires_at_ms);
        assert!(is_sha256_digest(envelope.aad_digest()));
        assert_ne!(envelope.aad_digest(), aad().arguments_digest);
        assert!(!envelope.ciphertext_base64().contains(CANARY));
        assert!(!format!("{envelope:?}").contains(CANARY));
        assert!(!format!("{envelope:?}").contains(key_ref.as_persisted_reference()));
        assert!(!format!("{key_ref:?}").contains(key_ref.as_persisted_reference()));

        drop(store);
        let reopened =
            DurableMcpApprovalPayloadStore::open(repository, credentials, key_ref).unwrap();
        assert_payload_canary(&reopened.load(&invocation_id, &aad()).unwrap());
        assert_payload_canary(&reopened.consume(&invocation_id, &aad()).unwrap());
        assert_eq!(
            reopened.load(&invocation_id, &aad()).unwrap_err(),
            McpApprovalPayloadStoreError::PayloadNotFound
        );
    }

    #[test]
    fn durable_envelope_rejects_ciphertext_and_aad_tampering() {
        let invocation_id = McpApprovalInvocationId::generate().unwrap();
        let repository = Arc::new(InMemoryMcpApprovalEnvelopeRepository::default());
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let (store, _) =
            DurableMcpApprovalPayloadStore::provision(repository.clone(), credentials).unwrap();
        store.seal(&invocation_id, aad(), payload()).unwrap();

        {
            let mut envelopes = repository.envelopes.lock().unwrap();
            let envelope = envelopes.get_mut(&invocation_id).unwrap();
            let mut ciphertext = URL_SAFE_NO_PAD.decode(&envelope.ciphertext_base64).unwrap();
            ciphertext[0] ^= 0x80;
            envelope.ciphertext_base64 = URL_SAFE_NO_PAD.encode(ciphertext);
        }
        assert_eq!(
            store.load(&invocation_id, &aad()).unwrap_err(),
            McpApprovalPayloadStoreError::AuthenticationFailed
        );

        let second_id = McpApprovalInvocationId::generate().unwrap();
        store.seal(&second_id, aad(), payload()).unwrap();
        {
            let mut envelopes = repository.envelopes.lock().unwrap();
            let envelope = envelopes.get_mut(&second_id).unwrap();
            let replacement = if envelope.aad_digest.starts_with('f') {
                "e"
            } else {
                "f"
            };
            envelope.aad_digest.replace_range(..1, replacement);
        }
        assert_eq!(
            store.load(&second_id, &aad()).unwrap_err(),
            McpApprovalPayloadStoreError::BindingMismatch
        );

        let third_id = McpApprovalInvocationId::generate().unwrap();
        store.seal(&third_id, aad(), payload()).unwrap();
        let mut reconstructed_wrong = aad();
        reconstructed_wrong.catalog_generation += 1;
        assert_eq!(
            store.load(&third_id, &reconstructed_wrong).unwrap_err(),
            McpApprovalPayloadStoreError::BindingMismatch
        );

        let fourth_id = McpApprovalInvocationId::generate().unwrap();
        store.seal(&fourth_id, aad(), payload()).unwrap();
        let mut wrong_epoch = aad();
        wrong_epoch.config_epoch = Uuid::new_v4().to_string();
        assert_eq!(
            store.load(&fourth_id, &wrong_epoch).unwrap_err(),
            McpApprovalPayloadStoreError::BindingMismatch
        );

        let fifth_id = McpApprovalInvocationId::generate().unwrap();
        store.seal(&fifth_id, aad(), payload()).unwrap();
        let mut wrong_revision = aad();
        wrong_revision.registry_revision += 1;
        assert_eq!(
            store.load(&fifth_id, &wrong_revision).unwrap_err(),
            McpApprovalPayloadStoreError::BindingMismatch
        );

        let sixth_id = McpApprovalInvocationId::generate().unwrap();
        store.seal(&sixth_id, aad(), payload()).unwrap();
        let mut wrong_persistence = aad();
        wrong_persistence.payload_persistence = AgentMcpApprovalPayloadPersistence::ProcessOnly;
        assert_eq!(
            store.load(&sixth_id, &wrong_persistence).unwrap_err(),
            McpApprovalPayloadStoreError::BindingMismatch
        );

        let legacy_id = McpApprovalInvocationId::generate().unwrap();
        store.seal(&legacy_id, aad(), payload()).unwrap();
        {
            let mut envelopes = repository.envelopes.lock().unwrap();
            envelopes.get_mut(&legacy_id).unwrap().envelope_version = 2;
        }
        assert_eq!(
            store.load(&legacy_id, &aad()).unwrap_err(),
            McpApprovalPayloadStoreError::InvalidEnvelope
        );
    }

    #[test]
    fn durable_envelope_fails_closed_when_master_key_is_missing() {
        let invocation_id = McpApprovalInvocationId::generate().unwrap();
        let repository = Arc::new(InMemoryMcpApprovalEnvelopeRepository::default());
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let (store, _) =
            DurableMcpApprovalPayloadStore::provision(repository, credentials).unwrap();
        store.seal(&invocation_id, aad(), payload()).unwrap();
        assert_eq!(
            store.delete_master_key_for_test().unwrap(),
            CredentialDeleteOutcome::Deleted
        );

        assert_eq!(
            store.load(&invocation_id, &aad()).unwrap_err(),
            McpApprovalPayloadStoreError::CredentialUnavailable
        );
    }

    #[test]
    fn unavailable_store_fails_closed_and_debug_output_is_redacted() {
        let invocation_id = McpApprovalInvocationId::generate().unwrap();
        let store = UnavailableMcpApprovalPayloadStore;
        assert_eq!(
            store.persistence(),
            McpApprovalPayloadPersistence::Unavailable
        );
        assert_eq!(
            store.seal(&invocation_id, aad(), payload()).unwrap_err(),
            McpApprovalPayloadStoreError::Unavailable
        );
        assert_eq!(format!("{:?}", payload()), "McpApprovalPayload([REDACTED])");
        assert!(!format!("{invocation_id:?}").contains(invocation_id.as_str()));
    }

    #[test]
    fn envelope_contains_only_minimal_ciphertext_metadata_and_key_handle_is_out_of_band() {
        let invocation_id = McpApprovalInvocationId::generate().unwrap();
        let repository = Arc::new(InMemoryMcpApprovalEnvelopeRepository::default());
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let (store, key_handle) =
            DurableMcpApprovalPayloadStore::provision(repository.clone(), credentials).unwrap();
        store.seal(&invocation_id, aad(), payload()).unwrap();
        let envelope = repository.get(&invocation_id).unwrap().unwrap();

        assert_eq!(envelope.version(), APPROVAL_ENVELOPE_VERSION);
        assert!(decode_nonce(envelope.nonce_base64()).is_ok());
        assert!(is_sha256_digest(envelope.aad_digest()));
        assert!(!format!("{envelope:?}").contains(CANARY));
        assert!(!format!("{envelope:?}").contains("arguments_digest"));
        assert!(!format!("{envelope:?}").contains("credential_ref"));
        let restored_handle =
            McpApprovalMasterKeyHandle::parse(key_handle.as_persisted_reference()).unwrap();
        assert_eq!(restored_handle, key_handle);
    }

    #[test]
    fn invocation_identity_accepts_only_core_canonical_uuid_v4() {
        let generated = McpApprovalInvocationId::generate().unwrap();
        assert_eq!(
            McpApprovalInvocationId::parse(generated.as_str().to_string()).unwrap(),
            generated
        );
        for invalid in [
            format!("{{{}}}", generated.as_str()),
            Uuid::nil().to_string(),
            "018f2ee6-e5fd-7bd0-bf93-55d598670001".to_string(),
            format!("mcp-approval/v1/{}", generated.as_str()),
        ] {
            assert_eq!(
                McpApprovalInvocationId::parse(invalid).unwrap_err(),
                McpApprovalPayloadStoreError::InvalidInvocationId
            );
        }
    }

    #[test]
    fn canonical_arguments_digest_is_bound_and_payload_is_capped_at_64_kib() {
        let left = McpApprovalPayload::from_json(&json!({
            "b": { "d": 4, "c": 3 },
            "a": 1
        }))
        .unwrap();
        let right = McpApprovalPayload::from_json(&json!({
            "a": 1,
            "b": { "c": 3, "d": 4 }
        }))
        .unwrap();
        assert_eq!(left.arguments_digest(), right.arguments_digest());

        let invocation_id = McpApprovalInvocationId::generate().unwrap();
        let store = InMemoryMcpApprovalPayloadStore::default();
        let mut mismatched = aad();
        mismatched.arguments_digest = sha256_hex(b"other arguments");
        assert_eq!(
            store
                .seal(&invocation_id, mismatched, payload())
                .unwrap_err(),
            McpApprovalPayloadStoreError::BindingMismatch
        );
        assert_eq!(
            McpApprovalPayload::from_json(&json!({
                "oversized": "x".repeat(MAX_APPROVAL_PAYLOAD_BYTES)
            }))
            .unwrap_err(),
            McpApprovalPayloadStoreError::InvalidPayload
        );
    }

    #[test]
    fn durable_repository_lists_expired_envelopes_with_a_hard_bound() {
        let first = McpApprovalInvocationId::generate().unwrap();
        let second = McpApprovalInvocationId::generate().unwrap();
        let repository = Arc::new(InMemoryMcpApprovalEnvelopeRepository::default());
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let (store, _) =
            DurableMcpApprovalPayloadStore::provision(repository.clone(), credentials).unwrap();
        let mut first_aad = aad();
        first_aad.expires_at_ms = 4_102_444_800_100;
        let mut second_aad = aad();
        second_aad.expires_at_ms = 4_102_444_800_200;
        store.seal(&first, first_aad, payload()).unwrap();
        store.seal(&second, second_aad, payload()).unwrap();

        assert_eq!(
            repository.list_expired(4_102_444_800_099, 10).unwrap(),
            Vec::new()
        );
        assert_eq!(
            repository.list_expired(4_102_444_800_150, 10).unwrap(),
            vec![first]
        );
        assert_eq!(
            repository
                .list_expired(4_102_444_800_250, MAX_EXPIRY_SCAN + 1)
                .unwrap_err(),
            McpApprovalPayloadStoreError::InvalidMetadata
        );
    }
}
