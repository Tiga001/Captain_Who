//! Isolated, process-local sampling for the composer. Opening another chat does not own or
//! cancel this work. Only a source/configuration change can invalidate a completed candidate.
use super::execution_access::ExecutionAccessToken;
use super::*;
use mycopilot_core::storage::models::{ModelConfigRecord, ModelConnectionConfig};
use mycopilot_core::{AgentPromptPredictionSeed, ProviderUsageSemantics};
use std::collections::VecDeque;
use tokio::sync::{OnceCell, Semaphore};

const MAX_RESULTS: usize = 64;
const MAX_HOT_SEEDS: usize = 4;
const MAX_HOT_BYTES: usize = 32 * 1024 * 1024;
const FAILURE_BACKOFF: Duration = Duration::from_secs(1);

#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PromptPredictionInput {
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub model_id: String,
}

struct HotSeed {
    conversation: String,
    assistant: String,
    run: String,
    model: String,
    protocol_revision: String,
    connection_revision: String,
    preference: i64,
    access: ExecutionAccessToken,
    version: Option<ConversationHistoryVersion>,
    seed: Arc<AgentPromptPredictionSeed>,
}

#[derive(PartialEq, Eq)]
struct Key {
    conversation: String,
    assistant: String,
    model: String,
    config: String,
    history: ConversationHistoryVersion,
    preference: i64,
    access: ExecutionAccessToken,
}

struct Entry {
    order: AtomicU64,
    key: Key,
    token: AgentCancellationToken,
    result: OnceCell<Result<Option<String>, Instant>>,
}

#[derive(Default)]
struct Cache {
    epoch: u64,
    hot: VecDeque<HotSeed>,
    entries: VecDeque<Arc<Entry>>,
}

pub(super) struct Predictions {
    cache: Mutex<Cache>,
    capacity: Semaphore,
    order: AtomicU64,
    closed: AtomicBool,
}

impl Default for Predictions {
    fn default() -> Self {
        Self {
            cache: Mutex::default(),
            capacity: Semaphore::new(3),
            order: AtomicU64::new(0),
            closed: AtomicBool::new(false),
        }
    }
}

impl Predictions {
    fn cache(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub(super) fn invalidate_chat(&self, conversation: &str) {
        let mut cache = self.cache();
        cache.hot.retain(|item| item.conversation != conversation);
        cache.entries.retain(|entry| {
            if entry.key.conversation == conversation {
                entry.token.cancel();
                return false;
            }
            true
        });
    }

    fn entry(&self, key: Key, order: u64) -> Option<Arc<Entry>> {
        let mut cache = self.cache();
        if let Some(index) = cache.entries.iter().position(|entry| entry.key == key) {
            let entry = cache.entries.remove(index).expect("located cache entry");
            let expired =
                matches!(entry.result.get(), Some(Err(at)) if at.elapsed() >= FAILURE_BACKOFF);
            if !expired {
                entry.order.fetch_max(order, Ordering::Relaxed);
                cache.entries.push_back(entry.clone());
                return Some(entry);
            }
        }
        // A slow history read for an older selection cannot cancel a newer model's sample.
        if cache.entries.iter().any(|entry| {
            entry.key.conversation == key.conversation
                && entry.order.load(Ordering::Relaxed) > order
        }) {
            return None;
        }
        cache.entries.retain(|entry| {
            if entry.key.conversation == key.conversation {
                entry.token.cancel();
                return false;
            }
            true
        });
        if cache.entries.len() >= MAX_RESULTS {
            if let Some(evicted) = cache.entries.pop_front() {
                evicted.token.cancel();
            }
        }
        let entry = Arc::new(Entry {
            order: AtomicU64::new(order),
            key,
            token: AgentCancellationToken::new(),
            result: OnceCell::new(),
        });
        cache.entries.push_back(entry.clone());
        Some(entry)
    }
}

struct SamplingModel {
    model: ModelConfigRecord,
    connection: ModelConnectionConfig,
    protocol_revision: String,
    connection_revision: String,
    semantics: ProviderUsageSemantics,
}

impl AgentService {
    pub(crate) fn stop_message_predictions(&self) {
        self.predictions.closed.store(true, Ordering::Release);
        self.clear_message_predictions();
    }

