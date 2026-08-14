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
import type { ReactNode } from 'react'
import { Split, X } from 'lucide-react'
import type {
  AgentContextWindowSnapshot,
  AgentProposedAction,
  AgentProviderTransitionOperation,
  GitTurnDiffSummary,
  StorageConversationForkPoint
} from '@mycopilot/protocol'
import { ChatComposer } from './components/ChatComposer'
import { AgentApprovalDialog } from './components/AgentApprovalDialog'
import { AgentTodoProgress } from './components/AgentTodoProgress'
import { ChatMessageItem } from './components/ChatMessageItem'
import { ConversationTurnNavigationRail } from './components/ConversationTurnNavigationRail'
import {
  ConversationModelTransitionDivider,
  ModelTransitionConfirmationDialog
} from './components/ConversationModelTransition'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatConversationContinuationOrigin,
  ChatMessage,
  ChatQueuedMessage,
  ChatSubmitOptions
} from './chatTypes'
import type { ModelTransitionConfirmation } from './modelTransitionUiState'
import { stripAttachmentSummary } from './chatAttachments'
import { getConversationTurnNavigationItems } from './conversationTurnNavigation'
import { getLatestAgentTodo } from './todoLifetime'
import { useTurnDiffSummaries } from './useTurnDiffSummaries'
import { getAgentActionApprovalStatus } from '../agentRun/agentActionUtils'
import {
  CollaborationTimelineActivityList,
  copyCollaborationTimelineSelection,
  type CollaborationTimelineActivity
} from '../agentCollaboration/CollaborationTimelineActivity'
import { projectCollaborationTimelineActivities } from '../agentCollaboration/collaborationTimelineModel'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import './ChatConversationPage.css'

interface AgentApprovalOptions {
  rememberForRun?: boolean
}

interface ConversationSurfaceCommonProps {
  conversation: ChatConversation
  initialScrollTop?: number | null
  onScrollPositionChange?: (conversationId: string, scrollTop: number) => void
  scrollTargetMessageId?: string | null
  scrollToBottomSignal?: number
  showTokenUsageDetails: boolean
}

export interface InteractiveConversationSurfaceProps extends ConversationSurfaceCommonProps {
  mode: 'interactive'
  /** Root-only semantic collaboration activity supplied by the durable tree/event projection. */
  collaborationContent?: ReactNode
  /** Typed durable semantic activity; generic Tool/Mailbox/model text never enters this path. */
  collaborationTimelineActivities?: readonly CollaborationTimelineActivity[]
  contextWindowIndicatorEnabled?: boolean
  contextWindowSnapshot?: AgentContextWindowSnapshot
  composerDraft: ChatComposerDraft
  editSelectedModelAvailable: boolean
  editSelectedModelSupportsImage: boolean
  modelTransitionConfirmation?: ModelTransitionConfirmation
  modelTransitionOperations?: AgentProviderTransitionOperation[]
  onApproveAgentAction?: (
    messageId: string,
    action: AgentProposedAction,
    options?: AgentApprovalOptions
  ) => void
  onCancelAgentAction?: (messageId: string, action: AgentProposedAction) => void
  onComposerDraftChange: (draft: ChatComposerDraft) => void
  onComposerDraftMessageChange?: (draft: ChatComposerDraft) => void
  onContinueInNewTask?: (forkPoint: StorageConversationForkPoint) => void | Promise<void>
  onEditLastUserMessage?: (messageId: string, content: string) => void | Promise<void>
  onGuideQueuedMessage?: (message: ChatQueuedMessage) => void
  onMessageUiStateChange?: (
    messageId: string,
    uiState: ChatConversation['messages'][number]['uiState']
  ) => void
  onModelTransitionCancel?: () => void
  onModelTransitionConfirm?: () => void | Promise<void>
  onModelTransitionRetry?: (operation: AgentProviderTransitionOperation) => void | Promise<void>
  onOpenCollaborationAgent?: (agentId: string) => void
  onOpenContinuationOrigin?: (origin: ChatConversationContinuationOrigin) => void | Promise<void>
  onRejectAgentAction?: (messageId: string, action: AgentProposedAction, message?: string) => void
  onReviewLastTurn?: (filePath?: string) => void
  onStopGenerating?: () => void
  onSubmitMessage: (
    message: string,
    options: ChatSubmitOptions
  ) => boolean | void | Promise<boolean | void>
  permissionModeAvailability: {
    custom: boolean
    full: boolean
  }
  skillCatalogRefreshToken?: number
}

