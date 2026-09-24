import type { ComposerCommand } from './components/ComposerCommands'
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
import { Split } from 'lucide-react'
import type {
  AgentApprovalScope,
  AgentContextWindowSnapshot,
  AgentProposedAction,
  AgentProviderTransitionOperation,
  AgentManualContextCompactionOperation,
  CollaborationApprovalProjection,
  GitTurnDiffSummary,
  StorageConversationForkPoint
} from '@mycopilot/protocol'
import { ChatComposer } from './components/ChatComposer'
import { AgentApprovalDialog } from './components/AgentApprovalDialog'
import {
  ConversationApprovalQueue,
  type ConversationApprovalQueueItem
} from './components/ConversationApprovalQueue'
import { ProjectedApprovalDecisionCard } from '../agentCollaboration/ProjectedApprovalDecisionCard'
import type { CollaborationApprovalsController } from '../agentCollaboration/useCollaborationApprovals'
import type { ApprovalSubmissionResult } from './components/approvalSubmission'
import { AgentTodoProgress } from './components/AgentTodoProgress'
import { ChatMessageItem } from './components/ChatMessageItem'
import { ConversationScrollToBottomButton } from './components/ConversationScrollToBottomButton'
import { ConversationTurnNavigationRail } from './components/ConversationTurnNavigationRail'
import {
  ConversationModelTransitionDivider,
  ConversationManualCompactionDivider,
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
import type { WorkspaceReferenceTarget } from './workspaceMentions'
import type { ModelTransitionConfirmation } from './modelTransitionUiState'
import { getConversationTurnNavigationItems } from './conversationTurnNavigation'
import { isAssistantMessageGenerating, isAssistantReplyComplete } from './assistantGeneration'
import { getLatestAgentTodo } from './todoLifetime'
import { useConversationBottomFollow } from './useConversationBottomFollow'
import { useTurnDiffSummaries } from './useTurnDiffSummaries'
import { getAgentActionApprovalStatus, getAgentActionId } from '../agentRun/agentActionUtils'
import {
  CollaborationTimelineActivityList,
  copyCollaborationTimelineSelection,
  type CollaborationTimelineActivity
} from '../agentCollaboration/CollaborationTimelineActivity'
import { projectCollaborationTimelineActivities } from '../agentCollaboration/collaborationTimelineModel'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { formatTranslation } from '../../config/translationFormat'
import {
  useHumanInteraction,
  type HumanInteractionControllerView
} from '../humanInteraction/useHumanInteraction'
import { HumanInteractionPanel } from '../humanInteraction/HumanInteractionPanel'
import { HumanInteractionTimelineEntry } from '../humanInteraction/HumanInteractionTimelineEntry'
import {
  projectHumanInteractionConversation,
  getUnanchoredHumanInteractionRequests,
  humanInteractionUserDisplay
} from '../humanInteraction/humanInteractionPresentation'
import './ChatConversationPage.css'

interface ConversationSurfaceCommonProps {
  /** Authoritative direct children; an empty list hides stale or inherited collaboration rows. */
  directChildAgentIds?: readonly string[]
  conversation: ChatConversation
  initialScrollTop?: number | null
  onScrollPositionChange?: (conversationId: string, scrollTop: number) => void
  scrollTargetMessageId?: string | null
  scrollToBottomSignal?: number
  showTokenUsageDetails: boolean
}

export interface InteractiveConversationSurfaceProps extends ConversationSurfaceCommonProps {
  mode: 'interactive'
  forkDisabledReason?: string
  commands?: readonly ComposerCommand[]
  isManualCompactionRunning?: boolean
  /** Root-scoped child approvals retain their own authoritative decision route. */
  collaborationApprovals?: CollaborationApprovalsController
  collaborationAgentLabelsById?: Readonly<Record<string, string>>
  /** Typed durable semantic activity; generic Tool/Mailbox/model text never enters this path. */
  collaborationTimelineActivities?: readonly CollaborationTimelineActivity[]
  contextWindowIndicatorEnabled?: boolean
  contextWindowSnapshot?: AgentContextWindowSnapshot
  composerDraft: ChatComposerDraft
  editSelectedModelAvailable: boolean
  editSelectedModelSupportsImage: boolean
  modelTransitionConfirmation?: ModelTransitionConfirmation
  manualCompactionOperations?: AgentManualContextCompactionOperation[]
  modelTransitionOperations?: AgentProviderTransitionOperation[]
  onApproveAgentAction?: (
    messageId: string,
    action: AgentProposedAction,
    approvalScope: AgentApprovalScope
  ) => ApprovalSubmissionResult
  onCancelAgentAction?: (messageId: string, action: AgentProposedAction) => ApprovalSubmissionResult
  onComposerDraftChange: (draft: ChatComposerDraft) => void
  onComposerDraftMessageChange?: (draft: ChatComposerDraft) => void
  onContinueInNewTask?: (forkPoint: StorageConversationForkPoint) => void | Promise<void>
  onEditLastUserMessage?: (messageId: string, content: string) => void | Promise<void>
  onGuideQueuedMessage?: (message: ChatQueuedMessage) => void
  queueAutoSendEnabled?: boolean
  onToggleQueueAutoSend?: () => void
  onMessageUiStateChange?: (
    messageId: string,
    uiState: ChatConversation['messages'][number]['uiState']
  ) => void
  onModelTransitionCancel?: () => void
  onModelTransitionConfirm?: () => void | Promise<void>
  onModelTransitionRetry?: (operation: AgentProviderTransitionOperation) => void | Promise<void>
  onOpenCollaborationAgent?: (agentId: string) => void
  onOpenWorkspaceReference?: (target: WorkspaceReferenceTarget) => void
  onOpenContinuationOrigin?: (origin: ChatConversationContinuationOrigin) => void | Promise<void>
  onRejectAgentAction?: (
    messageId: string,
    action: AgentProposedAction,
    message?: string
  ) => ApprovalSubmissionResult
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
  /** Direct descendants' durable semantic events, scoped by the Agent Center tree. */
  collaborationTimelineActivities?: readonly CollaborationTimelineActivity[]
  onOpenCollaborationAgent?: (agentId: string) => void
  /** Root authority paired with the exact child conversation for read-only Artifact access. */
  rootConversationId: string
  /** Direct parent identity makes the transport-origin badge precise without changing chat role. */
  parentAgentId?: string | null
  /** Presentation-safe labels from the already-authorized current-tree snapshot. */
  agentLabelsById?: Readonly<Record<string, string>>
}

export type ConversationSurfaceProps =
  InteractiveConversationSurfaceProps | ObserverConversationSurfaceProps

const EMPTY_COLLABORATION_TIMELINE_ACTIVITIES: readonly CollaborationTimelineActivity[] = []
const EMPTY_MANUAL_COMPACTION_OPERATIONS: AgentManualContextCompactionOperation[] = []
const EMPTY_MODEL_TRANSITION_OPERATIONS: AgentProviderTransitionOperation[] = []

function useLatestCallback<Args extends unknown[], Result>(
  callback: ((...args: Args) => Result) | undefined
): (...args: Args) => Result {
  const callbackRef = useRef(callback)

  useLayoutEffect(() => {
    callbackRef.current = callback
  }, [callback])

  return useCallback((...args: Args) => {
    const currentCallback = callbackRef.current
    if (!currentCallback) {
      throw new Error('Attempted to invoke an unavailable conversation callback')
    }
    return currentCallback(...args)
  }, [])
}

function useStableMessageIdentities(messages: readonly ChatMessage[]): readonly { id: string }[] {
  const identitySnapshot = JSON.stringify(messages.map((message) => message.id))
  return useMemo(
    () => (JSON.parse(identitySnapshot) as string[]).map((id) => ({ id })),
    [identitySnapshot]
  )
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
        runId: run.runId,
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
  directChildAgentIds?: readonly string[]
  humanInteraction?: HumanInteractionControllerView
  forkDisabledReason?: string
  agentLabelsById?: Readonly<Record<string, string>>
  collaborationTimelineActivities?: readonly CollaborationTimelineActivity[]
  conversation: ChatConversation
  editSelectedModelAvailable: boolean
  editSelectedModelSupportsImage: boolean
  editableLastUserMessageId: string | null
  lastAssistantMessageId?: string
  mode?: 'interactive' | 'observer'
  manualCompactionOperations?: AgentManualContextCompactionOperation[]
  modelTransitionOperations?: AgentProviderTransitionOperation[]
  onApproveAgentAction?: (
    messageId: string,
    action: AgentProposedAction,
    approvalScope: AgentApprovalScope
  ) => ApprovalSubmissionResult
  onCancelAgentAction?: (messageId: string, action: AgentProposedAction) => ApprovalSubmissionResult
  onContinueInNewTask?: (forkPoint: StorageConversationForkPoint) => void | Promise<void>
  onEditLastUserMessage?: (messageId: string, content: string) => void | Promise<void>
  onOpenContinuationOrigin?: (origin: ChatConversationContinuationOrigin) => void | Promise<void>
  onMessageUiStateChange?: (messageId: string, uiState: ChatMessage['uiState']) => void
  onModelTransitionRetry?: (operation: AgentProviderTransitionOperation) => void | Promise<void>
  onOpenCollaborationAgent?: (agentId: string) => void
  onOpenWorkspaceReference?: (target: WorkspaceReferenceTarget) => void
  onRejectAgentAction?: (
    messageId: string,
    action: AgentProposedAction,
    message?: string
  ) => ApprovalSubmissionResult
  onReviewLastTurn?: (filePath?: string) => void
  parentAgentId?: string | null
  observerRootConversationId?: string
  showTokenUsageDetails: boolean
  turnDiffSummariesByMessageId?: ReadonlyMap<string, GitTurnDiffSummary>
}

export const ChatMessageList = memo(function ChatMessageList({
  humanInteraction,
  forkDisabledReason,
  agentLabelsById,
  collaborationTimelineActivities = EMPTY_COLLABORATION_TIMELINE_ACTIVITIES,
  conversation,
  directChildAgentIds,
  editSelectedModelAvailable,
  editSelectedModelSupportsImage,
  editableLastUserMessageId,
  lastAssistantMessageId,
  mode = 'interactive',
  manualCompactionOperations = EMPTY_MANUAL_COMPACTION_OPERATIONS,
  modelTransitionOperations = EMPTY_MODEL_TRANSITION_OPERATIONS,
  onApproveAgentAction,
  onCancelAgentAction,
  onContinueInNewTask,
  onEditLastUserMessage,
  onOpenContinuationOrigin,
  onMessageUiStateChange,
  onModelTransitionRetry,
  onOpenCollaborationAgent,
  onOpenWorkspaceReference,
  onRejectAgentAction,
  onReviewLastTurn,
  parentAgentId,
  observerRootConversationId,
  showTokenUsageDetails,
  turnDiffSummariesByMessageId
}: ChatMessageListProps) {
  const presentedConversation = useMemo(
    () => projectHumanInteractionConversation(conversation, humanInteraction?.requests ?? []),
    [conversation, humanInteraction?.requests]
  )
  const continuationOrigin = conversation.continuationOrigin
  const collaborationAgentNavigation = onOpenCollaborationAgent
  const messageIdentities = useStableMessageIdentities(conversation.messages)
  const messageIds = useMemo(
    () => new Set(messageIdentities.map((message) => message.id)),
    [messageIdentities]
  )
  const collaborationTimeline = useMemo(
    () =>
      projectCollaborationTimelineActivities(
        collaborationTimelineActivities,
        messageIdentities,
        conversation.id,
        directChildAgentIds
      ),
    [collaborationTimelineActivities, conversation.id, directChildAgentIds, messageIdentities]
  )
  const modelTransitions = useMemo(() => {
    const completedByMessageId = new Map<string, AgentProviderTransitionOperation[]>()
    const trailing: AgentProviderTransitionOperation[] = []

    for (const operation of modelTransitionOperations) {
      const coveredMessageId = operation.coveredThroughMessageId
      if (
        operation.status === 'completed' &&
        coveredMessageId &&
        messageIds.has(coveredMessageId)
      ) {
        const operations = completedByMessageId.get(coveredMessageId) ?? []
        operations.push(operation)
        completedByMessageId.set(coveredMessageId, operations)
      } else {
        trailing.push(operation)
      }
    }

    return { completedByMessageId, trailing }
  }, [messageIds, modelTransitionOperations])
  const approveAgentAction = useLatestCallback(onApproveAgentAction)
  const cancelAgentAction = useLatestCallback(onCancelAgentAction)
  const continueInNewTask = useLatestCallback(onContinueInNewTask)
  const editLastUserMessage = useLatestCallback(onEditLastUserMessage)
  const messageUiStateChange = useLatestCallback(onMessageUiStateChange)
  const modelTransitionRetry = useLatestCallback(onModelTransitionRetry)
  const openCollaborationAgent = useLatestCallback(onOpenCollaborationAgent)
  const rejectAgentAction = useLatestCallback(onRejectAgentAction)
  const reviewLastTurn = useLatestCallback(onReviewLastTurn)
  const [observerTimelineCollapsed, setObserverTimelineCollapsed] = useState<
    Readonly<Record<string, boolean>>
  >({})
  const handleContinueAssistantReply = useCallback(
    (messageId: string) =>
      continueInNewTask({
        kind: 'assistant_reply',
        assistantMessageId: messageId
      }),
    [continueInNewTask]
  )
  const handleObserverTimelineCollapsedChange = useCallback(
    (messageId: string, collapsed: boolean) =>
      setObserverTimelineCollapsed((current) => ({
        ...current,
        [messageId]: collapsed
      })),
    []
  )

  useEffect(() => {
    setObserverTimelineCollapsed({})
  }, [conversation.id])

  return (
    <>
      {presentedConversation.messages.map((message) => (
        <Fragment key={message.id}>
          {collaborationAgentNavigation && (
            <CollaborationTimelineActivityList
              activities={
                collaborationTimeline.beforeMessage.get(message.id) ??
                EMPTY_COLLABORATION_TIMELINE_ACTIVITIES
              }
              onOpenAgent={openCollaborationAgent}
            />
          )}
          <ChatMessageItem
            humanInteraction={mode === 'interactive' ? humanInteraction : undefined}
            agentLabelsById={agentLabelsById}
            collaborationTimelineActivities={
              collaborationAgentNavigation
                ? (collaborationTimeline.anchoredMessage.get(message.id) ??
                  EMPTY_COLLABORATION_TIMELINE_ACTIVITIES)
                : EMPTY_COLLABORATION_TIMELINE_ACTIVITIES
            }
            conversationId={conversation.id}
            directChildAgentIds={directChildAgentIds}
            isLastAssistantMessage={message.id === lastAssistantMessageId}
            message={message}
            mode={mode}
            onApprove={
              mode === 'interactive' && onApproveAgentAction ? approveAgentAction : undefined
            }
            onCancel={mode === 'interactive' && onCancelAgentAction ? cancelAgentAction : undefined}
            editSelectedModelAvailable={editSelectedModelAvailable}
            editSelectedModelSupportsImage={editSelectedModelSupportsImage}
            onEditSubmit={
              mode === 'interactive' &&
              message.id === editableLastUserMessageId &&
              onEditLastUserMessage
                ? editLastUserMessage
                : undefined
            }
            onContinueInNewTask={
              mode === 'interactive' &&
              !forkDisabledReason &&
              isAssistantReplyComplete(message) &&
              onContinueInNewTask
                ? handleContinueAssistantReply
                : undefined
            }
            onOpenCollaborationAgent={
              collaborationAgentNavigation ? openCollaborationAgent : undefined
            }
            onOpenWorkspaceReference={onOpenWorkspaceReference}
            onReject={mode === 'interactive' && onRejectAgentAction ? rejectAgentAction : undefined}
            onReviewLastTurn={
              mode === 'interactive' && onReviewLastTurn ? reviewLastTurn : undefined
            }
            onTimelineCollapsedChange={
              mode === 'observer' ? handleObserverTimelineCollapsedChange : undefined
            }
            onUiStateChange={
              mode === 'interactive' && onMessageUiStateChange ? messageUiStateChange : undefined
            }
            parentAgentId={parentAgentId}
            observerRootConversationId={observerRootConversationId}
            projectId={conversation.projectId}
            showTokenUsageDetails={showTokenUsageDetails}
            timelineCollapsedOverride={
              mode === 'observer' ? observerTimelineCollapsed[message.id] : undefined
            }
            turnDiffSummary={turnDiffSummariesByMessageId?.get(message.id)}
          />
          {collaborationAgentNavigation && (
            <CollaborationTimelineActivityList
              activities={
                collaborationTimeline.afterMessage.get(message.id) ??
                EMPTY_COLLABORATION_TIMELINE_ACTIVITIES
              }
              onOpenAgent={openCollaborationAgent}
            />
          )}
          {continuationOrigin?.boundaryMessageId === message.id && (
            <ConversationContinuationDivider
              onOpen={
                mode === 'interactive' && onOpenContinuationOrigin
                  ? () => onOpenContinuationOrigin(continuationOrigin)
                  : undefined
              }
            />
          )}
          {[
            ...manualCompactionOperations
              .filter((operation) => operation.coveredThroughMessageId === message.id)
              .map((operation) => ({
                at: operation.startedAt,
                id: operation.operationId,
                element: (
                  <ConversationManualCompactionDivider
                    operation={operation}
                    forkDisabledReason={forkDisabledReason}
                    onContinueInNewTask={
                      mode === 'interactive' && onContinueInNewTask
                        ? () =>
                            continueInNewTask({
                              kind: 'manual_compaction_boundary',
                              operationId: operation.operationId
                            })
                        : undefined
                    }
                  />
                )
              })),
            ...(modelTransitions.completedByMessageId.get(message.id) ?? []).map((operation) => ({
              at: operation.startedAt,
              id: operation.operationId,
              element: (
                <ConversationModelTransitionDivider
                  forkDisabledReason={forkDisabledReason}
                  operation={operation}
                  onContinueInNewTask={
                    mode === 'interactive' &&
                    operation.status === 'completed' &&
                    operation.summaryId &&
                    onContinueInNewTask
                      ? () =>
                          continueInNewTask({
                            kind: 'provider_transition_boundary',
                            operationId: operation.operationId
                          })
                      : undefined
                  }
                />
              )
            }))
          ]
            .sort((a, b) => a.at - b.at || a.id.localeCompare(b.id))
            .map(({ id, element }) => (
              <Fragment key={id}>{element}</Fragment>
            ))}
        </Fragment>
      ))}
      {mode === 'interactive' &&
        humanInteraction &&
        getUnanchoredHumanInteractionRequests(conversation, humanInteraction.openRequests).map(
          (request) => (
            <HumanInteractionTimelineEntry
              key={request.requestId}
              request={request}
              interaction={humanInteraction}
            />
          )
        )}
      {manualCompactionOperations
        .filter(
          (operation) =>
            !operation.coveredThroughMessageId || !messageIds.has(operation.coveredThroughMessageId)
        )
        .map((operation) => (
          <ConversationManualCompactionDivider
            key={operation.operationId}
            operation={operation}
            forkDisabledReason={forkDisabledReason}
            onContinueInNewTask={
              mode === 'interactive' && onContinueInNewTask
                ? () =>
                    continueInNewTask({
                      kind: 'manual_compaction_boundary',
                      operationId: operation.operationId
                    })
                : undefined
            }
          />
        ))}
      {modelTransitions.trailing.map((operation) => (
        <ConversationModelTransitionDivider
          key={operation.operationId}
          forkDisabledReason={forkDisabledReason}
          onContinueInNewTask={
            mode === 'interactive' &&
            operation.status === 'completed' &&
            operation.summaryId &&
            onContinueInNewTask
              ? () =>
                  continueInNewTask({
                    kind: 'provider_transition_boundary',
                    operationId: operation.operationId
                  })
              : undefined
          }
          onRetry={
            mode === 'interactive' && operation.status === 'failed' && onModelTransitionRetry
              ? () => modelTransitionRetry(operation)
              : undefined
          }
          operation={operation}
        />
      ))}
      {collaborationAgentNavigation && (
        <CollaborationTimelineActivityList
          activities={collaborationTimeline.tail}
          onOpenAgent={openCollaborationAgent}
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
  const pendingApprovalTarget = useMemo(
    () => (interactive ? getPendingApprovalTarget(conversation) : null),
    [conversation, interactive]
  )
  const childApprovals = useMemo(() => {
    const byId = new Map<string, CollaborationApprovalProjection>()
    for (const approval of interactive?.collaborationApprovals?.approvals ?? []) {
      if (approval.rootConversationId !== conversation.id) continue
      const previous = byId.get(approval.approvalId)
      if (!previous || approval.updatedAt >= previous.updatedAt) {
        byId.set(approval.approvalId, approval)
      }
    }
    return [...byId.values()]
      .filter((approval) => approval.status === 'pending')
      .sort(
        (left, right) =>
          right.createdAt - left.createdAt || left.approvalId.localeCompare(right.approvalId)
      )
  }, [conversation.id, interactive?.collaborationApprovals?.approvals])
  const hasPendingApproval = Boolean(pendingApprovalTarget || childApprovals.length)
  const approvalItems: ConversationApprovalQueueItem[] = []
  if (interactive && pendingApprovalTarget) {
    approvalItems.push({
      id: JSON.stringify([
        'root',
        pendingApprovalTarget.runId,
        pendingApprovalTarget.messageId,
        getAgentActionId(pendingApprovalTarget.action)
      ]),
      label: t('agent.approval.rootAgent'),
      content: (
        <AgentApprovalDialog
          mcpInvocationState={pendingApprovalTarget.mcpInvocationState}
          target={pendingApprovalTarget}
          onApprove={interactive.onApproveAgentAction}
          onCancel={interactive.onCancelAgentAction}
          onReject={interactive.onRejectAgentAction}
        />
      )
    })
  }
  if (interactive?.collaborationApprovals) {
    for (const approval of childApprovals) {
      approvalItems.push({
        id: JSON.stringify(['child', approval.rootConversationId, approval.approvalId]),
        sourceAgentId: approval.sourceAgentId,
        label:
          interactive.collaborationAgentLabelsById?.[approval.sourceAgentId] ??
          approval.sourceTaskPath,
        content: (
          <ProjectedApprovalDecisionCard
            approval={approval}
            onDecision={interactive.collaborationApprovals.decide}
          />
        )
      })
    }
  }
  const humanInteraction = useHumanInteraction({
    conversationId: interactive ? conversation.id : null,
    hasApproval: hasPendingApproval,
    readOnly: !interactive
  })
  const messageSummary = useMemo(() => {
    let isGenerating = false
    let lastAssistantMessageId: string | undefined
    let lastCommittedUserMessageId: string | undefined
    let activeAssistantRun: ChatMessage['agentRun']

    for (let index = conversation.messages.length - 1; index >= 0; index -= 1) {
      const message = conversation.messages[index]
      if (!lastAssistantMessageId && message.role === 'assistant') {
        lastAssistantMessageId = message.id
      }
      if (
        !lastCommittedUserMessageId &&
        message.role === 'user' &&
        !humanInteractionUserDisplay(message, humanInteraction.requests)
      ) {
        lastCommittedUserMessageId = message.id
      }
      if (isAssistantMessageGenerating(message)) {
        isGenerating = true
        activeAssistantRun ??= message.agentRun
      }
    }

    return {
      activeAssistantRun,
      isGenerating,
      lastAssistantMessageId,
      lastCommittedUserMessageId
    }
  }, [conversation.messages, humanInteraction.requests])
  const { activeAssistantRun, isGenerating, lastAssistantMessageId, lastCommittedUserMessageId } =
    messageSummary
  const canGuideQueuedMessages = Boolean(
    activeAssistantRun?.runId && activeAssistantRun.status === 'running'
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
  const activeQuestion = humanInteraction.activeBatch
  const isComposerSuspended = Boolean(activeQuestion || hasPendingApproval)
  const questionSourceRun = activeQuestion
    ? conversation.messages.find((message) => message.agentRun?.runId === activeQuestion.runId)
        ?.agentRun
    : undefined
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

  const { isAtBottom, scrollToBottom } = useConversationBottomFollow(
    messagesRef,
    conversation.id,
    interactive !== null
  )

  useEffect(() => rememberCurrentScrollPosition, [rememberCurrentScrollPosition])

  return (
    <section
      className="chat-conversation-page conversation-surface"
      aria-label={conversation.title}
      data-approval-pending={hasPendingApproval ? 'true' : undefined}
      data-interaction-pending={activeQuestion ? 'true' : undefined}
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
            humanInteraction={interactive ? humanInteraction : undefined}
            directChildAgentIds={props.directChildAgentIds}
            agentLabelsById={props.mode === 'observer' ? props.agentLabelsById : undefined}
            collaborationTimelineActivities={
              interactive?.collaborationTimelineActivities ??
              (props.mode === 'observer' ? props.collaborationTimelineActivities : undefined)
            }
            conversation={conversation}
            editSelectedModelAvailable={interactive?.editSelectedModelAvailable ?? false}
            editSelectedModelSupportsImage={interactive?.editSelectedModelSupportsImage ?? false}
            editableLastUserMessageId={editableLastUserMessageId}
            lastAssistantMessageId={lastAssistantMessageId}
            mode={props.mode}
            manualCompactionOperations={interactive?.manualCompactionOperations}
            modelTransitionOperations={interactive?.modelTransitionOperations}
            onApproveAgentAction={interactive?.onApproveAgentAction}
            onCancelAgentAction={interactive?.onCancelAgentAction}
            onContinueInNewTask={interactive?.onContinueInNewTask}
            forkDisabledReason={
              interactive?.forkDisabledReason ??
              (interactive?.isManualCompactionRunning
                ? t('chat.commands.compacting')
                : isGenerating ||
                    hasPendingApproval ||
                    interactive?.modelTransitionConfirmation ||
                    interactive?.modelTransitionOperations?.some(
                      (operation) => operation.status === 'running'
                    )
                  ? t('chat.commands.idleOnly')
                  : undefined)
            }
            onEditLastUserMessage={
              interactive?.isManualCompactionRunning
                ? undefined
                : interactive?.onEditLastUserMessage
            }
            onOpenContinuationOrigin={interactive?.onOpenContinuationOrigin}
            onMessageUiStateChange={interactive?.onMessageUiStateChange}
            onModelTransitionRetry={interactive?.onModelTransitionRetry}
            onOpenCollaborationAgent={
              interactive?.onOpenCollaborationAgent ??
              (props.mode === 'observer' ? props.onOpenCollaborationAgent : undefined)
            }
            onOpenWorkspaceReference={interactive?.onOpenWorkspaceReference}
            onRejectAgentAction={interactive?.onRejectAgentAction}
            onReviewLastTurn={interactive?.onReviewLastTurn}
            parentAgentId={props.mode === 'observer' ? props.parentAgentId : undefined}
            observerRootConversationId={
              props.mode === 'observer' ? props.rootConversationId : undefined
            }
            showTokenUsageDetails={showTokenUsageDetails}
            turnDiffSummariesByMessageId={turnDiffSummariesByMessageId}
          />
        </div>
        <ConversationTurnNavigationRail
          items={turnNavigationItems}
          scrollContainerRef={messagesRef}
        />
        {interactive && (
          <ConversationScrollToBottomButton
            generating={isGenerating}
            onClick={scrollToBottom}
            visible={!isAtBottom}
          />
        )}
      </div>

      {interactive && (
        <div className="chat-conversation-page__composer">
          {activeQuestion && (
            <HumanInteractionPanel
              key={activeQuestion.requestId}
              request={activeQuestion}
              pageIndex={humanInteraction.activeDraft.pageIndex}
              answers={humanInteraction.activeDraft.answers}
              canSubmit={humanInteraction.canSubmit}
              isSubmitting={humanInteraction.isSubmitting}
              isDraftLocked={humanInteraction.isDraftLocked}
              error={humanInteraction.error}
              sourceRunEnded={
                activeQuestion.mode === 'async' &&
                Boolean(
                  questionSourceRun &&
                  ['completed', 'failed', 'cancelled'].includes(questionSourceRun.status)
                )
              }
              onPageChange={(index) => humanInteraction.setPage(activeQuestion.requestId, index)}
              onAnswerChange={(answer) =>
                humanInteraction.setAnswer(activeQuestion.requestId, answer)
              }
              onSubmit={() => void humanInteraction.submit(activeQuestion.requestId)}
              onIgnore={
                activeQuestion.mode === 'async' && humanInteraction.pendingAction !== 'submit'
                  ? () => void humanInteraction.ignore(activeQuestion.requestId)
                  : undefined
              }
              onMinimize={
                activeQuestion.mode === 'async'
                  ? () => humanInteraction.minimize(activeQuestion.requestId)
                  : undefined
              }
            />
          )}
          {!activeQuestion && humanInteraction.error && !hasPendingApproval && (
            <div role="alert">
              {humanInteraction.error}{' '}
              <button type="button" onClick={() => void humanInteraction.refresh()}>
                {t('chat.retryConversationLoad')}
              </button>
            </div>
          )}
          {activeTodo && !activeQuestion && (
            <AgentTodoProgress
              completedAt={activeTodo.completedAt}
              runStatus={activeTodo.runStatus}
              todo={activeTodo.todo}
            />
          )}
          <ConversationApprovalQueue
            key={conversation.id}
            items={approvalItems}
            onOpenAgent={interactive.onOpenCollaborationAgent}
          />
          {interactive.collaborationApprovals?.error && (
            <div className="conversation-approval-load-error" role="alert">
              <span>
                {formatTranslation(t, 'collaboration.approval.loadFailed', {
                  error: interactive.collaborationApprovals.error
                })}
              </span>
              <button
                type="button"
                onClick={() => void interactive.collaborationApprovals?.refresh()}
              >
                {t('collaboration.approval.retry')}
              </button>
            </div>
          )}
          {/* Keep the Composer mounted: its fast-path text draft is newer than the shell prop. */}
          <div
            className="conversation-composer-slot"
            hidden={isComposerSuspended}
            inert={isComposerSuspended}
          >
            <ChatComposer
              isSuspended={isComposerSuspended}
              commands={interactive.commands}
              isManualCompactionRunning={interactive.isManualCompactionRunning}
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
              onOpenWorkspaceReference={interactive.onOpenWorkspaceReference}
              queueAutoSendEnabled={interactive.queueAutoSendEnabled}
              onToggleQueueAutoSend={interactive.onToggleQueueAutoSend}
              onStopGenerating={interactive.onStopGenerating}
              onSubmitMessage={interactive.onSubmitMessage}
              permissionModeAvailability={interactive.permissionModeAvailability}
              resetKey={conversation.id}
              skillCatalogRefreshToken={interactive.skillCatalogRefreshToken}
            />
          </div>
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
    </section>
  )
}
