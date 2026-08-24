import { randomUUID } from 'node:crypto'
import { BrowserWindow, Notification } from 'electron'
import {
  NOTIFICATION_SCHEMA_VERSION,
  parseNotificationOpenRequest,
  type NotificationBatch,
  type NotificationBatchAcknowledgeInput,
  type NotificationBatchAcknowledgeOutput,
  type NotificationBatchesClaimInput,
  type NotificationBatchesClaimOutput,
  type NotificationBatchReleaseInput,
  type NotificationBatchReleaseOutput,
  type NotificationBatchValidateInput,
  type NotificationBatchValidateOutput,
  type NotificationEvent,
  type NotificationListItem,
  type NotificationListOutput,
  type NotificationOpenRequest,
  type NotificationPriority,
  type NotificationSoundLevel
} from '@mycopilot/protocol'
import type { CoreServer } from '../core/coreServer'
import { DEFAULT_APP_LANGUAGE, type AppLanguage } from '../../shared/i18n/languageRegistry'
import {
  formatNotificationBatch,
  type SystemNotificationPresentation
} from './notificationPresentation'

const DELIVERY_BATCH_SIZE = 10
const DELIVERY_LEASE_MS = 60_000
const DELIVERY_RETRY_MS = 60_000
const DELIVERY_SHOW_TIMEOUT_MS = 15_000
const NATIVE_UPDATE_THROTTLE_MS = 2_500
const POLL_INTERVAL_MS = 30_000

type DeliveryCore = Pick<
  CoreServer,
  | 'claimNotificationBatches'
  | 'validateNotificationBatch'
  | 'acknowledgeNotificationBatch'
  | 'releaseNotificationBatch'
  | 'listNotifications'
>

interface NativeNotificationOptions extends SystemNotificationPresentation {
  silent: boolean
}

interface NativeNotificationLike {
  on(event: 'click' | 'close' | 'show', listener: () => void): this
  on(event: 'failed', listener: (...args: unknown[]) => void): this
  show(): void
  close(): void
}

interface SystemNotificationCoordinatorDependencies {
  clearInterval: (timer: ReturnType<typeof setInterval>) => void
  clearTimeout: (timer: ReturnType<typeof setTimeout>) => void
  createClaimToken: () => string
  createNotification: (
    options: NativeNotificationOptions,
    replacementKey: string
  ) => NativeNotificationLike
  /**
   * Electron 39 does not expose a stable notification identifier. A future Host backend can
   * implement this hook once the runtime has genuine replace-in-place support. Returning null
   * selects the safe Electron 39 fallback.
   */
  replaceNotification: (
    current: NativeNotificationLike,
    options: NativeNotificationOptions,
    replacementKey: string
  ) => NativeNotificationLike | null
  getLocale: () => AppLanguage
  isAppForeground: () => boolean
  isNotificationSupported: () => boolean
  now: () => number
  setInterval: (handler: () => void, intervalMs: number) => ReturnType<typeof setInterval>
  setTimeout: (handler: () => void, timeoutMs: number) => ReturnType<typeof setTimeout>
}

const defaultDependencies: SystemNotificationCoordinatorDependencies = {
  clearInterval,
  clearTimeout,
  createClaimToken: () => `notification-batch-claim:${randomUUID()}`,
  createNotification: (options) => new Notification(options),
  replaceNotification: () => null,
  getLocale: () => DEFAULT_APP_LANGUAGE,
  isAppForeground: () =>
    BrowserWindow.getAllWindows().some(
      (window) =>
        !window.isDestroyed() && window.isVisible() && !window.isMinimized() && window.isFocused()
    ),
  isNotificationSupported: () => Notification.isSupported(),
  now: Date.now,
  setInterval,
  setTimeout
}

export interface SystemNotificationCoordinatorOptions {
  coreServer: DeliveryCore
  onOpenRequested: (request: NotificationOpenRequest, eventIds: string[]) => void
  startPaused?: boolean
  dependencies?: Partial<SystemNotificationCoordinatorDependencies>
}

