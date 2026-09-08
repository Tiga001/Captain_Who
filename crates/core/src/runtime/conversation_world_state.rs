//! In-memory counterpart of the Host journal, used by embedded Core callers and read-only previews.
use super::*;
use crate::world_state::{
    WorldStateDiff, WorldStateLifetime, WorldStateRecord, WorldStateReducer,
    WorldStateRequestBoundary, WorldStateSectionEnvelope, WorldStateSectionId, WorldStateSnapshot,
};
use std::collections::BTreeMap;

pub(super) struct MemoryConversationWorldState {
    records: Vec<crate::AnchoredWorldStateRecord>,
    base_sections: Vec<WorldStateSectionEnvelope>,
}

impl MemoryConversationWorldState {
    pub(super) fn new(input: &AgentChatInput) -> AgentResult<Self> {
        Ok(Self {
            records: input.world_state_records.clone(),
            base_sections: conversation_base_sections(input)?,
        })
    }

    pub(super) fn prepare(
        &mut self,
        boundary: &WorldStateRequestBoundary,
        sections: Vec<WorldStateSectionEnvelope>,
    ) -> AgentResult<Vec<crate::AnchoredWorldStateRecord>> {
        let current = fold_records(&self.records)?;
        let desired = self.preview_sections(sections)?;
        let target = WorldStateSnapshot::new(
            current
                .as_ref()
                .map(|s| s.epoch_id.clone())
                .unwrap_or_else(|| format!("{}:conversation-state", boundary.run_id)),
            current
                .as_ref()
                .map(|s| s.sequence.saturating_add(1))
                .unwrap_or(0),
            desired,
        )
        .map_err(|e| AgentError::new(e.to_string()))?;
        let record = match current {
            None => Some(crate::AnchoredWorldStateRecord::new(
                WorldStateRecord::Full(target),
                None,
            )),
            Some(current) if current.revision != target.revision => {
                Some(WorldStateDiff::between(&current, &target).and_then(|diff| {
                    crate::AnchoredWorldStateRecord::at_request(
                        WorldStateRecord::Diff(diff),
                        boundary.clone(),
                    )
                }))
            }
            Some(_) => None,
        };
        if let Some(record) = record {
            let mut record = record.map_err(|e| AgentError::new(e.to_string()))?;
            record.model_observed = false;
            self.records.push(record);
        }
        Ok(self.records.clone())
    }

    /// Computes the next state without constructing a durable request identity or changing the
    /// canonical ledger. Context-window previews use this same owner merge as actual requests.
    pub(super) fn preview_sections(
        &self,
        sections: Vec<WorldStateSectionEnvelope>,
    ) -> AgentResult<Vec<WorldStateSectionEnvelope>> {
        let current = fold_records(&self.records)?;
        let mut desired = current
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .sections
                    .iter()
                    .map(|s| (s.id.clone(), s.clone()))
                    .collect()
            })
            .unwrap_or_else(BTreeMap::new);
        for id in conversation_capability_section_ids()? {
            desired.remove(&id);
        }
        for section in self.base_sections.iter().cloned().chain(sections) {
            if section.lifetime != WorldStateLifetime::Conversation {
                return Err(AgentError::new(
                    "Conversation World State received a Run section.",
                ));
            }
            desired.insert(section.id.clone(), section);
        }
        Ok(desired.into_values().collect())
    }

    pub(super) fn mark_observed(&mut self) {
        for record in &mut self.records {
            record.model_observed = true;
        }
    }

    pub(super) fn adopt_compacted_records(
        &mut self,
        records: &[crate::AnchoredWorldStateRecord],
    ) -> AgentResult<()> {
        fold_records(records)?;
        self.records = records.to_vec();
        Ok(())
    }
}

pub(super) fn conversation_capability_section_ids() -> AgentResult<Vec<WorldStateSectionId>> {
    [
        "web.search",
        "agent.collaboration",
        "human.interaction",
        "builtin.capabilities.policy",
    ]
    .into_iter()
    .map(|id| WorldStateSectionId::extension(id).map_err(|e| AgentError::new(e.to_string())))
    .collect()
}

fn fold_records(
    records: &[crate::AnchoredWorldStateRecord],
) -> AgentResult<Option<WorldStateSnapshot>> {
    let Some(first) = records.first() else {
        return Ok(None);
    };
    first
        .validate()
        .map_err(|e| AgentError::new(e.to_string()))?;
    let WorldStateRecord::Full(full) = &first.record else {
        return Err(AgentError::new(
            "Conversation World State is missing its initial snapshot.",
        ));
    };
    let mut reducer =
        WorldStateReducer::new(full.clone()).map_err(|e| AgentError::new(e.to_string()))?;
    for entry in &records[1..] {
        entry
            .validate()
            .map_err(|e| AgentError::new(e.to_string()))?;
        let WorldStateRecord::Diff(diff) = &entry.record else {
            return Err(AgentError::new(
                "Conversation World State contains multiple initial snapshots.",
            ));
        };
        reducer
            .apply(diff)
            .map_err(|e| AgentError::new(e.to_string()))?;
    }
    Ok(Some(reducer.into_snapshot()))
}