    pub(crate) fn clear_message_predictions(&self) {
        let mut cache = self.predictions.cache();
        cache.epoch += 1;
        for entry in cache.entries.drain(..) {
            entry.token.cancel();
        }
        cache.hot.clear();
    }

    pub(super) fn prediction_seed_observer(
        &self,
        conversation: &str,
        assistant: &str,
        run: &str,
        input: &AgentChatInput,
    ) -> Option<Arc<dyn Fn(AgentPromptPredictionSeed) + Send + Sync>> {
        // A grant or preference change can occur while the parent is still generating.
        // An observer already handed to the runtime must never put a cleared seed back.
        let epoch = self.predictions.cache().epoch;
        let (enabled, generation) = self.storage.load_prediction_preference().ok()?;
        if !enabled {
            return None;
        }
        let access = self.capture_execution_access().ok()?;
        let model = input.model_config_id.clone()?;
        let protocol_revision = input.provider_configuration_revision.clone()?;
        let connection_revision = input.provider_connection_revision.clone()?;
        let (conversation, assistant, run) = (
            conversation.to_owned(),
            assistant.to_owned(),
            run.to_owned(),
        );
        let predictions = self.predictions.clone();
        Some(Arc::new(move |seed| {
            let bytes = seed.estimated_bytes();
            if bytes > MAX_HOT_BYTES {
                return;
            }
            let mut cache = predictions.cache();
            if cache.epoch != epoch || predictions.closed.load(Ordering::Acquire) {
                return;
            }
            cache.hot.retain(|item| item.conversation != conversation);
            while cache.hot.len() >= MAX_HOT_SEEDS
                || cache
                    .hot
                    .iter()
                    .map(|item| item.seed.estimated_bytes())
                    .sum::<usize>()
                    > MAX_HOT_BYTES - bytes
            {
                cache.hot.pop_front();
            }
            cache.hot.push_back(HotSeed {
                conversation: conversation.clone(),
                assistant: assistant.clone(),
                run: run.clone(),
                model: model.clone(),
                protocol_revision: protocol_revision.clone(),
                connection_revision: connection_revision.clone(),
                preference: generation,
                access,
                version: None,
                seed: Arc::new(seed),
            });
        }))
    }

    /// A staged sample cannot be read until Done's durable publication boundary.
    pub(super) fn publish_prediction_seed(&self, run: &str, assistant: &str) {
        let conversation = self
            .predictions
            .cache()
            .hot
            .iter()
            .find(|seed| seed.run == run && seed.assistant == assistant)
            .map(|seed| seed.conversation.clone());
        let Some(id) = conversation else { return };
        let Ok(Some(version)) = self.storage.current_conversation_history_version(&id) else {
            return;
        };
        if let Some(seed) = self
            .predictions
            .cache()
            .hot
            .iter_mut()
            .find(|seed| seed.run == run && seed.assistant == assistant)
        {
            seed.version = Some(version);
        }
    }

