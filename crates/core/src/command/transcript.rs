//! Bounded, non-destructive command output history.

use crate::AgentCommandOutputStream;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

const MAX_TRANSCRIPT_CHUNK_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandOutputChunk {
    pub sequence: u64,
    pub stream: AgentCommandOutputStream,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandOutputBatch {
    pub requested_after_sequence: u64,
    pub first_available_sequence: Option<u64>,
    pub latest_sequence: u64,
    pub truncated_before: bool,
    pub output_capture_truncated: bool,
    pub chunks: Vec<CommandOutputChunk>,
}

#[derive(Debug)]
pub(crate) struct CommandTranscript {
    head: VecDeque<CommandOutputChunk>,
    tail: VecDeque<CommandOutputChunk>,
    head_bytes: usize,
    head_sealed: bool,
    tail_bytes: usize,
    head_limit: usize,
    tail_limit: usize,
    latest_sequence: u64,
    truncated: bool,
    capture_truncated: bool,
    closed: bool,
}

impl CommandTranscript {
    pub(crate) fn new(max_bytes: usize) -> Self {
        let max_bytes = max_bytes.max(2);
        let head_limit = max_bytes / 2;
        Self {
            head: VecDeque::new(),
            tail: VecDeque::new(),
            head_bytes: 0,
            head_sealed: false,
            tail_bytes: 0,
            head_limit,
            tail_limit: max_bytes - head_limit,
            latest_sequence: 0,
            truncated: false,
            capture_truncated: false,
            closed: false,
        }
    }

    /// Commits output at the one authoritative sequencing point.  The order is
    /// the order in which the two pipe readers acquire this transcript lock; it
    /// is intentionally not claimed to reconstruct simultaneous OS writes.
    pub(crate) fn commit(
        &mut self,
        stream: AgentCommandOutputStream,
        text: String,
    ) -> Vec<CommandOutputChunk> {
        if self.closed || text.is_empty() {
            return Vec::new();
        }

        let mut committed = Vec::new();
        let mut remaining = text.as_str();
        while !remaining.is_empty() {
            let split = utf8_prefix_len(remaining, MAX_TRANSCRIPT_CHUNK_BYTES);
            let split = if split == 0 { remaining.len() } else { split };
            let (piece, rest) = remaining.split_at(split);
            self.commit_piece(stream, piece.to_string(), &mut committed);
            remaining = rest;
        }
        committed
    }

    fn commit_piece(
        &mut self,
        stream: AgentCommandOutputStream,
        mut text: String,
        committed: &mut Vec<CommandOutputChunk>,
    ) {
        if !self.head_sealed && self.head_bytes < self.head_limit {
            let available = self.head_limit - self.head_bytes;
            let split = utf8_prefix_len(&text, available);
            if split > 0 {
                let tail = text.split_off(split);
                let chunk = self.next_chunk(stream, text);
                self.head_bytes += chunk.text.len();
                self.head.push_back(chunk.clone());
                committed.push(chunk);
                text = tail;
            }
            if !text.is_empty() {
                // Once output crosses the head boundary, later output must
                // never return to the head even if the boundary fell inside a
                // multibyte character.  This preserves sequence order.
                self.head_sealed = true;
            }
        } else if !text.is_empty() {
            self.head_sealed = true;
        }

        if !text.is_empty() {
            let chunk = self.next_chunk(stream, text);
            self.tail_bytes += chunk.text.len();
            self.tail.push_back(chunk.clone());
            committed.push(chunk);
            self.trim_tail();
        }
    }

    fn next_chunk(&mut self, stream: AgentCommandOutputStream, text: String) -> CommandOutputChunk {
        self.latest_sequence = self.latest_sequence.saturating_add(1);
        CommandOutputChunk {
            sequence: self.latest_sequence,
            stream,
            text,
        }
    }

    fn trim_tail(&mut self) {
        while self.tail_bytes > self.tail_limit && self.tail.len() > 1 {
            if let Some(removed) = self.tail.pop_front() {
                self.tail_bytes = self.tail_bytes.saturating_sub(removed.text.len());
                self.truncated = true;
            }
        }
        if self.tail_bytes <= self.tail_limit {
            return;
        }
        let Some(mut only) = self.tail.pop_front() else {
            return;
        };
        let keep_from = utf8_suffix_start(&only.text, self.tail_limit);
        if keep_from > 0 {
            only.text.drain(..keep_from);
            self.truncated = true;
        }
        self.tail_bytes = only.text.len();
        if !only.text.is_empty() {
            self.tail.push_back(only);
        }
    }

    pub(crate) fn mark_capture_truncated(&mut self) {
        self.capture_truncated = true;
    }

    pub(crate) fn close(&mut self) {
        self.closed = true;
    }

    pub(crate) fn latest_sequence(&self) -> u64 {
        self.latest_sequence
    }

    pub(crate) fn output_truncated(&self) -> bool {
        self.truncated || self.capture_truncated
    }

    pub(crate) fn read_after(&self, after: u64, max_bytes: usize) -> CommandOutputBatch {
        let first_available_sequence = self
            .head
            .front()
            .or_else(|| self.tail.front())
            .map(|chunk| chunk.sequence);
        let mut chunks = Vec::new();
        let mut bytes = 0_usize;
        for chunk in self.head.iter().chain(self.tail.iter()) {
            if chunk.sequence <= after {
                continue;
            }
            if !chunks.is_empty() && bytes.saturating_add(chunk.text.len()) > max_bytes {
                break;
            }
            bytes = bytes.saturating_add(chunk.text.len());
            chunks.push(chunk.clone());
        }
        // Head-tail retention can omit bytes within one large committed chunk,
        // not only complete sequence numbers.  Therefore any read whose cursor
        // predates the latest sequence must be told that its requested range
        // contains an eviction once the transcript has truncated.
        let truncated_before = self.truncated && after < self.latest_sequence;
        CommandOutputBatch {
            requested_after_sequence: after,
            first_available_sequence,
            latest_sequence: self.latest_sequence,
            truncated_before,
            output_capture_truncated: self.capture_truncated,
            chunks,
        }
    }
}

fn utf8_prefix_len(text: &str, limit: usize) -> usize {
    let mut index = limit.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn utf8_suffix_start(text: &str, keep: usize) -> usize {
    let mut index = text.len().saturating_sub(keep);
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_utf8_safe_head_and_tail_with_a_monotonic_gap() {
        let mut transcript = CommandTranscript::new(12);
        for text in ["你好", "甲乙", "丙丁", "末尾"] {
            transcript.commit(AgentCommandOutputStream::Stdout, text.to_string());
        }
        let batch = transcript.read_after(0, 1024);
        assert!(transcript.output_truncated());
        assert!(batch.chunks.iter().all(|chunk| !chunk.text.is_empty()));
        assert!(batch
            .chunks
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence));
        assert!(batch
            .chunks
            .iter()
            .all(|chunk| chunk.text.is_char_boundary(0)));
    }

    #[test]
    fn explicit_reads_do_not_drain_each_other() {
        let mut transcript = CommandTranscript::new(1024);
        transcript.commit(AgentCommandOutputStream::Stdout, "one".to_string());
        let first = transcript.read_after(0, 1024);
        let second = transcript.read_after(0, 1024);
        assert_eq!(first, second);
    }

    #[test]
    fn multibyte_head_boundary_never_reorders_later_ascii() {
        let mut transcript = CommandTranscript::new(8);
        transcript.commit(AgentCommandOutputStream::Stdout, "abc".to_string());
        transcript.commit(AgentCommandOutputStream::Stdout, "你".to_string());
        transcript.commit(AgentCommandOutputStream::Stdout, "z".to_string());
        let batch = transcript.read_after(0, 1024);
        assert!(batch
            .chunks
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence));
        assert_eq!(
            batch
                .chunks
                .iter()
                .map(|chunk| chunk.text.as_str())
                .collect::<String>(),
            "abc你z"
        );
    }
}
