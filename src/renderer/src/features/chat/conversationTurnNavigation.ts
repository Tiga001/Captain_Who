// Renderer chat navigation: derives completed user/assistant turns without changing conversation data.
import type { ChatMessage } from './chatTypes'
import { getAssistantFinalContent, getUserVisibleContent } from './components/chatMessageItemUtils'

export interface ConversationTurnNavigationItem {
  favorited: boolean
  id: string
  userMessageId: string
  source: 'human' | 'agent' | 'workflow' | 'context'
  sourceLabel?: string
  senderAgentId?: string
  userPreview: string
  assistantPreview: string
}

function isAssistantReplySettled(message: ChatMessage) {
  if (message.role !== 'assistant' || message.status === 'pending') return false

  const runStatus = message.agentRun?.status
  if (
    runStatus === 'starting' ||
    runStatus === 'queued' ||
    runStatus === 'running' ||
    runStatus === 'waiting_for_approval' ||
    runStatus === 'waiting_for_user_input'
  ) {
    return false
  }

  if (
    runStatus === 'completed' ||
    runStatus === 'failed' ||
    runStatus === 'cancelled' ||
    runStatus === 'idle'
  ) {
    return true
  }

  return message.status === 'sent' || message.status === 'error'
}

export function normalizeTurnNavigationPreview(value: string) {
  return value
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/```[^\n]*\n?/g, '')
    .replace(/`([^`]+)`/g, '$1')
    .replace(/^\s{0,3}(?:#{1,6}\s+|>\s?|[-+*]\s+|\d+[.)]\s+)/gm, '')
    .replace(/(\*\*|__|~~)(.*?)\1/g, '$2')
    .replace(/\s+/g, ' ')
    .trim()
}

type PreviewNormalizer = (message: ChatMessage, content: string) => string

function getInputSource(
  message: ChatMessage
): Pick<ConversationTurnNavigationItem, 'source' | 'sourceLabel' | 'senderAgentId'> {
  if (message.workflowSource) {
    const names = [
      message.workflowSource.workflowName,
      ...message.workflowSource.sources.map((source) => source.nodeName)
    ]
    const sourceLabel = [...new Set(names.map((name) => name.trim()).filter(Boolean))].join(' · ')
    return { source: 'workflow', ...(sourceLabel ? { sourceLabel } : {}) }
  }
  const origin = message.inputOrigin
  if (origin?.kind === 'agent' || origin?.kind === 'historical_snapshot') {
    return {
      source: origin.kind === 'agent' ? 'agent' : 'context',
      ...(origin.senderAgentId ? { senderAgentId: origin.senderAgentId } : {})
    }
  }
  // Older root conversations have no origin proof and retain their human presentation.
  return { source: 'human' }
}

function getUserPreview(message: ChatMessage, normalize: PreviewNormalizer, sourceLabel?: string) {
  const sources = message.workflowSource?.sources
  // Legacy organization receipts may lack original bodies. Keep their source names instead of
  // guessing message boundaries or showing protocol headers from the assembled input.
  const content = message.workflowSource
    ? sources?.length && sources.every((source) => typeof source.content === 'string')
      ? sources.map((source) => source.content).join('\n\n')
      : ''
    : getUserVisibleContent(message)
  const visibleContent = normalize(message, content)
  if (visibleContent) return visibleContent

  const attachmentNames = message.attachments
    ?.map((attachment) => attachment.name.trim())
    .filter(Boolean)
    .join(' · ')
  return attachmentNames || sourceLabel || ''
}

export function getConversationTurnNavigationItems(
  messages: ChatMessage[]
): ConversationTurnNavigationItem[] {
  return deriveConversationTurnNavigationItems(messages, (_, content) =>
    normalizeTurnNavigationPreview(content)
  )
}

/** Keep one selector per conversation surface; removed/replaced messages are weakly held. */
export function createConversationTurnNavigationSelector(
  normalize = normalizeTurnNavigationPreview
) {
  const previews = new WeakMap<ChatMessage, { content: string; preview: string }>()

  return (messages: ChatMessage[]) =>
    deriveConversationTurnNavigationItems(messages, (message, content) => {
      const cached = previews.get(message)
      // Compare the actual display input too, so an in-place edit cannot leave stale text.
      if (cached?.content === content) return cached.preview
      const preview = normalize(content)
      previews.set(message, { content, preview })
      return preview
    })
}

function deriveConversationTurnNavigationItems(
  messages: ChatMessage[],
  normalize: PreviewNormalizer
): ConversationTurnNavigationItem[] {
  const items: ConversationTurnNavigationItem[] = []
  let inputs: ChatMessage[] = []
  let finalAssistant: ChatMessage | null = null

  const appendSettledInputs = () => {
    // Consecutive inputs share the following reply, including its final retry. Keep every input
    // anchor, but omit the whole pending group until that final assistant message settles.
    if (finalAssistant && isAssistantReplySettled(finalAssistant)) {
      const assistantPreview = normalize(finalAssistant, getAssistantFinalContent(finalAssistant))
      for (const input of inputs) {
        const source = getInputSource(input)
        items.push({
          favorited: input.uiState?.favorited === true,
          id: input.id,
          userMessageId: input.id,
          ...source,
          userPreview: getUserPreview(input, normalize, source.sourceLabel),
          assistantPreview
        })
      }
    }
    inputs = []
    finalAssistant = null
  }

  // Each message and input is visited once. A user after an assistant begins a new group;
  // it must never lend its later reply to an earlier unfinished group.
  for (const message of messages) {
    if (message.role === 'user') {
      if (finalAssistant) appendSettledInputs()
      inputs.push(message)
    } else if (inputs.length) {
      finalAssistant = message
    }
  }
  appendSettledInputs()

  return items
}
