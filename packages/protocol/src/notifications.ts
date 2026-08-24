import {
  expectArray,
  expectBoolean,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectSchemaVersion,
  expectString,
  invalidProtocolValue
} from './skills/validation'

export const NOTIFICATION_SCHEMA_VERSION = 1 as const

export const NOTIFICATION_BATCHES_CLAIM_METHOD = 'notifications.claim'
export const NOTIFICATION_BATCH_VALIDATE_METHOD = 'notifications.validate'
export const NOTIFICATION_BATCH_ACKNOWLEDGE_METHOD = 'notifications.acknowledge'
export const NOTIFICATION_BATCH_RELEASE_METHOD = 'notifications.release'
export const NOTIFICATION_BATCH_SUPPRESS_METHOD = 'notifications.suppress'
export const NOTIFICATION_LIST_METHOD = 'notifications.list'
export const NOTIFICATION_SUMMARY_METHOD = 'notifications.summary'
export const NOTIFICATION_MARK_SEEN_METHOD = 'notifications.markSeen'
export const NOTIFICATION_SETTINGS_GET_METHOD = 'notifications.settings.get'
export const NOTIFICATION_SETTINGS_UPDATE_METHOD = 'notifications.settings.update'
export const NOTIFICATION_EVENT_NOTIFICATION_METHOD = 'notification.event'
export const NOTIFICATION_RESYNC_NOTIFICATION_METHOD = 'notification.resync'

export type NotificationKind =
  | 'task_completed'
  | 'task_failed'
  | 'task_cancelled'
  | 'approval_required'
  | 'automation_completed'
  | 'automation_failed'
  | 'automation_cancelled'
  | 'automation_important_update'
  | 'automation_configuration_blocked'
export type NotificationSourceKind = 'human_root' | 'automation'
export type NotificationSubjectKind = 'prompt_excerpt' | 'automation_title' | 'attachment_task'
export type NotificationPriority =
  | 'completed'
  | 'cancelled'
  | 'important_update'
  | 'failed'
  | 'configuration_blocked'
  | 'approval_required'
export type NotificationBatchStatus =
  'collecting' | 'pending' | 'claimed' | 'displayed' | 'sealed' | 'suppressed'
export type NotificationSoundLevel = 'none' | 'initial' | 'upgrade'
export type NotificationDeliveryDisposition =
  | 'delivered'
  | 'suppressed_foreground'
  | 'suppressed_stale'
  | 'suppressed_deleted'
  | 'suppressed_resolved'
  | 'suppressed_disabled'

export interface NotificationCounts {
  completed: number
  failed: number
  cancelled: number
  approvalRequired: number
  importantUpdate: number
  configurationBlocked: number
}

export interface NotificationListItem {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  eventId: string
  batchId: string | null
  kind: NotificationKind
  sourceKind: NotificationSourceKind
  sourceId: string
  runId: string | null
  automationId: string | null
  conversationId: string | null
  userMessageId: string | null
  assistantMessageId: string | null
  approvalActionId: string | null
  subjectKind: NotificationSubjectKind
  subjectText: string
  priority: NotificationPriority
  resourceRevision: number | null
  seenAt: number | null
  resolvedAt: number | null
  occurredAt: number
}

export interface NotificationBatch {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  batchId: string
  revision: number
  status: NotificationBatchStatus
  highestPriority: NotificationPriority
  counts: NotificationCounts
  itemCount: number
  items: NotificationListItem[]
  collectUntil: number
  replaceUntil: number
  notificationsEnabled: boolean
  soundEnabled: boolean
  showTaskContent: boolean
  soundLevelPlayed: NotificationSoundLevel
  deliveredRevision: number | null
  deliveredPriority: NotificationPriority | null
  isUpdate: boolean
  createdAt: number
  updatedAt: number
}

