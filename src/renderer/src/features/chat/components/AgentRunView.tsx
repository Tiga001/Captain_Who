import { useEffect, useLayoutEffect, useMemo, useState } from 'react'
import { AlertTriangle, ChevronDown, WifiOff } from 'lucide-react'
import type { GitTurnDiffSummary } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../config/translationFormat'
import type { ChatAgentRunView, ChatAgentTimelineItem, ChatMessage } from '../chatTypes'
import type { ChatGuidanceTimelineItem } from '../chatTypes'
import { getUniqueWebSearchSources } from '../agentWebSearch'
import { getFinalTimeline } from '../../agentRun/messageTimeline'
import { ChatMarkdown } from './ChatMarkdown'
import { type HumanInteractionTimelineController } from '../../humanInteraction/HumanInteractionTimelineEntry'
import { humanInteractionRequestsForMessage } from '../../humanInteraction/humanInteractionPresentation'
import {
  formatElapsedDuration,
  getAssistantFinalContent,
  getRunInterruptionReason,
  groupTimelineItems,
  hasCollapsibleTimelineContent,
  hasDisplayableContent,
  hasRecentFileChangeActivity,
  hasTrustedAnchoredCollaborationActivity,
  isContentFullyRepresentedByTimeline,
  isRunSettled,
  isWaitingForCommandCompletion,
  shouldShowThinkingActivity,
  type RenderableTimelineItem
} from './chatMessageItemUtils'
import { EditSummaryCard } from './EditSummaryCard'
import { OfficeArtifactsCard } from './OfficeArtifactsCard'
import { ImageGenerationArtifactsCard } from './ImageGenerationArtifactsCard'
import { hostImageArtifactResolver } from '../../imageGeneration/artifacts/hostImageArtifactResolver'
import { AssistantSources } from './toolActivities/WebSearchSources'
import {
  CollaborationTimelineActivityList,
  normalizeCollaborationTimelineActivities,
  type CollaborationTimelineActivity
} from '../../agentCollaboration/CollaborationTimelineActivity'
import type { WorkspaceReferenceTarget } from '../workspaceMentions'
import { AgentTimelineItemView, GuidanceTimelineItemView } from './AgentTimelineItemView'
import { projectBasicToolTimeline, type BasicToolTimelineBlock } from './basicToolTimeline'
import { BasicToolActivityHeader } from './toolActivities/BasicToolActivityHeader'
import { BasicToolActivityItem } from './toolActivities/BasicToolActivityItem'
import { useBasicToolExpansion } from './useBasicToolExpansion'

const ACTIVE_STREAMING_GRACE_MS = 1200

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

function AgentRunElapsedHeader({
  canToggle,
  collapsed,
  createdAt,
  isThinking,
  onToggle,
  run
}: {
  canToggle: boolean
  collapsed: boolean
  createdAt: number
  isThinking: boolean
  onToggle: () => void
  run: ChatAgentRunView
}) {
  const { t } = useFrontendConfig()
  const now = useRunClock(run.runId, !isRunSettled(run))
  const label = isThinking
    ? t(
        run.modelActivity?.activity === 'reasoning'
          ? 'agent.thinking'
          : 'agent.waitingForNextAction'
      )
    : formatTranslation(t, run.status === 'cancelled' ? 'agent.stoppedAfter' : 'agent.processed', {
        duration: formatElapsedDuration((run.completedAt ?? now) - (run.startedAt ?? createdAt))
      })
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

// Only the two time-dependent leaves subscribe. A clock tick must not re-render timeline Markdown.
function useRunClock(runId: ChatAgentRunView['runId'], active: boolean) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!active) return undefined
    const timerId = window.setInterval(() => setNow(Date.now()), 500)
    return () => window.clearInterval(timerId)
  }, [active, runId])
  return now
}

