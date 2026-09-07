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

    /// Installs backend canonical authority alongside its already-rendered context. Recovery must
    /// validate both representations; sanitized model text is never used to reconstruct state.
    pub(crate) fn restore_conversation_world_state_records(
        &mut self,
        records: Vec<AnchoredWorldStateRecord>,
    ) -> AgentResult<()> {
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
}