export interface NotificationBatchesClaimInput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  claimToken: string
  leaseDurationMs: number
  limit: number
}
export interface NotificationBatchesClaimOutput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  claimToken: string
  batches: NotificationBatch[]
}
export interface NotificationBatchValidateInput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  batchId: string
  claimToken: string
}
export interface NotificationBatchValidateOutput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  batchId: string
  batch: NotificationBatch | null
}
export interface NotificationBatchAcknowledgeInput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  batchId: string
  claimToken: string
  disposition: NotificationDeliveryDisposition
  nativePriority: NotificationPriority
  soundLevelPlayed: NotificationSoundLevel
  nativeRevision: number
}
export interface NotificationBatchAcknowledgeOutput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  batchId: string
  status: NotificationBatchStatus
  disposition: NotificationDeliveryDisposition
  acknowledgedAt: number
}
export interface NotificationBatchReleaseInput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  batchId: string
  claimToken: string
  retryAt: number
  errorCode: 'native_notification_failed'
}
export interface NotificationBatchReleaseOutput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  batchId: string
  status: 'pending' | 'suppressed'
  retryAt: number
}
export interface NotificationBatchSuppressInput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  batchId: string
  reason: Exclude<NotificationDeliveryDisposition, 'delivered'>
}
export interface NotificationBatchSuppressOutput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  batchId: string
  status: 'suppressed'
  reason: Exclude<NotificationDeliveryDisposition, 'delivered'>
  suppressedAt: number
}

export interface NotificationListInput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  cursor?: string
  limit: number
  unreadOnly?: boolean
  batchId?: string
}
export interface NotificationListOutput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  items: NotificationListItem[]
  nextCursor: string | null
  unreadCount: number
  lastSequence: number
}
export interface NotificationSummaryInput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
}
export interface NotificationSummaryOutput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  unreadCount: number
  unresolvedCount: number
  counts: NotificationCounts
  latestOccurredAt: number | null
  lastSequence: number
}
export type NotificationMarkSeenTarget =
  { kind: 'events'; eventIds: string[] } | { kind: 'batch'; batchId: string } | { kind: 'all' }
export interface NotificationMarkSeenInput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  target: NotificationMarkSeenTarget
}
export interface NotificationMarkSeenOutput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  updatedCount: number
  seenAt: number
  summary: NotificationSummaryOutput
}

export interface NotificationSettings {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  enabled: boolean
  soundEnabled: boolean
  showTaskContent: boolean
  humanCompletedEnabled: boolean
  humanFailedEnabled: boolean
  humanApprovalEnabled: boolean
  humanCancelledEnabled: boolean
  revision: number
  updatedAt: number
}
export type NotificationSettingsValues = Omit<
  NotificationSettings,
  'schemaVersion' | 'revision' | 'updatedAt'
>
export interface NotificationSettingsGetInput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
}
export interface NotificationSettingsGetOutput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  settings: NotificationSettings
}
export interface NotificationSettingsUpdateInput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  expectedRevision: number
  settings: NotificationSettingsValues
}
export interface NotificationSettingsUpdateOutput {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  settings: NotificationSettings
}

export type NotificationChangeKind =
  'created' | 'updated' | 'seen' | 'resolved' | 'settings_updated'
export interface NotificationEvent {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  sequence: number
  eventId: string
  kind: NotificationChangeKind
  notificationId: string | null
  batchId: string | null
  resourceRevision: number | null
  occurredAt: number
}
export interface NotificationResync {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  reason: 'core_started'
  lastSequence: number
  occurredAt: number
}
export interface NotificationOpenRequest {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION
  batchId: string
  eventIds: string[]
  destination:
    | { kind: 'application' }
    | {
        kind: 'conversation'
        conversationId: string
        messageId: string | null
        approvalActionId: string | null
      }
    | { kind: 'automation'; automationId: string; runId: string | null }
}

const ID_MAX = 512
const SUBJECT_MAX = 512
const CURSOR_MAX = 1024
const KINDS = [
  'task_completed',
  'task_failed',
  'task_cancelled',
  'approval_required',
  'automation_completed',
  'automation_failed',
  'automation_cancelled',
  'automation_important_update',
  'automation_configuration_blocked'
] as const
const PRIORITIES = [
  'completed',
  'cancelled',
  'important_update',
  'failed',
  'configuration_blocked',
  'approval_required'
] as const
const STATUSES = ['collecting', 'pending', 'claimed', 'displayed', 'sealed', 'suppressed'] as const
const DISPOSITIONS = [
  'delivered',
  'suppressed_foreground',
  'suppressed_stale',
  'suppressed_deleted',
  'suppressed_resolved',
  'suppressed_disabled'
] as const

