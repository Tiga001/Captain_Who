//! Bounded, host-only reuse of validated persisted history. This cache never owns turn authority.
use super::*;
use mycopilot_core::{AgentChatMessage, AgentPreparedConversationHistory};
use std::collections::VecDeque;

pub(super) type PreparedHistory = PreparedConversationHistory;
const MAX_ENTRIES: usize = 32;
const MAX_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone)]
struct MeasuredHistory {
    history: AgentPreparedConversationHistory,
    input_fingerprint: String,
}
struct Entry {
    history: Arc<PreparedHistory>,
    measured: Option<MeasuredHistory>,
    bytes: usize,
}

#[cfg(any(test, debug_assertions))]
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct PreparedHistoryDiagnostics {
    pub history_assemblies: u64,
    pub context_rebuilds: u64,
    pub measured_installs: u64,
}

#[derive(Default)]
pub(super) struct PreparedHistoryCache {
    entries: VecDeque<Entry>,
    bytes: usize,
    #[cfg(any(test, debug_assertions))]
    diagnostics: HashMap<String, PreparedHistoryDiagnostics>,
}

pub(super) type PreparedHistoryInflight = Mutex<()>;

impl PreparedHistoryCache {
    fn remove(&mut self, id: &str) {
        if let Some(index) = self
            .entries
            .iter()
            .position(|e| e.history.source.conversation.id == id)
        {
            let entry = self.entries.remove(index).expect("located cache entry");
            self.bytes = self.bytes.saturating_sub(entry.bytes);
        }
    }

    fn measured_for_version(
        &self,
        id: &str,
        version: &mycopilot_core::storage::service::ConversationHistoryVersion,
    ) -> Option<MeasuredHistory> {
        self.entries
            .iter()
            .find(|entry| {
                entry.history.source.conversation.id == id
                    && entry.history.source.version == *version
            })
            .and_then(|entry| entry.measured.clone())
    }

    /// Replaces this conversation's entry. A same-version republish keeps any measured baseline.
    fn insert(&mut self, history: Arc<PreparedHistory>) -> bool {
        let id = &history.source.conversation.id;
        let measured = self.measured_for_version(id, &history.source.version);
        self.remove(id);
        // Bound shared source plus assembled and measured representations conservatively.
        let bytes = history.source.estimated_bytes.saturating_mul(4);
        if bytes > MAX_BYTES {
            return false;
        }
        while self.entries.len() >= MAX_ENTRIES || self.bytes.saturating_add(bytes) > MAX_BYTES {
            let Some(old) = self.entries.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(old.bytes);
        }
        self.bytes += bytes;
        self.entries.push_back(Entry {
            history,
            measured,
            bytes,
        });
        true
    }

    #[cfg(any(test, debug_assertions))]
    fn record(&mut self, id: &str, update: impl FnOnce(&mut PreparedHistoryDiagnostics)) {
        if self.diagnostics.len() >= 256 && !self.diagnostics.contains_key(id) {
            self.diagnostics.clear();
        }
        update(self.diagnostics.entry(id.to_string()).or_default());
    }
}

impl AgentService {
    pub(super) fn prepare_cached_history(
        &self,
        id: &str,
    ) -> Result<Option<Arc<PreparedHistory>>, String> {
        // Install or join the same per-conversation gate under one short map lock.
        // Release the registry lock before waiting so unrelated conversations can progress.
        let gate = {
            let mut inflight = self
                .prepared_history_inflight
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            inflight.retain(|_, gate| gate.strong_count() > 0);
            if let Some(gate) = inflight.get(id).and_then(std::sync::Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(Mutex::new(()));
                inflight.insert(id.to_string(), Arc::downgrade(&gate));
                gate
            }
        };
        let _preparation = gate.lock().unwrap_or_else(|error| error.into_inner());
        // Followers recheck the now-cached source rather than inheriting a result captured
        // before their arrival. Metadata CAS and history versions must both remain current.
        self.prepare_cached_history_inner(id)
    }

