import { useEffect, useId, useMemo, useRef, useState } from 'react'
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
import type { ChatAgentRunView, ChatMessage } from '../chatTypes'
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
import { McpToolActivity } from './toolActivities/McpToolActivity'
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

const ACTIVE_STREAMING_GRACE_MS = 1200
const COPIED_INDICATOR_MS = 1300

interface ChatMessageItemProps {
  editSelectedModelAvailable?: boolean
  editSelectedModelSupportsImage?: boolean
  isLastAssistantMessage?: boolean
  message: ChatMessage
  projectId?: string | null
  onApprove?: (
    messageId: string,
    action: AgentProposedAction,
    options?: { rememberForRun?: boolean }
  ) => void
  onCancel?: (messageId: string, action: AgentProposedAction) => void
  onEditSubmit?: (messageId: string, content: string) => void | Promise<void>
  onContinueInNewTask?: (messageId: string) => void | Promise<void>
  onReject?: (messageId: string, action: AgentProposedAction, message?: string) => void
  onReviewLastTurn?: (filePath?: string) => void
  onUiStateChange?: (messageId: string, uiState: ChatMessage['uiState']) => void
  showTokenUsageDetails: boolean
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

function AgentThinkingActivity() {
  const { t } = useFrontendConfig()

  return (
    <div className="agent-thinking">
      <span className="agent-running-text">{t('agent.thinking')}</span>
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
  item,
  projectId,
  run
}: {
  item: RenderableTimelineItem
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
    return <ReadToolActivityGroup items={items} projectId={projectId} />
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

  if (item.type === 'apply_patch_group') {
    const items = getApplyPatchGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <ApplyPatchToolActivityGroup items={items} projectId={projectId} />
  }

  if (item.type === 'write_file_group') {
    const items = getWriteFileGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <FileWriteToolActivityGroup items={items} />
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
        diff={diff}
        mcpInvocation={mcpInvocation}
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
  message,
  onReviewLastTurn,
  onUiStateChange,
  projectId,
  turnDiffSummary
}: {
  message: ChatMessage
  onReviewLastTurn?: (filePath?: string) => void
  onUiStateChange?: (messageId: string, uiState: ChatMessage['uiState']) => void
  projectId?: string | null
  turnDiffSummary?: GitTurnDiffSummary
}) {
  const { t } = useFrontendConfig()
  const run = message.agentRun
  const timeline = useMemo(() => run?.timeline ?? [], [run?.timeline])
  const finalAnswerContent = getAssistantFinalContent(message)
  const timelineWithoutFinalAnswer = useMemo(() => {
    if (!run || !isRunSettled(run) || !hasDisplayableContent(finalAnswerContent)) return timeline

    const normalizedFinalAnswer = finalAnswerContent.trim()
    const finalMessageIndex = timeline.findLastIndex(
      (item) => item.type === 'message' && item.content.trim() === normalizedFinalAnswer
    )
    if (finalMessageIndex < 0) return timeline
    return timeline.filter((_, index) => index !== finalMessageIndex)
  }, [finalAnswerContent, run, timeline])
  const finalAnswerRepresentedByTimeline = useMemo(
    () => isContentFullyRepresentedByTimeline(finalAnswerContent, timelineWithoutFinalAnswer),
    [finalAnswerContent, timelineWithoutFinalAnswer]
  )
  const displayTimeline = useMemo(
    () => (run ? groupTimelineItems(run, timelineWithoutFinalAnswer) : []),
    [run, timelineWithoutFinalAnswer]
  )
  const runId = run?.runId
  const runIsSettled = !run || isRunSettled(run)
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

  const headerState = useMemo(() => {
    const hasFirstResponse = Boolean(run?.firstResponseAt)
    const hasVisibleToolStatus = Boolean(run && hasCollapsibleTimelineContent(run, timeline))

    if (run && !hasFirstResponse && !hasVisibleToolStatus && !isRunSettled(run)) {
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
  }, [message.createdAt, now, run, t, timeline])
  if (!run) {
    return hasDisplayableContent(message.content) ? (
      <ChatMarkdown className="chat-agent-text" content={message.content} />
    ) : null
  }

  const hasGuidance = timeline.some((item) => item.type === 'user_guidance')
  const timelineCollapsed = canToggleTimeline
    ? (message.uiState?.timelineCollapsed ?? !hasGuidance)
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
    shouldShowThinkingActivity(run, timeline)
  const showTokenLimitNotice = isRunSettled(run) && isTokenLimitFinishReason(run.finishReason)
  const webSearchSources = getUniqueWebSearchSources(run)

  return (
    <div className="agent-run">
      <AgentRunElapsedHeader
        canToggle={canToggleTimeline}
        collapsed={timelineCollapsed}
        isThinking={headerState.isThinking}
        label={headerState.label}
        onToggle={() =>
          onUiStateChange?.(message.id, {
            ...message.uiState,
            timelineCollapsed: !timelineCollapsed
          })
        }
      />
      {showTimeline &&
        displayTimeline.map((item) => (
          <AgentTimelineItemView item={item} key={item.id} projectId={projectId} run={run} />
        ))}
      {showFinalContent && (
        <ChatMarkdown className="chat-agent-text" content={finalAnswerContent} />
      )}
      {isRunSettled(run) && (
        <ImageGenerationArtifactsCard resolver={hostImageArtifactResolver} run={run} />
      )}
      {isRunSettled(run) && <OfficeArtifactsCard projectId={projectId} run={run} />}
      {isRunSettled(run) && turnDiffSummary && (
        <EditSummaryCard onReview={onReviewLastTurn} summary={turnDiffSummary} />
      )}
      {isRunSettled(run) && <AssistantSources sources={webSearchSources} />}
      {showTokenLimitNotice && (
        <div className="agent-run__notice" role="status">
          <AlertTriangle aria-hidden="true" />
          <span>{t('agent.tokenLimitNotice')}</span>
        </div>
      )}
      {showThinkingActivity && <AgentThinkingActivity />}
      {showTimeline && run.error && !hasTimelineError && (
        <div className="agent-run__error">{run.error}</div>
      )}
    </div>
  )
}

function MessageContent({
  message,
  onReviewLastTurn,
  onUiStateChange,
  projectId,
  turnDiffSummary
}: ChatMessageItemProps) {
  if (message.role === 'assistant') {
    return (
      <AgentRunView
        message={message}
        onReviewLastTurn={onReviewLastTurn}
        onUiStateChange={onUiStateChange}
        projectId={projectId}
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

export function ChatMessageItem({
  editSelectedModelAvailable = true,
  editSelectedModelSupportsImage = true,
  isLastAssistantMessage = false,
  message,
  onApprove,
  onCancel,
  onEditSubmit,
  onContinueInNewTask,
  onReject,
  onReviewLastTurn,
  onUiStateChange,
  projectId,
  showTokenUsageDetails,
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
            message={message}
            onApprove={onApprove}
            onCancel={onCancel}
            onReject={onReject}
            onReviewLastTurn={onReviewLastTurn}
            onUiStateChange={onUiStateChange}
            projectId={projectId}
            showTokenUsageDetails={showTokenUsageDetails}
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
