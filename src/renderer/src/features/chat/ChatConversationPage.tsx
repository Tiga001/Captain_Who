import {
  Fragment,
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState
} from 'react'
import { Split, X } from 'lucide-react'
import type { AgentContextWindowSnapshot, AgentProposedAction } from '@mycopilot/protocol'
import { ChatComposer } from './components/ChatComposer'
import { AgentApprovalDialog } from './components/AgentApprovalDialog'
import { AgentTodoProgress } from './components/AgentTodoProgress'
import { ChatMessageItem } from './components/ChatMessageItem'
import { ConversationTurnNavigationRail } from './components/ConversationTurnNavigationRail'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatConversationContinuationOrigin,
  ChatMessage,
  ChatQueuedMessage,
  ChatSubmitOptions
} from './chatTypes'
import { stripAttachmentSummary } from './chatAttachments'
import { getConversationTurnNavigationItems } from './conversationTurnNavigation'
import { getLatestAgentTodo } from './todoLifetime'
import { getAgentActionApprovalStatus } from '../../app/agentActionUtils'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import './ChatConversationPage.css'

interface AgentApprovalOptions {
  rememberForRun?: boolean
}

interface ChatConversationPageProps {
  contextWindowIndicatorEnabled?: boolean
  contextWindowSnapshot?: AgentContextWindowSnapshot
  conversation: ChatConversation
  composerDraft: ChatComposerDraft
  editSelectedModelAvailable: boolean
  editSelectedModelSupportsImage: boolean
  initialScrollTop?: number | null
  scrollToBottomSignal?: number
  onApproveAgentAction?: (
    messageId: string,
    action: AgentProposedAction,
    options?: AgentApprovalOptions
  ) => void
  onCancelAgentAction?: (messageId: string, action: AgentProposedAction) => void
  onComposerDraftChange: (draft: ChatComposerDraft) => void
  onComposerDraftMessageChange?: (draft: ChatComposerDraft) => void
  onGuideQueuedMessage?: (message: ChatQueuedMessage) => void
  onEditLastUserMessage?: (messageId: string, content: string) => void | Promise<void>
  onContinueInNewTask?: (messageId: string) => void | Promise<void>
  onOpenContinuationOrigin?: (origin: ChatConversationContinuationOrigin) => void | Promise<void>
  onScrollPositionChange?: (conversationId: string, scrollTop: number) => void
  onRejectAgentAction?: (messageId: string, action: AgentProposedAction, message?: string) => void
  onReviewLastTurn?: (filePath?: string) => void
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
  skillCatalogRefreshToken?: number
  showTokenUsageDetails: boolean
  scrollTargetMessageId?: string | null
}