    fn prepare_cached_history_inner(
        &self,
        id: &str,
    ) -> Result<Option<Arc<PreparedHistory>>, String> {
        #[cfg(debug_assertions)]
        let started = Instant::now();
        let Some(source) = self.storage.load_conversation_history_snapshot(id)? else {
            self.prepared_histories
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(id);
            return Ok(None);
        };
        let mut excluded = self
            .storage
            .list_trace_bound_agent_projection_message_ids(id)
            .map_err(|e| e.to_string())?;
        excluded.extend(
            self.storage
                .workflow_execution_inputs_for_conversation(id)?
                .into_iter()
                .filter_map(|input| input.delivery_id),
        );
        let message_ids: HashSet<&str> = source
            .conversation
            .messages
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        excluded.retain(|id| message_ids.contains(id.as_str()));
        excluded.sort();
        excluded.dedup();
        let miss_reason;
        {
            let mut cache = self
                .prepared_histories
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(index) = cache
                .entries
                .iter()
                .position(|e| e.history.source.conversation.id == id)
            {
                let entry = &cache.entries[index];
                if entry.history.source.version == source.version
                    && entry.history.excluded_message_ids == excluded
                {
                    let history = if entry.history.source.conversation_revision
                        == source.conversation_revision
                    {
                        entry.history.clone()
                    } else {
                        Arc::new(PreparedHistory {
                            source,
                            messages: entry.history.messages.clone(),
                            excluded_message_ids: entry.history.excluded_message_ids.clone(),
                        })
                    };
                    // Refresh the metadata wrapper and its accounting while preserving the
                    // same-version measured history and shared message allocation.
                    cache.insert(history.clone());
                    return Ok(Some(history));
                }
                miss_reason = "历史或消息来源已变化";
                // Keep the stale entry until this preparation publishes a current replacement.
                // A failed or superseded prepare must not erase a still-usable measured baseline.
            } else {
                miss_reason = "尚未预热或缓存已淘汰";
            }
        }
        let conversation = source.conversation.to_record();
        let messages = conversation_history_messages_with_model_context(
            &conversation,
            &source.traces,
            &source.model_context_logs,
            source.compaction_summary.as_ref(),
            &excluded.iter().map(String::as_str).collect::<Vec<_>>(),
        )?;
        #[cfg(any(test, debug_assertions))]
        self.prepared_histories
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .record(id, |metrics| metrics.history_assemblies += 1);
        let stable = source
            .traces
            .iter()
            .all(|t| t.terminal_status.is_terminal());
        let history = Arc::new(PreparedHistory {
            source,
            messages: Arc::new(messages),
            excluded_message_ids: excluded,
        });
        let mut publication = "已写入缓存";
        if stable {
            if self
                .storage
                .is_conversation_history_snapshot_current(&history.source.version)?
            {
                let mut cache = self
                    .prepared_histories
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if !cache.insert(history.clone()) {
                    publication = "超过内存预算未写入缓存，发送时正常准备";
                }
            } else {
                publication = "准备期间历史发生变化，旧结果未写入缓存";
            }
        } else {
            publication = "轮次尚未结算，不缓存运行中历史";
        }
        #[cfg(debug_assertions)]
        eprintln!(
            "[历史上下文缓存] 未命中后准备完成：会话={id}，原因={miss_reason}，结果={publication}，耗时={}ms",
            started.elapsed().as_millis()
        );
        #[cfg(not(debug_assertions))]
        let _ = (miss_reason, publication);
        Ok(Some(history))
    }

    pub(super) fn remember_prepared_history(
        &self,
        history: Arc<PreparedHistory>,
        input: &AgentChatInput,
        state: &mut AgentConversationContextState,
    ) -> Result<(), String> {
        if !can_reuse_measured_history(input)?
            || history
                .source
                .traces
                .iter()
                .any(|t| !t.terminal_status.is_terminal())
            || !self
                .storage
                .is_conversation_history_snapshot_current(&history.source.version)?
        {
            return Ok(());
        }
        let measured = MeasuredHistory {
            history: state.share_prepared_history().map_err(|e| e.to_string())?,
            input_fingerprint: history_input_fingerprint(input, &input.messages)?,
        };
        let mut cache = self
            .prepared_histories
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = cache
            .entries
            .iter_mut()
            .find(|e| e.history.source.version == history.source.version)
        {
            entry.measured = Some(measured);
        }
        Ok(())
    }

