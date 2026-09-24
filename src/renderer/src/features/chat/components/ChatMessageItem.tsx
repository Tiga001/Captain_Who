import { memo, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react'
import {
  AlertTriangle,
  ChevronDown,
  Check,
  Copy,
  Database,
  LoaderCircle,
  Pencil,
  Split,
  Star,
  WifiOff
} from 'lucide-react'
import type {
  AgentApprovalScope,
  AgentInputAttachment,
  AgentProposedAction,
  AgentUsage,
  GitTurnDiffSummary
} from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { getUserFacingErrorMessage } from '../../../errors/userFacingError'
import { formatTranslation } from '../../../config/translationFormat'
import type { ChatAgentRunView, ChatAgentTimelineItem, ChatMessage } from '../chatTypes'
import type { ChatGuidanceTimelineItem } from '../chatTypes'
import { getUniqueWebSearchSources } from '../agentWebSearch'
import { getFinalTimeline } from '../../agentRun/messageTimeline'
import { loadComposerAttachmentImage, stripAttachmentSummary } from '../chatAttachments'
import { useComposerAttachmentPreviews } from '../useAttachmentImports'
import { getAttachmentPreviewUrl } from '../attachmentDisplay'
import { loadAttachmentImage } from '../../storage/storageClient'
import * as storageClient from '../../storage/storageClient'
import { ChatMarkdown } from './ChatMarkdown'
import { ComposerAttachments, type ComposerAttachmentPresentation } from './ComposerAttachments'
import { ComposerFolderReferences } from './ComposerFolderReferences'
import { HumanInteractionAnswerContent } from '../../humanInteraction/HumanInteractionAnswerContent'
import {
  HumanInteractionTimelineEntry,
  type HumanInteractionTimelineController
} from '../../humanInteraction/HumanInteractionTimelineEntry'
import {
  humanInteractionRequestsForMessage,
  readHumanInteractionGuidanceDisplay,
  humanInteractionDisplayText
} from '../../humanInteraction/humanInteractionPresentation'
import type { ApprovalSubmissionResult } from './approvalSubmission'
import {
  copyTextToClipboard,
  formatElapsedDuration,
  formatMessageTime,
  getAssistantFinalContent,
  getConversationHistoryGroupItems,
  getFileChangeGroupItems,
  getMcpActivityGroupItems,
  getOfficeGroupItems,
  getPreviousSuccessfulTodoResult,
  getReadGroupItems,
  getRunCommandGroupItems,
  getSearchGroupItems,
  getSendMessageGroupItems,
  getSkillResourceGroupItems,
  getSettledToolStatus,
  getToolResult,
  getUsageRows,
  getUserVisibleContent,
  getWebActivityGroupItems,
  groupTimelineItems,
  hasCollapsibleTimelineContent,
  hasDisplayableContent,
  hasRecentFileChangeActivity,
  hasTrustedAnchoredCollaborationActivity,
  isContentFullyRepresentedByTimeline,
  isRunSettled,
  isTokenLimitFinishReason,
  isWaitingForCommandCompletion,
  shouldShowAssistantActions,
  shouldShowThinkingActivity,
  type RenderableTimelineItem
} from './chatMessageItemUtils'
import { EditSummaryCard } from './EditSummaryCard'
import { OfficeArtifactsCard } from './OfficeArtifactsCard'
import { ImageGenerationArtifactsCard } from './ImageGenerationArtifactsCard'
import { hostImageArtifactResolver } from '../../imageGeneration/artifacts/hostImageArtifactResolver'
import { useImagePreview, useImagePreviewNotice } from './ImagePreview'
import { AgentToolActivity } from './toolActivities/AgentToolActivity'
import { McpToolActivity, McpToolActivityGroup } from './toolActivities/McpToolActivity'
import { ContextCompactionActivity } from './toolActivities/ContextCompactionActivity'
import { ConversationHistoryToolActivity } from './toolActivities/ConversationHistoryToolActivity'
import { FileChangeToolActivityGroup } from './toolActivities/FileChangeToolActivity'
import { ReadToolActivityGroup } from './toolActivities/ReadToolActivity'
import { RunCommandToolActivityGroup } from './toolActivities/RunCommandToolActivity'
import { SendMessageToolActivityGroup } from './toolActivities/SendMessageToolActivity'
import { OfficeToolActivityGroup } from './toolActivities/OfficeToolActivity'
import { SearchToolActivityGroup } from './toolActivities/SearchToolActivity'
import { WebSearchToolActivityGroup } from './toolActivities/WebSearchToolActivity'
import { AssistantSources } from './toolActivities/WebSearchSources'
import { SkillLoadActivity, SkillResourceActivityGroup } from './toolActivities/SkillToolActivity'
import {
  CollaborationTimelineActivityList,
  normalizeCollaborationTimelineActivities,
  type CollaborationTimelineActivity
} from '../../agentCollaboration/CollaborationTimelineActivity'
import type { WorkspaceReferenceTarget } from '../workspaceMentions'

const ACTIVE_STREAMING_GRACE_MS = 1200
const COPIED_INDICATOR_MS = 1300

function interruptionTranslationKey(
  reason: NonNullable<ChatAgentRunView['interruption']>['reason']
) {
  switch (reason) {
    case 'service_connection_failed':
      return 'agent.interruption.serviceConnectionFailed' as const
    case 'service_unavailable':
      return 'agent.interruption.serviceUnavailable' as const
    case 'authentication_failed':
      return 'agent.interruption.authenticationFailed' as const
    case 'quota_exhausted':
      return 'agent.interruption.quotaExhausted' as const
    case 'context_limit_exceeded':
      return 'agent.interruption.contextLimitExceeded' as const
    case 'request_rejected':
      return 'agent.interruption.requestRejected' as const
    case 'response_invalid':
      return 'agent.interruption.responseInvalid' as const
    case 'output_limit_reached':
      return 'agent.interruption.outputLimitReached' as const
    case 'empty_response':
      return 'agent.interruption.emptyResponse' as const
    case 'stream_interrupted':
      return 'agent.interruption.streamInterrupted' as const
    case 'admission_unconfirmed':
      return 'agent.interruption.admissionUnconfirmed' as const
    case 'request_failed':
      return 'agent.interruption.requestFailed' as const
  }
}

interface ChatMessageItemProps {
  agentLabelsById?: Readonly<Record<string, string>>
  collaborationTimelineActivities?: readonly CollaborationTimelineActivity[]
  directChildAgentIds?: readonly string[]
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

function AgentRunElapsedHeader({
  canToggle,
  collapsed,
  isThinking,
  label,
  onToggle
}: {
  canToggle: boolean
  collapsed: boolean
  isThinking: boolean
  label: string
  onToggle: () => void
}) {
  const content = (
    <>
      <span className={isThinking ? 'agent-running-text' : undefined}>{label}</span>
      {canToggle && (
        <ChevronDown
          aria-hidden="true"
          className="agent-run__elapsed-chevron"
          data-collapsed={collapsed ? 'true' : 'false'}
        />
      )}
    </>
  )

  if (!canToggle) {
    return <div className="agent-run__elapsed">{content}</div>
  }

  return (
    <button
      aria-expanded={!collapsed}
      className="agent-run__elapsed agent-run__elapsed-button"
      onClick={onToggle}
      type="button"
    >
      {content}
    </button>
  )
}

function UsageAction({ usage }: { usage: AgentUsage | undefined }) {
  const { language, showCacheHitRate, t } = useFrontendConfig()
  const popoverId = useId()
  const rows = getUsageRows(usage, language, t, showCacheHitRate)

  if (rows.length === 0) return null

  return (
    <div className="chat-message__usage">
      <button aria-describedby={popoverId} aria-label={t('chat.usage')} type="button">
        <Database aria-hidden="true" />
      </button>
      <div className="chat-message__usage-popover" id={popoverId} role="tooltip">
        <p className="chat-message__usage-title">{t('chat.usageTitle')}</p>
        <dl className="chat-message__usage-list">
          {rows.map((row) => (
            <div className="chat-message__usage-row" key={row.label}>
              <dt>{row.label}</dt>
              <dd>{row.value}</dd>
            </div>
          ))}
        </dl>
      </div>
    </div>
  )
}

function ChatMessageActions({
  canEdit = false,
  content,
  favorited = false,
  onEdit,
  onFavoriteChange,
  onContinueInNewTask,
  showTokenUsageDetails,
  timestamp,
  usage
}: {
  canEdit?: boolean
  content: string
  favorited?: boolean
  onEdit?: () => void
  onFavoriteChange?: (favorited: boolean) => void
  onContinueInNewTask?: () => void | Promise<void>
  showTokenUsageDetails: boolean
  timestamp: number | undefined
  usage?: AgentUsage
}) {
  const { language, t } = useFrontendConfig()
  const [copied, setCopied] = useState(false)
  const [isContinuing, setIsContinuing] = useState(false)
  const isContinuingRef = useRef(false)
  const timeLabel = formatMessageTime(timestamp, language, t)
  const canCopy = Boolean(content.trim())
  const Icon = copied ? Check : Copy

  useEffect(() => {
    if (!copied) return undefined

    const timerId = window.setTimeout(() => {
      setCopied(false)
    }, COPIED_INDICATOR_MS)

    return () => {
      window.clearTimeout(timerId)
    }
  }, [copied])

  return (
    <div className="chat-message__actions" aria-label={t('chat.messageActions')}>
      {timeLabel && <time dateTime={new Date(timestamp ?? 0).toISOString()}>{timeLabel}</time>}
      <button
        aria-label={copied ? t('chat.copied') : t('chat.copyMessage')}
        disabled={!canCopy}
        onClick={() => {
          if (!canCopy) return
          void copyTextToClipboard(content)
            .then(() => setCopied(true))
            .catch(() => setCopied(false))
        }}
        title={copied ? t('chat.copied') : t('chat.copyMessage')}
        type="button"
      >
        <Icon aria-hidden="true" />
        <span className="chat-message__action-tooltip" role="tooltip">
          {copied ? t('chat.copied') : t('chat.copy')}
        </span>
      </button>
      {onFavoriteChange && (
        <button
          aria-label={favorited ? t('chat.unfavoriteMessage') : t('chat.favoriteMessage')}
          aria-pressed={favorited}
          data-favorited={favorited ? 'true' : undefined}
          onClick={() => onFavoriteChange(!favorited)}
          title={favorited ? t('chat.unfavoriteMessage') : t('chat.favoriteMessage')}
          type="button"
        >
          <Star aria-hidden="true" fill={favorited ? 'currentColor' : 'none'} />
          <span className="chat-message__action-tooltip" role="tooltip">
            {favorited ? t('chat.unfavorite') : t('chat.favorite')}
          </span>
        </button>
      )}
      {canEdit && (
        <button
          aria-label={t('chat.editMessage')}
          onClick={onEdit}
          title={t('chat.editMessage')}
          type="button"
        >
          <Pencil aria-hidden="true" />
          <span className="chat-message__action-tooltip" role="tooltip">
            {t('chat.edit')}
          </span>
        </button>
      )}
      {showTokenUsageDetails && <UsageAction usage={usage} />}
      {/* The parent disables all fork entry points while a request is pending. Keep the
          clicked button mounted until its own request settles so its spinner stays visible. */}
      {(onContinueInNewTask || isContinuing) && (
        <button
          aria-label={t('chat.continueInNewTask')}
          disabled={isContinuing}
          onClick={() => {
            if (!onContinueInNewTask || isContinuingRef.current) return
            isContinuingRef.current = true
            setIsContinuing(true)
            void Promise.resolve(onContinueInNewTask()).finally(() => {
              isContinuingRef.current = false
              setIsContinuing(false)
            })
          }}
          title={t('chat.continueInNewTask')}
          type="button"
        >
          {isContinuing ? (
            <LoaderCircle aria-hidden="true" className="chat-message__action-spinner" />
          ) : (
            <Split aria-hidden="true" />
          )}
          <span className="chat-message__action-tooltip" role="tooltip">
            {t('chat.continueInNewTask')}
          </span>
        </button>
      )}
    </div>
  )
}

function AgentThinkingActivity({ label }: { label: string }) {
  return (
    <div className="agent-thinking">
      <span className="agent-running-text">{label}</span>
    </div>
  )
}

function GuidanceTimelineItemView({
  item,
  mode,
  onOpenWorkspaceReference,
  projectId
}: {
  item: ChatGuidanceTimelineItem
  mode: 'interactive' | 'observer'
  onOpenWorkspaceReference?: (target: WorkspaceReferenceTarget) => void
  projectId?: string | null
}) {
  const { t } = useFrontendConfig()
  const humanAnswer = readHumanInteractionGuidanceDisplay(item)
  const content = stripAttachmentSummary(item.content, item.attachments)
  const statusLabel =
    item.status === 'submitting'
      ? t('chat.guidanceSubmittingStatus')
      : item.status === 'queued'
        ? t('chat.guidanceQueued')
        : item.status === 'rejected'
          ? t('chat.guidanceInterrupted')
          : null

  return (
    <div
      className="chat-guidance"
      data-status={item.status}
      title={item.status === 'rejected' ? item.error : undefined}
    >
      {(item.folderReferences?.length ?? 0) > 0 && (
        <ComposerFolderReferences
          folders={item.folderReferences ?? []}
          label={t('chat.attachments')}
          removeLabel=""
        />
      )}
      {item.attachments.length > 0 && (
        <MessageAttachments attachments={item.attachments} messageId={item.id} mode={mode} />
      )}
      {(humanAnswer || content) && (
        <div className="chat-guidance__bubble">
          {humanAnswer ? (
            <HumanInteractionAnswerContent display={humanAnswer} />
          ) : (
            <ChatMarkdown
              content={content}
              onOpenWorkspaceReference={onOpenWorkspaceReference}
              projectId={projectId}
            />
          )}
        </div>
      )}
      {!humanAnswer && statusLabel && <span className="chat-guidance__status">{statusLabel}</span>}
    </div>
  )
}

function AgentTimelineItemView({
  assistantMessageId,
  conversationId,
  humanInteraction,
  item,
  mode,
  onOpenWorkspaceReference,
  observerRootConversationId,
  projectId,
  run
}: {
  assistantMessageId: string
  conversationId?: string
  humanInteraction?: HumanInteractionTimelineController
  item: RenderableTimelineItem
  mode: 'interactive' | 'observer'
  onOpenWorkspaceReference?: (target: WorkspaceReferenceTarget) => void
  observerRootConversationId?: string
  projectId?: string | null
  run: ChatAgentRunView
}) {
  if (item.type === 'skill_load_group') {
    return <SkillLoadActivity run={run} />
  }

  if (item.type === 'skill_resource_group') {
    const items = getSkillResourceGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <SkillResourceActivityGroup items={items} kind={item.kind} />
  }

  if (item.type === 'office_group') {
    const items = getOfficeGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <OfficeToolActivityGroup items={items} run={run} />
  }

  if (item.type === 'read_group') {
    const items = getReadGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return (
      <ReadToolActivityGroup
        assistantMessageId={assistantMessageId}
        conversationId={conversationId}
        items={items}
        observerRootConversationId={observerRootConversationId}
        projectId={projectId}
      />
    )
  }

  if (item.type === 'search_group') {
    const items = getSearchGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <SearchToolActivityGroup items={items} kind={item.kind} />
  }

  if (item.type === 'web_activity_group') {
    const items = getWebActivityGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <WebSearchToolActivityGroup items={items} />
  }

  if (item.type === 'run_command_group') {
    const items = getRunCommandGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <RunCommandToolActivityGroup items={items} />
  }

  if (item.type === 'send_message_group') {
    return <SendMessageToolActivityGroup items={getSendMessageGroupItems(run, item.callIds)} />
  }

  if (item.type === 'mcp_activity_group') {
    const items = getMcpActivityGroupItems(run, item.invocationIds)
    if (items.length === 0) return null
    return <McpToolActivityGroup items={items} />
  }

  if (item.type === 'file_change_group') {
    const items = getFileChangeGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return (
      <FileChangeToolActivityGroup
        assistantMessageId={assistantMessageId}
        conversationId={conversationId}
        items={items}
        observerRootConversationId={observerRootConversationId}
        projectId={projectId}
        runId={run.runId ?? undefined}
      />
    )
  }

  if (item.type === 'conversation_history_group') {
    const items = getConversationHistoryGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <ConversationHistoryToolActivity items={items} />
  }

  if (item.type === 'context_compaction') {
    return <ContextCompactionActivity status={item.status} />
  }

  if (item.type === 'message') {
    if (!item.content.trim()) return null
    return <ChatMarkdown className="chat-agent-text" content={item.content} />
  }

  if (item.type === 'user_guidance') {
    return (
      <GuidanceTimelineItemView
        item={item}
        mode={mode}
        onOpenWorkspaceReference={onOpenWorkspaceReference}
        projectId={projectId}
      />
    )
  }

  if (item.type === 'mcp_tool_call') {
    const invocation = run.mcpInvocations?.find(
      (candidate) => candidate.invocationId === item.invocationId
    )
    return <McpToolActivity invocation={invocation} />
  }

  if (item.type === 'tool_call') {
    const call = run.toolCalls.find((candidate) => candidate.id === item.callId)
    if (!call) return null
    if (call.tool === 'request_user_input' || call.tool === 'request_user_input_async') {
      const request = humanInteraction?.openRequests.find(
        (candidate) =>
          candidate.status === 'open' &&
          candidate.toolCallId === call.id &&
          candidate.runId === run.runId &&
          candidate.assistantMessageId === assistantMessageId
      )
      return request && humanInteraction ? (
        <HumanInteractionTimelineEntry request={request} interaction={humanInteraction} />
      ) : null
    }
    const mcpInvocation = run.mcpInvocations?.find((candidate) => candidate.callId === call.id)
    const webActivity = run.webSearchActivities?.find((candidate) => candidate.callId === call.id)
    const readActivity = run.readActivities?.find((candidate) => candidate.callId === call.id)
    const fileChangeProposal =
      call.tool === 'apply_patch'
        ? run.fileChangeProposals.find((candidate) => candidate.id === call.id)
        : undefined
    const result = getToolResult(run, call.id)
    const previousTodoResult =
      call.tool === 'todo_update' ? getPreviousSuccessfulTodoResult(run, call.id) : undefined
    const settledStatus = getSettledToolStatus(run, result)
    return (
      <AgentToolActivity
        assistantMessageId={assistantMessageId}
        cancelled={settledStatus === 'cancelled'}
        call={call}
        conversationId={conversationId}
        fileChangeProposal={fileChangeProposal}
        mcpInvocation={mcpInvocation}
        observerRootConversationId={observerRootConversationId}
        projectId={projectId}
        previousTodoResult={previousTodoResult}
        readActivity={readActivity}
        result={result}
        run={run}
        settledStatus={settledStatus}
        showImageGenerationPreview={!isRunSettled(run)}
        toolIdentity={item.identity}
        webActivity={webActivity}
      />
    )
  }

  return (
    <div className="agent-activity agent-activity--error">
      <AlertTriangle aria-hidden="true" />
      <span>{item.message}</span>
    </div>
  )
}

function AgentRunView({
  collaborationTimelineActivities = [],
  directChildAgentIds,
  conversationId,
  humanInteraction,
  message,
  mode,
  onReviewLastTurn,
  onTimelineCollapsedChange,
  onUiStateChange,
  onOpenCollaborationAgent,
  onOpenWorkspaceReference,
  observerRootConversationId,
  projectId,
  timelineCollapsedOverride,
  turnDiffSummary
}: {
  collaborationTimelineActivities?: readonly CollaborationTimelineActivity[]
  directChildAgentIds?: readonly string[]
  conversationId?: string
  humanInteraction?: HumanInteractionTimelineController
  message: ChatMessage
  mode: 'interactive' | 'observer'
  onReviewLastTurn?: (filePath?: string) => void
  onTimelineCollapsedChange?: (messageId: string, collapsed: boolean) => void
  onUiStateChange?: (messageId: string, uiState: ChatMessage['uiState']) => void
  onOpenCollaborationAgent?: (agentId: string) => void
  onOpenWorkspaceReference?: (target: WorkspaceReferenceTarget) => void
  observerRootConversationId?: string
  projectId?: string | null
  timelineCollapsedOverride?: boolean
  turnDiffSummary?: GitTurnDiffSummary
}) {
  const { t } = useFrontendConfig()
  const run = message.agentRun
  const runIsSettled = !run || isRunSettled(run)
  const timeline = useMemo(() => (run ? getFinalTimeline(run) : []), [run])
  const interactionCallIds = new Set(
    humanInteractionRequestsForMessage(message, humanInteraction?.openRequests ?? []).map(
      (request) => request.toolCallId
    )
  )
  const hasInteractionEntries = timeline.some(
    (item) => item.type === 'tool_call' && interactionCallIds.has(item.callId)
  )
  const finalAnswerContent = getAssistantFinalContent(message)
  const finalAnswerTimelineItemIndex = useMemo(() => {
    if (!run || !isRunSettled(run) || !hasDisplayableContent(finalAnswerContent)) return -1

    const normalizedFinalAnswer = finalAnswerContent.trim()
    return timeline.findLastIndex(
      (item) =>
        item.type === 'message' &&
        item.traceSequence === undefined &&
        item.content.trim() === normalizedFinalAnswer
    )
  }, [finalAnswerContent, run, timeline])
  const timelineWithoutFinalAnswer = useMemo(
    () =>
      finalAnswerTimelineItemIndex >= 0
        ? timeline.filter((_, index) => index !== finalAnswerTimelineItemIndex)
        : timeline,
    [finalAnswerTimelineItemIndex, timeline]
  )
  const finalAnswerBlockIndex =
    finalAnswerTimelineItemIndex >= 0
      ? finalAnswerTimelineItemIndex
      : runIsSettled &&
          run?.collaborationFinalResponseBoundary !== undefined &&
          hasDisplayableContent(finalAnswerContent)
        ? timeline.length
        : -1
  const effectiveCollaborationActivities = useMemo(
    () =>
      runIsSettled ? (run?.collaborationTimelineActivities ?? []) : collaborationTimelineActivities,
    [collaborationTimelineActivities, run?.collaborationTimelineActivities, runIsSettled]
  )
  const normalizedCollaborationActivities = useMemo(
    () =>
      normalizeCollaborationTimelineActivities(effectiveCollaborationActivities).filter(
        (activity) =>
          (directChildAgentIds === undefined || directChildAgentIds.includes(activity.agentId)) &&
          activity.parentConversationId === conversationId &&
          activity.anchorMessageId === message.id &&
          activity.traceBoundarySequence !== null
      ),
    [conversationId, directChildAgentIds, effectiveCollaborationActivities, message.id]
  )
  const [liveActivityAnchors, setLiveActivityAnchors] = useState<{
    runId: string | null
    byActivityId: Record<string, string | null>
  }>({ runId: run?.runId ?? null, byActivityId: {} })

  // A durable reload carries exact trace sequences. Before that reload, freeze the raw Timeline
  // tail present when an activity first arrives, so later narration or Tools cannot push it to the
  // end of the current run. useLayoutEffect applies the placement before paint.
  useLayoutEffect(() => {
    if (runIsSettled) return
    const nextRunId = run?.runId ?? null
    setLiveActivityAnchors((current) => {
      const currentAnchors = current.runId === nextRunId ? current.byActivityId : {}
      let nextAnchors = currentAnchors
      for (const activity of normalizedCollaborationActivities) {
        if (activity.traceBoundarySequence === null) continue
        if (Object.prototype.hasOwnProperty.call(nextAnchors, activity.activityId)) continue
        if (nextAnchors === currentAnchors) nextAnchors = { ...currentAnchors }
        nextAnchors[activity.activityId] = timeline.at(-1)?.id ?? null
      }
      if (current.runId === nextRunId && nextAnchors === currentAnchors) return current
      return { runId: nextRunId, byActivityId: nextAnchors }
    })
  }, [normalizedCollaborationActivities, run?.runId, runIsSettled, timeline])

  const displayTimelineBlocks = useMemo(() => {
    type TimelineBlock =
      | { kind: 'timeline'; items: RenderableTimelineItem[] }
      | { kind: 'final-answer' }
      | {
          id: string
          kind: 'collaboration'
          activities: readonly CollaborationTimelineActivity[]
        }

    const blocks: TimelineBlock[] = []
    if (!run) return blocks

    const syntheticItems = groupTimelineItems(run, [], { includeSkillLoadGroup: true })
    if (syntheticItems.length > 0) {
      blocks.push({ kind: 'timeline', items: syntheticItems })
    }

    const placements = new Map<number, CollaborationTimelineActivity[]>()
    for (const activity of normalizedCollaborationActivities) {
      const boundary = activity.traceBoundarySequence
      let insertionIndex = timeline.length
      if (boundary !== null) {
        const timelineSequence = (item: ChatAgentTimelineItem) =>
          item.traceSequence ?? (item.type === 'user_guidance' ? item.sequence : undefined)
        const lastCommittedBeforeBoundary = timeline.findLastIndex((item) => {
          const sequence = timelineSequence(item)
          return sequence !== undefined && sequence < boundary
        })
        const firstCommittedAtOrAfterBoundary = timeline.findIndex((item) => {
          const sequence = timelineSequence(item)
          return sequence !== undefined && sequence >= boundary
        })
        const hasLiveAnchor =
          !runIsSettled &&
          Object.prototype.hasOwnProperty.call(
            liveActivityAnchors.byActivityId,
            activity.activityId
          )
        const durableLowerBound = lastCommittedBeforeBoundary + 1
        const liveAnchor = hasLiveAnchor
          ? liveActivityAnchors.byActivityId[activity.activityId]
          : undefined
        const liveLowerBound =
          liveAnchor === undefined
            ? 0
            : liveAnchor === null
              ? 0
              : timeline.findIndex((item) => item.id === liveAnchor) + 1
        if (firstCommittedAtOrAfterBoundary >= 0) {
          // The durable trace item at/after the boundary is an exact upper bound. Unnumbered
          // presentation items that were already visible when the activity arrived remain before
          // it, but can never move it past that durable boundary.
          insertionIndex = Math.min(
            Math.max(durableLowerBound, liveLowerBound),
            firstCommittedAtOrAfterBoundary
          )
        } else if (lastCommittedBeforeBoundary >= 0 || hasLiveAnchor) {
          // With no later durable marker yet, preserve both the committed prefix and all
          // presentation items already shown at arrival. Later live items stay after the row.
          insertionIndex = Math.max(durableLowerBound, liveLowerBound)
        }
      }
      if (finalAnswerBlockIndex >= 0 && run.collaborationFinalResponseBoundary !== undefined) {
        insertionIndex =
          activity.sequence > run.collaborationFinalResponseBoundary
            ? finalAnswerBlockIndex + 1
            : Math.min(insertionIndex, finalAnswerBlockIndex)
      }
      const slot = placements.get(insertionIndex) ?? []
      slot.push(activity)
      placements.set(insertionIndex, slot)
    }

    let segmentStart = 0
    const appendCollaborationBlock = (activities: readonly CollaborationTimelineActivity[]) => {
      const previous = blocks.at(-1)
      if (previous?.kind === 'collaboration') {
        const mergedActivities = [...previous.activities, ...activities]
        blocks[blocks.length - 1] = {
          id: `collaboration-${mergedActivities.map((activity) => activity.activityId).join(':')}`,
          kind: 'collaboration',
          activities: mergedActivities
        }
        return
      }
      blocks.push({
        id: `collaboration-${activities.map((activity) => activity.activityId).join(':')}`,
        kind: 'collaboration',
        activities
      })
    }
    const appendTimelineSegment = (end: number) => {
      if (end <= segmentStart) return
      const grouped = groupTimelineItems(run, timeline.slice(segmentStart, end), {
        includeSkillLoadGroup: false
      })
      if (grouped.length > 0) {
        blocks.push({
          kind: 'timeline',
          items: grouped
        })
      }
      segmentStart = end
    }

    for (let index = 0; index <= Math.max(timeline.length, finalAnswerBlockIndex + 1); index += 1) {
      const activities = placements.get(index)
      if (activities?.length) {
        appendTimelineSegment(index)
        // Hidden Harness calls still occupy durable trace boundaries. If no user-visible Timeline
        // item was produced between two placements, keep their semantic activities in one compact
        // list. A visible Tool, narration, or final answer creates a real block and stops merging.
        appendCollaborationBlock(activities)
      }
      if (index === finalAnswerBlockIndex) {
        appendTimelineSegment(index)
        blocks.push({ kind: 'final-answer' })
        segmentStart = index + 1
      }
    }
    appendTimelineSegment(timeline.length)
    return blocks
  }, [
    finalAnswerBlockIndex,
    liveActivityAnchors.byActivityId,
    normalizedCollaborationActivities,
    run,
    runIsSettled,
    timeline
  ])
  const finalAnswerRepresentedByTimeline = useMemo(
    () =>
      run?.status !== 'completed' &&
      isContentFullyRepresentedByTimeline(finalAnswerContent, timelineWithoutFinalAnswer),
    [finalAnswerContent, run?.status, timelineWithoutFinalAnswer]
  )
  const displayTimeline = useMemo(
    () => displayTimelineBlocks.flatMap((block) => (block.kind === 'timeline' ? block.items : [])),
    [displayTimelineBlocks]
  )
  const runId = run?.runId
  const waitingForCommandCompletion = Boolean(run && isWaitingForCommandCompletion(run))
  const hasTimeline = displayTimeline.length > 0
  const hasTimelineError = timeline.some((item) => item.type === 'error')
  // Root and observer conversations use the same trusted message/trace ownership.
  const hasCollapsibleCollaborationActivity =
    Boolean(onOpenCollaborationAgent) &&
    hasTrustedAnchoredCollaborationActivity(normalizedCollaborationActivities, message.id)
  const canToggleTimeline = Boolean(
    run &&
    isRunSettled(run) &&
    (hasCollapsibleTimelineContent(run, timeline) || hasCollapsibleCollaborationActivity)
  )
  const [now, setNow] = useState(() => Date.now())

  useEffect(() => {
    if (runIsSettled) return undefined

    const timerId = window.setInterval(() => {
      setNow(Date.now())
    }, 500)

    return () => {
      window.clearInterval(timerId)
    }
  }, [runId, runIsSettled])

  const llmRetryLabel = useMemo(() => {
    const retry = run?.llmRetry
    if (!retry) return null
    return formatTranslation(t, 'agent.llmRetry.reconnecting', {
      attempt: String(Math.max(1, retry.attempt - 1)),
      maxAttempts: String(Math.max(1, retry.maxAttempts - 1))
    })
  }, [run?.llmRetry, t])

  const headerState = useMemo(() => {
    const hasFirstResponse = Boolean(run?.firstResponseAt)
    const hasVisibleToolStatus = Boolean(run && hasCollapsibleTimelineContent(run, timeline))

    if (
      run &&
      !llmRetryLabel &&
      !hasFirstResponse &&
      !hasVisibleToolStatus &&
      !waitingForCommandCompletion &&
      !isRunSettled(run)
    ) {
      return {
        isThinking: true,
        label: t('agent.thinking')
      }
    }

    const startedAt = run?.startedAt ?? message.createdAt
    const endedAt = run?.completedAt ?? now
    if (run?.status === 'cancelled') {
      return {
        isThinking: false,
        label: formatTranslation(t, 'agent.stoppedAfter', {
          duration: formatElapsedDuration(endedAt - startedAt)
        })
      }
    }

    return {
      isThinking: false,
      label: formatTranslation(t, 'agent.processed', {
        duration: formatElapsedDuration(endedAt - startedAt)
      })
    }
  }, [llmRetryLabel, message.createdAt, now, run, t, timeline, waitingForCommandCompletion])
  if (!run) {
    return (
      <>
        {hasDisplayableContent(message.content) ? (
          <ChatMarkdown className="chat-agent-text" content={message.content} />
        ) : null}
        {onOpenCollaborationAgent && (
          <CollaborationTimelineActivityList
            activities={normalizedCollaborationActivities}
            onOpenAgent={onOpenCollaborationAgent}
          />
        )}
      </>
    )
  }

  const hasGuidance = timeline.some((item) => item.type === 'user_guidance')
  const guidanceItems = timeline.filter(
    (item): item is ChatGuidanceTimelineItem => item.type === 'user_guidance'
  )
  const timelineCollapsed = canToggleTimeline
    ? (timelineCollapsedOverride ?? message.uiState?.timelineCollapsed ?? !hasGuidance)
    : false
  // Pending interaction entries and their surrounding narration stay in exact trace order even
  // when technical activity is collapsed. Appending a detached entry after the final answer would
  // erase the boundary between the question introduction and the model's subsequent work.
  const showTimeline =
    hasTimeline && (!(canToggleTimeline && timelineCollapsed) || hasInteractionEntries)
  const showFinalContent =
    hasDisplayableContent(finalAnswerContent) &&
    (runIsSettled || !hasTimeline) &&
    !(showTimeline && finalAnswerRepresentedByTimeline)
  const isStreamingAssistantText =
    !isRunSettled(run) &&
    Boolean(run.lastResponseAt) &&
    now - (run.lastResponseAt ?? 0) <= ACTIVE_STREAMING_GRACE_MS
  const isStreamingFileChange = hasRecentFileChangeActivity(run, now)
  const showThinkingActivity =
    Boolean(llmRetryLabel) ||
    (!(canToggleTimeline && timelineCollapsed) &&
      !headerState.isThinking &&
      !isStreamingAssistantText &&
      !isStreamingFileChange &&
      (waitingForCommandCompletion || shouldShowThinkingActivity(run, timeline)))
  const interruptionReason =
    run.interruption?.reason ??
    (isRunSettled(run) && isTokenLimitFinishReason(run.finishReason)
      ? 'output_limit_reached'
      : undefined)
  const webSearchSources = getUniqueWebSearchSources(run)

  return (
    <div className="agent-run">
      <AgentRunElapsedHeader
        canToggle={canToggleTimeline}
        collapsed={timelineCollapsed}
        isThinking={headerState.isThinking}
        label={headerState.label}
        onToggle={() => {
          if (onTimelineCollapsedChange) {
            onTimelineCollapsedChange(message.id, !timelineCollapsed)
            return
          }
          onUiStateChange?.(message.id, {
            ...message.uiState,
            timelineCollapsed: !timelineCollapsed
          })
        }}
      />
      {canToggleTimeline && timelineCollapsed && !hasInteractionEntries
        ? guidanceItems.map((item) => (
            <GuidanceTimelineItemView
              item={item}
              mode={mode}
              onOpenWorkspaceReference={onOpenWorkspaceReference}
              projectId={projectId}
              key={`timeline-item:${item.id}`}
            />
          ))
        : null}
      {displayTimelineBlocks.flatMap((block) => {
        if (block.kind === 'final-answer') {
          return showFinalContent ? (
            <ChatMarkdown
              className="chat-agent-text"
              content={finalAnswerContent}
              key="assistant-final-answer"
            />
          ) : (
            []
          )
        }
        if (block.kind === 'collaboration') {
          if (!onOpenCollaborationAgent || (canToggleTimeline && timelineCollapsed)) return []
          return (
            <CollaborationTimelineActivityList
              activities={block.activities}
              key={`timeline-${block.id}`}
              onOpenAgent={onOpenCollaborationAgent}
            />
          )
        }
        if (!showTimeline) return []
        // Timeline segments are presentation-only placement boundaries. Their start/end changes
        // whenever a later Tool or collaboration event arrives, so they must never own React
        // identity. Keep every semantic item directly under the run with its durable item id.
        const visibleItems =
          canToggleTimeline && timelineCollapsed && hasInteractionEntries
            ? block.items.filter(
                (item) =>
                  item.type === 'message' ||
                  item.type === 'user_guidance' ||
                  (item.type === 'tool_call' && interactionCallIds.has(item.callId))
              )
            : block.items
        return visibleItems.map((item) => (
          <AgentTimelineItemView
            assistantMessageId={message.id}
            conversationId={conversationId}
            humanInteraction={mode === 'interactive' ? humanInteraction : undefined}
            item={item}
            key={`timeline-item:${item.id}`}
            mode={mode}
            onOpenWorkspaceReference={onOpenWorkspaceReference}
            observerRootConversationId={observerRootConversationId}
            projectId={projectId}
            run={run}
          />
        ))
      })}
      {showFinalContent && finalAnswerBlockIndex < 0 && (
        <ChatMarkdown
          className="chat-agent-text"
          content={finalAnswerContent}
          key="assistant-final-answer"
        />
      )}
      {isRunSettled(run) && (
        <ImageGenerationArtifactsCard
          conversationId={conversationId}
          key={`image-artifacts:${run.runId}`}
          observerRootConversationId={observerRootConversationId}
          resolver={hostImageArtifactResolver}
          run={run}
        />
      )}
      {isRunSettled(run) && (
        <OfficeArtifactsCard
          assistantMessageId={message.id}
          conversationId={conversationId}
          key={`office-artifacts:${run.runId}`}
          observerRootConversationId={observerRootConversationId}
          projectId={projectId}
          run={run}
        />
      )}
      {isRunSettled(run) && turnDiffSummary && (
        <EditSummaryCard
          key={`edit-summary:${run.runId}`}
          onReview={onReviewLastTurn}
          readOnly={mode === 'observer'}
          summary={turnDiffSummary}
        />
      )}
      {isRunSettled(run) && (
        <AssistantSources key={`assistant-sources:${run.runId}`} sources={webSearchSources} />
      )}
      {interruptionReason && (
        <div className="agent-run__interruption" role="status">
          {interruptionReason === 'service_connection_failed' ||
          interruptionReason === 'stream_interrupted' ? (
            <WifiOff aria-hidden="true" />
          ) : (
            <AlertTriangle aria-hidden="true" />
          )}
          <span>{t(interruptionTranslationKey(interruptionReason))}</span>
        </div>
      )}
      {showThinkingActivity && (
        <AgentThinkingActivity
          label={
            llmRetryLabel ??
            t(waitingForCommandCompletion ? 'agent.command.waitingForCompletion' : 'agent.thinking')
          }
        />
      )}
      {showTimeline && run.error && !hasTimelineError && !interruptionReason && (
        <div className="agent-run__error">{run.error}</div>
      )}
    </div>
  )
}

function MessageContent({
  collaborationTimelineActivities,
  directChildAgentIds,
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
        directChildAgentIds={directChildAgentIds}
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

function EditableUserMessage({
  hasAttachments,
  hasImageAttachments,
  initialContent,
  selectedModelAvailable,
  selectedModelSupportsImage,
  onCancel,
  onSubmit
}: {
  hasAttachments: boolean
  hasImageAttachments: boolean
  initialContent: string
  selectedModelAvailable: boolean
  selectedModelSupportsImage: boolean
  onCancel: () => void
  onSubmit: (content: string) => void | Promise<void>
}) {
  const { t } = useFrontendConfig()
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const [content, setContent] = useState(initialContent)
  const [error, setError] = useState<string | null>(null)
  const [isSubmitting, setIsSubmitting] = useState(false)
  const validationMessage = !selectedModelAvailable
    ? t('chat.noEnabledModels')
    : hasImageAttachments && !selectedModelSupportsImage
      ? t('chat.unsupportedImageWarning')
      : null
  const canSend = (content.trim().length > 0 || hasAttachments) && !validationMessage

  useEffect(() => {
    const textarea = textareaRef.current
    if (!textarea) return

    textarea.style.height = 'auto'
    textarea.style.height = `${textarea.scrollHeight}px`
  }, [content])

  const submit = async () => {
    if (!canSend || isSubmitting) return

    setIsSubmitting(true)
    setError(null)
    try {
      await onSubmit(content.trim())
    } catch (submitError) {
      setError(getUserFacingErrorMessage(submitError, t, 'chat.editMessageFailed'))
      setIsSubmitting(false)
    }
  }

  return (
    <form
      className="chat-message-edit"
      onSubmit={(event) => {
        event.preventDefault()
        void submit()
      }}
    >
      <textarea
        ref={textareaRef}
        aria-label={t('chat.editMessage')}
        autoFocus
        value={content}
        onChange={(event) => setContent(event.target.value)}
        onKeyDown={(event) => {
          if (event.key !== 'Enter' || event.shiftKey) return
          event.preventDefault()
          void submit()
        }}
      />
      {validationMessage && <p className="chat-message-edit__warning">{validationMessage}</p>}
      {error && <p className="chat-message-edit__error">{error}</p>}
      <div className="chat-message-edit__actions">
        <button disabled={isSubmitting} onClick={onCancel} type="button">
          {t('chat.cancel')}
        </button>
        <button disabled={!canSend || isSubmitting} type="submit">
          {t('chat.send')}
        </button>
      </div>
    </form>
  )
}

function managedImageInput(
  attachment: NonNullable<ChatMessage['attachments']>[number]
): AgentInputAttachment | undefined {
  if (attachment.kind !== 'image' || attachment.encoding !== 'managed' || !attachment.data)
    return undefined
  return {
    id: attachment.id,
    kind: 'image',
    name: attachment.name,
    mimeType: attachment.mimeType ?? undefined,
    sizeBytes: attachment.sizeBytes,
    encoding: 'managed',
    data: attachment.data
  }
}

function MessageAttachments({
  attachments,
  messageId,
  mode
}: {
  attachments?: ChatMessage['attachments']
  messageId: string
  mode: 'interactive' | 'observer'
}) {
  const { t } = useFrontendConfig()
  const openImagePreview = useImagePreview()
  const showImagePreviewNotice = useImagePreviewNotice()
  const [hydratedManagedAttachments, setHydratedManagedAttachments] = useState<
    Map<string, AgentInputAttachment>
  >(new Map())
  const hydrationRequests = useRef(new Set<string>())
  useEffect(() => {
    const candidates =
      mode === 'interactive'
        ? (attachments ?? []).filter(
            (attachment) =>
              attachment.kind === 'image' &&
              !getAttachmentPreviewUrl(attachment) &&
              !managedImageInput(attachment)
          )
        : []
    const candidateIds = new Set(candidates.map((attachment) => attachment.id))
    hydrationRequests.current = new Set(
      [...hydrationRequests.current].filter((id) => candidateIds.has(id))
    )
    setHydratedManagedAttachments((previous) => {
      const next = new Map([...previous].filter(([id]) => candidateIds.has(id)))
      return next.size === previous.size ? previous : next
    })
    if (candidates.length === 0) return

    const pending = candidates.filter((attachment) => !hydrationRequests.current.has(attachment.id))
    if (pending.length === 0) return
    if (typeof storageClient.loadInputAttachments !== 'function') return
    for (const attachment of pending) hydrationRequests.current.add(attachment.id)
    let active = true
    void Promise.allSettled(
      pending.map(async (attachment) => {
        const [managed] = await storageClient.loadInputAttachments([attachment.id])
        return managed
      })
    ).then((results) => {
      if (!active) return
      setHydratedManagedAttachments((previous) => {
        const next = new Map(previous)
        for (const result of results) {
          if (result.status === 'fulfilled' && result.value) {
            next.set(result.value.id, result.value)
          }
        }
        return next
      })
    })
    return () => {
      active = false
    }
  }, [attachments, mode])
  const previewInputs = useMemo(
    () =>
      (attachments ?? []).map((attachment) => ({
        id: attachment.id,
        kind: attachment.kind,
        previewUrl: getAttachmentPreviewUrl(attachment),
        agentAttachment:
          mode === 'interactive'
            ? (managedImageInput(attachment) ?? hydratedManagedAttachments.get(attachment.id))
            : undefined
      })),
    [attachments, hydratedManagedAttachments, mode]
  )
  const previewAttachments = useComposerAttachmentPreviews(previewInputs)
  const previewUrls = new Map(
    previewAttachments.map((attachment) => [attachment.id, attachment.previewUrl])
  )
  const previewRequestRef = useRef(0)
  useEffect(
    () => () => {
      previewRequestRef.current += 1
    },
    [messageId]
  )
  if (!attachments?.length) return null

  const openOriginalImageAttachment = async (
    attachment: NonNullable<ChatMessage['attachments']>[number]
  ) => {
    const previewRequest = ++previewRequestRef.current
    if (mode === 'observer') {
      // Observer DTOs carry only authorized display bytes, never an ordinary attachment input.
      const src = getAttachmentPreviewUrl(attachment)
      if (src) openImagePreview({ alt: attachment.name, fileName: attachment.name, src })
      return
    }

    try {
      const managed = managedImageInput(attachment) ?? hydratedManagedAttachments.get(attachment.id)
      if (managed) {
        const src = await loadComposerAttachmentImage(managed)
        if (previewRequestRef.current !== previewRequest) return
        if (src) {
          openImagePreview({ alt: attachment.name, fileName: attachment.name, src })
          return
        }
      }
      const image = await loadAttachmentImage(attachment.id)
      if (previewRequestRef.current !== previewRequest) return
      if (!image?.mimeType.startsWith('image/') || !image.data) {
        showImagePreviewNotice(t('imagePreview.originalMissing'))
        return
      }

      openImagePreview({
        alt: image.name || attachment.name,
        fileName: image.name || attachment.name,
        src: `data:${image.mimeType};base64,${image.data}`
      })
    } catch (error) {
      if (previewRequestRef.current !== previewRequest) return
      console.error('Failed to load attachment image', error)
      showImagePreviewNotice(t('imagePreview.originalMissing'))
    }
  }

  const displayAttachments: ComposerAttachmentPresentation[] = attachments.map((attachment) => ({
    id: attachment.id,
    kind: attachment.kind,
    name: attachment.name,
    sizeBytes: attachment.sizeBytes,
    previewUrl: getAttachmentPreviewUrl(attachment) ?? previewUrls.get(attachment.id)
  }))

  return (
    <ComposerAttachments
      attachments={displayAttachments}
      label={t('chat.attachments')}
      messageId={messageId}
      onPreviewAttachment={(id) => {
        const attachment = attachments.find((candidate) => candidate.id === id)
        if (attachment) void openOriginalImageAttachment(attachment)
      }}
      removeLabel=""
      variant="message"
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
  const { t } = useFrontendConfig()
  if (message.role !== 'user' || !message.inputOrigin) return null
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
  directChildAgentIds,
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
  const isAssistantActionsVisible = shouldShowAssistantActions(message)
  const userVisibleContent = getUserVisibleContent(message)
  const humanAnswer = message.role === 'user' ? message.humanInteractionDisplay : null
  const actionContent =
    message.role === 'assistant'
      ? getAssistantFinalContent(message)
      : humanAnswer
        ? humanInteractionDisplayText(humanAnswer)
        : userVisibleContent
  const actionTimestamp =
    message.role === 'assistant'
      ? (message.agentRun?.completedAt ?? message.createdAt)
      : message.createdAt
  const actionUsage = message.role === 'assistant' ? message.agentRun?.usage : undefined
  const showActions = message.role === 'user' || isAssistantActionsVisible
  const canEdit = message.role === 'user' && !humanAnswer && Boolean(onEditSubmit)
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
          <MessageContent
            collaborationTimelineActivities={collaborationTimelineActivities}
            directChildAgentIds={directChildAgentIds}
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
        </div>
      ) : null}
      {showActions && !isEditing && (
        <ChatMessageActions
          canEdit={canEdit}
          content={actionContent}
          favorited={isFavorited}
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
