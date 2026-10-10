//! Conversation ownership, request adoption and current-run placement are independent. The same
//! exact records feed the measured conversation cache and the active run overlay.

use super::*;
use crate::world_state::{AnchoredWorldStateRecord, WorldStateRecord, WorldStateRequestBoundary};

pub(super) fn world_state_boundary(item: &ContextItem) -> Option<WorldStateRequestBoundary> {
    let origin = item.metadata.origin()?;
    if origin.kind() != ContextOriginKind::WorldStateRecord {
        return None;
    }
    serde_json::from_str(origin.id().strip_prefix("request:")?).ok()
}

/// Run World State material is replayed as an ordinary, trace-owned historical fact. Its
/// content class is still WorldStateSnapshot for capacity reporting, but it is not a member of
/// the canonical Conversation ledger and must neither be validated as one nor protected as one.
fn is_historical_run_world_state(item: &ContextItem) -> bool {
    item.metadata
        .sources
        .contains(&ContextSource::HistoricalRunContext)
        && item
            .metadata
            .sources
            .contains(&ContextSource::ConversationTrace)
        && item
            .metadata
            .origin()
            .is_some_and(|origin| origin.kind() == ContextOriginKind::ConversationTraceItem)
}

impl ContextFrame {
    /// A staged preview can be rebuilt after the active run's initial snapshot was journaled.
    /// Match that exact trace owner, not a similar record from an earlier run. Reading the typed
    /// projection here classifies existing measured text only; it never restores state authority.
    pub(crate) fn contains_historical_initial_run_world_state(
        &self,
        assistant_message_id: &str,
    ) -> bool {
        self.iter_items().any(|item| {
            is_historical_run_world_state(item)
                && item
                    .metadata
                    .sources
                    .contains(&ContextSource::WorldStateSnapshot)
                && item
                    .metadata
                    .origin()
                    .and_then(ContextOrigin::journal_cursor)
                    .is_some_and(|cursor| cursor.message_id() == assistant_message_id)
                && item
                    .message
                    .content()
                    .lines()
                    .filter_map(|line| {
                        serde_json::from_str::<crate::world_state::WorldStateModelRecord>(line).ok()
                    })
                    .any(|record| {
                        matches!(
                            record,
                            crate::world_state::WorldStateModelRecord::Full {
                                lifetime: crate::world_state::WorldStateLifetime::Run,
                                ..
                            }
                        )
                    })
        })
    }

    /// A direct Core compaction executor may return only its new message/trace journal. Preserve
    /// the independently owned exact ledger instead of treating an absent Host ledger as deletion.
    /// The model's retained order supplies placement when a covered anchor no longer has a row.
    pub(super) fn preserve_world_state_for_compaction(
        &self,
        baseline: MeasuredContextBaseline,
    ) -> MeasuredContextBaseline {
        let estimator = Arc::clone(&baseline.measurement.estimator);
        let mut rebuilt = Self::from_measured_baseline(baseline);
        rebuilt.materialize_baseline();
        let original = self.iter_items().collect::<Vec<_>>();
        for (source_index, source) in original.iter().enumerate() {
            if source.metadata.scope != ContextScope::Conversation
                || is_historical_run_world_state(source)
                || !(source
                    .metadata
                    .sources
                    .contains(&ContextSource::WorldStateSnapshot)
                    || source
                        .metadata
                        .sources
                        .contains(&ContextSource::WorldStateDiff))
            {
                continue;
            }
            if rebuilt
                .items
                .iter()
                .any(|item| item.metadata.origin() == source.metadata.origin())
            {
                continue;
            }
            let position = if source
                .metadata
                .sources
                .contains(&ContextSource::WorldStateSnapshot)
            {
                rebuilt
                    .items
                    .iter()
                    .position(|item| {
                        item.metadata.cache_band() > ContextCacheBand::ConversationEpochPrelude
                    })
                    .unwrap_or(rebuilt.items.len())
            } else {
                let preceding = original[..source_index].iter().rev().find_map(|earlier| {
                    let origin = earlier.metadata.origin()?;
                    rebuilt
                        .items
                        .iter()
                        .rposition(|item| item.metadata.origin() == Some(origin))
                        .map(|index| index + 1)
                });
                preceding.unwrap_or_else(|| {
                    original[source_index + 1..]
                        .iter()
                        .find_map(|later| {
                            let origin = later.metadata.origin()?;
                            rebuilt
                                .items
                                .iter()
                                .position(|item| item.metadata.origin() == Some(origin))
                        })
                        .unwrap_or(rebuilt.items.len())
                })
            };
            let mut item = (*source).clone();
            item.metadata.sources.retain(|source| {
                !matches!(source, ContextSource::RunTimeline | ContextSource::RunInput)
            });
            item.metadata.request_order = None;
            rebuilt.items.insert(position, item);
        }
        rebuilt.conversation_world_state_records =
            Arc::clone(&self.conversation_world_state_records);
        rebuilt.world_state_metadata_changed();
        rebuilt.measure_incrementally(estimator);
        rebuilt
            .share_measured_persistent_baseline()
            .expect("conversation ledger preserves measured baseline ownership")
    }

