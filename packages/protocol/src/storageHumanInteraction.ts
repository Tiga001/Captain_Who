import {
  parseHumanInteractionResponseDisplay,
  type HumanInteractionResponseDisplay
} from './humanInteraction'
import type { StorageChatConversationRecord, StorageChatMessageRecord } from './storage'
import { expectRecord, invalidProtocolValue } from './skills/validation'
import { parseWorkflowMessageSource } from './workflowRuntime'

/** Parse only Host-produced output metadata, bound to the complete immutable User content. */
export function parseStorageHumanInteractionResponse(
  message: Pick<StorageChatMessageRecord, 'role' | 'content'> & {
    humanInteractionResponse?: unknown
  }
): HumanInteractionResponseDisplay | undefined {
  if (message.humanInteractionResponse == null) return undefined
  const context = 'stored human interaction response'
  const response = parseHumanInteractionResponseDisplay(message.humanInteractionResponse, context)
  const content = parseHumanInteractionResponseDisplay(
    JSON.parse(message.content),
    `${context}.content`
  )
  if (message.role !== 'user' || JSON.stringify(response) !== JSON.stringify(content))
    throw invalidProtocolValue(context, 'display proof must match its complete User content')
  return response
}

export function validateStorageHumanInteractionResponses<T extends StorageChatConversationRecord>(
  conversation: T
): T {
  for (const message of conversation.messages) {
    parseStorageHumanInteractionResponse(message)
    if (message.workflowInput != null) {
      if (message.role !== 'user') throw new Error('Workflow input must be a received message')
      parseWorkflowMessageSource(message.workflowInput)
    }
  }
  return conversation
}

/** Renderer mutations cannot manufacture either Host proof or Renderer display metadata. */
export function assertNoHumanInteractionMessageProof(value: unknown): void {
  const record = expectRecord(value, 'stored message write')
  if ('workflowInput' in record || 'workflowSource' in record)
    throw invalidProtocolValue('stored message write', 'workflow message proof is read-only')
  if ('humanInteractionResponse' in record || 'humanInteractionDisplay' in record)
    throw invalidProtocolValue('stored message write', 'human interaction proof is read-only')
}
