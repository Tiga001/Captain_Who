import type { AgentConversationMessage, AgentInputAttachment } from '@mycopilot/protocol'
import { modelConfig } from '../config/modelConfig'
import type { ChatComposerDraft, ChatMessage } from '../features/chat/chatTypes'
import { normalizeSkillSelections } from '../features/skills/skillSelection'

export function createId(prefix: string) {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

export function createConversationTitle(message: string, fallbackTitle: string) {
  const firstLine = message.split(/\r?\n/)[0]?.replace(/\s+/g, ' ').trim() || fallbackTitle
  return firstLine.length > 24 ? `${firstLine.slice(0, 24)}...` : firstLine
}

function mapInputAttachmentToMessageAttachment(
  attachment: AgentInputAttachment
): NonNullable<ChatMessage['attachments']>[number] {
  return {
    id: attachment.id,
    kind: attachment.kind,
    name: attachment.name,
    mimeType: attachment.mimeType,
    sizeBytes: attachment.sizeBytes,
    encoding: attachment.encoding,
    data: attachment.data
  }
}

export function createUserMessage(
  content: string,
  attachments: AgentInputAttachment[] = []
): ChatMessage {
  return {
    id: createId('message'),
    role: 'user',
    content,
    createdAt: Date.now(),
    status: 'sent',
    attachments: attachments.map(mapInputAttachmentToMessageAttachment)
  }
}

export function createAssistantMessage(
  content: string,
  status: ChatMessage['status'] = 'sent'
): ChatMessage {
  return {
    id: createId('message'),
    role: 'assistant',
    content,
    createdAt: Date.now(),
    status
  }
}

export function createComposerDraft(overrides: Partial<ChatComposerDraft> = {}): ChatComposerDraft {
  const draft = {
    message: '',
    permissionMode: 'default',
    modelId: modelConfig.defaults.selectedModelId,
    projectId: null,
    attachments: [],
    skills: [],
    queuedMessages: [],
    updatedAt: Date.now(),
    ...overrides
  }

  return {
    ...draft,
    modelId: draft.modelId || modelConfig.defaults.selectedModelId,
    permissionMode:
      draft.permissionMode === 'default' ||
      draft.permissionMode === 'custom' ||
      draft.permissionMode === 'full'
        ? draft.permissionMode
        : 'default',
    attachments: draft.attachments ?? [],
    skills: normalizeSkillSelections(draft.skills),
    queuedMessages: draft.queuedMessages ?? []
  }
}

export function synchronizeComposerDraftForScope(
  renderedDrafts: Record<string, ChatComposerDraft>,
  authoritativeDrafts: Record<string, ChatComposerDraft>,
  scopeId: string
): Record<string, ChatComposerDraft> {
  const authoritativeDraft = authoritativeDrafts[scopeId]
  if (!authoritativeDraft || renderedDrafts[scopeId] === authoritativeDraft) {
    return renderedDrafts
  }
  return {
    ...renderedDrafts,
    [scopeId]: authoritativeDraft
  }
}

function getChatMessageStatusFromConversationMessage(
  status: AgentConversationMessage['status']
): ChatMessage['status'] {
  return status ?? undefined
}

export function mergeConversationMessageFromBackend(
  currentMessage: ChatMessage,
  backendMessage: AgentConversationMessage
): ChatMessage {
  return {
    ...currentMessage,
    id: backendMessage.id,
    role: backendMessage.role,
    content: backendMessage.content,
    createdAt: backendMessage.createdAt,
    status: getChatMessageStatusFromConversationMessage(backendMessage.status),
    attachments: backendMessage.attachments?.length
      ? backendMessage.attachments
      : currentMessage.attachments
  }
}