    /// Computes an ephemeral preview of a complete desired Conversation snapshot. It has no
    /// durable request identity and cannot mutate the canonical ledger or mark it adopted.
    pub(crate) fn conversation_world_state_preview(
        &self,
        sections: &[crate::WorldStateSectionEnvelope],
    ) -> AgentResult<Option<ContextItem>> {
        use crate::world_state::{
            WorldStateDiff, WorldStateLifetime, WorldStateReducer, WorldStateSnapshot,
        };
        let error =
            |error| AgentError::new(format!("Conversation World State preview 无效：{error}"));
        let (projection, source) = if let Some(initial) =
            self.conversation_world_state_records.first()
        {
            let WorldStateRecord::Full(snapshot) = &initial.record else {
                return Err(AgentError::new(
                    "Conversation World State preview 缺少 full snapshot。",
                ));
            };
            let mut reducer = WorldStateReducer::new(snapshot.clone()).map_err(error)?;
            for entry in self.conversation_world_state_records.iter().skip(1) {
                let WorldStateRecord::Diff(diff) = &entry.record else {
                    return Err(AgentError::new(
                        "Conversation World State preview 含重复 full snapshot。",
                    ));
                };
                reducer.apply(diff).map_err(error)?;
            }
            let current = reducer.snapshot();
            let next = current.sequence.checked_add(1).ok_or_else(|| {
                AgentError::new("Conversation World State preview sequence 已溢出。")
            })?;
            let target = WorldStateSnapshot::new(current.epoch_id.clone(), next, sections.to_vec())
                .map_err(error)?;
            if current.revision == target.revision {
                return Ok(None);
            }
            let difference = WorldStateDiff::between(current, &target).map_err(error)?;
            (
                difference
                    .model_projection_against(current, WorldStateLifetime::Conversation)
                    .map_err(error)?,
                ContextSource::WorldStateDiff,
            )
        } else {
            let snapshot =
                WorldStateSnapshot::new("context-preview", 0, sections.to_vec()).map_err(error)?;
            (
                Some(
                    snapshot
                        .model_projection(WorldStateLifetime::Conversation)
                        .map_err(error)?,
                ),
                ContextSource::WorldStateSnapshot,
            )
        };
        Ok(projection.map(|projection| {
            ContextItem::new(
                LlmMessage::backend_state(projection.render_sanitized_text()),
                ContextMetadata::new(source, ContextScope::Run, ContextRetention::RequestOnly),
            )
        }))
    }

    pub(crate) fn conversation_world_state_records(&self) -> &[AnchoredWorldStateRecord] {
        &self.conversation_world_state_records
    }

    /// Replace only the exact recorded CWS text affected by the model display
    /// policy. The ledger is the authority: arbitrary text is never parsed or sanitized into
    /// acceptance, and unrelated state/permission differences still fail exact comparison.
    fn adopt_model_display_policy(
        &mut self,
        records: &[AnchoredWorldStateRecord],
        check_observation: bool,
    ) -> AgentResult<()> {
        let stored =
            crate::context::assembler::stored_world_state_projections_for_validation(records)?;
        let directory =
            crate::context::assembler::directory_world_state_projections_for_validation(records)?;
        let legacy_subagent =
            crate::context::assembler::legacy_subagent_world_state_projections_for_validation(
                records,
            )?;
        let current = crate::context::assembler::project_conversation_world_state_records(records)?;
        let mut changes = Vec::new();
        for (_, old) in stored {
            let origin = old.metadata.origin().expect("CWS projection origin");
            let next = current
                .iter()
                .map(|(_, item)| item)
                .find(|item| item.metadata.origin() == Some(origin));
            if next.is_some_and(|next| next.message == old.message) {
                continue;
            }
            let mut matches = self
                .iter_items()
                .enumerate()
                .filter(|(_, item)| item.metadata.origin() == Some(origin));
            let Some((index, existing)) = matches.next() else {
                continue;
            };
            if matches.next().is_some() {
                return Err(AgentError::new(
                    "检查点 canonical Conversation World State 投影重复。",
                ));
            }
            if next.is_some_and(|next| existing.message == next.message) {
                continue;
            }
            if existing.metadata.scope != ContextScope::Conversation
                || (existing.message != old.message
                    && !directory
                        .iter()
                        .chain(&legacy_subagent)
                        .any(|(_, candidate)| {
                            candidate.metadata.origin() == Some(origin)
                                && candidate.message == existing.message
                        }))
                || check_observation
                    && existing
                        .metadata
                        .sources
                        .contains(&ContextSource::WorldStateUnobserved)
                        != old
                            .metadata
                            .sources
                            .contains(&ContextSource::WorldStateUnobserved)
            {
                return Err(AgentError::new(
                    "Conversation World State 与可信 ledger 的存储投影不一致。",
                ));
            }
            // Remove only records made invisible by the model display policy (internal metadata
            // or other members' observations). Authority and observation markers remain exact.
            changes.push((index, next.map(|next| next.message.clone())));
        }
        if !changes.is_empty() {
            self.materialize_baseline();
            changes.sort_by_key(|(index, _)| *index);
            for (index, message) in changes.into_iter().rev() {
                if let Some(message) = message {
                    self.items[index].message = message;
                    self.items[index].checkpoint_message = None;
                    self.items[index].measurement = None;
                } else {
                    self.items.remove(index);
                }
            }
            self.world_state_metadata_changed();
        }
        Ok(())
    }

    /// Installs backend canonical authority alongside its already-rendered context. Recovery must
    /// validate both representations; sanitized model text is never used to reconstruct state.
    pub(crate) fn restore_conversation_world_state_records(
        &mut self,
        records: Vec<AnchoredWorldStateRecord>,
    ) -> AgentResult<()> {
        self.adopt_model_display_policy(&records, true)?;
        let mut origins = Vec::new();
        for (_, projected) in
            crate::context::assembler::project_conversation_world_state_records(&records)?
        {
            let origin = projected
                .metadata
                .origin()
                .expect("projected World State origin")
                .clone();
            if self
                .iter_items()
                .filter(|item| item.metadata.origin() == Some(&origin))
                .count()
                != 1
            {
                return Err(AgentError::new(
                    "检查点 canonical Conversation World State 投影缺失或重复。",
                ));
            }
            origins.push(origin);
            let matching = self
                .iter_items()
                .find(|item| item.metadata.origin() == projected.metadata.origin())
                .ok_or_else(|| {
                    AgentError::new("检查点缺少 canonical Conversation World State 的模型投影。")
                })?;
            if matching.message != projected.message
                || matching
                    .metadata
                    .sources
                    .contains(&ContextSource::WorldStateUnobserved)
                    != projected
                        .metadata
                        .sources
                        .contains(&ContextSource::WorldStateUnobserved)
            {
                return Err(AgentError::new(
                    "检查点 Conversation World State 与模型投影或观察状态不一致。",
                ));
            }
        }
        if self.iter_items().any(|item| {
            item.metadata.scope == ContextScope::Conversation
                && !is_historical_run_world_state(item)
                && (item
                    .metadata
                    .sources
                    .contains(&ContextSource::WorldStateSnapshot)
                    || item
                        .metadata
                        .sources
                        .contains(&ContextSource::WorldStateDiff))
                && !item
                    .metadata
                    .origin()
                    .is_some_and(|origin| origins.contains(origin))
        }) {
            return Err(AgentError::new(
                "检查点包含 canonical ledger 未声明的 Conversation World State 投影。",
            ));
        }
        self.conversation_world_state_records = Arc::from(records);
        Ok(())
    }

