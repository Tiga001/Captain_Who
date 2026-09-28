use std::sync::{Arc as TraceArc, OnceLock as TraceOnceLock};

#[derive(Debug, Default)]
struct ConversationTracePublicationState {
    previous: Option<ConversationTracePublication>,
    operations: BTreeMap<String, Value>,
    failure_signatures: BTreeMap<String, String>,
}

/// Immutable Runtime publication. Only the recorder can mint an append certificate. Its shared
/// item bodies cannot be changed by a Host consumer; arbitrary restored/edited Vec snapshots
/// continue through the fully validated cold path.
#[derive(Debug, Clone)]
pub struct ConversationTracePublication {
    pub(crate) trace_items: Vec<TraceArc<ConversationTurnTraceItem>>,
    pub(crate) model_items: Vec<TraceArc<ConversationModelContextItem>>,
    pub(crate) identity: TraceArc<()>,
    pub(crate) parent: Option<TraceArc<()>>,
    pub(crate) prefix_trace_count: usize,
    pub(crate) prefix_model_count: usize,
    next_sequence: u64,
    truncated: bool,
    snapshot: TraceArc<TraceOnceLock<ConversationTraceSnapshot>>,
}

impl std::ops::Deref for ConversationTracePublication {
    type Target = ConversationTraceSnapshot;

    fn deref(&self) -> &Self::Target {
        self.snapshot.get_or_init(|| ConversationTraceSnapshot {
            items: self
                .trace_items
                .iter()
                .map(|item| (**item).clone())
                .collect(),
            model_context_items: self
                .model_items
                .iter()
                .map(|item| (**item).clone())
                .collect(),
            next_sequence: self.next_sequence,
            truncated: self.truncated,
        })
    }
}

impl ConversationTracePublication {
    pub fn into_snapshot(self) -> ConversationTraceSnapshot {
        match TraceArc::try_unwrap(self.snapshot) {
            Ok(snapshot) => snapshot
                .into_inner()
                .unwrap_or_else(|| ConversationTraceSnapshot {
                    items: self
                        .trace_items
                        .into_iter()
                        .map(TraceArc::unwrap_or_clone)
                        .collect(),
                    model_context_items: self
                        .model_items
                        .into_iter()
                        .map(TraceArc::unwrap_or_clone)
                        .collect(),
                    next_sequence: self.next_sequence,
                    truncated: self.truncated,
                }),
            Err(snapshot) => snapshot
                .get()
                .cloned()
                .unwrap_or_else(|| ConversationTraceSnapshot {
                    items: self
                        .trace_items
                        .iter()
                        .map(|item| (**item).clone())
                        .collect(),
                    model_context_items: self
                        .model_items
                        .iter()
                        .map(|item| (**item).clone())
                        .collect(),
                    next_sequence: self.next_sequence,
                    truncated: self.truncated,
                }),
        }
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    pub fn trace_items(&self) -> &[TraceArc<ConversationTurnTraceItem>] {
        &self.trace_items
    }

    pub fn model_items(&self) -> &[TraceArc<ConversationModelContextItem>] {
        &self.model_items
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }
}

impl From<ConversationTraceSnapshot> for ConversationTracePublication {
    fn from(snapshot: ConversationTraceSnapshot) -> Self {
        // Imported/restored/publicly mutable snapshots receive no prefix authority. The first
        // storage submission must validate all of their bytes against the durable journals.
        Self {
            trace_items: snapshot.items.into_iter().map(TraceArc::new).collect(),
            model_items: snapshot
                .model_context_items
                .into_iter()
                .map(TraceArc::new)
                .collect(),
            identity: TraceArc::new(()),
            parent: None,
            prefix_trace_count: 0,
            prefix_model_count: 0,
            next_sequence: snapshot.next_sequence,
            truncated: snapshot.truncated,
            snapshot: TraceArc::new(TraceOnceLock::new()),
        }
    }
}

impl ConversationTraceRecorder {
    fn invalidate_publication(&mut self) {
        *self
            .publication
            .get_mut()
            .unwrap_or_else(|error| error.into_inner()) = Default::default();
    }

    pub(crate) fn publication(&self) -> ConversationTracePublication {
        let mut state = self
            .publication
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let previous = state.previous.take();
        let prefix_trace_count = previous.as_ref().map_or(0, |value| value.trace_items.len());
        let prefix_model_count = previous.as_ref().map_or(0, |value| value.model_items.len());
        let (suffix, projected_truncated) = if self.items_are_durable {
            (self.items[prefix_trace_count..].to_vec(), false)
        } else {
            let ConversationTracePublicationState {
                operations,
                failure_signatures,
                ..
            } = &mut *state;
            project_durable_trace_suffix(
                &self.items[prefix_trace_count..],
                operations,
                failure_signatures,
            )
        };
        let mut trace_items = previous
            .as_ref()
            .map_or_else(Vec::new, |value| value.trace_items.clone());
        trace_items.extend(suffix.into_iter().map(TraceArc::new));
        let mut model_items = previous
            .as_ref()
            .map_or_else(Vec::new, |value| value.model_items.clone());
        model_items.extend(
            self.model_context_items[prefix_model_count..]
                .iter()
                .cloned()
                .map(TraceArc::new),
        );
        let publication = ConversationTracePublication {
            trace_items,
            model_items,
            identity: TraceArc::new(()),
            parent: previous.as_ref().map(|value| value.identity.clone()),
            prefix_trace_count,
            prefix_model_count,
            next_sequence: self.next_sequence,
            truncated: self.truncated
                || projected_truncated
                || previous.as_ref().is_some_and(|value| value.truncated),
            snapshot: TraceArc::new(TraceOnceLock::new()),
        };
        let mut retained = publication.clone();
        retained.snapshot = TraceArc::new(TraceOnceLock::new());
        state.previous = Some(retained);
        publication
    }
}