interface BatchPresentationState {
  batch: NotificationBatch
  presentedBatch: NotificationBatch | null
  notification: NativeNotificationLike | null
  lastNativeShowAt: number
}

interface PendingNativeUpdate {
  batch: NotificationBatch
  claimToken: string
  timer: ReturnType<typeof setTimeout>
}

/**
 * The single Main-owned native notification pump. Core owns facts, aggregation, leases, expiry,
 * deduplication and delivery dispositions. Main owns foreground policy and the native API boundary.
 */
export class SystemNotificationCoordinator {
  private readonly dependencies: SystemNotificationCoordinatorDependencies
  private readonly pendingAcknowledgements = new Map<
    string,
    {
      revision: number
      disposition: NotificationBatchAcknowledgeInput['disposition']
      nativePriority: NotificationPriority
      soundLevel: NotificationSoundLevel
      nativeRevision: number
    }
  >()
  private readonly presentations = new Map<string, BatchPresentationState>()
  private readonly pendingNativeUpdates = new Map<string, PendingNativeUpdate>()
  private readonly showTimeouts = new Map<string, ReturnType<typeof setTimeout>>()
  private readonly reconciliationEpochs = new Map<string, number>()
  private readonly timer: ReturnType<typeof setInterval>
  private drainRequested = false
  private draining = false
  private deliveryStarted: boolean
  private stopped = false

  constructor(private readonly options: SystemNotificationCoordinatorOptions) {
    this.dependencies = { ...defaultDependencies, ...options.dependencies }
    this.deliveryStarted = options.startPaused !== true
    this.timer = this.dependencies.setInterval(() => this.requestDrain(), POLL_INTERVAL_MS)
    this.timer.unref?.()
    if (this.deliveryStarted) this.requestDrain()
  }

  requestDrain(): void {
    if (this.stopped || !this.deliveryStarted) return
    this.drainRequested = true
    if (this.draining) return
    this.draining = true
    void this.drainLoop()
      .catch(() => console.warn('System notification delivery loop failed'))
      .finally(() => {
        this.draining = false
        if (this.drainRequested && !this.stopped) this.requestDrain()
      })
  }

  handleNotificationEvent(event: NotificationEvent): void {
    this.requestDrain()
    if (event.batchId === null || !this.presentations.has(event.batchId)) return
    const epoch = (this.reconciliationEpochs.get(event.batchId) ?? 0) + 1
    this.reconciliationEpochs.set(event.batchId, epoch)
    void this.reconcilePresentedBatch(
      event.batchId,
      event.kind === 'resolved' || event.kind === 'seen',
      epoch
    )
  }

  start(): void {
    if (this.stopped || this.deliveryStarted) return
    this.deliveryStarted = true
    this.requestDrain()
  }

  stop(): void {
    if (this.stopped) return
    this.stopped = true
    this.drainRequested = false
    this.dependencies.clearInterval(this.timer)
    for (const pending of this.pendingNativeUpdates.values()) {
      this.dependencies.clearTimeout(pending.timer)
    }
    this.pendingNativeUpdates.clear()
    for (const timeout of this.showTimeouts.values()) this.dependencies.clearTimeout(timeout)
    this.showTimeouts.clear()
    for (const state of this.presentations.values()) {
      if (state.notification === null) continue
      try {
        state.notification.close()
      } catch {
        // Native cleanup is best-effort; Core's durable disposition remains authoritative.
      }
      state.notification = null
    }
  }

  private async drainLoop(): Promise<void> {
    while (this.drainRequested && !this.stopped) {
      this.drainRequested = false
      await this.drainOnce()
    }
  }

  private async drainOnce(): Promise<void> {
    this.prunePresentationState()
    const claimToken = this.dependencies.createClaimToken()
    let output: NotificationBatchesClaimOutput
    try {
      const input: NotificationBatchesClaimInput = {
        schemaVersion: NOTIFICATION_SCHEMA_VERSION,
        claimToken,
        leaseDurationMs: DELIVERY_LEASE_MS,
        limit: DELIVERY_BATCH_SIZE
      }
      output = await this.options.coreServer.claimNotificationBatches(input)
    } catch {
      console.warn('Failed to claim pending system notification batches')
      return
    }

    for (const batch of output.batches) {
      if (this.stopped) return
      await this.deliver(batch, claimToken)
    }
  }