function getPendingApprovalTarget(conversation: ChatConversation) {
  for (let messageIndex = conversation.messages.length - 1; messageIndex >= 0; messageIndex -= 1) {
    const message = conversation.messages[messageIndex]
    const run = message.agentRun
    if (message.role !== 'assistant' || run?.status !== 'waiting_for_approval') continue

    const action = [...run.approvals]
      .reverse()
      .find((candidate) => getAgentActionApprovalStatus(candidate) === 'required')
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

interface ChatMessageListProps {
  conversation: ChatConversation
  editSelectedModelAvailable: boolean
  editSelectedModelSupportsImage: boolean
  editableLastUserMessageId: string | null
  lastAssistantMessageId?: string
  onApproveAgentAction?: (
    messageId: string,
    action: AgentProposedAction,
    options?: AgentApprovalOptions
  ) => void
  onCancelAgentAction?: (messageId: string, action: AgentProposedAction) => void
  onContinueInNewTask?: (messageId: string) => void | Promise<void>
  onEditLastUserMessage?: (messageId: string, content: string) => void | Promise<void>
  onOpenContinuationOrigin?: (origin: ChatConversationContinuationOrigin) => void | Promise<void>
  onMessageUiStateChange?: (messageId: string, uiState: ChatMessage['uiState']) => void
  onRejectAgentAction?: (messageId: string, action: AgentProposedAction, message?: string) => void
  onReviewLastTurn?: (filePath?: string) => void
  showTokenUsageDetails: boolean
}

export const ChatMessageList = memo(function ChatMessageList({
  conversation,
  editSelectedModelAvailable,
  editSelectedModelSupportsImage,
  editableLastUserMessageId,
  lastAssistantMessageId,
  onApproveAgentAction,
  onCancelAgentAction,
  onContinueInNewTask,
  onEditLastUserMessage,
  onOpenContinuationOrigin,
  onMessageUiStateChange,
  onRejectAgentAction,
  onReviewLastTurn,
  showTokenUsageDetails
}: ChatMessageListProps) {
  const continuationOrigin = conversation.continuationOrigin

  return (
    <>
      {conversation.messages.map((message) => (
        <Fragment key={message.id}>
          <ChatMessageItem
            isLastAssistantMessage={message.id === lastAssistantMessageId}
            message={message}
            onApprove={onApproveAgentAction}
            onCancel={onCancelAgentAction}
            editSelectedModelAvailable={editSelectedModelAvailable}
            editSelectedModelSupportsImage={editSelectedModelSupportsImage}
            onEditSubmit={
              message.id === editableLastUserMessageId ? onEditLastUserMessage : undefined
            }
            onContinueInNewTask={
              isAssistantReplyComplete(message) ? onContinueInNewTask : undefined
            }
            onReject={onRejectAgentAction}
            onReviewLastTurn={onReviewLastTurn}
            onUiStateChange={onMessageUiStateChange}
            projectId={conversation.projectId}
            showTokenUsageDetails={showTokenUsageDetails}
          />
          {continuationOrigin?.boundaryMessageId === message.id && (
            <ConversationContinuationDivider
              onOpen={
                onOpenContinuationOrigin
                  ? () => onOpenContinuationOrigin(continuationOrigin)
                  : undefined
              }
            />
          )}
        </Fragment>
      ))}
    </>
  )
})

export function ConversationContinuationDivider({
  onOpen
}: {
  onOpen?: () => void | Promise<void>
}) {
  const { t } = useFrontendConfig()
  const [isOpening, setIsOpening] = useState(false)
  const isOpeningRef = useRef(false)

  return (
    <div className="conversation-continuation-divider" data-testid="continuation-divider">
      <span aria-hidden="true" />
      <button
        aria-label={t('chat.continuationOrigin')}
        disabled={!onOpen || isOpening}
        onClick={() => {
          if (!onOpen || isOpeningRef.current) return
          isOpeningRef.current = true
          setIsOpening(true)
          void Promise.resolve(onOpen()).finally(() => {
            isOpeningRef.current = false
            setIsOpening(false)
          })
        }}
        type="button"
      >
        <Split aria-hidden="true" />
        <span>{t('chat.continuationOrigin')}</span>
      </button>
      <span aria-hidden="true" />
    </div>
  )
}

export function ChatConversationPage({
  contextWindowIndicatorEnabled = false,
  contextWindowSnapshot,
  composerDraft,
  conversation,
  editSelectedModelAvailable,
  editSelectedModelSupportsImage,
  initialScrollTop = null,
  scrollToBottomSignal = 0,
  onApproveAgentAction,
  onCancelAgentAction,
  onComposerDraftChange,
  onComposerDraftMessageChange,
  onGuideQueuedMessage,
  onEditLastUserMessage,
  onContinueInNewTask,
  onOpenContinuationOrigin,
  onScrollPositionChange,
  onRejectAgentAction,
  onReviewLastTurn,
  onStopGenerating,
  onSubmitMessage,
  onMessageUiStateChange,
  permissionModeAvailability,
  skillCatalogRefreshToken,
  showTokenUsageDetails,
  scrollTargetMessageId
}: ChatConversationPageProps) {
  const { t } = useFrontendConfig()
  const messagesRef = useRef<HTMLDivElement>(null)
  const handledScrollTargetRef = useRef<string | null>(null)
  const [sideChatPlaceholder, setSideChatPlaceholder] = useState<ChatQueuedMessage | null>(null)
  const isGenerating = conversation.messages.some(
    (message) => message.role === 'assistant' && message.status === 'pending'
  )
  const lastAssistantMessageId = [...conversation.messages]
    .reverse()
    .find((message) => message.role === 'assistant')?.id
  const activeAssistantRun = [...conversation.messages]
    .reverse()
    .find((message) => message.role === 'assistant' && message.status === 'pending')?.agentRun
  const canGuideQueuedMessages = Boolean(
    activeAssistantRun?.runId && activeAssistantRun.status === 'running'
  )
  const pendingApprovalTarget = useMemo(
    () => getPendingApprovalTarget(conversation),
    [conversation]
  )
  const turnNavigationItems = useMemo(
    () => getConversationTurnNavigationItems(conversation.messages),
    [conversation.messages]
  )
  const activeTodo = useMemo(() => getLatestAgentTodo(conversation), [conversation])
  const hasPendingApproval = Boolean(pendingApprovalTarget)
  const editableLastUserMessageId = hasPendingApproval
    ? null
    : getEditableLastUserMessageId(conversation)

  const rememberCurrentScrollPosition = useCallback(() => {
    const messagesElement = messagesRef.current
    if (!messagesElement) return
    onScrollPositionChange?.(conversation.id, messagesElement.scrollTop)
  }, [conversation.id, onScrollPositionChange])

  useLayoutEffect(() => {
    if (scrollTargetMessageId) return undefined
    const messagesElement = messagesRef.current
    if (!messagesElement) return undefined

    const animationFrameId = window.requestAnimationFrame(() => {
      if (initialScrollTop === null || initialScrollTop === undefined) {
        messagesElement.scrollTop = messagesElement.scrollHeight
        return
      }

      const maxScrollTop = Math.max(0, messagesElement.scrollHeight - messagesElement.clientHeight)
      messagesElement.scrollTop = Math.min(initialScrollTop, maxScrollTop)
    })

    return () => window.cancelAnimationFrame(animationFrameId)
  }, [conversation.id, initialScrollTop, scrollTargetMessageId])

  useLayoutEffect(() => {
    if (!hasPendingApproval || scrollTargetMessageId) return
    const messagesElement = messagesRef.current
    if (!messagesElement) return
    messagesElement.scrollTop = messagesElement.scrollHeight
  }, [conversation.messages, hasPendingApproval, scrollTargetMessageId])

  useLayoutEffect(() => {
    if (!scrollToBottomSignal || scrollTargetMessageId) return
    const messagesElement = messagesRef.current
    if (!messagesElement) return
    messagesElement.scrollTop = messagesElement.scrollHeight
  }, [scrollTargetMessageId, scrollToBottomSignal])

  useLayoutEffect(() => {
    if (!scrollTargetMessageId) {
      handledScrollTargetRef.current = null
      return
    }
    const messagesElement = messagesRef.current
    if (!messagesElement) return
    const targetKey = `${conversation.id}:${scrollTargetMessageId}`
    if (handledScrollTargetRef.current === targetKey) return

    const animationFrameId = window.requestAnimationFrame(() => {
      const target = Array.from(
        messagesElement.querySelectorAll<HTMLElement>('[data-message-id]')
      ).find((element) => element.dataset.messageId === scrollTargetMessageId)

      if (!target) return
      handledScrollTargetRef.current = targetKey
      target?.scrollIntoView({
        block: 'center',
        behavior: 'smooth'
      })
    })

    return () => window.cancelAnimationFrame(animationFrameId)
  }, [conversation.id, conversation.messages, scrollTargetMessageId])

  useEffect(() => {
    return rememberCurrentScrollPosition
  }, [rememberCurrentScrollPosition])

  useEffect(() => {
    setSideChatPlaceholder(null)
  }, [conversation.id])

  return (
    <section
      className="chat-conversation-page"
      aria-label={conversation.title}
      data-approval-pending={hasPendingApproval ? 'true' : undefined}
    >
      <div className="chat-conversation-page__messages-region">
        <div
          className="chat-conversation-page__messages"
          onScroll={rememberCurrentScrollPosition}
          ref={messagesRef}
        >
          <ChatMessageList
            conversation={conversation}
            editSelectedModelAvailable={editSelectedModelAvailable}
            editSelectedModelSupportsImage={editSelectedModelSupportsImage}
            editableLastUserMessageId={editableLastUserMessageId}
            lastAssistantMessageId={lastAssistantMessageId}
            onApproveAgentAction={onApproveAgentAction}
            onCancelAgentAction={onCancelAgentAction}
            onContinueInNewTask={onContinueInNewTask}
            onEditLastUserMessage={onEditLastUserMessage}
            onOpenContinuationOrigin={onOpenContinuationOrigin}
            onMessageUiStateChange={onMessageUiStateChange}
            onRejectAgentAction={onRejectAgentAction}
            onReviewLastTurn={onReviewLastTurn}
            showTokenUsageDetails={showTokenUsageDetails}
          />
        </div>
        <ConversationTurnNavigationRail
          items={turnNavigationItems}
          scrollContainerRef={messagesRef}
        />
      </div>

      <div className="chat-conversation-page__composer">
        {activeTodo && (
          <AgentTodoProgress
            completedAt={activeTodo.completedAt}
            runStatus={activeTodo.runStatus}
            todo={activeTodo.todo}
          />
        )}
        {pendingApprovalTarget ? (
          <AgentApprovalDialog
            target={pendingApprovalTarget}
            onApprove={onApproveAgentAction}
            onReject={onRejectAgentAction}
          />
        ) : (
          <ChatComposer
            canGuideQueuedMessages={canGuideQueuedMessages}
            contextWindowIndicatorEnabled={contextWindowIndicatorEnabled}
            contextWindowSnapshot={contextWindowSnapshot}
            defaultProjectId={conversation.projectId}
            draft={composerDraft}
            isGenerating={isGenerating}
            onDraftChange={onComposerDraftChange}
            onDraftMessageChange={onComposerDraftMessageChange}
            onGuideQueuedMessage={onGuideQueuedMessage}
            onOpenQueuedMessageInSideChat={setSideChatPlaceholder}
            onStopGenerating={onStopGenerating}
            onSubmitMessage={onSubmitMessage}
            permissionModeAvailability={permissionModeAvailability}
            resetKey={conversation.id}
            skillCatalogRefreshToken={skillCatalogRefreshToken}
          />
        )}
      </div>
      {sideChatPlaceholder && (
        <aside
          className="guidance-side-chat-placeholder"
          aria-label={t('chat.sideChatPlaceholderTitle')}
        >
          <header>
            <strong>{t('chat.sideChatPlaceholderTitle')}</strong>
            <button
              aria-label={t('chat.closeSideChatPlaceholder')}
              onClick={() => setSideChatPlaceholder(null)}
              type="button"
            >
              <X aria-hidden="true" />
            </button>
          </header>
          <div className="guidance-side-chat-placeholder__message">
            {stripAttachmentSummary(sideChatPlaceholder.content, sideChatPlaceholder.attachments) ||
              t('chat.attachmentOnlyMessage')}
          </div>
          {sideChatPlaceholder.attachments.length > 0 && (
            <p>
              {sideChatPlaceholder.attachments.map((attachment) => attachment.name).join(' · ')}
            </p>
          )}
          <span>{t('chat.sideChatPlaceholderDescription')}</span>
        </aside>
      )}
    </section>
  )
}
