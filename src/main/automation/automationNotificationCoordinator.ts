import { randomUUID } from 'node:crypto'
import { Notification } from 'electron'
import {
  AUTOMATION_SCHEMA_VERSION,
  parseAutomationOpenRequest,
  type AutomationNotificationDelivery,
  type AutomationOpenRequest
} from '@mycopilot/protocol'
import type { CoreServer } from '../core/coreServer'

const DELIVERY_BATCH_SIZE = 10
const DELIVERY_LEASE_MS = 60_000
const DELIVERY_RETRY_MS = 60_000
const DELIVERY_SHOW_TIMEOUT_MS = 15_000
const POLL_INTERVAL_MS = 30_000

type DeliveryCore = Pick<
  CoreServer,
  | 'claimAutomationNotifications'
  | 'validateAutomationNotification'
  | 'acknowledgeAutomationNotification'
  | 'releaseAutomationNotification'
>

interface NativeNotificationLike {
  on(event: 'click' | 'close' | 'show', listener: () => void): this
  on(event: 'failed', listener: (...args: unknown[]) => void): this
  show(): void
  close(): void
}

interface AutomationNotificationCoordinatorDependencies {
  clearInterval: (timer: ReturnType<typeof setInterval>) => void
  createClaimToken: () => string
  createNotification: (options: { title: string; body: string }) => NativeNotificationLike
  isNotificationSupported: () => boolean
  now: () => number
  setInterval: (handler: () => void, intervalMs: number) => ReturnType<typeof setInterval>
  setTimeout: (handler: () => void, timeoutMs: number) => ReturnType<typeof setTimeout>
  clearTimeout: (timer: ReturnType<typeof setTimeout>) => void
}

const defaultDependencies: AutomationNotificationCoordinatorDependencies = {
  clearInterval,
  createClaimToken: () => `automation-notification-claim:${randomUUID()}`,
  createNotification: (options) => new Notification(options),
  isNotificationSupported: () => Notification.isSupported(),
  now: Date.now,
  setInterval,
  setTimeout,
  clearTimeout
}

export interface AutomationNotificationCoordinatorOptions {
  coreServer: DeliveryCore
  onOpenRequested: (request: AutomationOpenRequest) => void
  dependencies?: Partial<AutomationNotificationCoordinatorDependencies>
}

/**
 * Main-owned durable native-notification pump. Core's outbox is authoritative; Core events only
 * reduce latency. A short claim lease fences concurrent/restarted Hosts and every successful show
 * is acknowledged back to Core.
 */
export class AutomationNotificationCoordinator {
  private readonly dependencies: AutomationNotificationCoordinatorDependencies
  private readonly displayedButUnacknowledged = new Set<string>()
  private readonly liveNotifications = new Set<NativeNotificationLike>()
  private readonly showTimeouts = new Map<string, ReturnType<typeof setTimeout>>()
  private readonly timer: ReturnType<typeof setInterval>
  private drainRequested = false
  private draining = false
  private stopped = false

  constructor(private readonly options: AutomationNotificationCoordinatorOptions) {
    this.dependencies = { ...defaultDependencies, ...options.dependencies }
    this.timer = this.dependencies.setInterval(() => this.requestDrain(), POLL_INTERVAL_MS)
    this.timer.unref?.()
    this.requestDrain()
  }

  requestDrain(): void {
    if (this.stopped) return
    this.drainRequested = true
    if (this.draining) return
    this.draining = true
    void this.drainLoop()
      .catch(() => console.warn('Automation notification delivery loop failed'))
      .finally(() => {
        this.draining = false
        if (this.drainRequested && !this.stopped) this.requestDrain()
      })
  }

  stop(): void {
    if (this.stopped) return
    this.stopped = true
    this.drainRequested = false
    this.dependencies.clearInterval(this.timer)
    for (const timeout of this.showTimeouts.values()) this.dependencies.clearTimeout(timeout)
    this.showTimeouts.clear()
    for (const notification of this.liveNotifications) {
      try {
        notification.close()
      } catch {
        // The durable outbox has already been acknowledged after show; closing is best-effort.
      }
    }
    this.liveNotifications.clear()
  }

  private async drainLoop(): Promise<void> {
    while (this.drainRequested && !this.stopped) {
      this.drainRequested = false
      await this.drainOnce()
    }
  }

  private async drainOnce(): Promise<void> {
    // Do not claim when the platform cannot display notifications. Pending outbox rows and
    // attention remain durable and can be recovered if support becomes available later.
    try {
      if (!this.dependencies.isNotificationSupported()) return
    } catch {
      console.warn('Failed to determine native Automation notification support')
      return
    }

    const claimToken = this.dependencies.createClaimToken()
    let deliveries: AutomationNotificationDelivery[]
    try {
      const output = await this.options.coreServer.claimAutomationNotifications({
        schemaVersion: AUTOMATION_SCHEMA_VERSION,
        claimToken,
        leaseDurationMs: DELIVERY_LEASE_MS,
        limit: DELIVERY_BATCH_SIZE
      })
      deliveries = output.notifications
    } catch {
      console.warn('Failed to claim pending Automation notifications')
      return
    }

    for (const delivery of deliveries) {
      if (this.stopped) return
      await this.deliver(delivery, claimToken)
    }
  }

