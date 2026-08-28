use crate::protocol::AgentResult;
use crate::tools::{
    ToolExecutionContext, ToolInputStreamChunk, ToolInputStreamObserver, ToolInputStreamPreview,
    ToolRegistry,
};
use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};

pub(super) struct ToolInputStreamObservation {
    pub handled: bool,
    pub preview: Option<ToolInputStreamPreview>,
}

#[derive(Default)]
pub(super) struct ToolInputStreamObservers {
    attempt: usize,
    observers: BTreeMap<usize, Box<dyn ToolInputStreamObserver>>,
    pending_inputs: BTreeMap<usize, String>,
    ignored_calls: BTreeSet<usize>,
}

impl ToolInputStreamObservers {
    pub(super) fn start_attempt(&mut self, attempt: usize) {
        self.attempt = attempt;
        self.observers.clear();
        self.pending_inputs.clear();
        self.ignored_calls.clear();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn on_delta(
        &mut self,
        registry: &ToolRegistry,
        context: &ToolExecutionContext,
        stream_id: &str,
        tool_call_index: usize,
        _tool_call_id: Option<&str>,
        tool: &str,
        input_delta: &str,
        received_bytes: u64,
    ) -> AgentResult<ToolInputStreamObservation> {
        if input_delta.is_empty() {
            return Ok(ToolInputStreamObservation {
                handled: false,
                preview: None,
            });
        }

        if self.ignored_calls.contains(&tool_call_index) {
            return Ok(ToolInputStreamObservation {
                handled: false,
                preview: None,
            });
        }

        if let Some(observer) = self.observers.get_mut(&tool_call_index) {
            let preview = observer.on_delta(&ToolInputStreamChunk {
                stream_id,
                attempt: self.attempt,
                tool_call_index,
                input_delta,
                received_bytes,
            })?;
            return Ok(ToolInputStreamObservation {
                handled: true,
                preview,
            });
        }

        self.pending_inputs
            .entry(tool_call_index)
            .or_default()
            .push_str(input_delta);
        if tool.trim().is_empty()
            || (!registry.contains_tool(tool) && registry.contains_tool_prefix(tool))
        {
            return Ok(ToolInputStreamObservation {
                handled: true,
                preview: None,
            });
        }

        if let Entry::Vacant(entry) = self.observers.entry(tool_call_index) {
            let Some(observer) = registry.input_stream_observer(tool, context.clone()) else {
                self.pending_inputs.remove(&tool_call_index);
                self.ignored_calls.insert(tool_call_index);
                return Ok(ToolInputStreamObservation {
                    handled: false,
                    preview: None,
                });
            };
            entry.insert(observer);
        }

        let pending_input = self
            .pending_inputs
            .remove(&tool_call_index)
            .unwrap_or_default();
        let preview = self
            .observers
            .get_mut(&tool_call_index)
            .expect("observer inserted above")
            .on_delta(&ToolInputStreamChunk {
                stream_id,
                attempt: self.attempt,
                tool_call_index,
                input_delta: &pending_input,
                received_bytes,
            })?;
        Ok(ToolInputStreamObservation {
            handled: true,
            preview,
        })
    }

    pub(super) fn flush(&mut self) -> Vec<ToolInputStreamPreview> {
        self.observers
            .values_mut()
            .filter_map(|observer| observer.flush().ok().flatten())
            .collect()
    }

    pub(super) fn reset(&mut self) {
        self.observers.clear();
        self.pending_inputs.clear();
        self.ignored_calls.clear();
    }

    pub(super) fn attempt(&self) -> usize {
        self.attempt
    }
}