function boundedString(value: unknown, context: string, max = ID_MAX): string {
  const string = expectString(value, context)
  if (string.length < 1 || string.length > max)
    throw invalidProtocolValue(context, `expected 1..${max} characters`)
  return string
}
function nullableString(value: unknown, context: string): string | null {
  return value === null ? null : boundedString(value, context)
}
function nullableInteger(value: unknown, context: string): number | null {
  return value === null ? null : expectSafeInteger(value, context, 0)
}
function parseCounts(value: unknown, context: string): NotificationCounts {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'completed',
      'failed',
      'cancelled',
      'approvalRequired',
      'importantUpdate',
      'configurationBlocked'
    ],
    context
  )
  return {
    completed: expectSafeInteger(record.completed, `${context}.completed`, 0),
    failed: expectSafeInteger(record.failed, `${context}.failed`, 0),
    cancelled: expectSafeInteger(record.cancelled, `${context}.cancelled`, 0),
    approvalRequired: expectSafeInteger(record.approvalRequired, `${context}.approvalRequired`, 0),
    importantUpdate: expectSafeInteger(record.importantUpdate, `${context}.importantUpdate`, 0),
    configurationBlocked: expectSafeInteger(
      record.configurationBlocked,
      `${context}.configurationBlocked`,
      0
    )
  }
}
function parseSchemaOnly(value: unknown, context: string): void {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion'], context)
  expectSchemaVersion(record, NOTIFICATION_SCHEMA_VERSION, context)
}

export function parseNotificationListItem(value: unknown): NotificationListItem {
  const c = 'Notification list item'
  const r = expectRecord(value, c)
  expectOnlyKeys(
    r,
    [
      'schemaVersion',
      'eventId',
      'batchId',
      'kind',
      'sourceKind',
      'sourceId',
      'runId',
      'automationId',
      'conversationId',
      'userMessageId',
      'assistantMessageId',
      'approvalActionId',
      'subjectKind',
      'subjectText',
      'priority',
      'resourceRevision',
      'seenAt',
      'resolvedAt',
      'occurredAt'
    ],
    c
  )
  expectSchemaVersion(r, NOTIFICATION_SCHEMA_VERSION, c)
  return {
    schemaVersion: NOTIFICATION_SCHEMA_VERSION,
    eventId: boundedString(r.eventId, `${c}.eventId`),
    batchId: nullableString(r.batchId, `${c}.batchId`),
    kind: expectEnum(r.kind, KINDS, `${c}.kind`),
    sourceKind: expectEnum(r.sourceKind, ['human_root', 'automation'] as const, `${c}.sourceKind`),
    sourceId: boundedString(r.sourceId, `${c}.sourceId`),
    runId: nullableString(r.runId, `${c}.runId`),
    automationId: nullableString(r.automationId, `${c}.automationId`),
    conversationId: nullableString(r.conversationId, `${c}.conversationId`),
    userMessageId: nullableString(r.userMessageId, `${c}.userMessageId`),
    assistantMessageId: nullableString(r.assistantMessageId, `${c}.assistantMessageId`),
    approvalActionId: nullableString(r.approvalActionId, `${c}.approvalActionId`),
    subjectKind: expectEnum(
      r.subjectKind,
      ['prompt_excerpt', 'automation_title', 'attachment_task'] as const,
      `${c}.subjectKind`
    ),
    subjectText: boundedString(r.subjectText, `${c}.subjectText`, SUBJECT_MAX),
    priority: expectEnum(r.priority, PRIORITIES, `${c}.priority`),
    resourceRevision: nullableInteger(r.resourceRevision, `${c}.resourceRevision`),
    seenAt: nullableInteger(r.seenAt, `${c}.seenAt`),
    resolvedAt: nullableInteger(r.resolvedAt, `${c}.resolvedAt`),
    occurredAt: expectSafeInteger(r.occurredAt, `${c}.occurredAt`, 0)
  }
}

