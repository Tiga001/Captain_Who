use super::*;
use base64::Engine;
use sha2::{Digest, Sha256};

/// Freeze material at its first model-visible position. The journal contains safe text and
/// immutable image identities, never executable extension state or Provider continuation.
pub(super) fn persist_run_context_materials(
    frame: &mut ContextFrame,
    recorder: &Arc<Mutex<ConversationTraceRecorder>>,
    observer: Option<&AgentConversationTraceObserver>,
    run_id: &str,
    assistant_message_id: Option<&str>,
    attachments: &[crate::AgentInputAttachment],
) -> AgentResult<()> {
    let pending = frame.pending_run_materials();
    if pending.is_empty() {
        return Ok(());
    }
    let owner = assistant_message_id
        .map(str::to_string)
        .unwrap_or_else(|| format!("{run_id}:assistant"));
    let bindings = update_trace_atomically(recorder, |staged| {
        let mut bindings = Vec::new();
        for (index, kind, message) in pending {
            let mut refs = Vec::new();
            let mut used_attachment_ids = std::collections::BTreeSet::new();
            for image in message.images() {
                let attachment = attachments
                    .iter()
                    .find(|attachment| {
                        !used_attachment_ids.contains(attachment.id.as_str())
                            && attachment.kind == crate::AgentInputAttachmentKind::Image
                            && attachment.encoding == crate::AgentInputAttachmentEncoding::Base64
                            && attachment.mime_type.as_deref() == Some(image.mime_type.as_str())
                            && attachment.data == image.data_base64
                    })
                    .ok_or_else(|| {
                        AgentError::new("Context image has no trusted attachment identity.")
                    })?;
                used_attachment_ids.insert(attachment.id.as_str());
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&image.data_base64)
                    .map_err(|_| AgentError::new("Context image has invalid base64 encoding."))?;
                refs.push(crate::ConversationContextImageRef {
                    attachment_id: attachment.id.clone(),
                    mime_type: image.mime_type.clone(),
                    sha256: format!("sha256:{:x}", Sha256::digest(bytes)),
                });
            }
            let event_id = format!("{run_id}:context-material:{}", staged.next_sequence());
            let sequence = staged
                .record_context_material(&event_id, kind, message.content(), &refs, now_ms())
                .map_err(AgentError::new)?;
            bindings.push((index, sequence, refs));
        }
        Ok(bindings)
    })?;
    for (index, sequence, refs) in bindings {
        frame.bind_run_material(index, &owner, sequence, refs);
    }
    publish_trace_snapshot(recorder, observer)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_images_keep_distinct_attachment_identities_and_are_journaled_once() {
        let attachments = ["first", "second"].map(|id| crate::AgentInputAttachment {
            id: id.to_string(),
            kind: crate::AgentInputAttachmentKind::Image,
            name: format!("{id}.png"),
            mime_type: Some("image/png".to_string()),
            size_bytes: 3,
            encoding: crate::AgentInputAttachmentEncoding::Base64,
            data: "YWJj".to_string(),
            truncated: None,
        });
        let mut message = LlmMessage::text(LlmMessageRole::User, "two images");
        *message.images_mut().unwrap() = attachments
            .iter()
            .map(|attachment| crate::llm::LlmImage {
                mime_type: attachment.mime_type.clone().unwrap(),
                data_base64: attachment.data.clone(),
            })
            .collect();
        let mut frame = ContextFrame::new(vec![ContextItem::new(
            message.clone(),
            ContextMetadata::new(
                ContextSource::InputAttachment,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_source(ContextSource::RunBootstrap),
        )]);
        let recorder = Arc::new(Mutex::new(ConversationTraceRecorder::default()));
        for _ in 0..2 {
            persist_run_context_materials(
                &mut frame,
                &recorder,
                None,
                "run",
                Some("assistant"),
                &attachments,
            )
            .unwrap();
        }
        let snapshot = recorder.lock().unwrap().snapshot();
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.model_context_items.len(), 1);
        assert_eq!(
            snapshot.model_context_items[0]
                .images
                .iter()
                .map(|image| image.attachment_id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        assert_eq!(frame.to_messages(), [message]);
    }
}