export interface ObserverConversationSurfaceProps extends ConversationSurfaceCommonProps {
  mode: 'observer'
  /** Root authority paired with the exact child conversation for read-only Artifact access. */
  rootConversationId: string
  /** Direct parent identity makes the transport-origin badge precise without changing chat role. */
  parentAgentId?: string | null
  /** Presentation-safe labels from the already-authorized current-tree snapshot. */
  agentLabelsById?: Readonly<Record<string, string>>
}

export type ConversationSurfaceProps =
  InteractiveConversationSurfaceProps | ObserverConversationSurfaceProps

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
        messageId: message.id,
        mcpInvocationState:
          action.type === 'mcp_tool_call'
            ? run.mcpInvocations?.find(
                (invocation) => invocation.invocationId === action.approval.identity.invocationId
              )?.state
            : undefined
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
  agentLabelsById?: Readonly<Record<string, string>>
  collaborationTimelineActivities?: readonly CollaborationTimelineActivity[]
  conversation: ChatConversation
  editSelectedModelAvailable: boolean
  editSelectedModelSupportsImage: boolean
  editableLastUserMessageId: string | null
  lastAssistantMessageId?: string
  mode?: 'interactive' | 'observer'
  modelTransitionOperations?: AgentProviderTransitionOperation[]
  onApproveAgentAction?: (
    messageId: string,
    action: AgentProposedAction,
    options?: AgentApprovalOptions
  ) => void
  onCancelAgentAction?: (messageId: string, action: AgentProposedAction) => void
  onContinueInNewTask?: (forkPoint: StorageConversationForkPoint) => void | Promise<void>
  onEditLastUserMessage?: (messageId: string, content: string) => void | Promise<void>
  onOpenContinuationOrigin?: (origin: ChatConversationContinuationOrigin) => void | Promise<void>
  onMessageUiStateChange?: (messageId: string, uiState: ChatMessage['uiState']) => void
  onModelTransitionRetry?: (operation: AgentProviderTransitionOperation) => void | Promise<void>
  onOpenCollaborationAgent?: (agentId: string) => void
  onRejectAgentAction?: (messageId: string, action: AgentProposedAction, message?: string) => void
  onReviewLastTurn?: (filePath?: string) => void
  parentAgentId?: string | null
  observerRootConversationId?: string
  showTokenUsageDetails: boolean
  turnDiffSummariesByMessageId?: ReadonlyMap<string, GitTurnDiffSummary>
}

