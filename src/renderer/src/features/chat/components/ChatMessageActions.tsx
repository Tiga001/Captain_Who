import { useEffect, useId, useRef, useState } from 'react'
import {
  Check,
  ChevronDown,
  ChevronUp,
  Copy,
  Database,
  LoaderCircle,
  Pencil,
  Split,
  Star
} from 'lucide-react'
import type { AgentUsage } from '@mycopilot/protocol'
import { Tooltip } from '../../../components/overlay/Tooltip'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { copyTextToClipboard, formatMessageTime, getUsageRows } from './chatMessageItemUtils'

const COPIED_INDICATOR_MS = 1300

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

export function ChatMessageActions({
  canEdit = false,
  content,
  favorited = false,
  onEdit,
  onFavoriteChange,
  onContinueInNewTask,
  workflowContextExpanded = false,
  onWorkflowContextToggle,
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
  workflowContextExpanded?: boolean
  onWorkflowContextToggle?: () => void
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
  const workflowContextLabel = language.startsWith('zh')
    ? workflowContextExpanded
      ? '收起工作流上下文'
      : '展开工作流上下文'
    : workflowContextExpanded
      ? 'Collapse workflow context'
      : 'Expand workflow context'

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
      {onWorkflowContextToggle && (
        <Tooltip content={workflowContextLabel}>
          <button
            type="button"
            aria-label={workflowContextLabel}
            aria-expanded={workflowContextExpanded}
            onClick={onWorkflowContextToggle}
          >
            {workflowContextExpanded ? (
              <ChevronUp aria-hidden="true" />
            ) : (
              <ChevronDown aria-hidden="true" />
            )}
          </button>
        </Tooltip>
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