  private async deliver(claimedBatch: NotificationBatch, claimToken: string): Promise<void> {
    const pendingAck = this.pendingAcknowledgements.get(claimedBatch.batchId)
    if (pendingAck && pendingAck.revision === claimedBatch.revision) {
      await this.acknowledge(
        claimedBatch,
        claimToken,
        pendingAck.disposition,
        pendingAck.nativePriority,
        pendingAck.soundLevel,
        pendingAck.nativeRevision
      )
      return
    }

    let batch: NotificationBatch
    try {
      const input: NotificationBatchValidateInput = {
        schemaVersion: NOTIFICATION_SCHEMA_VERSION,
        batchId: claimedBatch.batchId,
        claimToken
      }
      const validation: NotificationBatchValidateOutput =
        await this.options.coreServer.validateNotificationBatch(input)
      if (validation.batch === null) return
      batch = validation.batch
    } catch {
      if (!this.stopped) await this.releaseAfterFailure(claimedBatch.batchId, claimToken)
      return
    }
    if (this.stopped) return

    if (!batch.notificationsEnabled) {
      await this.suppress(batch, claimToken, 'suppressed_disabled')
      return
    }
    if (this.isForeground()) {
      await this.suppress(batch, claimToken, 'suppressed_foreground')
      return
    }
    let supported = false
    try {
      supported = this.dependencies.isNotificationSupported()
    } catch {
      // Treat an unavailable native subsystem as disabled; notification facts remain durable.
    }
    if (!supported) {
      await this.suppress(batch, claimToken, 'suppressed_disabled')
      return
    }
    const collectionDelay = batch.collectUntil - this.dependencies.now()
    if (collectionDelay > 0) {
      this.deferNativeUpdate(batch, claimToken, collectionDelay)
      return
    }

    const state = this.presentations.get(batch.batchId)
    const previousAcknowledgement = this.pendingAcknowledgements.get(batch.batchId)
    const deliveredPriority =
      batch.deliveredPriority ??
      state?.batch.deliveredPriority ??
      previousAcknowledgement?.nativePriority ??
      state?.presentedBatch?.highestPriority ??
      null
    const priorityUpgrade =
      deliveredPriority !== null &&
      !isAttentionPriority(deliveredPriority) &&
      priorityRank(batch.highestPriority) > priorityRank(deliveredPriority) &&
      isAttentionPriority(batch.highestPriority)
    const soundLevel = nextSoundLevel(batch, priorityUpgrade, previousAcknowledgement?.soundLevel)
    const isNativeUpdate = batch.isUpdate || batch.deliveredRevision !== null || state !== undefined

    // Throttle both today's Electron fallback and a future stable-ID replacement backend. The
    // durable claimed batch keeps accumulating while this timer is pending, then gets revalidated
    // before any native mutation.
    if (isNativeUpdate && state && state.lastNativeShowAt > 0) {
      const remaining = state.lastNativeShowAt + NATIVE_UPDATE_THROTTLE_MS - this.dependencies.now()
      if (remaining > 0) {
        this.deferNativeUpdate(batch, claimToken, remaining)
        return
      }
    }

    if (isNativeUpdate && !priorityUpgrade && state?.notification) {
      const presentation = formatNotificationBatch(batch, this.dependencies.getLocale())
      const replacement = this.dependencies.replaceNotification(
        state.notification,
        { ...presentation, silent: true },
        batch.batchId
      )
      if (replacement !== null) {
        await this.showValidatedReplacement(batch, claimToken, replacement, soundLevel)
        return
      }
    }

    // Electron 39 cannot atomically replace an existing native entry. Ordinary increments are
    // acknowledged and remain available in their owning chat, automation, or approval surface
    // without generating a second OS toast. Only an escalation to an attention state replaces once.
    if (isNativeUpdate && !priorityUpgrade) {
      this.rememberBatch(batch, state?.notification ?? null, state?.lastNativeShowAt ?? 0)
      await this.acknowledge(
        batch,
        claimToken,
        'delivered',
        deliveredPriority ?? batch.highestPriority,
        soundLevel,
        batch.revision
      )
      return
    }

    await this.showNewNative(batch, claimToken, soundLevel, priorityUpgrade)
  }