export function parseNotificationBatch(value: unknown): NotificationBatch {
  const c = 'Notification batch'
  const r = expectRecord(value, c)
  expectOnlyKeys(
    r,
    [
      'schemaVersion',
      'batchId',
      'revision',
      'status',
      'highestPriority',
      'counts',
      'itemCount',
      'items',
      'collectUntil',
      'replaceUntil',
      'notificationsEnabled',
      'soundEnabled',
      'showTaskContent',
      'soundLevelPlayed',
      'deliveredRevision',
      'deliveredPriority',
      'isUpdate',
      'createdAt',
      'updatedAt'
    ],
    c
  )
  expectSchemaVersion(r, NOTIFICATION_SCHEMA_VERSION, c)
  const items = expectArray(r.items, `${c}.items`).map(parseNotificationListItem)
  const batchId = boundedString(r.batchId, `${c}.batchId`)
  const counts = parseCounts(r.counts, `${c}.counts`)
  const itemCount = expectSafeInteger(r.itemCount, `${c}.itemCount`, 0)
  if (itemCount !== items.length)
    throw invalidProtocolValue(`${c}.itemCount`, 'must match items.length')
  if (items.some((item) => item.batchId !== batchId))
    throw invalidProtocolValue(`${c}.items`, 'every item must identify this batch')
  if (Object.values(counts).reduce((total, count) => total + count, 0) !== itemCount)
    throw invalidProtocolValue(`${c}.counts`, 'sum must match itemCount')
  return {
    schemaVersion: NOTIFICATION_SCHEMA_VERSION,
    batchId,
    revision: expectSafeInteger(r.revision, `${c}.revision`, 1),
    status: expectEnum(r.status, STATUSES, `${c}.status`),
    highestPriority: expectEnum(r.highestPriority, PRIORITIES, `${c}.highestPriority`),
    counts,
    itemCount,
    items,
    collectUntil: expectSafeInteger(r.collectUntil, `${c}.collectUntil`, 0),
    replaceUntil: expectSafeInteger(r.replaceUntil, `${c}.replaceUntil`, 0),
    notificationsEnabled: expectBoolean(r.notificationsEnabled, `${c}.notificationsEnabled`),
    soundEnabled: expectBoolean(r.soundEnabled, `${c}.soundEnabled`),
    showTaskContent: expectBoolean(r.showTaskContent, `${c}.showTaskContent`),
    soundLevelPlayed: expectEnum(
      r.soundLevelPlayed,
      ['none', 'initial', 'upgrade'] as const,
      `${c}.soundLevelPlayed`
    ),
    deliveredRevision: nullableInteger(r.deliveredRevision, `${c}.deliveredRevision`),
    deliveredPriority:
      r.deliveredPriority === null
        ? null
        : expectEnum(r.deliveredPriority, PRIORITIES, `${c}.deliveredPriority`),
    isUpdate: expectBoolean(r.isUpdate, `${c}.isUpdate`),
    createdAt: expectSafeInteger(r.createdAt, `${c}.createdAt`, 0),
    updatedAt: expectSafeInteger(r.updatedAt, `${c}.updatedAt`, 0)
  }
}

