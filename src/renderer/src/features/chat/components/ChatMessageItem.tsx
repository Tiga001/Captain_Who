import { Fragment, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react'
import {
  AlertTriangle,
  ChevronDown,
  Check,
  Copy,
  Database,
  LoaderCircle,
  Pencil,
  Split,
  Star
} from 'lucide-react'
import type { AgentProposedAction, AgentUsage, GitTurnDiffSummary } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../config/translationFormat'
import type { ChatAgentRunView, ChatAgentTimelineItem, ChatMessage } from '../chatTypes'
import type { ChatGuidanceTimelineItem } from '../chatTypes'
import { getUniqueWebSearchSources } from '../agentWebSearch'
import { stripAttachmentSummary } from '../chatAttachments'
import {
  getAttachmentBadgeLabel,
  getAttachmentExtension,
  getAttachmentIcon,
  getAttachmentPreviewUrl
} from '../attachmentDisplay'
import { loadAttachmentImage } from '../../storage/storageClient'
import { ChatMarkdown } from './ChatMarkdown'
import {
  copyTextToClipboard,
  formatElapsedDuration,
  formatMessageTime,
  getApplyPatchGroupItems,
  getAssistantFinalContent,
  getConversationHistoryGroupItems,
  getMcpActivityGroupItems,
  getOfficeGroupItems,
  getPreviousSuccessfulTodoResult,
  getReadGroupItems,
  getRunCommandGroupItems,
  getSearchGroupItems,
  getSkillResourceGroupItems,
  getSettledToolStatus,
  getToolResult,
  getUsageRows,
  getUserVisibleContent,
  getWebActivityGroupItems,
  getWriteFileGroupItems,
  groupTimelineItems,
  hasCollapsibleTimelineContent,
  hasDisplayableContent,
  hasRecentFileWriteActivity,
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
import { FileWriteToolActivityGroup } from './toolActivities/FileWriteToolActivity'
import { ApplyPatchToolActivityGroup } from './toolActivities/ApplyPatchToolActivity'
import { ReadToolActivityGroup } from './toolActivities/ReadToolActivity'
import { RunCommandToolActivityGroup } from './toolActivities/RunCommandToolActivity'
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

const ACTIVE_STREAMING_GRACE_MS = 1200
const COPIED_INDICATOR_MS = 1300

interface ChatMessageItemProps {
  agentLabelsById?: Readonly<Record<string, string>>
  collaborationTimelineActivities?: readonly CollaborationTimelineActivity[]
  conversationId?: string
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
    options?: { rememberForRun?: boolean }
  ) => void
  onCancel?: (messageId: string, action: AgentProposedAction) => void
  onEditSubmit?: (messageId: string, content: string) => void | Promise<void>
  onContinueInNewTask?: (messageId: string) => void | Promise<void>
  onOpenCollaborationAgent?: (agentId: string) => void
  onReject?: (messageId: string, action: AgentProposedAction, message?: string) => void
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
  const { language, t } = useFrontendConfig()
  const popoverId = useId()
  const rows = getUsageRows(usage, language, t)

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
      {onContinueInNewTask && (
        <button
          aria-label={t('chat.continueInNewTask')}
          disabled={isContinuing}
          onClick={() => {
            if (isContinuingRef.current) return
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

function GuidanceTimelineItemView({ item }: { item: ChatGuidanceTimelineItem }) {
  const { t } = useFrontendConfig()
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
      <div className="chat-guidance__bubble">
        <MessageAttachments attachments={item.attachments} messageId={item.id} />
        {content && <ChatMarkdown content={content} />}
      </div>
      {statusLabel && <span className="chat-guidance__status">{statusLabel}</span>}
    </div>
  )
}

function AgentTimelineItemView({
  conversationId,
  item,
  observerRootConversationId,
  projectId,
  run
}: {
  conversationId?: string
  item: RenderableTimelineItem
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

  if (item.type === 'mcp_activity_group') {
    const items = getMcpActivityGroupItems(run, item.invocationIds)
    if (items.length === 0) return null
    return <McpToolActivityGroup items={items} />
  }

  if (item.type === 'apply_patch_group') {
    const items = getApplyPatchGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <ApplyPatchToolActivityGroup items={items} projectId={projectId} />
  }

  if (item.type === 'write_file_group') {
    const items = getWriteFileGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return (
      <FileWriteToolActivityGroup
        items={items}
        observerRootConversationId={observerRootConversationId}
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
    return <GuidanceTimelineItemView item={item} />
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
    const mcpInvocation = run.mcpInvocations?.find((candidate) => candidate.callId === call.id)
    const webActivity = run.webSearchActivities?.find((candidate) => candidate.callId === call.id)
    const readActivity = run.readActivities?.find((candidate) => candidate.callId === call.id)
    const diff =
      call.tool === 'apply_patch'
        ? run.diffs.find((candidate) => candidate.id === call.id)
        : undefined
    const result = getToolResult(run, call.id)
    const previousTodoResult =
      call.tool === 'todo_update' ? getPreviousSuccessfulTodoResult(run, call.id) : undefined
    const settledStatus = getSettledToolStatus(run, result)
    return (
      <AgentToolActivity
        cancelled={settledStatus === 'cancelled'}
        call={call}
        conversationId={conversationId}
        diff={diff}
        mcpInvocation={mcpInvocation}
        observerRootConversationId={observerRootConversationId}
        projectId={projectId}
        previousTodoResult={previousTodoResult}
        readActivity={readActivity}
        result={result}
        run={run}
        settledStatus={settledStatus}
        showImageGenerationPreview={!isRunSettled(run)}
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
  conversationId,
  message,
  mode,
  onReviewLastTurn,
  onTimelineCollapsedChange,
  onUiStateChange,
  onOpenCollaborationAgent,
  observerRootConversationId,
  projectId,
  timelineCollapsedOverride,
  turnDiffSummary
}: {
  collaborationTimelineActivities?: readonly CollaborationTimelineActivity[]
  conversationId?: string
  message: ChatMessage
  mode: 'interactive' | 'observer'
  onReviewLastTurn?: (filePath?: string) => void
  onTimelineCollapsedChange?: (messageId: string, collapsed: boolean) => void
  onUiStateChange?: (messageId: string, uiState: ChatMessage['uiState']) => void
  onOpenCollaborationAgent?: (agentId: string) => void
  observerRootConversationId?: string
  projectId?: string | null
  timelineCollapsedOverride?: boolean
  turnDiffSummary?: GitTurnDiffSummary
}) {
  const { t } = useFrontendConfig()
  const run = message.agentRun
  const timeline = useMemo(() => run?.timeline ?? [], [run?.timeline])
  const finalAnswerContent = getAssistantFinalContent(message)
  const finalAnswerTimelineItemIndex = useMemo(() => {
    if (!run || !isRunSettled(run) || !hasDisplayableContent(finalAnswerContent)) return -1

    const normalizedFinalAnswer = finalAnswerContent.trim()
    return timeline.findLastIndex(
      (item) => item.type === 'message' && item.content.trim() === normalizedFinalAnswer
    )
  }, [finalAnswerContent, run, timeline])
  const timelineWithoutFinalAnswer = useMemo(
    () =>
      finalAnswerTimelineItemIndex >= 0
        ? timeline.filter((_, index) => index !== finalAnswerTimelineItemIndex)
        : timeline,
    [finalAnswerTimelineItemIndex, timeline]
  )
  const normalizedCollaborationActivities = useMemo(
    () => normalizeCollaborationTimelineActivities(collaborationTimelineActivities),
    [collaborationTimelineActivities]
  )
  const [liveActivityAnchors, setLiveActivityAnchors] = useState<{
    runId: string | null
    byActivityId: Record<string, string | null>
  }>({ runId: run?.runId ?? null, byActivityId: {} })

  // A durable reload carries exact trace sequences. Before that reload, freeze the raw Timeline
  // tail present when an activity first arrives, so later narration or Tools cannot push it to the
  // end of the current run. useLayoutEffect applies the placement before paint.
  useLayoutEffect(() => {
    const nextRunId = run?.runId ?? null
    setLiveActivityAnchors((current) => {
      const currentAnchors = current.runId === nextRunId ? current.byActivityId : {}
      let nextAnchors = currentAnchors
      for (const activity of normalizedCollaborationActivities) {
        if (activity.rootTraceBoundarySequence === null) continue
        if (Object.prototype.hasOwnProperty.call(nextAnchors, activity.activityId)) continue
        if (nextAnchors === currentAnchors) nextAnchors = { ...currentAnchors }
        nextAnchors[activity.activityId] = timeline.at(-1)?.id ?? null
      }
      if (current.runId === nextRunId && nextAnchors === currentAnchors) return current
      return { runId: nextRunId, byActivityId: nextAnchors }
    })
  }, [normalizedCollaborationActivities, run?.runId, timeline])

  const displayTimelineBlocks = useMemo(() => {
    type TimelineBlock =
      | { id: string; kind: 'timeline'; items: RenderableTimelineItem[] }
      | { id: string; kind: 'final-answer' }
      | {
          id: string
          kind: 'collaboration'
          activities: readonly CollaborationTimelineActivity[]
        }

    const blocks: TimelineBlock[] = []
    if (!run) return blocks

    const syntheticItems = groupTimelineItems(run, [], { includeSkillLoadGroup: true })
    if (syntheticItems.length > 0) {
      blocks.push({ id: 'timeline-synthetic', kind: 'timeline', items: syntheticItems })
    }

    const placements = new Map<number, CollaborationTimelineActivity[]>()
    for (const activity of normalizedCollaborationActivities) {
      const boundary = activity.rootTraceBoundarySequence
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
        const hasLiveAnchor = Object.prototype.hasOwnProperty.call(
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
      const slot = placements.get(insertionIndex) ?? []
      slot.push(activity)
      placements.set(insertionIndex, slot)
    }

    let segmentStart = 0
    const appendTimelineSegment = (end: number) => {
      if (end <= segmentStart) return
      const grouped = groupTimelineItems(run, timeline.slice(segmentStart, end), {
        includeSkillLoadGroup: false
      })
      if (grouped.length > 0) {
        blocks.push({
          id: `timeline-${segmentStart}-${end}`,
          kind: 'timeline',
          items: grouped
        })
      }
      segmentStart = end
    }

    for (let index = 0; index <= timeline.length; index += 1) {
      const activities = placements.get(index)
      if (activities?.length) {
        appendTimelineSegment(index)
        blocks.push({
          id: `collaboration-${activities.map((activity) => activity.activityId).join(':')}`,
          kind: 'collaboration',
          activities
        })
      }
      if (index === finalAnswerTimelineItemIndex) {
        appendTimelineSegment(index)
        blocks.push({ id: 'final-answer', kind: 'final-answer' })
        segmentStart = index + 1
      }
    }
    appendTimelineSegment(timeline.length)
    return blocks
  }, [
    finalAnswerTimelineItemIndex,
    liveActivityAnchors.byActivityId,
    normalizedCollaborationActivities,
    run,
    timeline
  ])
  const finalAnswerRepresentedByTimeline = useMemo(
    () => isContentFullyRepresentedByTimeline(finalAnswerContent, timelineWithoutFinalAnswer),
    [finalAnswerContent, timelineWithoutFinalAnswer]
  )
  const displayTimeline = useMemo(
    () => displayTimelineBlocks.flatMap((block) => (block.kind === 'timeline' ? block.items : [])),
    [displayTimelineBlocks]
  )
  const runId = run?.runId
  const runIsSettled = !run || isRunSettled(run)
  const waitingForCommandCompletion = Boolean(run && isWaitingForCommandCompletion(run))
  const hasTimeline = displayTimeline.length > 0
  const hasTimelineError = timeline.some((item) => item.type === 'error')
  const canToggleTimeline = Boolean(
    run && isRunSettled(run) && hasCollapsibleTimelineContent(run, timeline)
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
    const substitutions = {
      attempt: String(retry.attempt),
      maxAttempts: String(retry.maxAttempts)
    }
    if (retry.retryAt <= 0 && retry.delayMs <= 0) {
      return formatTranslation(t, 'agent.llmRetry.retrying', substitutions)
    }
    const remainingMs = retry.retryAt > 0 ? Math.max(0, retry.retryAt - now) : retry.delayMs
    const key =
      retry.category === 'rate_limited'
        ? 'agent.llmRetry.rateLimited'
        : 'agent.llmRetry.temporarilyUnavailable'
    return formatTranslation(t, key, {
      ...substitutions,
      seconds: String(Math.max(0, Math.ceil(remainingMs / 1000)))
    })
  }, [now, run?.llmRetry, t])

  const headerState = useMemo(() => {
    if (llmRetryLabel) {
      return {
        isThinking: true,
        label: llmRetryLabel
      }
    }
    const hasFirstResponse = Boolean(run?.firstResponseAt)
    const hasVisibleToolStatus = Boolean(run && hasCollapsibleTimelineContent(run, timeline))

    if (
      run &&
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
  const timelineCollapsed = canToggleTimeline
    ? (timelineCollapsedOverride ?? message.uiState?.timelineCollapsed ?? !hasGuidance)
    : false
  const showTimeline = hasTimeline && !(canToggleTimeline && timelineCollapsed)
  const showFinalContent =
    hasDisplayableContent(finalAnswerContent) &&
    (runIsSettled || !hasTimeline) &&
    !(showTimeline && finalAnswerRepresentedByTimeline)
  const isStreamingAssistantText =
    !isRunSettled(run) &&
    Boolean(run.lastResponseAt) &&
    now - (run.lastResponseAt ?? 0) <= ACTIVE_STREAMING_GRACE_MS
  const isStreamingFileWrite = hasRecentFileWriteActivity(run, now)
  const showThinkingActivity =
    !(canToggleTimeline && timelineCollapsed) &&
    !headerState.isThinking &&
    !isStreamingAssistantText &&
    !isStreamingFileWrite &&
    (waitingForCommandCompletion || shouldShowThinkingActivity(run, timeline))
  const showTokenLimitNotice = isRunSettled(run) && isTokenLimitFinishReason(run.finishReason)
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
      {displayTimelineBlocks.map((block) => {
        if (block.kind === 'final-answer') {
          return showFinalContent ? (
            <ChatMarkdown className="chat-agent-text" content={finalAnswerContent} key={block.id} />
          ) : null
        }
        if (block.kind === 'collaboration') {
          if (!onOpenCollaborationAgent) return null
          return (
            <CollaborationTimelineActivityList
              activities={block.activities}
              key={block.id}
              onOpenAgent={onOpenCollaborationAgent}
            />
          )
        }
        if (!showTimeline) return null
        return (
          <Fragment key={block.id}>
            {block.items.map((item) => (
              <AgentTimelineItemView
                conversationId={conversationId}
                item={item}
                key={item.id}
                observerRootConversationId={observerRootConversationId}
                projectId={projectId}
                run={run}
              />
            ))}
          </Fragment>
        )
      })}
      {showFinalContent && finalAnswerTimelineItemIndex < 0 && (
        <ChatMarkdown className="chat-agent-text" content={finalAnswerContent} />
      )}
      {isRunSettled(run) && (
        <ImageGenerationArtifactsCard
          conversationId={conversationId}
          observerRootConversationId={observerRootConversationId}
          resolver={hostImageArtifactResolver}
          run={run}
        />
      )}
      {isRunSettled(run) && (
        <OfficeArtifactsCard
          conversationId={conversationId}
          observerRootConversationId={observerRootConversationId}
          projectId={projectId}
          run={run}
        />
      )}
      {isRunSettled(run) && turnDiffSummary && (
        <EditSummaryCard
          onReview={onReviewLastTurn}
          readOnly={mode === 'observer'}
          summary={turnDiffSummary}
        />
      )}
      {isRunSettled(run) && <AssistantSources sources={webSearchSources} />}
      {showTokenLimitNotice && (
        <div className="agent-run__notice" role="status">
          <AlertTriangle aria-hidden="true" />
          <span>{t('agent.tokenLimitNotice')}</span>
        </div>
      )}
      {showThinkingActivity && (
        <AgentThinkingActivity
          label={t(
            waitingForCommandCompletion ? 'agent.command.waitingForCompletion' : 'agent.thinking'
          )}
        />
      )}
      {showTimeline && run.error && !hasTimelineError && (
        <div className="agent-run__error">{run.error}</div>
      )}
    </div>
  )
}

function MessageContent({
  collaborationTimelineActivities,
  conversationId,
  message,
  mode = 'interactive',
  onReviewLastTurn,
  onTimelineCollapsedChange,
  onUiStateChange,
  onOpenCollaborationAgent,
  observerRootConversationId,
  projectId,
  timelineCollapsedOverride,
  turnDiffSummary
}: ChatMessageItemProps) {
  if (message.role === 'assistant') {
    return (
      <AgentRunView
        collaborationTimelineActivities={collaborationTimelineActivities}
        conversationId={conversationId}
        message={message}
        mode={mode}
        onReviewLastTurn={onReviewLastTurn}
        onTimelineCollapsedChange={onTimelineCollapsedChange}
        onUiStateChange={onUiStateChange}
        onOpenCollaborationAgent={onOpenCollaborationAgent}
        observerRootConversationId={observerRootConversationId}
        projectId={projectId}
        timelineCollapsedOverride={timelineCollapsedOverride}
        turnDiffSummary={turnDiffSummary}
      />
    )
  }

  return <ChatMarkdown content={getUserVisibleContent(message)} />
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
      setError(submitError instanceof Error ? submitError.message : String(submitError))
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

function MessageAttachments({
  attachments,
  messageId
}: {
  attachments?: ChatMessage['attachments']
  messageId: string
}) {
  const { t } = useFrontendConfig()
  const openImagePreview = useImagePreview()
  const showImagePreviewNotice = useImagePreviewNotice()
  if (!attachments?.length) return null

  const imageAttachments = attachments.filter((attachment) => attachment.kind === 'image')
  const fileAttachments = attachments.filter((attachment) => attachment.kind !== 'image')

  const openOriginalImageAttachment = async (
    attachment: NonNullable<ChatMessage['attachments']>[number]
  ) => {
    if (
      attachment.encoding === 'base64' &&
      attachment.mimeType?.startsWith('image/') &&
      attachment.data
    ) {
      openImagePreview({
        alt: attachment.name,
        fileName: attachment.name,
        src: `data:${attachment.mimeType};base64,${attachment.data}`
      })
      return
    }

    try {
      const image = await loadAttachmentImage(attachment.id)
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
      console.error('Failed to load attachment image', error)
      showImagePreviewNotice(t('imagePreview.originalMissing'))
    }
  }

  const renderAttachment = (attachment: NonNullable<ChatMessage['attachments']>[number]) => {
    const extension = getAttachmentExtension(attachment.name)
    const AttachmentIcon = getAttachmentIcon(attachment.kind, extension)
    const badgeLabel = getAttachmentBadgeLabel(extension)
    const previewUrl = getAttachmentPreviewUrl(attachment)
    const isImagePreview = attachment.kind === 'image' && Boolean(previewUrl)

    if (isImagePreview) {
      return (
        <button
          className="chat-message-attachment"
          data-chat-attachment-id={attachment.id}
          data-chat-attachment-message-id={messageId}
          data-kind="image"
          key={attachment.id}
          onClick={() => void openOriginalImageAttachment(attachment)}
          title={attachment.name}
          type="button"
        >
          <img src={previewUrl} alt={attachment.name} />
        </button>
      )
    }

    return (
      <div
        className="chat-message-attachment"
        data-chat-attachment-id={attachment.id}
        data-chat-attachment-message-id={messageId}
        data-kind="file"
        key={attachment.id}
        title={attachment.name}
      >
        <span className="chat-message-attachment__icon" aria-hidden="true">
          {badgeLabel ? (
            <span className="chat-message-attachment__badge">{badgeLabel}</span>
          ) : (
            <AttachmentIcon />
          )}
        </span>
        <span className="chat-message-attachment__name">{attachment.name}</span>
      </div>
    )
  }

  return (
    <div className="chat-message__attachments" aria-label={t('chat.attachments')}>
      {imageAttachments.length > 0 && (
        <div className="chat-message__attachment-row" data-kind="image">
          {imageAttachments.map(renderAttachment)}
        </div>
      )}
      {fileAttachments.length > 0 && (
        <div className="chat-message__attachment-row" data-kind="file">
          {fileAttachments.map(renderAttachment)}
        </div>
      )}
    </div>
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

export function ChatMessageItem({
  agentLabelsById,
  collaborationTimelineActivities,
  conversationId,
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
  const [isEditing, setIsEditing] = useState(false)
  const isAssistantActionsVisible = shouldShowAssistantActions(message)
  const userVisibleContent = getUserVisibleContent(message)
  const actionContent =
    message.role === 'assistant' ? getAssistantFinalContent(message) : userVisibleContent
  const actionTimestamp =
    message.role === 'assistant'
      ? (message.agentRun?.completedAt ?? message.createdAt)
      : message.createdAt
  const actionUsage = message.role === 'assistant' ? message.agentRun?.usage : undefined
  const showActions = message.role === 'user' || isAssistantActionsVisible
  const canEdit = message.role === 'user' && Boolean(onEditSubmit)
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
        <MessageAttachments attachments={message.attachments} messageId={message.id} />
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
            conversationId={conversationId}
            message={message}
            mode={mode}
            onApprove={onApprove}
            onCancel={onCancel}
            onReject={onReject}
            onReviewLastTurn={onReviewLastTurn}
            onTimelineCollapsedChange={onTimelineCollapsedChange}
            onUiStateChange={onUiStateChange}
            onOpenCollaborationAgent={onOpenCollaborationAgent}
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
}
