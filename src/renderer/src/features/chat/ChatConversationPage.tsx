// Renderer UI.
import { useEffect, useMemo, useRef } from 'react'
import type { AgentProposedAction } from '@mycopilot/protocol'
import { ChatComposer } from './components/ChatComposer'
import { AgentApprovalDialog } from './components/AgentApprovalDialog'
import { ChatMessageItem } from './components/ChatMessageItem'
import type { ChatComposerDraft, ChatConversation, ChatSubmitOptions } from './chatTypes'
import './ChatConversationPage.css'

interface AgentApprovalOptions {
  rememberForRun?: boolean
}

interface ChatConversationPageProps {
  conversation: ChatConversation
  composerDraft: ChatComposerDraft
  editSelectedModelAvailable: boolean
  editSelectedModelSupportsImage: boolean
  onApproveAgentAction?: (
    messageId: string,
    action: AgentProposedAction,
    options?: AgentApprovalOptions
  ) => void
  onCancelAgentAction?: (messageId: string, action: AgentProposedAction) => void
  onComposerDraftChange: (draft: ChatComposerDraft) => void
  onEditLastUserMessage?: (messageId: string, content: string) => void | Promise<void>
  onRejectAgentAction?: (messageId: string, action: AgentProposedAction, message?: string) => void
  onStopGenerating?: () => void
  onSubmitMessage: (message: string, options: ChatSubmitOptions) => void
  onMessageUiStateChange?: (
    messageId: string,
    uiState: ChatConversation['messages'][number]['uiState']
  ) => void
  permissionModeAvailability: {
    custom: boolean
    full: boolean
  }
  showTokenUsageDetails: boolean
}

function getActionApprovalStatus(action: AgentProposedAction) {
  if (action.type === 'diff') return action.diff.approvalStatus
  if (action.type === 'command') return action.command.approvalStatus
  return action.call.approvalStatus
}

function getPendingApprovalTarget(conversation: ChatConversation) {
  for (let messageIndex = conversation.messages.length - 1; messageIndex >= 0; messageIndex -= 1) {
    const message = conversation.messages[messageIndex]
    const run = message.agentRun
    if (message.role !== 'assistant' || run?.status !== 'waiting_for_approval') continue

    const action = [...run.approvals]
      .reverse()
      .find((candidate) => getActionApprovalStatus(candidate) === 'required')
    if (action) {
      return {
        action,
        messageId: message.id
      }
    }
  }

  return null
}

function isAssistantReplyComplete(message: ChatConversation['messages'][number] | undefined) {
  if (!message || message.role !== 'assistant' || message.status !== 'sent') return false
  const status = message.agentRun?.status
  return (
    !status ||
    status === 'completed' ||
    status === 'failed' ||
    status === 'cancelled' ||
    status === 'idle'
  )
}

function getEditableLastUserMessageId(conversation: ChatConversation) {
  const messages = conversation.messages
  if (messages.length < 2) return null

  const userMessage = messages[messages.length - 2]
  const assistantMessage = messages[messages.length - 1]
  if (userMessage?.role !== 'user') return null
  if (!isAssistantReplyComplete(assistantMessage)) return null

  return userMessage.id
}

export function ChatConversationPage({
  composerDraft,
  conversation,
  editSelectedModelAvailable,
  editSelectedModelSupportsImage,
  onApproveAgentAction,
  onCancelAgentAction,
  onComposerDraftChange,
  onEditLastUserMessage,
  onRejectAgentAction,
  onStopGenerating,
  onSubmitMessage,
  onMessageUiStateChange,
  permissionModeAvailability,
  showTokenUsageDetails
}: ChatConversationPageProps) {
  const messagesRef = useRef<HTMLDivElement>(null)
  const isGenerating = conversation.messages.some(
    (message) => message.role === 'assistant' && message.status === 'pending'
  )
  const lastAssistantMessageId = [...conversation.messages]
    .reverse()
    .find((message) => message.role === 'assistant')?.id
  const pendingApprovalTarget = useMemo(
    () => getPendingApprovalTarget(conversation),
    [conversation]
  )
  const hasPendingApproval = Boolean(pendingApprovalTarget)
  const editableLastUserMessageId = hasPendingApproval
    ? null
    : getEditableLastUserMessageId(conversation)

  useEffect(() => {
    if (!hasPendingApproval) return
    const messagesElement = messagesRef.current
    if (!messagesElement) return
    messagesElement.scrollTop = messagesElement.scrollHeight
  }, [conversation.messages, hasPendingApproval])

  return (
    <section
      className="chat-conversation-page"
      aria-label={conversation.title}
      data-approval-pending={hasPendingApproval ? 'true' : undefined}
    >
      <div className="chat-conversation-page__messages" ref={messagesRef}>
        {conversation.messages.map((message) => (
          <ChatMessageItem
            isLastAssistantMessage={message.id === lastAssistantMessageId}
            key={message.id}
            message={message}
            onApprove={onApproveAgentAction}
            onCancel={onCancelAgentAction}
            editSelectedModelAvailable={editSelectedModelAvailable}
            editSelectedModelSupportsImage={editSelectedModelSupportsImage}
            onEditSubmit={
              message.id === editableLastUserMessageId ? onEditLastUserMessage : undefined
            }
            onReject={onRejectAgentAction}
            onUiStateChange={onMessageUiStateChange}
            projectId={conversation.projectId}
            showTokenUsageDetails={showTokenUsageDetails}
          />
        ))}
      </div>

      <div className="chat-conversation-page__composer">
        {pendingApprovalTarget ? (
          <AgentApprovalDialog
            target={pendingApprovalTarget}
            onApprove={onApproveAgentAction}
            onReject={onRejectAgentAction}
          />
        ) : (
          <ChatComposer
            defaultProjectId={conversation.projectId}
            draft={composerDraft}
            isGenerating={isGenerating}
            onDraftChange={onComposerDraftChange}
            onStopGenerating={onStopGenerating}
            onSubmitMessage={onSubmitMessage}
            permissionModeAvailability={permissionModeAvailability}
            resetKey={conversation.id}
          />
        )}
      </div>
    </section>
  )
}