export const ChatMessageList = memo(function ChatMessageList({
  agentLabelsById,
  collaborationTimelineActivities = [],
  conversation,
  editSelectedModelAvailable,
  editSelectedModelSupportsImage,
  editableLastUserMessageId,
  lastAssistantMessageId,
  mode = 'interactive',
  modelTransitionOperations = [],
  onApproveAgentAction,
  onCancelAgentAction,
  onContinueInNewTask,
  onEditLastUserMessage,
  onOpenContinuationOrigin,
  onMessageUiStateChange,
  onModelTransitionRetry,
  onOpenCollaborationAgent,
  onRejectAgentAction,
  onReviewLastTurn,
  parentAgentId,
  observerRootConversationId,
  showTokenUsageDetails,
  turnDiffSummariesByMessageId
}: ChatMessageListProps) {
  const continuationOrigin = conversation.continuationOrigin
  const collaborationAgentNavigation = mode === 'interactive' ? onOpenCollaborationAgent : undefined
  const messageIds = useMemo(
    () => new Set(conversation.messages.map((message) => message.id)),
    [conversation.messages]
  )
  const collaborationTimeline = useMemo(
    () =>
      projectCollaborationTimelineActivities(
        collaborationTimelineActivities,
        conversation.messages
      ),
    [collaborationTimelineActivities, conversation.messages]
  )
  const [observerTimelineCollapsed, setObserverTimelineCollapsed] = useState<
    Readonly<Record<string, boolean>>
  >({})

  useEffect(() => {
    setObserverTimelineCollapsed({})
  }, [conversation.id])

  return (
    <>
      {conversation.messages.map((message) => (
        <Fragment key={message.id}>
          {collaborationAgentNavigation && (
            <CollaborationTimelineActivityList
              activities={collaborationTimeline.beforeMessage.get(message.id) ?? []}
              onOpenAgent={collaborationAgentNavigation}
            />
          )}
          <ChatMessageItem
            agentLabelsById={agentLabelsById}
            collaborationTimelineActivities={
              collaborationAgentNavigation
                ? (collaborationTimeline.anchoredMessage.get(message.id) ?? [])
                : []
            }
            conversationId={conversation.id}
            isLastAssistantMessage={message.id === lastAssistantMessageId}
            message={message}
            mode={mode}
            onApprove={mode === 'interactive' ? onApproveAgentAction : undefined}
            onCancel={mode === 'interactive' ? onCancelAgentAction : undefined}
            editSelectedModelAvailable={editSelectedModelAvailable}
            editSelectedModelSupportsImage={editSelectedModelSupportsImage}
            onEditSubmit={
              mode === 'interactive' && message.id === editableLastUserMessageId
                ? onEditLastUserMessage
                : undefined
            }
            onContinueInNewTask={
              mode === 'interactive' && isAssistantReplyComplete(message) && onContinueInNewTask
                ? (messageId) =>
                    onContinueInNewTask({
                      kind: 'assistant_reply',
                      assistantMessageId: messageId
                    })
                : undefined
            }
            onOpenCollaborationAgent={collaborationAgentNavigation}
            onReject={mode === 'interactive' ? onRejectAgentAction : undefined}
            onReviewLastTurn={mode === 'interactive' ? onReviewLastTurn : undefined}
            onTimelineCollapsedChange={
              mode === 'observer'
                ? (messageId, collapsed) =>
                    setObserverTimelineCollapsed((current) => ({
                      ...current,
                      [messageId]: collapsed
                    }))
                : undefined
            }
            onUiStateChange={mode === 'interactive' ? onMessageUiStateChange : undefined}
            parentAgentId={parentAgentId}
            observerRootConversationId={observerRootConversationId}
            projectId={conversation.projectId}
            showTokenUsageDetails={showTokenUsageDetails}
            timelineCollapsedOverride={
              mode === 'observer' ? observerTimelineCollapsed[message.id] : undefined
            }
            turnDiffSummary={turnDiffSummariesByMessageId?.get(message.id)}
          />
          {continuationOrigin?.boundaryMessageId === message.id && (
            <ConversationContinuationDivider
              onOpen={
                mode === 'interactive' && onOpenContinuationOrigin
                  ? () => onOpenContinuationOrigin(continuationOrigin)
                  : undefined
              }
            />
          )}
          {modelTransitionOperations
            .filter(
              (operation) =>
                operation.status === 'completed' && operation.coveredThroughMessageId === message.id
            )
            .map((operation) => (
              <ConversationModelTransitionDivider
                key={operation.operationId}
                onContinueInNewTask={
                  mode === 'interactive' &&
                  operation.status === 'completed' &&
                  operation.summaryId &&
                  onContinueInNewTask
                    ? () =>
                        onContinueInNewTask({
                          kind: 'provider_transition_boundary',
                          operationId: operation.operationId
                        })
                    : undefined
                }
                operation={operation}
              />
            ))}
        </Fragment>
      ))}
      {modelTransitionOperations
        .filter(
          (operation) =>
            operation.status !== 'completed' ||
            !operation.coveredThroughMessageId ||
            !messageIds.has(operation.coveredThroughMessageId)
        )
        .map((operation) => (
          <ConversationModelTransitionDivider
            key={operation.operationId}
            onContinueInNewTask={
              mode === 'interactive' &&
              operation.status === 'completed' &&
              operation.summaryId &&
              onContinueInNewTask
                ? () =>
                    onContinueInNewTask({
                      kind: 'provider_transition_boundary',
                      operationId: operation.operationId
                    })
                : undefined
            }
            onRetry={
              mode === 'interactive' && operation.status === 'failed' && onModelTransitionRetry
                ? () => onModelTransitionRetry(operation)
                : undefined
            }
            operation={operation}
          />
        ))}
      {collaborationAgentNavigation && (
        <CollaborationTimelineActivityList
          activities={collaborationTimeline.tail}
          onOpenAgent={collaborationAgentNavigation}
        />
      )}
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

export function ConversationSurface(props: ConversationSurfaceProps) {
  const { t } = useFrontendConfig()
  const {
    conversation,
    initialScrollTop = null,
    onScrollPositionChange,
    scrollTargetMessageId,
    scrollToBottomSignal = 0,
    showTokenUsageDetails
  } = props
  const interactive = props.mode === 'interactive' ? props : null
  const messagesRef = useRef<HTMLDivElement>(null)
  const handledScrollTargetRef = useRef<string | null>(null)
  const [sideChatPlaceholder, setSideChatPlaceholder] = useState<ChatQueuedMessage | null>(null)
  const isGenerating = conversation.messages.some(
    (message) => message.role === 'assistant' && message.status === 'pending'
  )
  const lastAssistantMessageId = [...conversation.messages]
    .reverse()
    .find((message) => message.role === 'assistant')?.id
  const lastCommittedUserMessageId = [...conversation.messages]
    .reverse()
    .find((message) => message.role === 'user')?.id
  const activeAssistantRun = [...conversation.messages]
    .reverse()
    .find((message) => message.role === 'assistant' && message.status === 'pending')?.agentRun
  const canGuideQueuedMessages = Boolean(
    activeAssistantRun?.runId && activeAssistantRun.status === 'running'
  )
  const pendingApprovalTarget = useMemo(
    () => (interactive ? getPendingApprovalTarget(conversation) : null),
    [conversation, interactive]
  )
  const turnNavigationItems = useMemo(
    () => getConversationTurnNavigationItems(conversation.messages),
    [conversation.messages]
  )
  const activeTodo = useMemo(
    () => (interactive ? getLatestAgentTodo(conversation) : null),
    [conversation, interactive]
  )
  const turnDiffSummariesByMessageId = useTurnDiffSummaries(conversation, {
    enabled: props.mode === 'interactive'
  })
  const hasPendingApproval = Boolean(pendingApprovalTarget)
  const editableLastUserMessageId =
    interactive && !hasPendingApproval ? getEditableLastUserMessageId(conversation) : null

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
      target.scrollIntoView({
        block: 'center',
        behavior: 'smooth'
      })
    })

    return () => window.cancelAnimationFrame(animationFrameId)
  }, [conversation.id, conversation.messages, scrollTargetMessageId])

  useEffect(() => rememberCurrentScrollPosition, [rememberCurrentScrollPosition])

  useEffect(() => {
    setSideChatPlaceholder(null)
  }, [conversation.id])

  return (
    <section
      className="chat-conversation-page conversation-surface"
      aria-label={conversation.title}
      data-approval-pending={hasPendingApproval ? 'true' : undefined}
      data-conversation-id={conversation.id}
      data-conversation-surface-mode={props.mode}
    >
      <div className="chat-conversation-page__messages-region">
        <div
          className="chat-conversation-page__messages"
          onCopy={copyCollaborationTimelineSelection}
          onScroll={rememberCurrentScrollPosition}
          ref={messagesRef}
        >
          <ChatMessageList
            agentLabelsById={props.mode === 'observer' ? props.agentLabelsById : undefined}
            collaborationTimelineActivities={interactive?.collaborationTimelineActivities}
            conversation={conversation}
            editSelectedModelAvailable={interactive?.editSelectedModelAvailable ?? false}
            editSelectedModelSupportsImage={interactive?.editSelectedModelSupportsImage ?? false}
            editableLastUserMessageId={editableLastUserMessageId}
            lastAssistantMessageId={lastAssistantMessageId}
            mode={props.mode}
            modelTransitionOperations={interactive?.modelTransitionOperations}
            onApproveAgentAction={interactive?.onApproveAgentAction}
            onCancelAgentAction={interactive?.onCancelAgentAction}
            onContinueInNewTask={interactive?.onContinueInNewTask}
            onEditLastUserMessage={interactive?.onEditLastUserMessage}
            onOpenContinuationOrigin={interactive?.onOpenContinuationOrigin}
            onMessageUiStateChange={interactive?.onMessageUiStateChange}
            onModelTransitionRetry={interactive?.onModelTransitionRetry}
            onOpenCollaborationAgent={interactive?.onOpenCollaborationAgent}
            onRejectAgentAction={interactive?.onRejectAgentAction}
            onReviewLastTurn={interactive?.onReviewLastTurn}
            parentAgentId={props.mode === 'observer' ? props.parentAgentId : undefined}
            observerRootConversationId={
              props.mode === 'observer' ? props.rootConversationId : undefined
            }
            showTokenUsageDetails={showTokenUsageDetails}
            turnDiffSummariesByMessageId={turnDiffSummariesByMessageId}
          />
          {interactive?.collaborationContent}
        </div>
        <ConversationTurnNavigationRail
          items={turnNavigationItems}
          scrollContainerRef={messagesRef}
        />
      </div>

      {interactive && (
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
              mcpInvocationState={pendingApprovalTarget.mcpInvocationState}
              target={pendingApprovalTarget}
              onApprove={interactive.onApproveAgentAction}
              onCancel={interactive.onCancelAgentAction}
              onReject={interactive.onRejectAgentAction}
            />
          ) : (
            <ChatComposer
              canGuideQueuedMessages={canGuideQueuedMessages}
              contextWindowIndicatorEnabled={interactive.contextWindowIndicatorEnabled}
              contextWindowSnapshot={interactive.contextWindowSnapshot}
              defaultProjectId={conversation.projectId}
              draft={interactive.composerDraft}
              isGenerating={isGenerating}
              isModelTransitionRunning={Boolean(
                interactive.modelTransitionOperations?.some(
                  (operation) => operation.status === 'running'
                )
              )}
              messageSyncKey={lastCommittedUserMessageId}
              onDraftChange={interactive.onComposerDraftChange}
              onDraftMessageChange={interactive.onComposerDraftMessageChange}
              onGuideQueuedMessage={interactive.onGuideQueuedMessage}
              onOpenQueuedMessageInSideChat={setSideChatPlaceholder}
              onStopGenerating={interactive.onStopGenerating}
              onSubmitMessage={interactive.onSubmitMessage}
              permissionModeAvailability={interactive.permissionModeAvailability}
              resetKey={conversation.id}
              skillCatalogRefreshToken={interactive.skillCatalogRefreshToken}
            />
          )}
        </div>
      )}
      {interactive?.modelTransitionConfirmation &&
        interactive.onModelTransitionCancel &&
        interactive.onModelTransitionConfirm && (
          <ModelTransitionConfirmationDialog
            onCancel={interactive.onModelTransitionCancel}
            onConfirm={interactive.onModelTransitionConfirm}
            preflight={interactive.modelTransitionConfirmation}
          />
        )}
      {interactive && sideChatPlaceholder && (
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