  private deferNativeUpdate(batch: NotificationBatch, claimToken: string, delayMs: number): void {
    const existing = this.pendingNativeUpdates.get(batch.batchId)
    if (existing) this.dependencies.clearTimeout(existing.timer)
    const timer = this.dependencies.setTimeout(() => {
      this.pendingNativeUpdates.delete(batch.batchId)
      if (this.stopped) return
      void this.deliver(batch, claimToken)
    }, delayMs)
    timer.unref?.()
    this.pendingNativeUpdates.set(batch.batchId, { batch, claimToken, timer })
  }

  private async showValidatedReplacement(
    batch: NotificationBatch,
    claimToken: string,
    replacement: NativeNotificationLike,
    soundLevel: NotificationSoundLevel
  ): Promise<void> {
    // A replacement backend may have waited internally. Revalidate immediately before show.
    const latest = await this.revalidate(batch, claimToken)
    if (latest === null || this.stopped) return
    if (this.isForeground()) {
      await this.suppress(latest, claimToken, 'suppressed_foreground')
      return
    }
    await this.showHandle(latest, claimToken, replacement, soundLevel, true)
  }

  private async showNewNative(
    batch: NotificationBatch,
    claimToken: string,
    soundLevel: NotificationSoundLevel,
    replacingForUpgrade: boolean
  ): Promise<void> {
    // Close a previous Electron 39 entry only for the single permitted priority upgrade. Routine
    // increments never create another native toast on runtimes without stable replacement IDs.
    const state = this.presentations.get(batch.batchId)
    if (replacingForUpgrade && state?.notification) {
      try {
        state.notification.close()
      } catch {
        // Showing the new high-priority notice remains useful if cleanup races the OS.
      }
      state.notification = null
    }

    const latest = await this.revalidate(batch, claimToken)
    if (latest === null || this.stopped) return
    if (this.isForeground()) {
      await this.suppress(latest, claimToken, 'suppressed_foreground')
      return
    }
    const presentation = formatNotificationBatch(latest, this.dependencies.getLocale())
    let notification: NativeNotificationLike
    try {
      notification = this.dependencies.createNotification(
        {
          ...presentation,
          silent: !latest.soundEnabled || soundLevel === latest.soundLevelPlayed
        },
        latest.batchId
      )
    } catch {
      await this.releaseAfterFailure(latest.batchId, claimToken)
      return
    }
    await this.showHandle(latest, claimToken, notification, soundLevel, false)
  }