    /// Synchronizes a complete authoritative active epoch without appending duplicate model
    /// records. `live` controls only placement of newly discovered diffs. The Host's adoption
    /// marker controls token ownership even when a record was committed before a failed request.
    pub(crate) fn sync_conversation_world_state_records(
        &mut self,
        records: &[AnchoredWorldStateRecord],
        live: bool,
    ) -> AgentResult<usize> {
        if records.is_empty() {
            return Ok(0);
        }
        self.adopt_model_display_policy(records, false)?;
        let projected =
            crate::context::assembler::project_conversation_world_state_records(records)?;
        let mut appended = 0;
        for (record, mut incoming) in projected {
            let origin = incoming
                .metadata
                .origin()
                .expect("World State projection has an origin")
                .clone();
            let existing = self.iter_items().enumerate().find_map(|(index, item)| {
                (item.metadata.origin() == Some(&origin)).then_some((index, item))
            });
            if let Some((index, existing)) = existing {
                if existing.message != incoming.message {
                    return Err(AgentError::new(
                        "Conversation World State origin 与既有模型投影冲突。",
                    ));
                }
                let unobserved = incoming
                    .metadata
                    .sources
                    .contains(&ContextSource::WorldStateUnobserved);
                if existing
                    .metadata
                    .sources
                    .contains(&ContextSource::WorldStateUnobserved)
                    != unobserved
                {
                    self.materialize_baseline();
                    let metadata = &mut self.items[index].metadata;
                    metadata
                        .sources
                        .retain(|source| *source != ContextSource::WorldStateUnobserved);
                    if unobserved {
                        metadata.sources.push(ContextSource::WorldStateUnobserved);
                    }
                    self.world_state_metadata_changed();
                }
                continue;
            }
            if matches!(record.record, WorldStateRecord::Full(_)) {
                if self.iter_items().any(|item| {
                    item.metadata.scope == ContextScope::Conversation
                        && !is_historical_run_world_state(item)
                        && item
                            .metadata
                            .sources
                            .contains(&ContextSource::WorldStateSnapshot)
                }) {
                    return Err(AgentError::new(
                        "Conversation World State epoch 已变化，需要采用压缩后的会话基线。",
                    ));
                }
                self.materialize_baseline();
                let position = self
                    .items
                    .iter()
                    .position(|item| {
                        item.metadata.cache_band() > ContextCacheBand::ConversationEpochPrelude
                    })
                    .unwrap_or(self.items.len());
                self.items.insert(position, incoming);
                self.world_state_metadata_changed();
            } else if live {
                incoming.metadata = incoming.metadata.with_source(ContextSource::RunTimeline);
                self.push(incoming);
            } else {
                let position = self.world_state_insertion_position(record)?;
                if position == self.iter_items().count() {
                    self.push(incoming);
                } else {
                    self.materialize_baseline();
                    self.items.insert(position, incoming);
                    self.world_state_metadata_changed();
                }
            }
            appended += 1;
        }
        self.conversation_world_state_records = Arc::from(records.to_vec());
        Ok(appended)
    }

    /// Called only after the Host acknowledged the successful request's prepared ledger prefix.
    /// Placement survives adoption: a newly durable observation still happened inside this run.
    pub(crate) fn mark_conversation_world_state_observed(&mut self) {
        for record in Arc::make_mut(&mut self.conversation_world_state_records) {
            record.model_observed = true;
        }
        if !self.iter_items().any(|item| {
            item.metadata.scope == ContextScope::Conversation
                && item
                    .metadata
                    .sources
                    .contains(&ContextSource::WorldStateUnobserved)
        }) {
            return;
        }
        self.materialize_baseline();
        for item in &mut self.items {
            if item.metadata.scope == ContextScope::Conversation {
                item.metadata
                    .sources
                    .retain(|source| *source != ContextSource::WorldStateUnobserved);
            }
        }
        self.world_state_metadata_changed();
    }

    fn world_state_metadata_changed(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.persistent_revision = persistent_frame_revision(&self.items);
        self.measurement = None;
    }