    pub(super) fn remember_terminal_measured_history(
        &self,
        agent_input: &AgentChatInput,
        conversation_id: &str,
        trace: &mycopilot_core::ConversationTurnTrace,
        model_context_items: &[mycopilot_core::ConversationModelContextItem],
        assistant_content: &str,
        assistant_created_at: i64,
    ) -> Result<bool, String> {
        let persisted = self.persisted_conversation_context_state(agent_input, conversation_id)?;
        let Some(history) = persisted.prepared_history else {
            return Ok(false);
        };
        let assistant_id = trace.assistant_message_id.as_str();
        // A terminal state is an incremental extension of one precise admitted prefix. The
        // assistant id alone cannot certify that earlier messages or this terminal record
        // stayed unchanged while the complete persisted snapshot was being prepared.
        let prefix = terminal_prefix_fingerprint(&persisted.preview_input, Some(assistant_id))?;
        let current_message = history
            .source
            .conversation
            .messages
            .iter()
            .find(|message| message.id == assistant_id);
        let current_trace = history
            .source
            .traces
            .iter()
            .find(|item| item.assistant_message_id == assistant_id);
        let current_items = history
            .source
            .model_context_logs
            .iter()
            .find(|log| log.assistant_message_id == assistant_id)
            .map(|log| log.items.as_slice())
            .unwrap_or_default();
        if current_trace != Some(trace)
            || current_items != model_context_items
            || !current_message.is_some_and(|message| {
                message.content.trim() == assistant_content.trim()
                    && message.created_at == assistant_created_at
            })
        {
            return Ok(false);
        }
        let mut states = self
            .conversation_context_states
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(entry) = states.get_mut(conversation_id) else {
            return Ok(false);
        };
        if !entry.terminal
            || entry.active_assistant_message_id.as_deref() != Some(assistant_id)
            || entry.terminal_prefix_fingerprint.as_deref() != Some(prefix.as_str())
        {
            return Ok(false);
        }
        entry
            .state
            .sync_conversation_world_state_records(&persisted.preview_input.world_state_records)
            .map_err(|error| error.to_string())?;
        self.remember_prepared_history(
            history.clone(),
            &persisted.preview_input,
            &mut entry.state,
        )?;
        entry.history_version = Some(history.source.version.clone());
        Ok(true)
    }

    pub(super) fn record_context_rebuild(&self, conversation_id: &str) {
        #[cfg(any(test, debug_assertions))]
        self.prepared_histories
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .record(conversation_id, |metrics| metrics.context_rebuilds += 1);
        #[cfg(not(any(test, debug_assertions)))]
        let _ = conversation_id;
    }

    /// Called only after the source version has passed the durable admission transaction.
    /// Current permissions/tool projection and run overlays are still resolved at launch.
    pub(super) fn install_prepared_history(
        &self,
        prepared: &PreparedConversationTurn,
    ) -> Result<bool, String> {
        let Some(source) = prepared.prepared_history.as_ref() else {
            return Ok(false);
        };
        let id = &prepared.output.conversation_id;
        if !can_reuse_measured_history(&prepared.agent_input)? {
            #[cfg(debug_assertions)]
            eprintln!("[历史上下文缓存] 计量基线未复用：会话={id}，原因=当前模型需要实时校验私有回放记录，继续复用通用历史并正常恢复回放");
            return Ok(false);
        }
        let measured = {
            let cache = self
                .prepared_histories
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            cache
                .entries
                .iter()
                .find(|entry| entry.history.source.version == source.source.version)
                .and_then(|entry| entry.measured.clone())
        };
        let Some(measured) = measured else {
            #[cfg(debug_assertions)]
            eprintln!(
                "[历史上下文缓存] 计量基线未命中：会话={id}，原因=尚未预热或已淘汰，转为正常准备"
            );
            return Ok(false);
        };
        let configuration = conversation_context_configuration_revision(&prepared.agent_input)
            .map_err(|e| e.to_string())?;
        if measured.history.configuration_revision() != configuration {
            #[cfg(debug_assertions)]
            eprintln!("[历史上下文缓存] 计量基线已失效：会话={id}，原因=模型或上下文预算配置已变化，转为正常准备");
            return Ok(false);
        }
        let current_user_id = &prepared.output.user_message_id;
        let mut prefix = prepared.agent_input.messages.as_slice();
        let new_user = prefix.last().filter(|message| {
            message.message_id.as_deref() == Some(current_user_id.as_str())
                && !source
                    .source
                    .conversation
                    .messages
                    .iter()
                    .any(|old| &old.id == current_user_id)
        });
        if new_user.is_some() {
            prefix = &prefix[..prefix.len() - 1];
        }
        let fingerprint = history_input_fingerprint(&prepared.agent_input, prefix)?;
        if measured.input_fingerprint != fingerprint {
            #[cfg(debug_assertions)]
            eprintln!("[历史上下文缓存] 计量基线已失效：会话={id}，原因=世界状态、消息或附件投影已变化，转为正常准备");
            return Ok(false);
        }
        let mut state = measured.history.into_context_state();
        if let Some(user) = new_user {
            state
                .append_user_message(user.message_id.as_deref(), &user.content, user.created_at)
                .map_err(|e| e.to_string())?;
        }
        // Start at this run's cursor. Workflow delivery has no new HumanText and is injected by
        // its existing sampling inbox; never append the previous human message again.
        self.insert_conversation_context_state(
            id,
            ConversationContextStateEntry {
                state,
                configuration_revision: configuration,
                active_run_id: Some(prepared.output.run_id.clone()),
                active_assistant_message_id: Some(prepared.output.assistant_message_id.clone()),
                committed_activity_items: 0,
                awaiting_initial_publication: true,
                terminal: false,
                history_version: Some(source.source.version.clone()),
                terminal_prefix_fingerprint: Some(terminal_prefix_fingerprint(
                    &prepared.agent_input,
                    Some(&prepared.output.assistant_message_id),
                )?),
                last_access: self.next_conversation_context_state_access(),
            },
        );
        #[cfg(any(test, debug_assertions))]
        self.prepared_histories
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .record(id, |metrics| metrics.measured_installs += 1);
        Ok(true)
    }

