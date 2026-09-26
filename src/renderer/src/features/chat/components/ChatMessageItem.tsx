import { memo, useEffect, useState } from 'react'
import type {
  AgentApprovalScope,
  AgentProposedAction,
  GitTurnDiffSummary
} from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { ChatMessage } from '../chatTypes'
import { ChatMarkdown } from './ChatMarkdown'
import { ComposerFolderReferences } from './ComposerFolderReferences'
import { HumanInteractionAnswerContent } from '../../humanInteraction/HumanInteractionAnswerContent'
import { type HumanInteractionTimelineController } from '../../humanInteraction/HumanInteractionTimelineEntry'
import { humanInteractionDisplayText } from '../../humanInteraction/humanInteractionPresentation'
import type { ApprovalSubmissionResult } from './approvalSubmission'
import {
  getAssistantFinalContent,
  getUserVisibleContent,
  shouldShowAssistantActions
} from './chatMessageItemUtils'
import { type CollaborationTimelineActivity } from '../../agentCollaboration/CollaborationTimelineActivity'
import type { WorkspaceReferenceTarget } from '../workspaceMentions'
import { ChatMessageActions } from './ChatMessageActions'
import { EditableUserMessage } from './EditableUserMessage'
import { MessageAttachments } from './MessageAttachments'
import { AgentRunView } from './AgentRunView'
import {
  WorkflowReceivedContent,
  workflowReceivedBodies,
  workflowReceivedCopyText
} from './WorkflowReceivedContent'

interface ChatMessageItemProps {
  agentLabelsById?: Readonly<Record<string, string>>
  collaborationTimelineActivities?: readonly CollaborationTimelineActivity[]
  collaborationTreeAgentIds?: readonly string[]
  conversationId?: string
  humanInteraction?: HumanInteractionTimelineController
  editSelectedModelAvailable?: boolean
  editSelectedModelSupportsImage?: boolean
  isLastAssistantMessage?: boolean
  message: ChatMessage
  mode?: 'interactive' | 'observer'
  parentAgentId?: string | null
  projectId?: string | null
  onApprove?: (
    messageId: string,
    action: AgentProposedAction,
    approvalScope: AgentApprovalScope
  ) => ApprovalSubmissionResult
  onCancel?: (messageId: string, action: AgentProposedAction) => ApprovalSubmissionResult
  onEditSubmit?: (messageId: string, content: string) => void | Promise<void>
  onContinueInNewTask?: (messageId: string) => void | Promise<void>
  onOpenCollaborationAgent?: (agentId: string) => void
  onOpenWorkspaceReference?: (target: WorkspaceReferenceTarget) => void
  onReject?: (
    messageId: string,
    action: AgentProposedAction,
    message?: string
  ) => ApprovalSubmissionResult
  onReviewLastTurn?: (filePath?: string) => void
  onTimelineCollapsedChange?: (messageId: string, collapsed: boolean) => void
  onUiStateChange?: (messageId: string, uiState: ChatMessage['uiState']) => void
  observerRootConversationId?: string
  showTokenUsageDetails: boolean
  timelineCollapsedOverride?: boolean
  turnDiffSummary?: GitTurnDiffSummary
}

function MessageContent({
  collaborationTimelineActivities,
  collaborationTreeAgentIds,
  conversationId,
  humanInteraction,
  message,
  mode = 'interactive',
  onReviewLastTurn,
  onTimelineCollapsedChange,
  onUiStateChange,
  onOpenCollaborationAgent,
  onOpenWorkspaceReference,
  observerRootConversationId,
  projectId,
  timelineCollapsedOverride,
  turnDiffSummary
}: ChatMessageItemProps) {
  if (message.role === 'assistant') {
    return (
      <AgentRunView
        collaborationTimelineActivities={collaborationTimelineActivities}
        collaborationTreeAgentIds={collaborationTreeAgentIds}
        conversationId={conversationId}
        humanInteraction={humanInteraction}
        message={message}
        mode={mode}
        onReviewLastTurn={onReviewLastTurn}
        onTimelineCollapsedChange={onTimelineCollapsedChange}
        onUiStateChange={onUiStateChange}
        onOpenCollaborationAgent={onOpenCollaborationAgent}
        onOpenWorkspaceReference={onOpenWorkspaceReference}
        observerRootConversationId={observerRootConversationId}
        projectId={projectId}
        timelineCollapsedOverride={timelineCollapsedOverride}
        turnDiffSummary={turnDiffSummary}
      />
    )
  }

  const humanAnswer = message.humanInteractionDisplay
  return humanAnswer ? (
    <HumanInteractionAnswerContent display={humanAnswer} />
  ) : (
    <ChatMarkdown
      enableMath={false}
      content={getUserVisibleContent(message)}
      onOpenWorkspaceReference={onOpenWorkspaceReference}
      projectId={projectId}
    />
  )
}