function AgentThinkingActivity({
  canShowThinking,
  retryLabel,
  run,
  waitingForCommandCompletion
}: {
  canShowThinking: boolean
  retryLabel: string | null
  run: ChatAgentRunView
  waitingForCommandCompletion: boolean
}) {
  const { t } = useFrontendConfig()
  const now = useRunClock(run.runId, !isRunSettled(run))
  const isStreamingAssistantText =
    !isRunSettled(run) &&
    Boolean(run.lastResponseAt) &&
    now - (run.lastResponseAt ?? 0) <= ACTIVE_STREAMING_GRACE_MS
  if (
    !retryLabel &&
    (!canShowThinking ||
      isStreamingAssistantText ||
      hasRecentFileChangeActivity(run, now) ||
      (run.finalAnswerReady &&
        !waitingForCommandCompletion &&
        run.modelActivity?.activity !== 'reasoning'))
  ) {
    return null
  }

  return (
    <div className="agent-thinking">
      <span className="agent-running-text">
        {retryLabel ??
          t(
            waitingForCommandCompletion
              ? 'agent.command.waitingForCompletion'
              : run.modelActivity?.activity === 'reasoning'
                ? 'agent.thinking'
                : 'agent.waitingForNextAction'
          )}
      </span>
    </div>
  )
}

