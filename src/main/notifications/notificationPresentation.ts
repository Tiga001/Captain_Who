import type { NotificationBatch, NotificationKind, NotificationListItem } from '@mycopilot/protocol'
import {
  getTranslation,
  type AppLanguage,
  type TranslationKey
} from '../../shared/i18n/languageRegistry'

export interface SystemNotificationPresentation {
  title: string
  body: string
}

type SystemNotificationTranslationKey = Extract<TranslationKey, `notification.system.${string}`>

interface TaskPresentationKeys {
  readonly title: SystemNotificationTranslationKey
  readonly withSubject: SystemNotificationTranslationKey
  readonly hidden: SystemNotificationTranslationKey | null
}

const TASK_UPDATE_KEYS = {
  title: 'notification.system.taskUpdateTitle',
  withSubject: 'notification.system.taskUpdateWithSubject',
  hidden: null
} as const satisfies TaskPresentationKeys

const TASK_PRESENTATION_KEYS = {
  task_completed: {
    title: 'notification.system.taskCompletedTitle',
    withSubject: 'notification.system.taskCompletedWithSubject',
    hidden: 'notification.system.taskCompletedHidden'
  },
  task_failed: {
    title: 'notification.system.taskFailedTitle',
    withSubject: 'notification.system.taskFailedWithSubject',
    hidden: 'notification.system.taskFailedHidden'
  },
  task_cancelled: {
    title: 'notification.system.taskCancelledTitle',
    withSubject: 'notification.system.taskCancelledWithSubject',
    hidden: 'notification.system.taskCancelledHidden'
  },
  approval_required: {
    title: 'notification.system.approvalRequiredTitle',
    withSubject: 'notification.system.approvalRequiredWithSubject',
    hidden: 'notification.system.approvalRequiredHidden'
  },
  automation_completed: TASK_UPDATE_KEYS,
  automation_failed: TASK_UPDATE_KEYS,
  automation_cancelled: TASK_UPDATE_KEYS,
  automation_important_update: TASK_UPDATE_KEYS,
  automation_configuration_blocked: TASK_UPDATE_KEYS
} as const satisfies Record<NotificationKind, TaskPresentationKeys>

const AUTOMATION_TITLE_KEYS = {
  task_completed: 'notification.system.automationUpdateTitle',
  task_failed: 'notification.system.automationUpdateTitle',
  task_cancelled: 'notification.system.automationUpdateTitle',
  approval_required: 'notification.system.automationApprovalRequiredTitle',
  automation_completed: 'notification.system.automationCompletedTitle',
  automation_failed: 'notification.system.automationFailedTitle',
  automation_cancelled: 'notification.system.automationCancelledTitle',
  automation_important_update: 'notification.system.automationImportantUpdateTitle',
  automation_configuration_blocked: 'notification.system.automationConfigurationBlockedTitle'
} as const satisfies Record<NotificationKind, SystemNotificationTranslationKey>

const BATCH_PART_KEYS = [
  ['approvalRequired', 'notification.system.batchApprovalRequired'],
  ['configurationBlocked', 'notification.system.batchConfigurationBlocked'],
  ['failed', 'notification.system.batchFailed'],
  ['importantUpdate', 'notification.system.batchImportantUpdate'],
  ['cancelled', 'notification.system.batchCancelled'],
  ['completed', 'notification.system.batchCompleted']
] as const satisfies ReadonlyArray<
  readonly [keyof NotificationBatch['counts'], SystemNotificationTranslationKey]
>

/**
 * Native notifications intentionally contain only trusted, bounded subjects and localized copy.
 * Results, model summaries, raw errors, commands and approval payloads never enter this layer.
 */
export function formatNotificationBatch(
  batch: NotificationBatch,
  language: AppLanguage
): SystemNotificationPresentation {
  if (batch.itemCount === 1 && batch.items.length === 1) {
    return formatSingleItem(batch.items[0], batch.showTaskContent, language)
  }
  return formatBatchCounts(batch, language)
}

function formatSingleItem(
  item: NotificationListItem,
  showTaskContent: boolean,
  language: AppLanguage
): SystemNotificationPresentation {
  if (item.sourceKind === 'automation') {
    return {
      title: formatAutomationTitle(item, showTaskContent, language),
      body: ''
    }
  }

  const keys = TASK_PRESENTATION_KEYS[item.kind]
  const subject = showTaskContent ? localizedSubject(item, language) : null
  return {
    title: systemNotificationText(language, keys.title),
    body: subject
      ? systemNotificationText(language, keys.withSubject, { subject })
      : keys.hidden
        ? systemNotificationText(language, keys.hidden)
        : ''
  }
}

function localizedSubject(item: NotificationListItem, language: AppLanguage): string | null {
  if (item.subjectKind === 'attachment_task') {
    return getTranslation(language, 'notification.attachmentTask')
  }
  return safeSubject(item.subjectText)
}

function formatAutomationTitle(
  item: NotificationListItem,
  showTaskContent: boolean,
  language: AppLanguage
): string {
  const subject = showTaskContent
    ? (safeSubject(item.subjectText) ??
      systemNotificationText(language, 'notification.system.scheduledTask'))
    : systemNotificationText(language, 'notification.system.scheduledTask')
  return systemNotificationText(language, AUTOMATION_TITLE_KEYS[item.kind], { subject })
}

function formatBatchCounts(
  batch: NotificationBatch,
  language: AppLanguage
): SystemNotificationPresentation {
  const parts = BATCH_PART_KEYS.flatMap(([countKey, translationKey]) => {
    const count = batch.counts[countKey]
    return count > 0
      ? [systemNotificationText(language, translationKey, { count: String(count) })]
      : []
  })
  const titleKey =
    batch.counts.completed === batch.itemCount
      ? 'notification.system.batchCompletedTitle'
      : 'notification.system.batchUpdatesTitle'

  return {
    title: systemNotificationText(language, titleKey, { count: String(batch.itemCount) }),
    body: parts.join(' · ')
  }
}

function systemNotificationText(
  language: AppLanguage,
  key: SystemNotificationTranslationKey,
  replacements: Readonly<Record<string, string>> = {}
): string {
  return getTranslation(language, key).replace(/\{([^{}]+)\}/gu, (placeholder, name: string) =>
    Object.prototype.hasOwnProperty.call(replacements, name) ? replacements[name] : placeholder
  )
}

function safeSubject(value: string): string | null {
  const normalized = value.replace(/\s+/gu, ' ').trim()
  if (normalized.length === 0) return null
  const segments = [
    ...new Intl.Segmenter(undefined, { granularity: 'grapheme' }).segment(normalized)
  ]
  if (segments.length <= 48) return normalized
  return `${segments
    .slice(0, 48)
    .map((segment) => segment.segment)
    .join('')}…`
}