function MessageInputOrigin({
  agentLabelsById,
  message,
  parentAgentId
}: {
  agentLabelsById?: Readonly<Record<string, string>>
  message: ChatMessage
  parentAgentId?: string | null
}) {
  const { t, language } = useFrontendConfig()
  if (message.role !== 'user') return null
  if (message.workflowSource) {
    const source = message.workflowSource
    return (
      <div
        className="chat-message__input-origin"
        data-input-origin="workflow"
        title={source.sources
          .map((item) => `${item.nodeName} · ${item.conversationTitle}`)
          .join('\n')}
      >
        <span>{language.startsWith('zh') ? '来自工作流' : 'From workflow'}</span>
        <strong>{source.workflowName}</strong>
        <span>· {source.sources.map((item) => item.nodeName).join('、')}</span>
      </div>
    )
  }
  if (!message.inputOrigin) return null
  const origin = message.inputOrigin
  if (origin.kind === 'human') return null

  const senderAgentId = origin.senderAgentId
  const senderLabel = senderAgentId ? agentLabelsById?.[senderAgentId] : undefined
  const relationLabel = senderAgentId
    ? senderAgentId === parentAgentId
      ? t('chat.fromParentAgent')
      : t('chat.fromCollaboratingAgent')
    : t('chat.historicalContext')

  return (
    <div
      className="chat-message__input-origin"
      data-input-origin={origin.kind}
      data-sender-agent-id={senderAgentId ?? undefined}
    >
      <span>{relationLabel}</span>
      {senderLabel ? <strong>{senderLabel}</strong> : null}
      {origin.kind === 'historical_snapshot' && senderAgentId ? (
        <span>{t('chat.historicalContext')}</span>
      ) : null}
    </div>
  )
}