  private async showHandle(
    batch: NotificationBatch,
    claimToken: string,
    notification: NativeNotificationLike,
    soundLevel: NotificationSoundLevel,
    replacement: boolean
  ): Promise<void> {
    if (this.stopped) return
    this.rememberBatch(batch, notification, this.dependencies.now())
    let settled = false
    const settle = (): boolean => {
      if (settled) return false
      settled = true
      const timeout = this.showTimeouts.get(batch.batchId)
      if (timeout !== undefined) this.dependencies.clearTimeout(timeout)
      this.showTimeouts.delete(batch.batchId)
      return true
    }
    notification.on('click', () => {
      const latest = this.presentations.get(batch.batchId)?.presentedBatch ?? batch
      try {
        notification.close()
      } catch {
        // Navigation remains authoritative if native cleanup loses an OS-level race.
      }
      void this.openPresentedBatch(latest)
    })
    notification.on('close', () => {
      const current = this.presentations.get(batch.batchId)
      if (current?.notification === notification) current.notification = null
    })
    notification.on('show', () => {
      if (!settle() || this.stopped) return
      const current = this.presentations.get(batch.batchId)
      if (current) {
        current.batch = batch
        current.presentedBatch = batch
        current.lastNativeShowAt = this.dependencies.now()
      }
      this.pendingAcknowledgements.set(batch.batchId, {
        revision: batch.revision,
        disposition: 'delivered',
        nativePriority: batch.highestPriority,
        soundLevel,
        nativeRevision: batch.revision
      })
      void this.acknowledge(
        batch,
        claimToken,
        'delivered',
        batch.highestPriority,
        soundLevel,
        batch.revision
      )
    })
    notification.on('failed', () => {
      if (!settle() || this.stopped) return
      const current = this.presentations.get(batch.batchId)
      if (current?.notification === notification) current.notification = null
      void this.releaseAfterFailure(batch.batchId, claimToken)
    })
    const showTimeout = this.dependencies.setTimeout(() => {
      if (!settle() || this.stopped) return
      const current = this.presentations.get(batch.batchId)
      if (current?.notification === notification) current.notification = null
      try {
        notification.close()
      } catch {
        // The durable lease remains the retry fallback when native cleanup also fails.
      }
      void this.releaseAfterFailure(batch.batchId, claimToken)
    }, DELIVERY_SHOW_TIMEOUT_MS)
    showTimeout.unref?.()
    this.showTimeouts.set(batch.batchId, showTimeout)
    try {
      notification.show()
    } catch {
      settle()
      const current = this.presentations.get(batch.batchId)
      if (current?.notification === notification) current.notification = null
      if (!replacement) {
        try {
          notification.close()
        } catch {
          // Continue to release the durable claim.
        }
      }
      await this.releaseAfterFailure(batch.batchId, claimToken)
    }
  }

  private rememberBatch(
    batch: NotificationBatch,
    notification: NativeNotificationLike | null,
    lastNativeShowAt: number
  ): BatchPresentationState {
    const existing = this.presentations.get(batch.batchId)
    if (existing) {
      existing.batch = batch
      if (notification !== null) existing.notification = notification
      if (lastNativeShowAt > 0) existing.lastNativeShowAt = lastNativeShowAt
      return existing
    }
    const state = { batch, presentedBatch: null, notification, lastNativeShowAt }
    this.presentations.set(batch.batchId, state)
    return state
  }

  private prunePresentationState(): void {
    const now = this.dependencies.now()
    for (const [batchId, state] of this.presentations) {
      if (state.notification === null && state.batch.replaceUntil <= now) {
        this.presentations.delete(batchId)
      }
    }
  }

  private async openPresentedBatch(fallback: NotificationBatch): Promise<void> {
    const items = await this.listLiveBatchItems(fallback.batchId)
    if (items === null || this.stopped) return
    const presentedIds = new Set(fallback.items.map((item) => item.eventId))
    const presentedItems = items.filter((item) => presentedIds.has(item.eventId))
    if (presentedItems.length === 0) return
    const latest = batchWithItems(fallback, presentedItems)
    this.rememberBatch(latest, null, this.dependencies.now())
    this.options.onOpenRequested(
      notificationOpenRequest(latest),
      latest.items.map((item) => item.eventId)
    )
  }

  private async reconcilePresentedBatch(
    batchId: string,
    closeStaleNative: boolean,
    epoch: number
  ): Promise<void> {
    const items = await this.listLiveBatchItems(batchId)
    if (items === null || this.stopped || this.reconciliationEpochs.get(batchId) !== epoch) return
    const state = this.presentations.get(batchId)
    if (!state) return

    if (items.length === 0) {
      this.closePresentation(batchId, state)
      this.presentations.delete(batchId)
      this.reconciliationEpochs.delete(batchId)
      return
    }

    state.batch = batchWithItems(state.batch, items)
    if (closeStaleNative && state.notification) {
      // A merged native card can no longer truthfully represent its displayed snapshot once one
      // member is resolved or deleted. Electron 39 cannot update it in place, so withdraw the stale
      // card without replaying the remaining facts; they remain authoritative in their owning UI.
      this.closePresentation(batchId, state)
    }
  }

