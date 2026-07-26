// Renderer chat navigation: derives completed user/assistant turns without changing conversation data.
import type { ChatMessage } from './chatTypes'
import { getAssistantFinalContent, getUserVisibleContent } from './components/chatMessageItemUtils'

export interface ConversationTurnNavigationItem {
  id: string
  userMessageId: string
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
    runStatus === 'waiting_for_approval'
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

function getUserPreview(message: ChatMessage) {
  const visibleContent = normalizeTurnNavigationPreview(getUserVisibleContent(message))
  if (visibleContent) return visibleContent

  return (
    message.attachments
      ?.map((attachment) => attachment.name.trim())
      .filter(Boolean)
      .join(' · ') ?? ''
  )
}

export function getConversationTurnNavigationItems(
  messages: ChatMessage[]
): ConversationTurnNavigationItem[] {
  const items: ConversationTurnNavigationItem[] = []

  for (let userIndex = 0; userIndex < messages.length; userIndex += 1) {
    const userMessage = messages[userIndex]
    if (userMessage.role !== 'user') continue

    let finalAssistant: ChatMessage | null = null
    for (let messageIndex = userIndex + 1; messageIndex < messages.length; messageIndex += 1) {
      const candidate = messages[messageIndex]
      if (candidate.role === 'user') break
      if (candidate.role === 'assistant') finalAssistant = candidate
    }

    // A pending final turn is deliberately absent from the rail until its reply settles.
    if (!finalAssistant || !isAssistantReplySettled(finalAssistant)) continue

    items.push({
      id: userMessage.id,
      userMessageId: userMessage.id,
      userPreview: getUserPreview(userMessage),
      assistantPreview: normalizeTurnNavigationPreview(getAssistantFinalContent(finalAssistant))
    })
  }

  return items
}