export function parseNotificationBatchesClaimInput(value: unknown): NotificationBatchesClaimInput {
  const c = 'Notification batches claim input',
    r = expectRecord(value, c)
  expectOnlyKeys(r, ['schemaVersion', 'claimToken', 'leaseDurationMs', 'limit'], c)
  expectSchemaVersion(r, 1, c)
  return {
    schemaVersion: 1,
    claimToken: boundedString(r.claimToken, `${c}.claimToken`),
    leaseDurationMs: expectSafeInteger(r.leaseDurationMs, `${c}.leaseDurationMs`, 5000),
    limit: expectSafeInteger(r.limit, `${c}.limit`, 1)
  }
}
export function parseNotificationBatchesClaimOutput(
  value: unknown
): NotificationBatchesClaimOutput {
  const c = 'Notification batches claim output',
    r = expectRecord(value, c)
  expectOnlyKeys(r, ['schemaVersion', 'claimToken', 'batches'], c)
  expectSchemaVersion(r, 1, c)
  const batches = expectArray(r.batches, `${c}.batches`).map(parseNotificationBatch)
  if (batches.some((batch) => batch.status !== 'claimed'))
    throw invalidProtocolValue(`${c}.batches`, 'claimed batches must have claimed status')
  return { schemaVersion: 1, claimToken: boundedString(r.claimToken, `${c}.claimToken`), batches }
}
export function parseNotificationBatchValidateInput(
  value: unknown
): NotificationBatchValidateInput {
  const c = 'Notification batch validate input',
    r = expectRecord(value, c)
  expectOnlyKeys(r, ['schemaVersion', 'batchId', 'claimToken'], c)
  expectSchemaVersion(r, 1, c)
  return {
    schemaVersion: 1,
    batchId: boundedString(r.batchId, `${c}.batchId`),
    claimToken: boundedString(r.claimToken, `${c}.claimToken`)
  }
}
export function parseNotificationBatchValidateOutput(
  value: unknown
): NotificationBatchValidateOutput {
  const c = 'Notification batch validate output',
    r = expectRecord(value, c)
  expectOnlyKeys(r, ['schemaVersion', 'batchId', 'batch'], c)
  expectSchemaVersion(r, 1, c)
  const batch = r.batch === null ? null : parseNotificationBatch(r.batch)
  if (batch !== null && batch.status !== 'claimed')
    throw invalidProtocolValue(`${c}.batch`, 'validated batch must have claimed status')
  return { schemaVersion: 1, batchId: boundedString(r.batchId, `${c}.batchId`), batch }
}
export function parseNotificationBatchAcknowledgeInput(
  value: unknown
): NotificationBatchAcknowledgeInput {
  const c = 'Notification batch acknowledge input',
    r = expectRecord(value, c)
  expectOnlyKeys(
    r,
    [
      'schemaVersion',
      'batchId',
      'claimToken',
      'disposition',
      'nativePriority',
      'soundLevelPlayed',
      'nativeRevision'
    ],
    c
  )
  expectSchemaVersion(r, 1, c)
  return {
    schemaVersion: 1,
    batchId: boundedString(r.batchId, `${c}.batchId`),
    claimToken: boundedString(r.claimToken, `${c}.claimToken`),
    disposition: expectEnum(r.disposition, DISPOSITIONS, `${c}.disposition`),
    nativePriority: expectEnum(r.nativePriority, PRIORITIES, `${c}.nativePriority`),
    soundLevelPlayed: expectEnum(
      r.soundLevelPlayed,
      ['none', 'initial', 'upgrade'] as const,
      `${c}.soundLevelPlayed`
    ),
    nativeRevision: expectSafeInteger(r.nativeRevision, `${c}.nativeRevision`, 1)
  }
}
export function parseNotificationBatchAcknowledgeOutput(
  value: unknown
): NotificationBatchAcknowledgeOutput {
  const c = 'Notification batch acknowledge output',
    r = expectRecord(value, c)
  expectOnlyKeys(r, ['schemaVersion', 'batchId', 'status', 'disposition', 'acknowledgedAt'], c)
  expectSchemaVersion(r, 1, c)
  return {
    schemaVersion: 1,
    batchId: boundedString(r.batchId, `${c}.batchId`),
    status: expectEnum(r.status, STATUSES, `${c}.status`),
    disposition: expectEnum(r.disposition, DISPOSITIONS, `${c}.disposition`),
    acknowledgedAt: expectSafeInteger(r.acknowledgedAt, `${c}.acknowledgedAt`, 0)
  }
}
export function parseNotificationBatchReleaseInput(value: unknown): NotificationBatchReleaseInput {
  const c = 'Notification batch release input',
    r = expectRecord(value, c)
  expectOnlyKeys(r, ['schemaVersion', 'batchId', 'claimToken', 'retryAt', 'errorCode'], c)
  expectSchemaVersion(r, 1, c)
  if (r.errorCode !== 'native_notification_failed')
    throw invalidProtocolValue(`${c}.errorCode`, 'unexpected value')
  return {
    schemaVersion: 1,
    batchId: boundedString(r.batchId, `${c}.batchId`),
    claimToken: boundedString(r.claimToken, `${c}.claimToken`),
    retryAt: expectSafeInteger(r.retryAt, `${c}.retryAt`, 0),
    errorCode: 'native_notification_failed'
  }
}
export function parseNotificationBatchReleaseOutput(
  value: unknown
): NotificationBatchReleaseOutput {
  const c = 'Notification batch release output',
    r = expectRecord(value, c)
  expectOnlyKeys(r, ['schemaVersion', 'batchId', 'status', 'retryAt'], c)
  expectSchemaVersion(r, 1, c)
  const status = expectEnum(r.status, ['pending', 'suppressed'] as const, `${c}.status`)
  return {
    schemaVersion: 1,
    batchId: boundedString(r.batchId, `${c}.batchId`),
    status,
    retryAt: expectSafeInteger(r.retryAt, `${c}.retryAt`, 0)
  }
}
export function parseNotificationBatchSuppressInput(
  value: unknown
): NotificationBatchSuppressInput {
  const c = 'Notification batch suppress input',
    r = expectRecord(value, c)
  expectOnlyKeys(r, ['schemaVersion', 'batchId', 'reason'], c)
  expectSchemaVersion(r, 1, c)
  return {
    schemaVersion: 1,
    batchId: boundedString(r.batchId, `${c}.batchId`),
    reason: expectEnum(
      r.reason,
      DISPOSITIONS.filter((v) => v !== 'delivered') as Exclude<
        NotificationDeliveryDisposition,
        'delivered'
      >[],
      `${c}.reason`
    )
  }
}
export function parseNotificationBatchSuppressOutput(
  value: unknown
): NotificationBatchSuppressOutput {
  const c = 'Notification batch suppress output',
    r = expectRecord(value, c)
  expectOnlyKeys(r, ['schemaVersion', 'batchId', 'status', 'reason', 'suppressedAt'], c)
  expectSchemaVersion(r, 1, c)
  if (r.status !== 'suppressed') throw invalidProtocolValue(`${c}.status`, 'must be suppressed')
  return {
    schemaVersion: 1,
    batchId: boundedString(r.batchId, `${c}.batchId`),
    status: 'suppressed',
    reason: expectEnum(
      r.reason,
      DISPOSITIONS.filter((v) => v !== 'delivered') as Exclude<
        NotificationDeliveryDisposition,
        'delivered'
      >[],
      `${c}.reason`
    ),
    suppressedAt: expectSafeInteger(r.suppressedAt, `${c}.suppressedAt`, 0)
  }
}