  private async listLiveBatchItems(batchId: string): Promise<NotificationListItem[] | null> {
    try {
      const output: NotificationListOutput = await this.options.coreServer.listNotifications({
        schemaVersion: NOTIFICATION_SCHEMA_VERSION,
        batchId,
        limit: 100
      })
      return output.items.filter(
        (item) =>
          item.seenAt === null &&
          !(
            item.resolvedAt !== null &&
            (item.kind === 'approval_required' || item.kind === 'automation_configuration_blocked')
          )
      )
    } catch {
      return null
    }
  }

  private closePresentation(batchId: string, state: BatchPresentationState): void {
    const pending = this.pendingNativeUpdates.get(batchId)
    if (pending) {
      this.dependencies.clearTimeout(pending.timer)
      this.pendingNativeUpdates.delete(batchId)
    }
    const notification = state.notification
    state.notification = null
    if (!notification) return
    try {
      notification.close()
    } catch {
      // The owning chat, automation, or approval surface remains authoritative if the OS card is gone.
    }
  }

  private async revalidate(
    batch: NotificationBatch,
    claimToken: string
  ): Promise<NotificationBatch | null> {
    try {
      const input: NotificationBatchValidateInput = {
        schemaVersion: NOTIFICATION_SCHEMA_VERSION,
        batchId: batch.batchId,
        claimToken
      }
      const output: NotificationBatchValidateOutput =
        await this.options.coreServer.validateNotificationBatch(input)
      return output.batch
    } catch {
      if (!this.stopped) await this.releaseAfterFailure(batch.batchId, claimToken)
      return null
    }
  }

  private isForeground(): boolean {
    try {
      return this.dependencies.isAppForeground()
    } catch {
      // Fail closed for user attention: an uncertain focus state must not create a notification
      // storm in front of an actively used window.
      return true
    }
  }

  private async suppress(
    batch: NotificationBatch,
    claimToken: string,
    disposition: Exclude<NotificationBatchAcknowledgeInput['disposition'], 'delivered'>
  ): Promise<void> {
    this.rememberBatch(batch, this.presentations.get(batch.batchId)?.notification ?? null, 0)
    await this.acknowledge(
      batch,
      claimToken,
      disposition,
      batch.deliveredPriority ?? batch.highestPriority,
      batch.soundLevelPlayed,
      batch.deliveredRevision ?? batch.revision
    )
  }

  private async acknowledge(
    batch: NotificationBatch,
    claimToken: string,
    disposition: NotificationBatchAcknowledgeInput['disposition'],
    nativePriority: NotificationPriority,
    soundLevelPlayed: NotificationSoundLevel,
    nativeRevision: number
  ): Promise<void> {
    const input: NotificationBatchAcknowledgeInput = {
      schemaVersion: NOTIFICATION_SCHEMA_VERSION,
      batchId: batch.batchId,
      claimToken,
      disposition,
      nativePriority,
      soundLevelPlayed,
      nativeRevision
    }
    try {
      const output: NotificationBatchAcknowledgeOutput =
        await this.options.coreServer.acknowledgeNotificationBatch(input)
      if (output.batchId === batch.batchId) {
        this.pendingAcknowledgements.delete(batch.batchId)
      }
    } catch {
      this.pendingAcknowledgements.set(batch.batchId, {
        revision: batch.revision,
        disposition,
        nativePriority,
        soundLevel: soundLevelPlayed,
        nativeRevision
      })
      console.warn('Failed to acknowledge a system notification batch')
    }
  }

  private async releaseAfterFailure(batchId: string, claimToken: string): Promise<void> {
    const input: NotificationBatchReleaseInput = {
      schemaVersion: NOTIFICATION_SCHEMA_VERSION,
      batchId,
      claimToken,
      retryAt: this.dependencies.now() + DELIVERY_RETRY_MS,
      errorCode: 'native_notification_failed'
    }
    try {
      const output: NotificationBatchReleaseOutput =
        await this.options.coreServer.releaseNotificationBatch(input)
      if (output.batchId !== batchId) throw new Error('Notification release identity mismatch')
    } catch {
      // The durable claim lease is itself a retry fallback.
      console.warn('Failed to release a system notification batch claim')
    }
  }
}

