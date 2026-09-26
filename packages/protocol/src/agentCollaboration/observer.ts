import type {
  AgentObserverConversationRequest,
  AgentObserverInputOrigin,
  AgentObserverStreamCursor,
  AgentObserverLiveStreamSnapshot,
  AgentObserverConversation
} from './types'
import {
  record,
  exact,
  text,
  oneOf,
  nullableText,
  integer,
  bool,
  schema,
  boundedOptionalString
} from './validation'
import { parseStorageHumanInteractionResponse } from '../storageHumanInteraction'

const MAX_OBSERVER_PREVIEW_BYTES = 48 * 1024 * 1024

const MAX_OBSERVER_STATE_JSON_BYTES = 32 * 1024 * 1024

export function parseAgentObserverConversationRequest(
  value: unknown
): AgentObserverConversationRequest {
  const item = record(value, 'AgentObserverConversationRequest')
  exact(item, ['rootConversationId', 'conversationId'], 'AgentObserverConversationRequest')
  return {
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    conversationId: text(item.conversationId, 'conversationId')
  }
}

function parseObserverOrigin(value: unknown, context: string): AgentObserverInputOrigin | null {
  if (value === null) return null
  const item = record(value, context)
  exact(
    item,
    [
      'kind',
      'senderAgentId',
      'sourceAgentMessageId',
      'snapshotSourceConversationId',
      'snapshotSourceMessageId'
    ],
    context
  )
  const parsed = {
    kind: oneOf(item.kind, ['human', 'agent', 'historical_snapshot'] as const, `${context}.kind`),
    senderAgentId: nullableText(item.senderAgentId, `${context}.senderAgentId`),
    sourceAgentMessageId: nullableText(
      item.sourceAgentMessageId,
      `${context}.sourceAgentMessageId`
    ),
    snapshotSourceConversationId: nullableText(
      item.snapshotSourceConversationId,
      `${context}.snapshotSourceConversationId`
    ),
    snapshotSourceMessageId: nullableText(
      item.snapshotSourceMessageId,
      `${context}.snapshotSourceMessageId`
    )
  }
  const hasAgent = parsed.senderAgentId !== null && parsed.sourceAgentMessageId !== null
  const hasNoAgent = parsed.senderAgentId === null && parsed.sourceAgentMessageId === null
  const hasSnapshot =
    parsed.snapshotSourceConversationId !== null && parsed.snapshotSourceMessageId !== null
  const hasNoSnapshot =
    parsed.snapshotSourceConversationId === null && parsed.snapshotSourceMessageId === null
  if (
    (parsed.kind === 'human' && (!hasNoAgent || !hasNoSnapshot)) ||
    (parsed.kind === 'agent' && (!hasAgent || !hasNoSnapshot)) ||
    (parsed.kind === 'historical_snapshot' && (!hasSnapshot || (!hasAgent && !hasNoAgent)))
  ) {
    throw new Error(`Invalid ${context} identity`)
  }
  return parsed
}

export function parseObserverStreamCursor(value: unknown): AgentObserverStreamCursor {
  const item = record(value, 'AgentObserverStreamCursor')
  exact(item, ['generation', 'sequence'], 'AgentObserverStreamCursor')
  return {
    generation: text(item.generation, 'AgentObserverStreamCursor.generation'),
    sequence: integer(item.sequence, 'AgentObserverStreamCursor.sequence', 1)
  }
}

function parseObserverLiveStream(value: unknown): AgentObserverLiveStreamSnapshot {
  const item = record(value, 'AgentObserverLiveStreamSnapshot')
  exact(
    item,
    ['runId', 'assistantMessageId', 'cursor', 'stream'],
    'AgentObserverLiveStreamSnapshot'
  )
  let stream: AgentObserverLiveStreamSnapshot['stream'] = null
  if (item.stream !== null) {
    const value = record(item.stream, 'AgentObserverLiveStream')
    exact(
      value,
      ['streamId', 'attempt', 'content', 'traceBoundarySequence', 'committed'],
      'AgentObserverLiveStream'
    )
    if (typeof value.content !== 'string')
      throw new Error('Invalid AgentObserverLiveStream.content')
    stream = {
      streamId: text(value.streamId, 'AgentObserverLiveStream.streamId', 1_024),
      attempt: integer(value.attempt, 'AgentObserverLiveStream.attempt', 1),
      content: value.content,
      traceBoundarySequence: integer(
        value.traceBoundarySequence,
        'AgentObserverLiveStream.traceBoundarySequence'
      ),
      committed: bool(value.committed, 'AgentObserverLiveStream.committed')
    }
  }
  return {
    runId: text(item.runId, 'AgentObserverLiveStreamSnapshot.runId'),
    assistantMessageId: text(
      item.assistantMessageId,
      'AgentObserverLiveStreamSnapshot.assistantMessageId'
    ),
    cursor: parseObserverStreamCursor(item.cursor),
    stream
  }
}