    #[cfg(test)]
    pub(super) fn republish_prepared_history_for_test(
        &self,
        history: Arc<PreparedHistory>,
    ) -> bool {
        self.prepared_histories
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(history)
    }

    #[cfg(test)]
    pub(super) fn has_prepared_history_for_test(&self, id: &str) -> bool {
        let version = self
            .prepared_histories
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .iter()
            .find(|e| e.history.source.conversation.id == id)
            .map(|e| e.history.source.version.clone());
        version.is_some_and(|v| {
            self.storage
                .is_conversation_history_snapshot_current(&v)
                .unwrap_or(false)
        })
    }

    #[cfg(test)]
    pub(super) fn has_measured_prepared_history_for_test(&self, id: &str) -> bool {
        let cache = self
            .prepared_histories
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(entry) = cache
            .entries
            .iter()
            .find(|e| e.history.source.conversation.id == id)
        else {
            return false;
        };
        entry.measured.is_some()
            && self
                .storage
                .is_conversation_history_snapshot_current(&entry.history.source.version)
                .unwrap_or(false)
    }

    #[cfg(test)]
    pub(super) fn prepared_history_diagnostics_for_test(
        &self,
        id: &str,
    ) -> PreparedHistoryDiagnostics {
        self.prepared_histories
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .diagnostics
            .get(id)
            .copied()
            .unwrap_or_default()
    }
}

pub(super) fn terminal_prefix_fingerprint(
    input: &AgentChatInput,
    assistant_id: Option<&str>,
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    // The current assistant is appended by the certified trace observer and finalized
    // separately. World State has its own append-only, anchored synchronization path.
    let prefix: Vec<_> = input
        .messages
        .iter()
        .filter(|message| {
            !assistant_id.is_some_and(|id| message.message_id.as_deref() == Some(id))
                && (!message.content.trim().is_empty() || message.conversation_turn_trace.is_some())
        })
        .collect();
    let bytes = serde_json::to_vec(&(prefix, &input.context_compaction_summary))
        .map_err(|_| "无法校验终态历史前缀。".to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn history_input_fingerprint(
    input: &AgentChatInput,
    messages: &[AgentChatMessage],
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    // Excludes credentials, live permissions, selected skills and run overlays. Those never
    // enter the persistent baseline. Images and projected message provenance remain exact.
    let data = serde_json::to_vec(&(
        messages,
        &input.world_state_records,
        &input.context_image_attachments,
        &input.context_compaction_summary,
    ))
    .map_err(|_| "无法校验历史上下文输入身份。".to_string())?;
    Ok(format!("{:x}", Sha256::digest(data)))
}

fn can_reuse_measured_history(input: &AgentChatInput) -> Result<bool, String> {
    // The history journal does not version private replay sidecars. Keep their existing live
    // vault validation, including missing/released records, rather than trusting hydrated bytes.
    input
        .provider_protocol_key
        .as_ref()
        .map(mycopilot_core::resolve_provider_runtime_capabilities)
        .transpose()
        .map(|capabilities| {
            capabilities.is_some_and(|capabilities| {
                capabilities.private_replay()
                    == mycopilot_core::ProviderPrivateReplaySemantics::None
            })
        })
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measured_history_requires_live_replay_for_private_provider_profiles() {
        for (profile, reusable) in [
            (
                mycopilot_core::ProviderProfileConfig::generic_for_dialect(
                    ProviderProtocolDialect::OpenAiChatCompletions,
                ),
                true,
            ),
            (
                mycopilot_core::ProviderProfileConfig::deepseek_flash_default(),
                false,
            ),
        ] {
            let mut input: AgentChatInput = serde_json::from_value(serde_json::json!({
                "apiUrl": "https://example.test/v1/chat/completions",
                "apiToken": "test", "model": "deepseek-flash", "messages": [],
                "modelCapabilities": { "imageInput": false }
            }))
            .unwrap();
            input.provider_protocol_key = Some(
                ProviderProtocolKey::new(
                    ProviderProtocolDialect::OpenAiChatCompletions,
                    &profile,
                    "deepseek-flash",
                    None,
                )
                .unwrap(),
            );
            input.provider_profile_config = Some(profile);
            assert_eq!(can_reuse_measured_history(&input).unwrap(), reusable);
        }
    }
}