    fn world_state_insertion_position(
        &self,
        record: &AnchoredWorldStateRecord,
    ) -> AgentResult<usize> {
        let items = self.iter_items().collect::<Vec<_>>();
        if let Some(boundary) = &record.request_boundary {
            let preceding_boundary = items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    let existing = world_state_boundary(item)?;
                    (existing.run_id == boundary.run_id
                        && existing.assistant_message_id == boundary.assistant_message_id
                        && existing.after_trace_sequence == boundary.after_trace_sequence
                        && existing.request_index < boundary.request_index)
                        .then_some(index + 1)
                })
                .max();
            let position = match boundary.after_trace_sequence {
                Some(after) => items
                    .iter()
                    .enumerate()
                    .filter_map(|(index, item)| {
                        let cursor = item.metadata.origin()?.journal_cursor()?;
                        (cursor.message_id() == boundary.assistant_message_id
                            && cursor
                                .trace_sequence()
                                .is_some_and(|sequence| sequence <= after))
                        .then_some(index + 1)
                    })
                    .max()
                    .unwrap_or_else(|| {
                        items
                            .iter()
                            .position(|item| {
                                item.metadata
                                    .origin()
                                    .and_then(ContextOrigin::journal_cursor)
                                    .is_some_and(|cursor| {
                                        cursor.message_id() == boundary.assistant_message_id
                                    })
                            })
                            .unwrap_or_else(|| {
                                items
                                    .iter()
                                    .position(|item| {
                                        item.metadata
                                            .origin()
                                            .and_then(ContextOrigin::journal_cursor)
                                            .is_some()
                                    })
                                    .unwrap_or(items.len())
                            })
                    }),
                None => items
                    .iter()
                    .position(|item| {
                        item.metadata
                            .origin()
                            .and_then(ContextOrigin::journal_cursor)
                            .is_some_and(|cursor| {
                                cursor.message_id() == boundary.assistant_message_id
                            })
                    })
                    .unwrap_or(items.len()),
            };
            return Ok(position.max(preceding_boundary.unwrap_or(0)));
        }
        let anchor = record
            .effective_before_message_id
            .as_deref()
            .expect("validated message anchor");
        items
            .iter()
            .position(|item| {
                item.metadata
                    .origin()
                    .and_then(ContextOrigin::journal_cursor)
                    .is_some_and(|cursor| cursor.message_id() == anchor)
            })
            .ok_or_else(|| {
                AgentError::new("Conversation World State diff 引用了不存在的消息 anchor。")
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextCapacityDetector;
    use crate::world_state::{
        WorldStateDiff, WorldStateLifetime, WorldStateSectionEnvelope, WorldStateSectionId,
        WorldStateSnapshot,
    };

    fn snapshot(epoch: &str, sequence: u64, available: bool) -> WorldStateSnapshot {
        let value = serde_json::json!({"available": available});
        WorldStateSnapshot::new(
            epoch,
            sequence,
            vec![WorldStateSectionEnvelope::model_visible(
                WorldStateSectionId::extension("web.search").unwrap(),
                WorldStateLifetime::Conversation,
                value.clone(),
                value,
            )
            .unwrap()],
        )
        .unwrap()
    }

    fn records(epoch: &str, observed: bool) -> Vec<AnchoredWorldStateRecord> {
        let initial = snapshot(epoch, 0, false);
        let target = snapshot(epoch, 1, true);
        let mut diff = AnchoredWorldStateRecord::at_request(
            WorldStateRecord::Diff(WorldStateDiff::between(&initial, &target).unwrap()),
            WorldStateRequestBoundary {
                run_id: "run".into(),
                assistant_message_id: "assistant".into(),
                request_index: 2,
                after_trace_sequence: Some(1),
            },
        )
        .unwrap();
        diff.model_observed = observed;
        vec![
            AnchoredWorldStateRecord::new(WorldStateRecord::Full(initial), None).unwrap(),
            diff,
        ]
    }

    fn frame() -> ContextFrame {
        ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "rules",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::User,
                "input",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            )
            .with_origin(ContextOrigin::conversation_message("user")),
        ])
    }

    fn narration(sequence: u64, live: bool) -> ContextItem {
        ContextItem::text(
            LlmMessageRole::Assistant,
            format!("narration-{sequence}"),
            if live {
                ContextSource::ModelResponse
            } else {
                ContextSource::ConversationTrace
            },
            if live {
                ContextScope::Run
            } else {
                ContextScope::Conversation
            },
            ContextRetention::Retained,
        )
        .with_origin(ContextOrigin::conversation_trace_item(
            "assistant",
            sequence,
        ))
    }

    fn text(frame: &ContextFrame) -> Vec<String> {
        frame
            .model_request_items()
            .iter()
            .map(|item| item.message.content().to_string())
            .collect()
    }

    fn diff_item(frame: &ContextFrame) -> &ContextItem {
        frame
            .iter_items()
            .find(|item| {
                item.metadata
                    .sources
                    .contains(&ContextSource::WorldStateDiff)
            })
            .unwrap()
    }

    #[test]
    fn live_conversation_world_state_adoption_preserves_causal_placement_and_checkpoint() {
        let ledger = records("epoch", false);
        let mut frame = frame();
        frame
            .sync_conversation_world_state_records(&ledger[..1], true)
            .unwrap();
        frame.mark_initial_run_input();
        frame.push(narration(1, true));
        assert_eq!(
            frame
                .sync_conversation_world_state_records(&ledger, true)
                .unwrap(),
            1
        );
        let before = text(&frame);
        assert_eq!(before[before.len() - 2], "narration-1");
        assert!(before.last().unwrap().contains("\"recordType\":\"diff\""));
        assert_eq!(
            diff_item(&frame).metadata.usage_class(),
            ContextUsageClass::RunTransient
        );
        assert_eq!(
            frame
                .sync_conversation_world_state_records(&ledger, true)
                .unwrap(),
            0
        );
        frame.validate_cache_layout().unwrap();
        let mut restored =
            ContextFrame::from_checkpoint_items(frame.checkpoint_items().unwrap()).unwrap();
        restored
            .restore_conversation_world_state_records(
                frame.conversation_world_state_records().to_vec(),
            )
            .unwrap();
        assert_eq!(text(&restored), before);
        assert_eq!(
            diff_item(&restored).metadata.usage_class(),
            ContextUsageClass::RunTransient
        );
        restored.mark_conversation_world_state_observed();
        assert!(restored
            .conversation_world_state_records()
            .iter()
            .all(|record| record.model_observed));
        assert_eq!(
            diff_item(&restored).metadata.usage_class(),
            ContextUsageClass::Durable
        );
        restored.validate_cache_layout().unwrap();
        assert_eq!(text(&restored), before);
    }

    #[test]
    fn checkpoint_canonical_world_state_rejects_omitted_ledger_and_false_adoption() {
        let ledger = records("epoch", false);
        let mut frame = frame();
        frame
            .sync_conversation_world_state_records(&ledger[..1], true)
            .unwrap();
        frame.push(narration(1, true));
        frame
            .sync_conversation_world_state_records(&ledger, true)
            .unwrap();
        let mut restored =
            ContextFrame::from_checkpoint_items(frame.checkpoint_items().unwrap()).unwrap();
        assert!(restored
            .restore_conversation_world_state_records(Vec::new())
            .is_err());
        assert!(restored
            .restore_conversation_world_state_records(records("epoch", true))
            .is_err());
        restored
            .restore_conversation_world_state_records(ledger)
            .unwrap();
    }

    #[test]
    fn durable_world_state_sync_inserts_at_trace_cursor_without_reordering_later_guidance() {
        let ledger = records("epoch", true);
        let mut frame = frame();
        frame.push(narration(1, false));
        frame.push(
            ContextItem::text(
                LlmMessageRole::User,
                "guidance",
                ContextSource::ConversationTrace,
                ContextScope::Conversation,
                ContextRetention::Retained,
            )
            .with_origin(ContextOrigin::conversation_trace_item("assistant", 2)),
        );
        frame.push(narration(3, false));
        assert_eq!(
            frame
                .sync_conversation_world_state_records(&ledger, false)
                .unwrap(),
            2
        );
        let contents = text(&frame);
        let difference = contents
            .iter()
            .position(|text| text.contains("\"recordType\":\"diff\""))
            .unwrap();
        assert_eq!(contents[difference - 1], "narration-1");
        assert_eq!(contents[difference + 1], "guidance");
        assert_eq!(
            frame
                .sync_conversation_world_state_records(&ledger, false)
                .unwrap(),
            0
        );
        frame.validate_cache_layout().unwrap();
    }

    #[test]
    fn core_compaction_without_a_host_ledger_preserves_full_diff_and_canonical_records() {
        let ledger = records("epoch", false);
        let mut live = frame();
        live.sync_conversation_world_state_records(&ledger[..1], true)
            .unwrap();
        live.mark_initial_run_input();
        live.push(narration(1, true));
        live.sync_conversation_world_state_records(&ledger, true)
            .unwrap();
        live.push(narration(2, true));
        let mut rebuilt = frame();
        rebuilt.push(narration(2, false));
        ContextCapacityDetector::for_model(
            "test-model",
            crate::AgentApiStyle::OpenAiCompatible,
            &[],
        )
        .prepare_frame(&mut rebuilt);
        let replaced = live
            .replace_compacted_model_history(rebuilt.share_measured_persistent_baseline().unwrap());
        replaced.validate_cache_layout().unwrap();
        assert_eq!(replaced.conversation_world_state_records(), ledger);
        let contents = text(&replaced);
        assert!(contents[1].contains("\"recordType\":\"full\""));
        let difference = contents
            .iter()
            .position(|text| text.contains("\"recordType\":\"diff\""))
            .unwrap();
        assert_eq!(contents[difference + 1], "narration-2");
        assert_eq!(
            contents
                .iter()
                .filter(|text| text.contains("\"recordType\":\"full\""))
                .count(),
            1
        );
        let mut restored =
            ContextFrame::from_checkpoint_items(replaced.checkpoint_items().unwrap()).unwrap();
        restored
            .restore_conversation_world_state_records(
                replaced.conversation_world_state_records().to_vec(),
            )
            .unwrap();
    }

    #[test]
    fn historical_run_material_is_not_conversation_ledger_authority_on_restore() {
        let ledger = records("epoch", true);
        let mut live = frame();
        let historical = ContextItem::new(
            LlmMessage::backend_state("historical run browser authorization"),
            ContextMetadata::new(
                ContextSource::ConversationTrace,
                ContextScope::Conversation,
                ContextRetention::Retained,
            )
            .with_source(ContextSource::HistoricalRunContext)
            .with_source(ContextSource::WorldStateSnapshot)
            .with_origin(ContextOrigin::conversation_trace_item("old-assistant", 0)),
        );
        live.push(historical);
        live.sync_conversation_world_state_records(&ledger[..1], true)
            .unwrap();
        let mut restored =
            ContextFrame::from_checkpoint_items(live.checkpoint_items().unwrap()).unwrap();
        restored
            .restore_conversation_world_state_records(ledger[..1].to_vec())
            .unwrap();
        assert_eq!(restored.conversation_world_state_records(), &ledger[..1]);
        assert!(text(&restored)
            .iter()
            .any(|content| *content == "historical run browser authorization"));
        // An unbound World State record is still rejected, including one carrying the historical
        // marker without the required trace-owned history provenance.
        restored.push(
            ContextItem::text(
                LlmMessageRole::User,
                "forged",
                ContextSource::WorldStateSnapshot,
                ContextScope::Conversation,
                ContextRetention::Retained,
            )
            .with_source(ContextSource::HistoricalRunContext),
        );
        assert!(restored
            .restore_conversation_world_state_records(ledger[..1].to_vec())
            .is_err());
    }

    #[test]
    fn unobserved_world_state_shares_baseline_and_survives_rebase_without_overlay_duplicates() {
        let ledger = records("epoch", false);
        let mut live = frame();
        live.sync_conversation_world_state_records(&ledger[..1], true)
            .unwrap();
        live.mark_initial_run_input();
        live.push(narration(1, true));
        live.sync_conversation_world_state_records(&ledger, true)
            .unwrap();
        live.push(narration(2, true));
        let expected = text(&live);

        let mut rebuilt = frame();
        rebuilt.push(narration(1, false));
        rebuilt.push(narration(2, false));
        rebuilt
            .sync_conversation_world_state_records(&records("rebased", false), false)
            .unwrap();
        ContextCapacityDetector::for_model(
            "test-model",
            crate::AgentApiStyle::OpenAiCompatible,
            &[],
        )
        .prepare_frame(&mut rebuilt);
        let baseline = rebuilt.share_measured_persistent_baseline().unwrap();
        let replaced = live.replace_compacted_model_history(baseline.clone());
        replaced.validate_cache_layout().unwrap();
        assert_eq!(text(&replaced), expected);
        assert_eq!(
            replaced
                .iter_items()
                .filter(|item| item
                    .metadata
                    .sources
                    .contains(&ContextSource::WorldStateDiff))
                .count(),
            1
        );
        assert_eq!(
            diff_item(&replaced).metadata.usage_class(),
            ContextUsageClass::RunTransient
        );
        assert!(diff_item(&replaced).metadata.request_order().is_some());
        let restored =
            ContextFrame::from_checkpoint_items(replaced.checkpoint_items().unwrap()).unwrap();
        let replaced_again = restored.replace_compacted_model_history(baseline);
        replaced_again.validate_cache_layout().unwrap();
        assert_eq!(text(&replaced_again), expected);
    }

    fn workflow_template_ledger() -> Vec<AnchoredWorldStateRecord> {
        let make = |sequence, revision, state: &str| {
            let execution = serde_json::json!({"available":true,"organization":{"instanceId":"team-id","templateId":"private-template-id","templateRevision":revision,"executionVersion":format!("private-version-{revision}"),"name":"Review team","task":"Review artifacts","nodeId":"reviewer","nodeName":"Reviewer","members":[{"nodeId":"writer","nodeName":"Writer","task":"Draft private work"}]}});
            let awareness = serde_json::json!({"available":true,"instanceId":"team-id","executionVersion":format!("private-version-{revision}"),"state":state});
            let permissions = serde_json::json!({"mode":"review-only"});
            WorldStateSnapshot::new(
                "workflow-display",
                sequence,
                vec![
                    WorldStateSectionEnvelope::model_visible(
                        WorldStateSectionId::extension("organization.execution").unwrap(),
                        WorldStateLifetime::Conversation,
                        execution.clone(),
                        execution,
                    )
                    .unwrap(),
                    WorldStateSectionEnvelope::model_visible(
                        WorldStateSectionId::extension("organization.awareness").unwrap(),
                        WorldStateLifetime::Conversation,
                        awareness.clone(),
                        awareness,
                    )
                    .unwrap(),
                    WorldStateSectionEnvelope::model_visible(
                        WorldStateSectionId::EffectivePermissions,
                        WorldStateLifetime::Conversation,
                        permissions.clone(),
                        permissions,
                    )
                    .unwrap(),
                ],
            )
            .unwrap()
        };
        let full = make(0, 1, "idle");
        let metadata = make(1, 2, "idle");
        let running = make(2, 2, "running");
        let mut records =
            vec![
                AnchoredWorldStateRecord::new(WorldStateRecord::Full(full.clone()), None).unwrap(),
            ];
        for (index, (before, after)) in [(&full, &metadata), (&metadata, &running)]
            .into_iter()
            .enumerate()
        {
            let mut record = AnchoredWorldStateRecord::at_request(
                WorldStateRecord::Diff(WorldStateDiff::between(before, after).unwrap()),
                WorldStateRequestBoundary {
                    run_id: "run".into(),
                    assistant_message_id: "assistant".into(),
                    request_index: index as u64 + 2,
                    after_trace_sequence: Some(1),
                },
            )
            .unwrap();
            record.model_observed = false;
            records.push(record);
        }
        records
    }

    fn frame_with_recorded_workflow_text(ledger: &[AnchoredWorldStateRecord]) -> ContextFrame {
        let mut frame = frame();
        frame.push(narration(1, false));
        for (record, item) in
            crate::context::assembler::stored_world_state_projections_for_validation(ledger)
                .unwrap()
        {
            if matches!(record.record, WorldStateRecord::Full(_)) {
                frame.items.insert(1, item);
            } else {
                frame.push(item);
            }
        }
        frame
    }

    #[test]
    fn subagent_scope_restores_full_and_diff_text_without_changing_policy_or_accepting_tampering() {
        let snapshot = |sequence, enabled| {
            let state = serde_json::json!({
                "enabled": enabled, "available": enabled,
                "reason": if enabled { "available" } else { "disabled_by_user" },
            });
            WorldStateSnapshot::new(
                "subagent-scope",
                sequence,
                vec![WorldStateSectionEnvelope::model_visible(
                    WorldStateSectionId::extension("agent.collaboration").unwrap(),
                    WorldStateLifetime::Conversation,
                    state.clone(),
                    state,
                )
                .unwrap()],
            )
            .unwrap()
        };
        let full = snapshot(0, false);
        let after = snapshot(1, true);
        let ledger = vec![
            AnchoredWorldStateRecord::new(WorldStateRecord::Full(full.clone()), None).unwrap(),
            AnchoredWorldStateRecord::at_request(
                WorldStateRecord::Diff(WorldStateDiff::between(&full, &after).unwrap()),
                WorldStateRequestBoundary {
                    run_id: "run".into(),
                    assistant_message_id: "assistant".into(),
                    request_index: 2,
                    after_trace_sequence: Some(1),
                },
            )
            .unwrap(),
        ];
        let old = frame_with_recorded_workflow_text(&ledger);
        let mut restored =
            ContextFrame::from_checkpoint_items(old.checkpoint_items().unwrap()).unwrap();
        restored
            .restore_conversation_world_state_records(ledger.clone())
            .unwrap();
        let visible = text(&restored).join("\n");
        assert!(visible.contains("\"subagentToolsAvailable\":false"));
        assert!(visible.contains("\"subagentToolsAvailable\":true"));
        assert!(!visible.contains("\"enabled\"") && !visible.contains("disabled_by_user"));
        assert_eq!(restored.conversation_world_state_records(), ledger);
        restored.validate_cache_layout().unwrap();
        let mut again =
            ContextFrame::from_checkpoint_items(restored.checkpoint_items().unwrap()).unwrap();
        again
            .restore_conversation_world_state_records(ledger.clone())
            .unwrap();
        assert_eq!(text(&again), text(&restored));
        assert_eq!(
            again
                .sync_conversation_world_state_records(&ledger, false)
                .unwrap(),
            0
        );

        let mut forged = old;
        let item = forged
            .items
            .iter_mut()
            .find(|item| item.message.content().contains("disabled_by_user"))
            .unwrap();
        item.message = LlmMessage::backend_state(
            item.message
                .content()
                .replace("disabled_by_user", "host_unavailable"),
        );
        assert!(forged
            .restore_conversation_world_state_records(ledger)
            .is_err());
    }

    #[test]
    fn subagent_scope_restores_already_narrowed_organization_full_and_diff_checkpoints() {
        let old_ledger = workflow_template_ledger();
        let WorldStateRecord::Full(original) = &old_ledger[0].record else {
            unreachable!()
        };
        let snapshot = |sequence, enabled, task: &str| {
            let mut sections = original.sections.clone();
            let organization = sections
                .iter_mut()
                .find(|section| section.id.as_str() == "organization.execution")
                .unwrap();
            let mut value = organization.model_projection.clone().unwrap();
            value["organization"]["task"] = serde_json::json!(task);
            *organization = WorldStateSectionEnvelope::model_visible(
                organization.id.clone(),
                organization.lifetime,
                value.clone(),
                value,
            )
            .unwrap();
            let subagents = serde_json::json!({
                "enabled": enabled, "available": enabled,
                "reason": if enabled { "available" } else { "disabled_by_user" },
            });
            sections.push(
                WorldStateSectionEnvelope::model_visible(
                    WorldStateSectionId::extension("agent.collaboration").unwrap(),
                    WorldStateLifetime::Conversation,
                    subagents.clone(),
                    subagents,
                )
                .unwrap(),
            );
            WorldStateSnapshot::new("successive-display-upgrades", sequence, sections).unwrap()
        };
        let full = snapshot(0, false, "Review artifacts");
        let after = snapshot(1, true, "Review updated artifacts");
        let ledger = vec![
            AnchoredWorldStateRecord::new(WorldStateRecord::Full(full.clone()), None).unwrap(),
            AnchoredWorldStateRecord::at_request(
                WorldStateRecord::Diff(WorldStateDiff::between(&full, &after).unwrap()),
                WorldStateRequestBoundary {
                    run_id: "run".into(),
                    assistant_message_id: "assistant".into(),
                    request_index: 2,
                    after_trace_sequence: Some(1),
                },
            )
            .unwrap(),
        ];
        let canonical = serde_json::to_value(&ledger).unwrap();
        let mut prior = frame();
        prior.push(narration(1, false));
        for (record, item) in
            crate::context::assembler::legacy_subagent_world_state_projections_for_validation(
                &ledger,
            )
            .unwrap()
        {
            if matches!(record.record, WorldStateRecord::Full(_)) {
                prior.items.insert(1, item);
            } else {
                prior.push(item);
            }
        }
        let before = text(&prior).join("\n");
        assert!(before.contains("disabled_by_user") && before.contains("Review updated artifacts"));
        assert!(!before.contains("Writer") && !before.contains("private-template-id"));
        assert!(canonical.to_string().contains("Writer"));
        assert!(canonical.to_string().contains("private-template-id"));

        let checkpoint = prior.checkpoint_items().unwrap();
        let mut restored = ContextFrame::from_checkpoint_items(checkpoint.clone()).unwrap();
        restored
            .restore_conversation_world_state_records(ledger.clone())
            .unwrap();
        let visible = text(&restored).join("\n");
        assert!(visible.contains("\"subagentToolsAvailable\":false"));
        assert!(visible.contains("\"subagentToolsAvailable\":true"));
        assert!(visible.contains("Review updated artifacts") && visible.contains("review-only"));
        assert!(!visible.contains("disabled_by_user") && !visible.contains("private-template-id"));
        assert_eq!(
            serde_json::to_value(restored.conversation_world_state_records()).unwrap(),
            canonical
        );
        restored.validate_cache_layout().unwrap();

        let mut synced = ContextFrame::from_checkpoint_items(checkpoint.clone()).unwrap();
        assert_eq!(
            synced
                .sync_conversation_world_state_records(&ledger, false)
                .unwrap(),
            0
        );
        assert_eq!(text(&synced), text(&restored));

        for (needle, replacement) in [
            ("Review updated artifacts", "Forged duties"),
            ("review-only", "full-access"),
            ("disabled_by_user", "host_unavailable"),
        ] {
            let mut forged = ContextFrame::from_checkpoint_items(checkpoint.clone()).unwrap();
            let item = forged
                .items
                .iter_mut()
                .find(|item| item.message.content().contains(needle))
                .unwrap();
            item.message =
                LlmMessage::backend_state(item.message.content().replace(needle, replacement));
            let before = text(&forged);
            assert!(forged
                .restore_conversation_world_state_records(ledger.clone())
                .is_err());
            assert_eq!(text(&forged), before);
        }
    }

    #[test]
    fn workflow_template_display_restores_exact_old_checkpoint_without_rewriting_authority() {
        let ledger = workflow_template_ledger();
        let serialized = serde_json::to_value(&ledger).unwrap();
        let old = frame_with_recorded_workflow_text(&ledger);
        assert!(text(&old).join(" ").contains("private-template-id"));
        let mut restored =
            ContextFrame::from_checkpoint_items(old.checkpoint_items().unwrap()).unwrap();
        restored
            .restore_conversation_world_state_records(ledger.clone())
            .unwrap();
        let visible = text(&restored).join(" ");
        assert!(
            !visible.contains("templateId")
                && !visible.contains("templateRevision")
                && !visible.contains("executionVersion")
        );
        assert!(
            visible.contains("Review team")
                && visible.contains("Reviewer")
                && visible.contains("Review artifacts")
                && visible.contains("review-only")
        );
        assert!(
            !visible.contains("running")
                && !visible.contains("Writer")
                && !visible.contains("organization.awareness")
        );
        assert!(!visible.contains("team-id") && !visible.contains("nodeId"));
        assert!(serialized.to_string().contains("team-id"));
        assert!(serialized.to_string().contains("private-template-id"));
        assert_eq!(
            serde_json::to_value(restored.conversation_world_state_records()).unwrap(),
            serialized
        );
        let diffs = restored
            .iter_items()
            .filter(|item| {
                item.metadata
                    .sources
                    .contains(&ContextSource::WorldStateDiff)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            diffs.len(),
            0,
            "metadata-only and global-awareness diffs have no current model representation"
        );
        let mut again =
            ContextFrame::from_checkpoint_items(restored.checkpoint_items().unwrap()).unwrap();
        again
            .restore_conversation_world_state_records(ledger)
            .unwrap();
        assert_eq!(text(&again), text(&restored));
    }

    #[test]
    fn organization_self_context_restores_prior_directory_projection_but_rejects_forged_peers() {
        let ledger = workflow_template_ledger();
        let mut prior = frame();
        prior.push(narration(1, false));
        for (record, item) in
            crate::context::assembler::directory_world_state_projections_for_validation(&ledger)
                .unwrap()
        {
            if matches!(record.record, WorldStateRecord::Full(_)) {
                prior.items.insert(1, item);
            } else {
                prior.push(item);
            }
        }
        let text_before = text(&prior).join(" ");
        assert!(text_before.contains("Writer") && text_before.contains("organization.awareness"));
        assert!(!text_before.contains("private-template-id"));
        let mut restored =
            ContextFrame::from_checkpoint_items(prior.checkpoint_items().unwrap()).unwrap();
        restored
            .restore_conversation_world_state_records(ledger.clone())
            .unwrap();
        let visible = text(&restored).join(" ");
        assert!(visible.contains("Reviewer") && visible.contains("review-only"));
        assert!(!visible.contains("Writer") && !visible.contains("organization.awareness"));
        let mut forged =
            ContextFrame::from_checkpoint_items(prior.checkpoint_items().unwrap()).unwrap();
        let item = forged
            .items
            .iter_mut()
            .find(|item| item.message.content().contains("Writer"))
            .unwrap();
        item.message =
            LlmMessage::backend_state(item.message.content().replace("Writer", "Forged colleague"));
        assert!(forged
            .restore_conversation_world_state_records(ledger)
            .is_err());
    }

    #[test]
    fn workflow_template_display_sync_updates_measured_context_and_preserves_observation_adoption()
    {
        let mut ledger = workflow_template_ledger();
        let mut old = frame_with_recorded_workflow_text(&ledger);
        ContextCapacityDetector::for_model(
            "test-model",
            crate::AgentApiStyle::OpenAiCompatible,
            &[],
        )
        .prepare_frame(&mut old);
        let baseline = old.share_measured_persistent_baseline().unwrap();
        let mut cached = frame().replace_compacted_model_history(baseline);
        for record in &mut ledger {
            record.model_observed = true;
        }
        assert_eq!(
            cached
                .sync_conversation_world_state_records(&ledger, false)
                .unwrap(),
            0
        );
        assert!(!text(&cached).join(" ").contains("private-template-id"));
        assert!(cached.iter_items().all(|item| !item
            .metadata
            .sources
            .contains(&ContextSource::WorldStateUnobserved)));
        cached.validate_cache_layout().unwrap();
        let before = text(&cached);
        assert_eq!(
            cached
                .sync_conversation_world_state_records(&ledger, false)
                .unwrap(),
            0
        );
        assert_eq!(text(&cached), before);
    }

    #[test]
    fn workflow_template_display_rejects_tampering_hidden_values_permissions_and_observation() {
        let ledger = workflow_template_ledger();
        for (needle, replacement) in [
            ("private-template-id", "forged-template-id"),
            ("review-only", "full-access"),
            ("Review artifacts", "Untrusted role"),
        ] {
            let mut forged = frame_with_recorded_workflow_text(&ledger);
            let index = forged
                .items
                .iter()
                .position(|item| item.message.content().contains(needle))
                .unwrap();
            let metadata = forged.items[index].metadata.clone();
            let message = LlmMessage::backend_state(
                forged.items[index]
                    .message
                    .content()
                    .replace(needle, replacement),
            );
            forged.items[index] = ContextItem::new(message, metadata);
            let before = text(&forged);
            assert!(forged
                .restore_conversation_world_state_records(ledger.clone())
                .is_err());
            assert_eq!(
                text(&forged),
                before,
                "failed policy validation must be atomic"
            );
            assert!(forged
                .sync_conversation_world_state_records(&ledger, false)
                .is_err());
        }
        let mut forged = frame_with_recorded_workflow_text(&ledger);
        let metadata_only = forged
            .items
            .iter_mut()
            .find(|item| {
                world_state_boundary(item).is_some_and(|boundary| boundary.request_index == 2)
            })
            .unwrap();
        metadata_only
            .metadata
            .sources
            .retain(|source| *source != ContextSource::WorldStateUnobserved);
        assert!(
            forged
                .restore_conversation_world_state_records(ledger.clone())
                .is_err(),
            "hidden-only records must still respect observed-prefix authority"
        );
        let mut duplicate = frame_with_recorded_workflow_text(&ledger);
        duplicate.items.push(duplicate.items[1].clone());
        assert!(duplicate
            .restore_conversation_world_state_records(ledger.clone())
            .is_err());
        let mut extra = frame_with_recorded_workflow_text(&ledger);
        let extra_item = extra.items[1]
            .clone()
            .with_origin(ContextOrigin::world_state_record("forged-extra"));
        extra.items.push(extra_item);
        assert!(
            extra
                .restore_conversation_world_state_records(ledger)
                .is_err(),
            "extra canonical-looking records are never silently dropped"
        );
    }
}