export function parseAgentObserverConversation(value: unknown): AgentObserverConversation | null {
  if (value === null) return null
  const item = record(value, 'AgentObserverConversation')
  exact(
    item,
    [
      'schemaVersion',
      'agentId',
      'rootConversationId',
      'conversationId',
      'projectId',
      'modelId',
      'title',
      'createdAt',
      'updatedAt',
      'messages',
      ...('liveStream' in item ? ['liveStream'] : [])
    ],
    'AgentObserverConversation'
  )
  if (!Array.isArray(item.messages) || item.messages.length > 100_000) {
    throw new Error('Invalid AgentObserverConversation.messages')
  }
  const parsed: AgentObserverConversation = {
    ...(item.liveStream === undefined
      ? {}
      : { liveStream: parseObserverLiveStream(item.liveStream) }),
    schemaVersion: schema(item.schemaVersion, 'AgentObserverConversation'),
    agentId: text(item.agentId, 'agentId'),
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    conversationId: text(item.conversationId, 'conversationId'),
    projectId: nullableText(item.projectId, 'projectId'),
    modelId: nullableText(item.modelId, 'modelId'),
    title:
      typeof item.title === 'string'
        ? item.title
        : (() => {
            throw new Error('Invalid title')
          })(),
    createdAt: integer(item.createdAt, 'createdAt'),
    updatedAt: integer(item.updatedAt, 'updatedAt'),
    messages: item.messages.map((value, index) => {
      const context = `messages[${index}]`
      const message = record(value, context)
      exact(
        message,
        [
          'messageId',
          'role',
          'content',
          'createdAt',
          'status',
          'inputOrigin',
          'attachments',
          'agentRunJson',
          'uiStateJson',
          ...('humanInteractionResponse' in message ? ['humanInteractionResponse'] : [])
        ],
        context
      )
      if (typeof message.content !== 'string') throw new Error(`Invalid ${context}.content`)
      const role = oneOf(message.role, ['user', 'assistant'] as const, `${context}.role`)
      const status =
        message.status === null
          ? null
          : oneOf(message.status, ['pending', 'sent', 'error'] as const, `${context}.status`)
      const inputOrigin = parseObserverOrigin(message.inputOrigin, `${context}.inputOrigin`)
      if (
        (role === 'user' && inputOrigin === null) ||
        (role === 'assistant' && inputOrigin !== null)
      ) {
        throw new Error(`Invalid ${context} origin`)
      }
      if (!Array.isArray(message.attachments) || message.attachments.length > 256) {
        throw new Error(`Invalid ${context}.attachments`)
      }
      return {
        ...(message.humanInteractionResponse == null
          ? {}
          : {
              humanInteractionResponse: parseStorageHumanInteractionResponse({
                role,
                content: message.content,
                humanInteractionResponse: message.humanInteractionResponse
              })
            }),
        messageId: text(message.messageId, `${context}.messageId`),
        role,
        content: message.content,
        createdAt: integer(message.createdAt, `${context}.createdAt`),
        status,
        inputOrigin,
        attachments: message.attachments.map((value, attachmentIndex) => {
          const attachmentContext = `${context}.attachments[${attachmentIndex}]`
          const attachment = record(value, attachmentContext)
          exact(
            attachment,
            [
              'attachmentId',
              'kind',
              'name',
              'mimeType',
              'sizeBytes',
              'previewData',
              'previewMimeType',
              'createdAt'
            ],
            attachmentContext
          )
          return {
            attachmentId: text(attachment.attachmentId, `${attachmentContext}.attachmentId`),
            kind: text(attachment.kind, `${attachmentContext}.kind`),
            name: text(attachment.name, `${attachmentContext}.name`, 4_096),
            mimeType: nullableText(attachment.mimeType, `${attachmentContext}.mimeType`),
            sizeBytes: integer(attachment.sizeBytes, `${attachmentContext}.sizeBytes`),
            previewData: boundedOptionalString(
              attachment.previewData,
              `${attachmentContext}.previewData`,
              MAX_OBSERVER_PREVIEW_BYTES
            ),
            previewMimeType: nullableText(
              attachment.previewMimeType,
              `${attachmentContext}.previewMimeType`
            ),
            createdAt: integer(attachment.createdAt, `${attachmentContext}.createdAt`)
          }
        }),
        agentRunJson: boundedOptionalString(
          message.agentRunJson,
          `${context}.agentRunJson`,
          MAX_OBSERVER_STATE_JSON_BYTES,
          true
        ),
        uiStateJson: boundedOptionalString(
          message.uiStateJson,
          `${context}.uiStateJson`,
          MAX_OBSERVER_STATE_JSON_BYTES,
          true
        )
      }
    })
  }
  if (
    parsed.liveStream &&
    !parsed.messages.some(
      (message) =>
        message.messageId === parsed.liveStream!.assistantMessageId &&
        message.role === 'assistant' &&
        message.agentRunJson !== null &&
        (JSON.parse(message.agentRunJson) as { runId?: unknown })?.runId ===
          parsed.liveStream!.runId
    )
  ) {
    throw new Error('Invalid AgentObserverLiveStreamSnapshot message identity')
  }
  return parsed
}
