import type { ChatAgentTimelineItem } from '../chat/chatTypes'
import {
  hasExactKeys,
  hasOwn,
  isBoundedString,
  isOptionalBoundedString,
  isOptionalSafeInteger,
  isRecord,
  isRecordArray,
  isSafeInteger
} from './persistedAgentRunValidation'

function isTimelineAttachment(record: Record<string, unknown>): boolean {
  return (
    hasExactKeys(
      record,
      ['id', 'kind', 'name', 'sizeBytes'],
      ['mimeType', 'encoding', 'data', 'previewData', 'previewMimeType', 'createdAt']
    ) &&
    isBoundedString(record.id, 1024) &&
    (record.kind === 'file' || record.kind === 'image') &&
    isBoundedString(record.name, 4096) &&
    isSafeInteger(record.sizeBytes) &&
    (!hasOwn(record, 'mimeType') ||
      record.mimeType === null ||
      isBoundedString(record.mimeType, 1024, true)) &&
    (!hasOwn(record, 'encoding') || record.encoding === 'utf8' || record.encoding === 'base64') &&
    isOptionalBoundedString(record, 'data', 32 * 1024 * 1024, true) &&
    (!hasOwn(record, 'previewData') ||
      record.previewData === null ||
      isBoundedString(record.previewData, 32 * 1024 * 1024, true)) &&
    (!hasOwn(record, 'previewMimeType') ||
      record.previewMimeType === null ||
      isBoundedString(record.previewMimeType, 1024, true)) &&
    isOptionalSafeInteger(record, 'createdAt')
  )
}

export function parseTimelineItem(value: unknown): ChatAgentTimelineItem | undefined {
  if (!isRecord(value) || !isBoundedString(value.id, 1024) || typeof value.type !== 'string') {
    return undefined
  }
  if (
    value.type === 'message' &&
    hasExactKeys(value, ['id', 'type', 'content'], ['streamId', 'traceSequence']) &&
    isBoundedString(value.content, 4 * 1024 * 1024, true) &&
    isOptionalBoundedString(value, 'streamId', 1024) &&
    isOptionalSafeInteger(value, 'traceSequence')
  ) {
    return value as unknown as ChatAgentTimelineItem
  }
  if (
    value.type === 'tool_call' &&
    hasExactKeys(value, ['id', 'type', 'callId'], ['traceSequence']) &&
    isBoundedString(value.callId, 1024) &&
    isOptionalSafeInteger(value, 'traceSequence')
  ) {
    return value as unknown as ChatAgentTimelineItem
  }
  if (
    value.type === 'mcp_tool_call' &&
    hasExactKeys(value, ['id', 'type', 'invocationId'], ['traceSequence']) &&
    isBoundedString(value.invocationId, 1024) &&
    isOptionalSafeInteger(value, 'traceSequence')
  ) {
    return value as unknown as ChatAgentTimelineItem
  }
  if (
    value.type === 'context_compaction' &&
    hasExactKeys(value, ['id', 'type', 'operationId', 'status'], ['traceSequence']) &&
    isBoundedString(value.operationId, 1024) &&
    ['running', 'applied', 'skipped', 'failed', 'cancelled'].includes(value.status as string) &&
    isOptionalSafeInteger(value, 'traceSequence')
  ) {
    return value as unknown as ChatAgentTimelineItem
  }
  if (
    value.type === 'error' &&
    hasExactKeys(value, ['id', 'type', 'message'], ['traceSequence']) &&
    isBoundedString(value.message, 128 * 1024, true) &&
    isOptionalSafeInteger(value, 'traceSequence')
  ) {
    return value as unknown as ChatAgentTimelineItem
  }
  if (
    value.type === 'user_guidance' &&
    hasExactKeys(
      value,
      ['id', 'type', 'clientMessageId', 'content', 'attachments', 'status', 'createdAt'],
      ['guidanceId', 'rejectionCode', 'error', 'recoverable', 'sequence', 'traceSequence']
    ) &&
    isBoundedString(value.clientMessageId, 1024) &&
    isBoundedString(value.content, 4 * 1024 * 1024, true) &&
    isRecordArray(value.attachments, isTimelineAttachment) &&
    ['submitting', 'queued', 'applied', 'rejected'].includes(value.status as string) &&
    isSafeInteger(value.createdAt) &&
    isOptionalBoundedString(value, 'guidanceId', 1024) &&
    isOptionalBoundedString(value, 'rejectionCode', 1024) &&
    isOptionalBoundedString(value, 'error', 128 * 1024, true) &&
    (!hasOwn(value, 'recoverable') || typeof value.recoverable === 'boolean') &&
    isOptionalSafeInteger(value, 'sequence') &&
    isOptionalSafeInteger(value, 'traceSequence')
  ) {
    return value as unknown as ChatAgentTimelineItem
  }
  return undefined
}