    fn prediction_target(
        &self,
        input: &PromptPredictionInput,
        access: ExecutionAccessToken,
    ) -> Result<Option<(Key, SamplingModel)>, AgentServiceError> {
        self.check_execution_access_token(access)
            .map_err(|err| err.agent_error())?;
        let (enabled, preference) = self.storage.load_prediction_preference()?;
        if !enabled || self.predictions.closed.load(Ordering::Acquire) {
            return Ok(None);
        }
        let id = &input.conversation_id;
        self.authorize_user_conversation_write(id)?;
        if self.has_conversation_turn_occupancy(id)? {
            return Err("Conversation is busy.".to_string().into());
        }
        let Some(source) = self.storage.load_conversation_model_history_snapshot(id)? else {
            return Ok(None);
        };
        if source.conversation.archived_at.is_some() {
            return Ok(None);
        }
        let Some(last) = source.conversation.messages.last() else {
            return Ok(None);
        };
        if last.id != input.assistant_message_id || last.role != "assistant" {
            return Err("Prediction source changed.".to_string().into());
        }
        if last.content.trim().is_empty()
            || last
                .status
                .as_deref()
                .is_some_and(|status| status != "sent")
            || source.traces.iter().any(|trace| {
                trace.assistant_message_id == last.id
                    && trace.terminal_status != ConversationTurnTraceTerminalStatus::Completed
            })
        {
            return Ok(None);
        }
        self.ensure_provider_transition_ready_for_send(id, &input.model_id)?;
        let settings = self
            .storage
            .load_model_settings_snapshot_for_model(&input.model_id, false)?
            .ok_or_else(|| "Missing model configuration.".to_string())?;
        let model = settings
            .settings
            .models
            .iter()
            .find(|model| model.id == input.model_id && model.enabled)
            .ok_or_else(|| "Prediction model is unavailable.".to_string())?
            .clone();
        let connection = settings.settings.effective_connection_for(&model)?;
        let connection_revision = settings
            .provider_connection_revisions
            .get(&model.id)
            .ok_or_else(|| "Missing connection revision.".to_string())?
            .clone();
        let protocol_revision = settings
            .provider_protocol_revisions
            .get(&model.id)
            .ok_or_else(|| "Missing model protocol revision.".to_string())?
            .clone();
        let dialect = ProviderProtocolDialect::detect_from_api_url(&connection.api_url);
        let profile = model
            .resolved_provider_profile_config(dialect)
            .map_err(|e| e.to_string())?;
        let protocol = ProviderProtocolKey::new(
            dialect,
            &profile,
            model.provider_model_id.clone(),
            Some(protocol_revision.clone()),
        )
        .map_err(|e| e.to_string())?;
        let semantics = mycopilot_core::resolve_provider_runtime_capabilities(&protocol)
            .map_err(|e| e.to_string())?
            .usage();
        Ok(Some((
            Key {
                conversation: id.clone(),
                assistant: last.id.clone(),
                model: model.id.clone(),
                config: settings.configuration_revision,
                preference,
                history: source.version.clone(),
                access,
            },
            SamplingModel {
                model,
                connection,
                protocol_revision,
                connection_revision,
                semantics,
            },
        )))
    }

    fn prediction_current(&self, key: &Key) -> Result<bool, String> {
        if self.predictions.closed.load(Ordering::Acquire)
            || self.check_execution_access_token(key.access).is_err()
            || self.storage.load_prediction_preference()? != (true, key.preference)
            || self
                .authorize_user_conversation_write(&key.conversation)
                .is_err()
            || self
                .storage
                .load_conversation_meta(&key.conversation)?
                .is_none_or(|meta| meta.archived_at.is_some())
            || self.has_conversation_turn_occupancy(&key.conversation)?
            || !self
                .storage
                .is_conversation_history_snapshot_current(&key.history)?
        {
            return Ok(false);
        }
        Ok(self
            .storage
            .load_model_projection_cached()?
            .is_some_and(|projection| {
                projection.configuration_revision == key.config
                    && projection
                        .entry(&key.model)
                        .is_some_and(|model| model.execution.is_available())
            }))
    }