export function parseNotificationListInput(value: unknown): NotificationListInput {
  const c = 'Notification list input',
    r = expectRecord(value, c)
  expectOnlyKeys(r, ['schemaVersion', 'cursor', 'limit', 'unreadOnly', 'batchId'], c)
  expectSchemaVersion(r, 1, c)
  const o: NotificationListInput = {
    schemaVersion: 1,
    limit: expectSafeInteger(r.limit, `${c}.limit`, 1)
  }
  if (r.cursor !== undefined) o.cursor = boundedString(r.cursor, `${c}.cursor`, CURSOR_MAX)
  if (r.unreadOnly !== undefined) o.unreadOnly = expectBoolean(r.unreadOnly, `${c}.unreadOnly`)
  if (r.batchId !== undefined) o.batchId = boundedString(r.batchId, `${c}.batchId`)
  return o
}
export function parseNotificationListOutput(value: unknown): NotificationListOutput {
  const c = 'Notification list output',
    r = expectRecord(value, c)
  expectOnlyKeys(r, ['schemaVersion', 'items', 'nextCursor', 'unreadCount', 'lastSequence'], c)
  expectSchemaVersion(r, 1, c)
  return {
    schemaVersion: 1,
    items: expectArray(r.items, `${c}.items`).map(parseNotificationListItem),
    nextCursor:
      r.nextCursor === null ? null : boundedString(r.nextCursor, `${c}.nextCursor`, CURSOR_MAX),
    unreadCount: expectSafeInteger(r.unreadCount, `${c}.unreadCount`, 0),
    lastSequence: expectSafeInteger(r.lastSequence, `${c}.lastSequence`, 0)
  }
}
export function parseNotificationSummaryInput(value: unknown): NotificationSummaryInput {
  parseSchemaOnly(value, 'Notification summary input')
  return { schemaVersion: 1 }
}
export function parseNotificationSummaryOutput(value: unknown): NotificationSummaryOutput {
  const c = 'Notification summary output',
    r = expectRecord(value, c)
  expectOnlyKeys(
    r,
    [
      'schemaVersion',
      'unreadCount',
      'unresolvedCount',
      'counts',
      'latestOccurredAt',
      'lastSequence'
    ],
    c
  )
  expectSchemaVersion(r, 1, c)
  return {
    schemaVersion: 1,
    unreadCount: expectSafeInteger(r.unreadCount, `${c}.unreadCount`, 0),
    unresolvedCount: expectSafeInteger(r.unresolvedCount, `${c}.unresolvedCount`, 0),
    counts: parseCounts(r.counts, `${c}.counts`),
    latestOccurredAt: nullableInteger(r.latestOccurredAt, `${c}.latestOccurredAt`),
    lastSequence: expectSafeInteger(r.lastSequence, `${c}.lastSequence`, 0)
  }
}
export function parseNotificationMarkSeenInput(value: unknown): NotificationMarkSeenInput {
  const c = 'Notification mark seen input',
    r = expectRecord(value, c)
  expectOnlyKeys(r, ['schemaVersion', 'target'], c)
  expectSchemaVersion(r, 1, c)
  const t = expectRecord(r.target, `${c}.target`),
    kind = expectEnum(t.kind, ['events', 'batch', 'all'] as const, `${c}.target.kind`)
  if (kind === 'all') {
    expectOnlyKeys(t, ['kind'], `${c}.target`)
    return { schemaVersion: 1, target: { kind } }
  }
  if (kind === 'batch') {
    expectOnlyKeys(t, ['kind', 'batchId'], `${c}.target`)
    return {
      schemaVersion: 1,
      target: { kind, batchId: boundedString(t.batchId, `${c}.target.batchId`) }
    }
  }
  expectOnlyKeys(t, ['kind', 'eventIds'], `${c}.target`)
  const ids = expectArray(t.eventIds, `${c}.target.eventIds`).map((v, i) =>
    boundedString(v, `${c}.target.eventIds[${i}]`)
  )
  if (ids.length < 1 || ids.length > 100)
    throw invalidProtocolValue(`${c}.target.eventIds`, 'must contain 1..100 ids')
  if (new Set(ids).size !== ids.length)
    throw invalidProtocolValue(`${c}.target.eventIds`, 'must contain unique ids')
  return { schemaVersion: 1, target: { kind, eventIds: ids } }
}
export function parseNotificationMarkSeenOutput(value: unknown): NotificationMarkSeenOutput {
  const c = 'Notification mark seen output',
    r = expectRecord(value, c)
  expectOnlyKeys(r, ['schemaVersion', 'updatedCount', 'seenAt', 'summary'], c)
  expectSchemaVersion(r, 1, c)
  return {
    schemaVersion: 1,
    updatedCount: expectSafeInteger(r.updatedCount, `${c}.updatedCount`, 0),
    seenAt: expectSafeInteger(r.seenAt, `${c}.seenAt`, 0),
    summary: parseNotificationSummaryOutput(r.summary)
  }
}