  private async deliver(
    claimedDelivery: AutomationNotificationDelivery,
    claimToken: string
  ): Promise<void> {
    if (!this.displayedButUnacknowledged.has(claimedDelivery.notificationId)) {
      let delivery: AutomationNotificationDelivery
      try {
        const validation = await this.options.coreServer.validateAutomationNotification({
          schemaVersion: AUTOMATION_SCHEMA_VERSION,
          notificationId: claimedDelivery.notificationId,
          claimToken
        })
        if (validation.notification === null) return
        delivery = validation.notification
      } catch {
        if (this.stopped) return
        await this.releaseAfterFailure(claimedDelivery.notificationId, claimToken)
        return
      }
      if (this.stopped) return
      let notification: NativeNotificationLike | null = null
      let settleShow: (() => boolean) | null = null
      try {
        notification = this.dependencies.createNotification({
          title: delivery.title,
          body: delivery.body
        })
        const displayedNotification = notification
        this.liveNotifications.add(displayedNotification)
        displayedNotification.on('click', () => {
          this.options.onOpenRequested(notificationOpenRequest(delivery))
        })
        displayedNotification.on('close', () => {
          this.liveNotifications.delete(displayedNotification)
        })
        let settled = false
        const settle = (): boolean => {
          if (settled) return false
          settled = true
          const timeout = this.showTimeouts.get(delivery.notificationId)
          if (timeout !== undefined) this.dependencies.clearTimeout(timeout)
          this.showTimeouts.delete(delivery.notificationId)
          return true
        }
        settleShow = settle
        displayedNotification.on('show', () => {
          if (!settle() || this.stopped) return
          this.displayedButUnacknowledged.add(delivery.notificationId)
          void this.acknowledgeDisplayed(delivery.notificationId, claimToken)
        })
        displayedNotification.on('failed', () => {
          if (!settle() || this.stopped) return
          this.liveNotifications.delete(displayedNotification)
          void this.releaseAfterFailure(delivery.notificationId, claimToken)
        })
        const showTimeout = this.dependencies.setTimeout(() => {
          if (!settle() || this.stopped) return
          this.liveNotifications.delete(displayedNotification)
          try {
            displayedNotification.close()
          } catch {
            // The durable lease remains recoverable even when native cleanup fails.
          }
          void this.releaseAfterFailure(delivery.notificationId, claimToken)
        }, DELIVERY_SHOW_TIMEOUT_MS)
        showTimeout.unref?.()
        this.showTimeouts.set(delivery.notificationId, showTimeout)
        displayedNotification.show()
      } catch {
        settleShow?.()
        if (notification !== null) {
          this.liveNotifications.delete(notification)
          try {
            notification.close()
          } catch {
            // Continue to release the durable lease even if cleanup also fails.
          }
        }
        await this.releaseAfterFailure(delivery.notificationId, claimToken)
        return
      }
      return
    }

    await this.acknowledgeDisplayed(claimedDelivery.notificationId, claimToken)
  }

  private async acknowledgeDisplayed(notificationId: string, claimToken: string): Promise<void> {
    try {
      await this.options.coreServer.acknowledgeAutomationNotification({
        schemaVersion: AUTOMATION_SCHEMA_VERSION,
        notificationId,
        claimToken
      })
      this.displayedButUnacknowledged.delete(notificationId)
    } catch {
      // Keep the in-process marker: if the lease is reclaimed in this process, retry only the ACK
      // instead of displaying a duplicate. A process crash between show and ACK is the unavoidable
      // native API boundary; Core's lease and unique outbox identity bound that window.
      console.warn('Failed to acknowledge a displayed Automation notification')
    }
  }

  private async releaseAfterFailure(notificationId: string, claimToken: string): Promise<void> {
    try {
      await this.options.coreServer.releaseAutomationNotification({
        schemaVersion: AUTOMATION_SCHEMA_VERSION,
        notificationId,
        claimToken,
        retryAt: this.dependencies.now() + DELIVERY_RETRY_MS,
        errorCode: 'native_notification_failed'
      })
    } catch {
      // The claim lease is itself a durable retry fallback.
      console.warn('Failed to release an Automation notification delivery claim')
    }
  }
}

function notificationOpenRequest(delivery: AutomationNotificationDelivery): AutomationOpenRequest {
  const destination: AutomationOpenRequest['destination'] = delivery.conversationId
    ? {
        kind: 'conversation',
        conversationId: delivery.conversationId,
        messageId: delivery.assistantMessageId ?? delivery.userMessageId
      }
    : { kind: 'task' }
  return parseAutomationOpenRequest({
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    automationId: delivery.automationId,
    runId: delivery.runId,
    destination
  })
}