export function AgentRunView({
  collaborationTimelineActivities = [],
  collaborationTreeAgentIds,
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
  collaborationTreeAgentIds?: readonly string[]
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
  const hasWorkflowDeliveries = timeline.some((item) => item.type === 'workflow_delivery')
  const preserveInputSequence = hasInteractionEntries || hasWorkflowDeliveries
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
          (collaborationTreeAgentIds === undefined ||
            (collaborationTreeAgentIds.includes(activity.agentId) &&
              collaborationTreeAgentIds.includes(activity.ownerAgentId))) &&
          activity.ownerConversationId === conversationId &&
          activity.anchorMessageId === message.id &&
          activity.traceBoundarySequence !== null
      ),
    [conversationId, collaborationTreeAgentIds, effectiveCollaborationActivities, message.id]
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
      | Extract<BasicToolTimelineBlock, { kind: 'basic_tools' }>
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
      for (const block of projectBasicToolTimeline(run, timeline.slice(segmentStart, end))) {
        blocks.push(block.kind === 'legacy' ? { kind: 'timeline', items: block.items } : block)
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
  const basicToolGroups = useMemo(
    () => displayTimelineBlocks.filter((block) => block.kind === 'basic_tools'),
    [displayTimelineBlocks]
  )
  const basicToolExpansion = useBasicToolExpansion(run?.runId ?? null, basicToolGroups)
  const waitingForCommandCompletion = Boolean(run && isWaitingForCommandCompletion(run))
  const hasTimeline = displayTimeline.length > 0 || basicToolGroups.length > 0
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
  const llmRetryLabel = useMemo(() => {
    const retry = run?.llmRetry
    if (!retry) return null
    return formatTranslation(t, 'agent.llmRetry.reconnecting', {
      attempt: String(Math.max(1, retry.attempt - 1)),
      maxAttempts: String(Math.max(1, retry.maxAttempts - 1))
    })
  }, [run?.llmRetry, t])

  const headerIsThinking = Boolean(
    run &&
    !run.finalAnswerReady &&
    !llmRetryLabel &&
    !run.firstResponseAt &&
    !hasCollapsibleTimelineContent(run, timeline) &&
    !waitingForCommandCompletion &&
    !runIsSettled
  )
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
    ? (timelineCollapsedOverride ??
      message.uiState?.timelineCollapsed ??
      !(hasGuidance || hasWorkflowDeliveries))
    : false
  // Keep incoming mail in trace order while collapsed. Only open human interactions retain their
  // surrounding narration; mail must not make the entire run's intermediate text stay visible.
  const showTimeline =
    hasTimeline && (!(canToggleTimeline && timelineCollapsed) || preserveInputSequence)
  const showTimelineNarration = !(canToggleTimeline && timelineCollapsed) || hasInteractionEntries
  // A terminal failure arrives as both an Error event and Done.content. Keep both durable
  // projections, but use the visible error entry instead of rendering its text again as an
  // answer. Collapsing the timeline still leaves the failure explanation visible.
  const finalAnswerIsTerminalError =
    run.status === 'failed' && run.error?.trim() === finalAnswerContent.trim()
  const terminalErrorVisibleInTimeline =
    finalAnswerIsTerminalError &&
    showTimeline &&
    !(canToggleTimeline && timelineCollapsed) &&
    displayTimeline.some(
      (item) => item.type === 'error' && item.message.trim() === finalAnswerContent.trim()
    )
  const showFinalContent =
    hasDisplayableContent(finalAnswerContent) &&
    (runIsSettled || !hasTimeline) &&
    !(showTimeline && showTimelineNarration && finalAnswerRepresentedByTimeline) &&
    !terminalErrorVisibleInTimeline
  const canShowThinkingActivity =
    !(canToggleTimeline && timelineCollapsed) &&
    !headerIsThinking &&
    (waitingForCommandCompletion || shouldShowThinkingActivity(run, timeline))
  const interruptionReason = getRunInterruptionReason(run)
  const webSearchSources = getUniqueWebSearchSources(run)

  return (
    <div className="agent-run">
      <AgentRunElapsedHeader
        canToggle={canToggleTimeline}
        collapsed={timelineCollapsed}
        createdAt={message.createdAt}
        isThinking={headerIsThinking}
        run={run}
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
      {canToggleTimeline && timelineCollapsed && !preserveInputSequence
        ? guidanceItems.map((item) => (
            <GuidanceTimelineItemView
              assistantMessageId={message.id}
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
        if (block.kind === 'basic_tools') {
          const grouped = block.items.length > 1
          const expanded = basicToolExpansion.isExpanded(block.items)
          const hidden = !showTimeline || (canToggleTimeline && timelineCollapsed)
          const leafId = (id: string) => `basic-tool-${message.id}-${id}`
          return [
            grouped && !hidden ? (
              <BasicToolActivityHeader
                controls={block.items.map((item) => leafId(item.id)).join(' ')}
                expanded={expanded}
                items={block.items}
                key={`basic-header:${block.id}`}
                onToggle={() => basicToolExpansion.setExpanded(block.items, !expanded)}
                run={run}
              />
            ) : null,
            ...block.items.map((item, index) => (
              <div
                className="basic-tool-activity__leaf"
                data-grouped={grouped ? 'true' : 'false'}
                data-first={index === 0 ? 'true' : undefined}
                data-last={index === block.items.length - 1 ? 'true' : undefined}
                data-tool-anchor={item.id}
                hidden={hidden || (grouped && !expanded)}
                id={leafId(item.id)}
                key={`basic-leaf:${item.id}`}
              >
                <BasicToolActivityItem
                  assistantMessageId={message.id}
                  conversationId={conversationId}
                  item={item}
                  mode={mode}
                  observerRootConversationId={observerRootConversationId}
                  onExpandedChange={(open) => {
                    basicToolExpansion.setDetailExpanded(item, open)
                  }}
                  onOpenWorkspaceReference={onOpenWorkspaceReference}
                  presentation={grouped ? 'compact' : 'default'}
                  projectId={projectId}
                  run={run}
                />
              </div>
            ))
          ]
        }
        if (!showTimeline) return []
        // Timeline segments are presentation-only placement boundaries. Their start/end changes
        // whenever a later Tool or collaboration event arrives, so they must never own React
        // identity. Keep every semantic item directly under the run with its durable item id.
        const visibleItems =
          canToggleTimeline && timelineCollapsed && preserveInputSequence
            ? block.items.filter(
                (item) =>
                  (item.type === 'message' && showTimelineNarration) ||
                  item.type === 'user_guidance' ||
                  item.type === 'workflow_delivery' ||
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
      <AgentThinkingActivity
        canShowThinking={canShowThinkingActivity}
        retryLabel={llmRetryLabel}
        run={run}
        waitingForCommandCompletion={waitingForCommandCompletion}
      />
      {showTimeline &&
        run.error &&
        !hasTimelineError &&
        !interruptionReason &&
        !(showFinalContent && finalAnswerIsTerminalError) && (
          <div className="agent-run__error">{run.error}</div>
        )}
    </div>
  )
}