export function parseNotificationSettings(value: unknown): NotificationSettings {
  const c = 'Notification settings',
    r = expectRecord(value, c)
  expectOnlyKeys(
    r,
    [
      'schemaVersion',
      'enabled',
      'soundEnabled',
      'showTaskContent',
      'humanCompletedEnabled',
      'humanFailedEnabled',
      'humanApprovalEnabled',
      'humanCancelledEnabled',
      'revision',
      'updatedAt'
    ],
    c
  )
  expectSchemaVersion(r, 1, c)
  return {
    schemaVersion: 1,
    enabled: expectBoolean(r.enabled, `${c}.enabled`),
    soundEnabled: expectBoolean(r.soundEnabled, `${c}.soundEnabled`),
    showTaskContent: expectBoolean(r.showTaskContent, `${c}.showTaskContent`),
    humanCompletedEnabled: expectBoolean(r.humanCompletedEnabled, `${c}.humanCompletedEnabled`),
    humanFailedEnabled: expectBoolean(r.humanFailedEnabled, `${c}.humanFailedEnabled`),
    humanApprovalEnabled: expectBoolean(r.humanApprovalEnabled, `${c}.humanApprovalEnabled`),
    humanCancelledEnabled: expectBoolean(r.humanCancelledEnabled, `${c}.humanCancelledEnabled`),
    revision: expectSafeInteger(r.revision, `${c}.revision`, 1),
    updatedAt: expectSafeInteger(r.updatedAt, `${c}.updatedAt`, 0)
  }
}
export function parseNotificationSettingsGetInput(value: unknown): NotificationSettingsGetInput {
  parseSchemaOnly(value, 'Notification settings get input')
  return { schemaVersion: 1 }
}
export function parseNotificationSettingsGetOutput(value: unknown): NotificationSettingsGetOutput {
  const c = 'Notification settings get output',
    r = expectRecord(value, c)
  expectOnlyKeys(r, ['schemaVersion', 'settings'], c)
  expectSchemaVersion(r, 1, c)
  return { schemaVersion: 1, settings: parseNotificationSettings(r.settings) }
}
export function parseNotificationSettingsUpdateInput(
  value: unknown
): NotificationSettingsUpdateInput {
  const c = 'Notification settings update input',
    r = expectRecord(value, c)
  expectOnlyKeys(r, ['schemaVersion', 'expectedRevision', 'settings'], c)
  expectSchemaVersion(r, 1, c)
  const s = expectRecord(r.settings, `${c}.settings`)
  expectOnlyKeys(
    s,
    [
      'enabled',
      'soundEnabled',
      'showTaskContent',
      'humanCompletedEnabled',
      'humanFailedEnabled',
      'humanApprovalEnabled',
      'humanCancelledEnabled'
    ],
    `${c}.settings`
  )
  return {
    schemaVersion: 1,
    expectedRevision: expectSafeInteger(r.expectedRevision, `${c}.expectedRevision`, 1),
    settings: {
      enabled: expectBoolean(s.enabled, `${c}.settings.enabled`),
      soundEnabled: expectBoolean(s.soundEnabled, `${c}.settings.soundEnabled`),
      showTaskContent: expectBoolean(s.showTaskContent, `${c}.settings.showTaskContent`),
      humanCompletedEnabled: expectBoolean(
        s.humanCompletedEnabled,
        `${c}.settings.humanCompletedEnabled`
      ),
      humanFailedEnabled: expectBoolean(s.humanFailedEnabled, `${c}.settings.humanFailedEnabled`),
      humanApprovalEnabled: expectBoolean(
        s.humanApprovalEnabled,
        `${c}.settings.humanApprovalEnabled`
      ),
      humanCancelledEnabled: expectBoolean(
        s.humanCancelledEnabled,
        `${c}.settings.humanCancelledEnabled`
      )
    }
  }
}
export function parseNotificationSettingsUpdateOutput(
  value: unknown
): NotificationSettingsUpdateOutput {
  return parseNotificationSettingsGetOutput(value)
}