function nextSoundLevel(
  batch: NotificationBatch,
  priorityUpgrade: boolean,
  unacknowledgedSoundLevel?: NotificationSoundLevel
): NotificationSoundLevel {
  const previousSoundLevel =
    batch.soundLevelPlayed === 'none' && unacknowledgedSoundLevel
      ? unacknowledgedSoundLevel
      : batch.soundLevelPlayed
  if (!batch.soundEnabled) return previousSoundLevel
  if (priorityUpgrade) return 'upgrade'
  if (batch.deliveredPriority === null && previousSoundLevel === 'none') return 'initial'
  return previousSoundLevel
}

function priorityRank(priority: NotificationPriority): number {
  switch (priority) {
    case 'approval_required':
      return 6
    case 'configuration_blocked':
      return 5
    case 'failed':
      return 4
    case 'important_update':
      return 3
    case 'cancelled':
      return 2
    case 'completed':
      return 1
  }
}

function isAttentionPriority(priority: NotificationPriority): boolean {
  return (
    priority === 'approval_required' ||
    priority === 'configuration_blocked' ||
    priority === 'failed' ||
    priority === 'important_update'
  )
}

function batchWithItems(
  batch: NotificationBatch,
  items: NotificationListItem[]
): NotificationBatch {
  const counts = {
    completed: 0,
    failed: 0,
    cancelled: 0,
    approvalRequired: 0,
    importantUpdate: 0,
    configurationBlocked: 0
  }
  let highestPriority: NotificationPriority = 'completed'
  for (const item of items) {
    switch (item.kind) {
      case 'task_completed':
      case 'automation_completed':
        counts.completed += 1
        break
      case 'task_failed':
      case 'automation_failed':
        counts.failed += 1
        break
      case 'task_cancelled':
      case 'automation_cancelled':
        counts.cancelled += 1
        break
      case 'approval_required':
        counts.approvalRequired += 1
        break
      case 'automation_important_update':
        counts.importantUpdate += 1
        break
      case 'automation_configuration_blocked':
        counts.configurationBlocked += 1
        break
    }
    if (priorityRank(item.priority) > priorityRank(highestPriority)) {
      highestPriority = item.priority
    }
  }
  return { ...batch, counts, highestPriority, itemCount: items.length, items }
}

function notificationOpenRequest(batch: NotificationBatch): NotificationOpenRequest {
  const eventIds = batch.items.map((item) => item.eventId)
  const item = [...batch.items]
    .filter((candidate) => candidate.conversationId !== null || candidate.automationId !== null)
    .sort((left, right) => {
      const priorityDifference = priorityRank(right.priority) - priorityRank(left.priority)
      if (priorityDifference !== 0) return priorityDifference
      if (right.occurredAt !== left.occurredAt) return right.occurredAt - left.occurredAt
      return right.eventId.localeCompare(left.eventId)
    })[0]

  if (!item) {
    return parseNotificationOpenRequest({
      schemaVersion: NOTIFICATION_SCHEMA_VERSION,
      batchId: batch.batchId,
      eventIds,
      destination: { kind: 'application' }
    })
  }
  if (item.conversationId) {
    return parseNotificationOpenRequest({
      schemaVersion: NOTIFICATION_SCHEMA_VERSION,
      batchId: batch.batchId,
      eventIds,
      destination: {
        kind: 'conversation',
        conversationId: item.conversationId,
        messageId: item.assistantMessageId ?? item.userMessageId,
        approvalActionId: item.approvalActionId
      }
    })
  }
  if (item.automationId) {
    return parseNotificationOpenRequest({
      schemaVersion: NOTIFICATION_SCHEMA_VERSION,
      batchId: batch.batchId,
      eventIds,
      destination: {
        kind: 'automation',
        automationId: item.automationId,
        runId: item.runId
      }
    })
  }
  return parseNotificationOpenRequest({
    schemaVersion: NOTIFICATION_SCHEMA_VERSION,
    batchId: batch.batchId,
    eventIds,
    destination: { kind: 'application' }
  })
}