export const ChatMessageItem = memo(function ChatMessageItem({
  agentLabelsById,
  collaborationTimelineActivities,
  collaborationTreeAgentIds,
  conversationId,
  humanInteraction,
  editSelectedModelAvailable = true,
  editSelectedModelSupportsImage = true,
  isLastAssistantMessage = false,
  message,
  mode = 'interactive',
  onApprove,
  onCancel,
  onEditSubmit,
  onContinueInNewTask,
  onOpenCollaborationAgent,
  onOpenWorkspaceReference,
  onReject,
  onReviewLastTurn,
  onTimelineCollapsedChange,
  onUiStateChange,
  observerRootConversationId,
  parentAgentId,
  projectId,
  showTokenUsageDetails,
  timelineCollapsedOverride,
  turnDiffSummary
}: ChatMessageItemProps) {
  const { t } = useFrontendConfig()
  const [isEditing, setIsEditing] = useState(false)
  const [expandedWorkflowMessageId, setExpandedWorkflowMessageId] = useState<string | null>(null)
  const workflowBodies = workflowReceivedBodies(message)
  const isWorkflowExpanded = expandedWorkflowMessageId === message.id
  const isWorkflowCollapsed = Boolean(workflowBodies && !isWorkflowExpanded)
  const isAssistantActionsVisible = shouldShowAssistantActions(message)
  const userVisibleContent = getUserVisibleContent(message)
  const humanAnswer = message.role === 'user' ? message.humanInteractionDisplay : null
  const actionContent =
    message.role === 'assistant'
      ? getAssistantFinalContent(message)
      : humanAnswer
        ? humanInteractionDisplayText(humanAnswer)
        : isWorkflowCollapsed && workflowBodies
          ? workflowReceivedCopyText(workflowBodies)
          : userVisibleContent
  const actionTimestamp =
    message.role === 'assistant'
      ? (message.agentRun?.completedAt ?? message.createdAt)
      : message.createdAt
  const actionUsage = message.role === 'assistant' ? message.agentRun?.usage : undefined
  const showActions = message.role === 'user' || isAssistantActionsVisible
  const canEdit =
    message.role === 'user' && !humanAnswer && !message.workflowSource && Boolean(onEditSubmit)
  const pinCopyAction =
    message.role === 'assistant' && isLastAssistantMessage && isAssistantActionsVisible
  const showBody = message.role === 'assistant' || Boolean(userVisibleContent.trim())
  const isFavorited = message.role === 'user' && message.uiState?.favorited === true

  const updateFavorite = (favorited: boolean) => {
    if (message.role !== 'user' || !onUiStateChange) return

    const nextUiState = { ...message.uiState }
    if (favorited) {
      nextUiState.favorited = true
    } else {
      delete nextUiState.favorited
    }
    onUiStateChange(message.id, Object.keys(nextUiState).length > 0 ? nextUiState : undefined)
  }

  useEffect(() => {
    setIsEditing(false)
  }, [canEdit, message.id])

  return (
    <article
      className={`chat-message chat-message--${message.role}`}
      data-copy-pinned={pinCopyAction ? 'true' : undefined}
      data-editing={isEditing ? 'true' : undefined}
      data-message-id={message.id}
      data-status={message.status}
      key={message.id}
    >
      <MessageInputOrigin
        agentLabelsById={agentLabelsById}
        message={message}
        parentAgentId={parentAgentId}
      />
      {message.role === 'user' && (
        <>
          <ComposerFolderReferences
            folders={message.folderReferences ?? []}
            label={t('chat.attachments')}
            removeLabel=""
          />
          <MessageAttachments
            attachments={message.attachments}
            messageId={message.id}
            mode={mode}
          />
        </>
      )}
      {isEditing ? (
        <div className="chat-message__body">
          <EditableUserMessage
            hasAttachments={Boolean(message.attachments?.length)}
            hasImageAttachments={Boolean(
              message.attachments?.some((attachment) => attachment.kind === 'image')
            )}
            initialContent={userVisibleContent}
            onCancel={() => setIsEditing(false)}
            onSubmit={(content) => onEditSubmit?.(message.id, content)}
            selectedModelAvailable={editSelectedModelAvailable}
            selectedModelSupportsImage={editSelectedModelSupportsImage}
          />
        </div>
      ) : showBody ? (
        <div className="chat-message__body">
          {isWorkflowCollapsed && workflowBodies ? (
            <WorkflowReceivedContent
              bodies={workflowBodies}
              onOpenWorkspaceReference={onOpenWorkspaceReference}
              projectId={projectId}
            />
          ) : (
            <MessageContent
              collaborationTimelineActivities={collaborationTimelineActivities}
              collaborationTreeAgentIds={collaborationTreeAgentIds}
              conversationId={conversationId}
              humanInteraction={humanInteraction}
              message={message}
              mode={mode}
              onApprove={onApprove}
              onCancel={onCancel}
              onReject={onReject}
              onReviewLastTurn={onReviewLastTurn}
              onTimelineCollapsedChange={onTimelineCollapsedChange}
              onUiStateChange={onUiStateChange}
              onOpenCollaborationAgent={onOpenCollaborationAgent}
              onOpenWorkspaceReference={onOpenWorkspaceReference}
              observerRootConversationId={observerRootConversationId}
              projectId={projectId}
              showTokenUsageDetails={showTokenUsageDetails}
              timelineCollapsedOverride={timelineCollapsedOverride}
              turnDiffSummary={turnDiffSummary}
            />
          )}
        </div>
      ) : null}
      {showActions && !isEditing && (
        <ChatMessageActions
          canEdit={canEdit}
          content={actionContent}
          favorited={isFavorited}
          workflowContextExpanded={isWorkflowExpanded}
          onWorkflowContextToggle={
            workflowBodies
              ? () => setExpandedWorkflowMessageId(isWorkflowExpanded ? null : message.id)
              : undefined
          }
          onEdit={() => setIsEditing(true)}
          onFavoriteChange={message.role === 'user' && onUiStateChange ? updateFavorite : undefined}
          onContinueInNewTask={
            message.role === 'assistant' && onContinueInNewTask
              ? () => onContinueInNewTask(message.id)
              : undefined
          }
          showTokenUsageDetails={showTokenUsageDetails}
          timestamp={actionTimestamp}
          usage={actionUsage}
        />
      )}
    </article>
  )
})