export function parseNotificationEvent(value: unknown): NotificationEvent {
  const c = 'Notification event',
    r = expectRecord(value, c)
  expectOnlyKeys(
    r,
    [
      'schemaVersion',
      'sequence',
      'eventId',
      'kind',
      'notificationId',
      'batchId',
      'resourceRevision',
      'occurredAt'
    ],
    c
  )
  expectSchemaVersion(r, 1, c)
  return {
    schemaVersion: 1,
    sequence: expectSafeInteger(r.sequence, `${c}.sequence`, 1),
    eventId: boundedString(r.eventId, `${c}.eventId`),
    kind: expectEnum(
      r.kind,
      ['created', 'updated', 'seen', 'resolved', 'settings_updated'] as const,
      `${c}.kind`
    ),
    notificationId: nullableString(r.notificationId, `${c}.notificationId`),
    batchId: nullableString(r.batchId, `${c}.batchId`),
    resourceRevision: nullableInteger(r.resourceRevision, `${c}.resourceRevision`),
    occurredAt: expectSafeInteger(r.occurredAt, `${c}.occurredAt`, 0)
  }
}
export function parseNotificationResync(value: unknown): NotificationResync {
  const c = 'Notification resync',
    r = expectRecord(value, c)
  expectOnlyKeys(r, ['schemaVersion', 'reason', 'lastSequence', 'occurredAt'], c)
  expectSchemaVersion(r, 1, c)
  if (r.reason !== 'core_started') throw invalidProtocolValue(`${c}.reason`, 'must be core_started')
  return {
    schemaVersion: 1,
    reason: 'core_started',
    lastSequence: expectSafeInteger(r.lastSequence, `${c}.lastSequence`, 0),
    occurredAt: expectSafeInteger(r.occurredAt, `${c}.occurredAt`, 0)
  }
}
export function parseNotificationOpenRequest(value: unknown): NotificationOpenRequest {
  const c = 'Notification open request',
    r = expectRecord(value, c)
  expectOnlyKeys(r, ['schemaVersion', 'batchId', 'eventIds', 'destination'], c)
  expectSchemaVersion(r, 1, c)
  const eventIds = expectArray(r.eventIds, `${c}.eventIds`).map((value, index) =>
    boundedString(value, `${c}.eventIds[${index}]`)
  )
  if (eventIds.length < 1 || eventIds.length > 100)
    throw invalidProtocolValue(`${c}.eventIds`, 'must contain 1..100 ids')
  if (new Set(eventIds).size !== eventIds.length)
    throw invalidProtocolValue(`${c}.eventIds`, 'must contain unique ids')
  const d = expectRecord(r.destination, `${c}.destination`),
    kind = expectEnum(
      d.kind,
      ['application', 'conversation', 'automation'] as const,
      `${c}.destination.kind`
    ),
    base = {
      schemaVersion: 1 as const,
      batchId: boundedString(r.batchId, `${c}.batchId`),
      eventIds
    }
  if (kind === 'application') {
    expectOnlyKeys(d, ['kind'], `${c}.destination`)
    return { ...base, destination: { kind } }
  }
  if (kind === 'automation') {
    expectOnlyKeys(d, ['kind', 'automationId', 'runId'], `${c}.destination`)
    return {
      ...base,
      destination: {
        kind,
        automationId: boundedString(d.automationId, `${c}.destination.automationId`),
        runId: nullableString(d.runId, `${c}.destination.runId`)
      }
    }
  }
  expectOnlyKeys(d, ['kind', 'conversationId', 'messageId', 'approvalActionId'], `${c}.destination`)
  return {
    ...base,
    destination: {
      kind,
      conversationId: boundedString(d.conversationId, `${c}.destination.conversationId`),
      messageId: nullableString(d.messageId, `${c}.destination.messageId`),
      approvalActionId: nullableString(d.approvalActionId, `${c}.destination.approvalActionId`)
    }
  }
}