    fn prepare_prediction(
        &self,
        entry: &Entry,
        protocol_revision: &str,
        connection_revision: &str,
    ) -> Result<Arc<AgentPromptPredictionSeed>, AgentServiceError> {
        let key = &entry.key;
        if entry.token.is_cancelled() || !self.prediction_current(key)? {
            return Err("Prediction source changed.".to_string().into());
        }
        let hot = self
            .predictions
            .cache()
            .hot
            .iter()
            .find(|seed| {
                seed.conversation == key.conversation
                    && seed.assistant == key.assistant
                    && seed.model == key.model
                    && seed.protocol_revision == protocol_revision
                    && seed.connection_revision == connection_revision
                    && seed.preference == key.preference
                    && seed.access == key.access
                    && seed.version.as_ref() == Some(&key.history)
            })
            .map(|seed| seed.seed.clone());
        if let Some(seed) = hot {
            return Ok(seed);
        }

        let draft = self.storage.load_composer_draft(&key.conversation)?;
        let ui = self.storage.load_ui_preferences()?;
        use mycopilot_protocol_rs::{
            AutomationPermissionModeDto as Mode, AUTOMATION_PERMISSION_MODE_VERSION,
        };
        let mode = match draft.as_ref().map(|d| d.permission_mode.as_str()) {
            Some("full") if ui.full_permission_enabled => Mode::Full,
            Some("custom") if ui.custom_permission_enabled => Mode::Custom,
            _ => Mode::Default,
        };
        let permissions =
            crate::application::automation::permissions::resolve_automation_permissions(
                mode,
                AUTOMATION_PERMISSION_MODE_VERSION,
                &ui,
            )
            .map_err(|e| e.to_string())?
            .permissions;
        let skills = draft
            .map(|d| serde_json::from_str(&d.skills_json))
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        let view = self.prepare_context_view(AgentContextWindowSnapshotInput {
            conversation_id: Some(key.conversation.clone()),
            project_id: None,
            model_id: key.model.clone(),
            max_tokens: None,
            prompt_preferences: None,
            permissions,
            skills,
        })?;
        let context_window::PreparedContextView::Ready(view) = view else {
            return Err("Conversation is busy.".to_string().into());
        };
        if view.input.provider_configuration_revision.as_deref() != Some(protocol_revision)
            || view.input.provider_connection_revision.as_deref() != Some(connection_revision)
        {
            return Err("Prediction model changed.".to_string().into());
        }
        mycopilot_core::prepare_prompt_prediction_seed(
            view.input,
            &key.conversation,
            &self.context_window_provider_host_services(),
            &view
                .projection
                .with_active_assistant_message_id(key.assistant.clone()),
        )
        .map(Arc::new)
        .map_err(|error| error.to_string().into())
    }

