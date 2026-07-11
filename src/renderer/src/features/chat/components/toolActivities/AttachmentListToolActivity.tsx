import { Files } from 'lucide-react'
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../../../config/translationFormat'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

interface AttachmentListToolActivityProps {
  cancelled?: boolean
  call: AgentToolCall
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

interface ListedAttachment {
  id: string
  messageId: string
  name: string
}

type AttachmentListStatus = 'running' | 'completed' | 'failed' | 'cancelled'

const CONVERSATION_STATUS_LABELS: Record<AttachmentListStatus, TranslationKey> = {
  running: 'agent.attachments.running',
  completed: 'agent.attachments.completed',
  failed: 'agent.attachments.failed',
  cancelled: 'agent.attachments.cancelled'
}

const PROJECT_STATUS_LABELS: Record<AttachmentListStatus, TranslationKey> = {
  running: 'agent.attachments.project.running',
  completed: 'agent.attachments.project.completed',
  failed: 'agent.attachments.project.failed',
  cancelled: 'agent.attachments.project.cancelled'
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}

function stringValue(value: unknown) {
  return typeof value === 'string' ? value.trim() : ''
}

function numberValue(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}

function getStatus(
  cancelled: boolean,
  result: AgentToolResult | undefined,
  settledStatus?: SettledToolStatus
): AttachmentListStatus {
  if (cancelled && !result) return 'cancelled'
  if (result?.ok === false) return 'failed'
  if (result) return 'completed'
  if (settledStatus) return settledStatus
  return 'running'
}

function getAttachments(result: AgentToolResult | undefined): ListedAttachment[] {
  if (!isRecord(result?.result) || !Array.isArray(result.result.attachments)) return []

  return result.result.attachments.reduce<ListedAttachment[]>((items, item) => {
    if (!isRecord(item)) return items
    const id = stringValue(item.id)
    const messageId = stringValue(item.messageId)
    const name = stringValue(item.name) || stringValue(item.readPath) || id
    if (!id || !name) return items
    return [...items, { id, messageId, name }]
  }, [])
}

function getAttachmentCount(result: AgentToolResult | undefined, attachments: ListedAttachment[]) {
  if (!isRecord(result?.result)) return attachments.length
  return numberValue(result.result.total) ?? attachments.length
}

function getStatusLabel(
  t: Translate,
  isProjectScope: boolean,
  status: AttachmentListStatus,
  count: number
) {
  const labelKey = isProjectScope
    ? PROJECT_STATUS_LABELS[status]
    : CONVERSATION_STATUS_LABELS[status]
  return status === 'completed' ? formatTranslation(t, labelKey, { count }) : t(labelKey)
}

function escapeAttributeValue(value: string) {
  return value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')
}

function prefersReducedMotion() {
  return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false
}

function scrollToConversationAttachment(attachment: ListedAttachment) {
  const attachmentId = escapeAttributeValue(attachment.id)
  const messageId = escapeAttributeValue(attachment.messageId)
  const selectors = [
    messageId
      ? `[data-chat-attachment-id="${attachmentId}"][data-chat-attachment-message-id="${messageId}"]`
      : '',
    `[data-chat-attachment-id="${attachmentId}"]`
  ].filter(Boolean)
  const target = selectors
    .map((selector) => document.querySelector<HTMLElement>(selector))
    .find(Boolean)

  if (!target) return

  target.scrollIntoView({
    block: 'center',
    inline: 'nearest',
    behavior: prefersReducedMotion() ? 'auto' : 'smooth'
  })
  target.classList.add('chat-message-attachment--jump-target')
  window.setTimeout(() => {
    target.classList.remove('chat-message-attachment--jump-target')
  }, 1500)
}

export function AttachmentListToolActivity({
  cancelled = false,
  call,
  result,
  settledStatus
}: AttachmentListToolActivityProps) {
  const { t } = useFrontendConfig()
  const isProjectScope = call.tool === 'attachments_list_project'
  const status = getStatus(cancelled, result, settledStatus)
  const attachments = getAttachments(result)
  const count = getAttachmentCount(result, attachments)
  const hasDetails = status !== 'running'
  const error = result?.error
  const label = getStatusLabel(t, isProjectScope, status, count)

  return (
    <AgentActivityDisclosure
      className="agent-activity--attachment-list"
      hasDetails={hasDetails}
      icon={Files}
      isPending={status === 'running'}
      label={label}
    >
      {hasDetails && (
        <div className="agent-activity__details attachment-list-activity__details">
          {error ? <p className="attachment-list-activity__error">{error}</p> : null}
          {!error && attachments.length === 0 ? <p>{t('agent.attachments.empty')}</p> : null}
          {!error && attachments.length > 0 ? (
            <div className="attachment-list-activity__items">
              {attachments.map((attachment) =>
                isProjectScope ? (
                  <div
                    className="attachment-list-activity__item"
                    key={`${attachment.messageId}:${attachment.id}`}
                    title={attachment.name}
                  >
                    {formatTranslation(t, 'agent.attachments.item', { name: attachment.name })}
                  </div>
                ) : (
                  <button
                    className="attachment-list-activity__item"
                    key={`${attachment.messageId}:${attachment.id}`}
                    onClick={() => scrollToConversationAttachment(attachment)}
                    title={attachment.name}
                    type="button"
                  >
                    {formatTranslation(t, 'agent.attachments.item', { name: attachment.name })}
                  </button>
                )
              )}
            </div>
          ) : null}
        </div>
      )}
    </AgentActivityDisclosure>
  )
}
