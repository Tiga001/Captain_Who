use crate::AgentCommandSessionOutputChunk;
use serde::{Deserialize, Serialize};
use std::io::Read;

pub(crate) const COMMAND_SESSION_RECEIPT_PAYLOAD_COMPRESSION: &str = "zstd_json_v1";
pub(crate) const MAX_COMMAND_SESSION_RECEIPT_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;
const MAX_COMMAND_SESSION_RECEIPT_DECODED_BYTES: u64 = 32 * 1024 * 1024;
const COMMAND_SESSION_RECEIPT_PAYLOAD_SCHEMA_VERSION: u32 = 1;
const ZSTD_LEVEL: i32 = 3;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CommandSessionReceiptPayload {
    schema_version: u32,
    chunks: Vec<AgentCommandSessionOutputChunk>,
}

pub(crate) struct EncodedCommandSessionReceiptPayload {
    pub(crate) compression: &'static str,
    pub(crate) payload: Vec<u8>,
    pub(crate) chunk_count: usize,
}

pub(crate) fn encode_command_session_receipt_payload(
    chunks: &[AgentCommandSessionOutputChunk],
) -> Result<EncodedCommandSessionReceiptPayload, String> {
    validate_chunks(chunks)?;
    let encoded = serde_json::to_vec(&CommandSessionReceiptPayload {
        schema_version: COMMAND_SESSION_RECEIPT_PAYLOAD_SCHEMA_VERSION,
        chunks: chunks.to_vec(),
    })
    .map_err(|error| format!("序列化命令 Session 模型回执失败：{error}"))?;
    if u64::try_from(encoded.len()).unwrap_or(u64::MAX) > MAX_COMMAND_SESSION_RECEIPT_DECODED_BYTES
    {
        return Err("命令 Session 模型回执解压后超过安全上限。".to_string());
    }
    let payload = zstd::stream::encode_all(encoded.as_slice(), ZSTD_LEVEL)
        .map_err(|error| format!("压缩命令 Session 模型回执失败：{error}"))?;
    if payload.len() > MAX_COMMAND_SESSION_RECEIPT_PAYLOAD_BYTES {
        return Err("命令 Session 模型回执压缩后超过安全上限。".to_string());
    }
    Ok(EncodedCommandSessionReceiptPayload {
        compression: COMMAND_SESSION_RECEIPT_PAYLOAD_COMPRESSION,
        payload,
        chunk_count: chunks.len(),
    })
}

pub(crate) fn decode_command_session_receipt_payload(
    compression: &str,
    payload: &[u8],
    expected_chunk_count: usize,
) -> Result<Vec<AgentCommandSessionOutputChunk>, String> {
    if compression != COMMAND_SESSION_RECEIPT_PAYLOAD_COMPRESSION {
        return Err(format!(
            "命令 Session 模型回执使用了不支持的压缩格式：{compression}"
        ));
    }
    if payload.is_empty() || payload.len() > MAX_COMMAND_SESSION_RECEIPT_PAYLOAD_BYTES {
        return Err("命令 Session 模型回执压缩载荷大小无效。".to_string());
    }
    let decoder = zstd::stream::read::Decoder::new(payload)
        .map_err(|error| format!("打开命令 Session 模型回执失败：{error}"))?;
    let mut decoded = Vec::new();
    decoder
        .take(MAX_COMMAND_SESSION_RECEIPT_DECODED_BYTES.saturating_add(1))
        .read_to_end(&mut decoded)
        .map_err(|error| format!("解压命令 Session 模型回执失败：{error}"))?;
    if u64::try_from(decoded.len()).unwrap_or(u64::MAX) > MAX_COMMAND_SESSION_RECEIPT_DECODED_BYTES
    {
        return Err("命令 Session 模型回执解压后超过安全上限。".to_string());
    }
    let decoded: CommandSessionReceiptPayload = serde_json::from_slice(&decoded)
        .map_err(|error| format!("解析命令 Session 模型回执失败：{error}"))?;
    if decoded.schema_version != COMMAND_SESSION_RECEIPT_PAYLOAD_SCHEMA_VERSION {
        return Err("命令 Session 模型回执版本不受支持。".to_string());
    }
    if decoded.chunks.len() != expected_chunk_count {
        return Err("命令 Session 模型回执 chunk 数量不一致。".to_string());
    }
    validate_chunks(&decoded.chunks)?;
    Ok(decoded.chunks)
}

fn validate_chunks(chunks: &[AgentCommandSessionOutputChunk]) -> Result<(), String> {
    let mut previous = None;
    for chunk in chunks {
        if chunk.sequence == 0
            || chunk.output.is_empty()
            || previous.is_some_and(|sequence| chunk.sequence <= sequence)
        {
            return Err("命令 Session 模型回执包含无效 chunk。".to_string());
        }
        previous = Some(chunk.sequence);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentCommandOutputStream;

    #[test]
    fn payload_round_trip_preserves_stream_sequence_and_order() {
        let chunks = vec![
            AgentCommandSessionOutputChunk {
                sequence: 7,
                stream: AgentCommandOutputStream::Stdout,
                output: "out".to_string(),
            },
            AgentCommandSessionOutputChunk {
                sequence: 9,
                stream: AgentCommandOutputStream::Stderr,
                output: "err".to_string(),
            },
        ];
        let encoded = encode_command_session_receipt_payload(&chunks).unwrap();
        assert_eq!(
            decode_command_session_receipt_payload(
                encoded.compression,
                &encoded.payload,
                encoded.chunk_count,
            )
            .unwrap(),
            chunks
        );
    }
}