    async fn sample_prediction(
        &self,
        entry: &Arc<Entry>,
        target: SamplingModel,
    ) -> Result<Option<String>, String> {
        let _permit = self
            .predictions
            .capacity
            .acquire()
            .await
            .map_err(|e| e.to_string())?;
        let host = self.clone();
        let handle = entry.clone();
        let seed = tokio::task::spawn_blocking(move || {
            let seed = host.prepare_prediction(
                &handle,
                &target.protocol_revision,
                &target.connection_revision,
            )?;
            if !host.prediction_current(&handle.key)? {
                return Err(AgentServiceError::from(
                    "Prediction source changed.".to_string(),
                ));
            }
            Ok(seed)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        if entry.token.is_cancelled()
            || self.check_execution_access_token(entry.key.access).is_err()
        {
            return Err("Prediction cancelled.".to_string());
        }
        let mut dispatched = false;
        let response = seed
            .predict_observed(
                &target.connection.api_url,
                &target.connection.api_token,
                entry.token.clone(),
                || {
                    dispatched = true;
                },
            )
            .await;
        let host = self.clone();
        let handle = entry.clone();
        tokio::task::spawn_blocking(move || {
            let usage = match &response {
                Ok(output) => output.usage.as_ref(),
                Err(error) => error.usage(),
            };
            if dispatched {
                host.storage.record_prediction_usage(
                    &create_id("prediction"),
                    &handle.key.conversation,
                    &target.model,
                    usage,
                    usage.and_then(|u| target.semantics.billable_output_tokens(u)),
                )?;
            }
            response
                .map(|output| output.suggestion)
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|error| error.to_string())?
    }

    pub(crate) async fn predict_next_user_message(
        &self,
        input: PromptPredictionInput,
        access: ExecutionAccessToken,
    ) -> Result<Option<String>, AgentServiceError> {
        let order = self.predictions.order.fetch_add(1, Ordering::Relaxed);
        let cached = {
            let cache = self.predictions.cache();
            cache
                .entries
                .iter()
                .find(|entry| {
                    entry.key.conversation == input.conversation_id
                        && entry.key.assistant == input.assistant_message_id
                        && entry.key.model == input.model_id
                        && entry.key.access == access
                        && entry.result.get().is_some_and(Result::is_ok)
                })
                .cloned()
        };
        if let Some(entry) = cached {
            let host = self.clone();
            let hit = tokio::task::spawn_blocking(move || -> Result<_, String> {
                if !host.prediction_current(&entry.key)? {
                    return Ok(None);
                }
                // Pair the hit with the order fence under the same lock as replacement.
                let cache = host.predictions.cache();
                if entry.token.is_cancelled()
                    || !cache.entries.iter().any(|item| Arc::ptr_eq(item, &entry))
                {
                    return Ok(None);
                }
                entry.order.fetch_max(order, Ordering::Relaxed);
                Ok(entry
                    .result
                    .get()
                    .and_then(|result| result.as_ref().ok())
                    .cloned())
            })
            .await
            .map_err(|e| e.to_string())??;
            if let Some(suggestion) = hit {
                return Ok(suggestion);
            }
        }
        let host = self.clone();
        let target = tokio::task::spawn_blocking(move || host.prediction_target(&input, access))
            .await
            .map_err(|error| error.to_string())??;
        let Some((key, target)) = target else {
            return Ok(None);
        };
        let Some(entry) = self.predictions.entry(key, order) else {
            return Ok(None);
        };
        let result = entry
            .result
            .get_or_init(|| async {
                self.sample_prediction(&entry, target)
                    .await
                    .map_err(|_| Instant::now())
            })
            .await;
        // A model failure may already have incurred usage. A later reopen can retry after
        // the backoff, but returning a quiet miss prevents an automatic second HTTP attempt.
        let Ok(candidate) = result else {
            return Ok(None);
        };
        let candidate = candidate.clone();
        let host = self.clone();
        let current = tokio::task::spawn_blocking(move || {
            !entry.token.is_cancelled() && host.prediction_current(&entry.key).unwrap_or(false)
        })
        .await
        .map_err(|e| e.to_string())?;
        Ok(if current { candidate } else { None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::storage::models::ChatConversationRecord;

    #[test]
    fn message_prediction_switching_views_preserves_other_inflight_samples_and_rejects_late_model_changes(
    ) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(StorageService::open(&dir.path().join("source.sqlite")).unwrap());
        let host = AgentService::new_authorized_for_test(storage.clone());
        for id in ["chat-a", "chat-b"] {
            storage
                .save_conversation(ChatConversationRecord {
                    id: id.into(),
                    project_id: None,
                    model_id: None,
                    title: id.into(),
                    messages: vec![],
                    created_at: 1,
                    updated_at: 1,
                    pinned_at: None,
                    archived_at: None,
                    unread_at: None,
                })
                .unwrap();
        }
        let key = |chat: &str, model: &str| Key {
            conversation: chat.into(),
            assistant: "latest".into(),
            model: model.into(),
            config: "rev".into(),
            history: storage
                .current_conversation_history_version(chat)
                .unwrap()
                .unwrap(),
            preference: 1,
            access: host.capture_execution_access().unwrap(),
        };
        let a = host
            .predictions
            .entry(key("chat-a", "original"), 1)
            .unwrap();
        let b = host
            .predictions
            .entry(key("chat-b", "original"), 2)
            .unwrap();
        // Order 3 is still loading the other model; order 4 revisits the first model.
        let again = host
            .predictions
            .entry(key("chat-a", "original"), 4)
            .unwrap();
        assert!(Arc::ptr_eq(&again, &a));
        assert!(host.predictions.entry(key("chat-a", "other"), 3).is_none());
        assert!(!a.token.is_cancelled());
        assert!(!b.token.is_cancelled());

        host.predictions.entry(key("chat-a", "other"), 5).unwrap();
        assert!(a.token.is_cancelled());
        assert!(!b.token.is_cancelled());
        host.clear_message_predictions();
        assert!(b.token.is_cancelled());
    }
}
