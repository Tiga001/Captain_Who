import type { AppWindowState } from '@mycopilot/host-api'
import type { AgentInputAttachment, SkillSelection } from '@mycopilot/protocol'
import type { ChatConversation } from '../features/chat/chatTypes'

export const DEFAULT_APP_WINDOW_STATE: AppWindowState = {
  isFullScreen: false,
  isMaximized: false
}

export function buildMessageContentWithAttachments(
  content: string,
  _attachments: AgentInputAttachment[]
) {
  void _attachments
  const trimmedContent = content.trim()
  // Attachments are persisted and rendered as structured message data. Keep file names out of
  // the user-visible message body so the same information is not duplicated in prose.
  return trimmedContent
}

export function getEditableLastTurn(conversation: ChatConversation) {
  const messages = conversation.messages
  if (messages.length < 2) return null

  const userIndex = messages.length - 2
  const assistantIndex = messages.length - 1
  const userMessage = messages[userIndex]
  const assistantMessage = messages[assistantIndex]
  if (userMessage?.role !== 'user' || assistantMessage?.role !== 'assistant') return null
  if (userMessage.workflowSource) return null
  if (assistantMessage.status !== 'sent') return null

  const assistantRunStatus = assistantMessage.agentRun?.status
  const assistantSettled =
    !assistantRunStatus ||
    assistantRunStatus === 'completed' ||
    assistantRunStatus === 'failed' ||
    assistantRunStatus === 'cancelled' ||
    assistantRunStatus === 'idle'
  if (!assistantSettled) return null

  return {
    assistantIndex,
    assistantMessage,
    userIndex,
    userMessage
  }
}

export function getActiveRunModelId(conversation: ChatConversation | null) {
  if (!conversation?.modelId) return null

  const latestAssistantMessage = [...conversation.messages]
    .reverse()
    .find((message) => message.role === 'assistant')
  if (!latestAssistantMessage) return null

  const runStatus = latestAssistantMessage.agentRun?.status
  const runIsActive =
    latestAssistantMessage.status === 'pending' ||
    runStatus === 'starting' ||
    runStatus === 'running' ||
    runStatus === 'waiting_for_approval' ||
    runStatus === 'waiting_for_user_input'

  return runIsActive ? conversation.modelId : null
}

export function getActiveRunSkillSelections(
  conversation: ChatConversation | null
): SkillSelection[] {
  if (!conversation) return []

  const latestAssistantMessage = [...conversation.messages]
    .reverse()
    .find((message) => message.role === 'assistant')
  return (
    latestAssistantMessage?.agentRun?.activatedSkills?.map((skill) => ({
      id: skill.id,
      revision: skill.revision
    })) ?? []
  )
}
