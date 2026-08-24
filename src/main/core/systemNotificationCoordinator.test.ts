import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('electron', () => ({
  app: { getLocale: () => 'zh-CN' },
  BrowserWindow: { getAllWindows: () => [] },
  Notification: class {
    static isSupported(): boolean {
      return false
    }
  }
}))

import type { NotificationBatch, NotificationListItem } from '@mycopilot/protocol'
import { SystemNotificationCoordinator } from '../notifications/systemNotificationCoordinator'
import type { AppLanguage } from '../../shared/i18n/languageRegistry'

function item(overrides: Partial<NotificationListItem> = {}): NotificationListItem {
  return {
    schemaVersion: 1,
    eventId: 'event-1',
    batchId: 'batch-1',
    kind: 'task_completed',
    sourceKind: 'human_root',
    sourceId: 'run-1',
    runId: 'run-1',
    automationId: null,
    conversationId: 'conversation-1',
    userMessageId: 'user-message-1',
    assistantMessageId: 'assistant-message-1',
    approvalActionId: null,
    subjectKind: 'prompt_excerpt',
    subjectText: '制定单元设备技术报告方案',
    priority: 'completed',
    resourceRevision: 1,
    seenAt: null,
    resolvedAt: null,
    occurredAt: 100,
    ...overrides
  } as NotificationListItem
}

function batch(overrides: Partial<NotificationBatch> = {}): NotificationBatch {
  return {
    schemaVersion: 1,
    batchId: 'batch-1',
    revision: 1,
    status: 'claimed',
    highestPriority: 'completed',
    counts: {
      completed: 1,
      failed: 0,
      cancelled: 0,
      approvalRequired: 0,
      importantUpdate: 0,
      configurationBlocked: 0
    },
    itemCount: 1,
    items: [item()],
    collectUntil: 100,
    replaceUntil: 60_000,
    notificationsEnabled: true,
    soundEnabled: true,
    showTaskContent: true,
    soundLevelPlayed: 'none',
    deliveredRevision: null,
    deliveredPriority: null,
    isUpdate: false,
    createdAt: 100,
    updatedAt: 100,
    ...overrides
  } as NotificationBatch
}

function nativeNotification() {
  const listeners = new Map<string, () => void>()
  const result = {
    listeners,
    on: vi.fn(),
    show: vi.fn(),
    close: vi.fn()
  }
  result.on.mockImplementation((event: string, listener: () => void) => {
    listeners.set(event, listener)
    return result
  })
  result.show.mockImplementation(() => listeners.get('show')?.())
  return result
}

async function flush(): Promise<void> {
  for (let index = 0; index < 12; index += 1) await Promise.resolve()
}

function harness(initialBatch = batch(), startPaused = false) {
  let currentBatch = initialBatch
  let foreground = false
  let supported = true
  let locale: AppLanguage = 'zh-CN'
  let now = 1_000
  const batches: NotificationBatch[][] = [[initialBatch]]
  const claim = vi.fn(async (input: { claimToken: string }) => ({
    schemaVersion: 1,
    claimToken: input.claimToken,
    batches: batches.shift() ?? []
  }))
  const validate = vi.fn(async () => ({
    schemaVersion: 1,
    batchId: currentBatch.batchId,
    batch: currentBatch
  }))
  const acknowledge = vi.fn(async (input: { batchId: string; disposition: string }) => ({
    schemaVersion: 1,
    batchId: input.batchId,
    status: input.disposition === 'delivered' ? 'displayed' : 'suppressed',
    disposition: input.disposition,
    acknowledgedAt: now
  }))
  const release = vi.fn(async (input: { batchId: string; retryAt: number }) => ({
    schemaVersion: 1,
    batchId: input.batchId,
    status: 'pending',
    retryAt: input.retryAt
  }))
  const list = vi.fn(async () => ({
    schemaVersion: 1,
    items: currentBatch.items,
    nextCursor: null,
    unreadCount: currentBatch.items.filter((entry) => entry.seenAt === null).length,
    lastSequence: 0
  }))
  const notifications: ReturnType<typeof nativeNotification>[] = []
  const createNotification = vi.fn(() => {
    const notification = nativeNotification()
    notifications.push(notification)
    return notification
  })
  const onOpenRequested = vi.fn()
  const clearInterval = vi.fn()
  const clearTimeout = vi.fn()
  const timers: Array<{ handler: () => void; timeoutMs: number }> = []
  const coordinator = new SystemNotificationCoordinator({
    coreServer: {
      claimNotificationBatches: claim,
      validateNotificationBatch: validate,
      acknowledgeNotificationBatch: acknowledge,
      releaseNotificationBatch: release,
      listNotifications: list
    } as never,
    onOpenRequested,
    startPaused,
    dependencies: {
      clearInterval: clearInterval as never,
      clearTimeout: clearTimeout as never,
      createClaimToken: () => 'claim-1',
      createNotification: createNotification as never,
      getLocale: () => locale,
      isAppForeground: () => foreground,
      isNotificationSupported: () => supported,
      now: () => now,
      replaceNotification: () => null,
      setInterval: (() => ({ unref: vi.fn() })) as never,
      setTimeout: ((handler: () => void, timeoutMs: number) => {
        timers.push({ handler, timeoutMs })
        return { unref: vi.fn() }
      }) as never
    }
  })
  return {
    acknowledge,
    batches,
    claim,
    clearInterval,
    coordinator,
    createNotification,
    notifications,
    onOpenRequested,
    release,
    list,
    setBatch(value: NotificationBatch) {
      currentBatch = value
    },
    setForeground(value: boolean) {
      foreground = value
    },
    setLocale(value: AppLanguage) {
      locale = value
    },
    setNow(value: number) {
      now = value
    },
    setSupported(value: boolean) {
      supported = value
    },
    timers,
    validate
  }
}

describe('System notification coordinator', () => {
  beforeEach(() => vi.restoreAllMocks())

  it('holds a cold-start backlog until the first application window is ready', async () => {
    const state = harness(batch(), true)
    await flush()
    expect(state.claim).not.toHaveBeenCalled()
    expect(state.createNotification).not.toHaveBeenCalled()

    state.coordinator.start()
    await flush()
    expect(state.claim).toHaveBeenCalledOnce()
    expect(state.createNotification).toHaveBeenCalledOnce()
    state.coordinator.stop()
  })

  it('shows one background task notification with sound and exact navigation', async () => {
    const state = harness()
    await flush()

    expect(state.createNotification).toHaveBeenCalledWith(
      {
        title: '任务已完成',
        body: '“制定单元设备技术报告方案”已完成。',
        silent: false
      },
      'batch-1'
    )
    expect(state.acknowledge).toHaveBeenCalledWith({
      schemaVersion: 1,
      batchId: 'batch-1',
      claimToken: 'claim-1',
      disposition: 'delivered',
      nativePriority: 'completed',
      soundLevelPlayed: 'initial',
      nativeRevision: 1
    })

    state.notifications[0]?.listeners.get('click')?.()
    await flush()
    expect(state.notifications[0]?.close).toHaveBeenCalledOnce()
    expect(state.onOpenRequested).toHaveBeenCalledWith(
      {
        schemaVersion: 1,
        batchId: 'batch-1',
        eventIds: ['event-1'],
        destination: {
          kind: 'conversation',
          conversationId: 'conversation-1',
          messageId: 'assistant-message-1',
          approvalActionId: null
        }
      },
      ['event-1']
    )
    state.coordinator.stop()
  })

  it('suppresses a batch created while the app is foreground and never shows it later', async () => {
    const state = harness()
    state.setForeground(true)
    await flush()

    expect(state.createNotification).not.toHaveBeenCalled()
    expect(state.acknowledge).toHaveBeenCalledWith(
      expect.objectContaining({
        batchId: 'batch-1',
        disposition: 'suppressed_foreground',
        soundLevelPlayed: 'none'
      })
    )
    state.setForeground(false)
    state.coordinator.requestDrain()
    await flush()
    expect(state.createNotification).not.toHaveBeenCalled()
    state.coordinator.stop()
  })

  it('rechecks focus immediately before native show and closes the validation race', async () => {
    const state = harness()
    const focusChecks = [false, true]
    const isAppForeground = vi.fn(() => focusChecks.shift() ?? true)
    state.coordinator.stop()
    state.batches.push([batch()])

    const fresh = new SystemNotificationCoordinator({
      coreServer: {
        claimNotificationBatches: state.claim,
        validateNotificationBatch: state.validate,
        acknowledgeNotificationBatch: state.acknowledge,
        releaseNotificationBatch: state.release
      } as never,
      onOpenRequested: state.onOpenRequested,
      dependencies: {
        clearInterval: vi.fn() as never,
        createClaimToken: () => 'claim-1',
        createNotification: state.createNotification as never,
        getLocale: () => 'zh-CN',
        isAppForeground,
        isNotificationSupported: () => true,
        setInterval: (() => ({ unref: vi.fn() })) as never,
        setTimeout: (() => ({ unref: vi.fn() })) as never
      }
    })
    await flush()

    expect(isAppForeground).toHaveBeenCalledTimes(2)
    expect(state.createNotification).not.toHaveBeenCalled()
    expect(state.acknowledge).toHaveBeenCalledWith(
      expect.objectContaining({ disposition: 'suppressed_foreground' })
    )
    fresh.stop()
  })

  it('opens the highest-priority navigable member from the exact merged snapshot', async () => {
    const many = batch({
      revision: 4,
      highestPriority: 'approval_required',
      counts: {
        completed: 2,
        failed: 1,
        cancelled: 0,
        approvalRequired: 1,
        importantUpdate: 0,
        configurationBlocked: 0
      },
      itemCount: 4,
      items: [
        item(),
        item({ eventId: 'event-2', kind: 'task_completed', runId: 'run-2' }),
        item({ eventId: 'event-3', kind: 'task_failed', priority: 'failed', runId: 'run-3' }),
        item({
          eventId: 'event-4',
          kind: 'approval_required',
          priority: 'approval_required',
          runId: 'run-4',
          conversationId: 'conversation-4',
          assistantMessageId: 'assistant-message-4',
          approvalActionId: 'approval-action-4',
          occurredAt: 400
        })
      ]
    })
    const state = harness(many)
    await flush()

    expect(state.createNotification).toHaveBeenCalledWith(
      {
        title: '4 项任务有新状态',
        body: '1 项需要批准 · 1 项失败 · 2 项已完成',
        silent: false
      },
      'batch-1'
    )
    state.notifications[0]?.listeners.get('click')?.()
    await flush()
    expect(state.onOpenRequested).toHaveBeenCalledWith(
      {
        schemaVersion: 1,
        batchId: 'batch-1',
        eventIds: ['event-1', 'event-2', 'event-3', 'event-4'],
        destination: {
          kind: 'conversation',
          conversationId: 'conversation-4',
          messageId: 'assistant-message-4',
          approvalActionId: 'approval-action-4'
        }
      },
      ['event-1', 'event-2', 'event-3', 'event-4']
    )
    state.coordinator.stop()
  })

  it('uses the newest member within one priority and skips unrouteable or silently added facts', async () => {
    const shown = batch({
      revision: 4,
      highestPriority: 'approval_required',
      counts: {
        completed: 1,
        failed: 2,
        cancelled: 0,
        approvalRequired: 1,
        importantUpdate: 0,
        configurationBlocked: 0
      },
      itemCount: 4,
      items: [
        item(),
        item({
          eventId: 'event-failed-old',
          kind: 'automation_failed',
          sourceKind: 'automation',
          priority: 'failed',
          occurredAt: 200,
          conversationId: null,
          automationId: 'automation-old',
          runId: 'automation-run-old'
        }),
        item({
          eventId: 'event-failed-new',
          kind: 'task_failed',
          priority: 'failed',
          occurredAt: 300,
          conversationId: 'conversation-new',
          assistantMessageId: null,
          userMessageId: 'user-message-new'
        }),
        item({
          eventId: 'event-unrouteable-approval',
          kind: 'approval_required',
          priority: 'approval_required',
          occurredAt: 400,
          conversationId: null,
          automationId: null,
          approvalActionId: 'approval-unrouteable'
        })
      ]
    })
    const state = harness(shown)
    await flush()

    // This member joined the durable batch after the native card was shown. It must not hijack
    // the click target or be marked seen by a click on the earlier OS snapshot.
    state.setBatch(
      batch({
        ...shown,
        revision: 5,
        itemCount: 5,
        items: [
          ...shown.items,
          item({
            eventId: 'event-silent-approval',
            kind: 'approval_required',
            priority: 'approval_required',
            occurredAt: 500,
            conversationId: 'conversation-silent',
            approvalActionId: 'approval-silent'
          })
        ]
      })
    )
    state.notifications[0]?.listeners.get('click')?.()
    await flush()

    expect(state.onOpenRequested).toHaveBeenCalledWith(
      {
        schemaVersion: 1,
        batchId: 'batch-1',
        eventIds: ['event-1', 'event-failed-old', 'event-failed-new', 'event-unrouteable-approval'],
        destination: {
          kind: 'conversation',
          conversationId: 'conversation-new',
          messageId: 'user-message-new',
          approvalActionId: null
        }
      },
      ['event-1', 'event-failed-old', 'event-failed-new', 'event-unrouteable-approval']
    )
    state.coordinator.stop()
  })

  it('falls back to focusing the application when a live snapshot has no navigation target', async () => {
    const state = harness(
      batch({
        items: [item({ conversationId: null, automationId: null })]
      })
    )
    await flush()

    state.notifications[0]?.listeners.get('click')?.()
    await flush()
    expect(state.onOpenRequested).toHaveBeenCalledWith(
      {
        schemaVersion: 1,
        batchId: 'batch-1',
        eventIds: ['event-1'],
        destination: { kind: 'application' }
      },
      ['event-1']
    )
    state.coordinator.stop()
  })

  it('routes a configuration-blocked automation ahead of failures and lower priorities', async () => {
    const blocked = item({
      eventId: 'event-blocked',
      kind: 'automation_configuration_blocked',
      sourceKind: 'automation',
      priority: 'configuration_blocked',
      occurredAt: 200,
      conversationId: null,
      automationId: 'automation-blocked',
      runId: 'automation-run-blocked'
    })
    const state = harness(
      batch({
        highestPriority: 'configuration_blocked',
        counts: {
          completed: 1,
          failed: 1,
          cancelled: 0,
          approvalRequired: 0,
          importantUpdate: 0,
          configurationBlocked: 1
        },
        itemCount: 3,
        items: [
          item(),
          item({ eventId: 'event-failed', kind: 'task_failed', priority: 'failed' }),
          blocked
        ]
      })
    )
    await flush()

    state.notifications[0]?.listeners.get('click')?.()
    await flush()
    expect(state.onOpenRequested).toHaveBeenCalledWith(
      {
        schemaVersion: 1,
        batchId: 'batch-1',
        eventIds: ['event-1', 'event-failed', 'event-blocked'],
        destination: {
          kind: 'automation',
          automationId: 'automation-blocked',
          runId: 'automation-run-blocked'
        }
      },
      ['event-1', 'event-failed', 'event-blocked']
    )
    state.coordinator.stop()
  })

  it('honors a collection window extended after claim before showing the batch', async () => {
    const collecting = batch({ collectUntil: 2_000 })
    const state = harness(collecting)
    await flush()

    expect(state.createNotification).not.toHaveBeenCalled()
    expect(state.acknowledge).not.toHaveBeenCalled()
    const collectionTimer = state.timers.find((timer) => timer.timeoutMs === 1_000)
    expect(collectionTimer).toBeDefined()

    state.setNow(2_000)
    collectionTimer?.handler()
    await flush()
    expect(state.createNotification).toHaveBeenCalledOnce()
    expect(state.acknowledge).toHaveBeenCalledWith(
      expect.objectContaining({ disposition: 'delivered', nativeRevision: 1 })
    )
    state.coordinator.stop()
  })

  it('reads the mirrored application locale at presentation time', async () => {
    const collecting = batch({ collectUntil: 2_000 })
    const state = harness(collecting)
    await flush()
    const collectionTimer = state.timers.find((timer) => timer.timeoutMs === 1_000)
    expect(collectionTimer).toBeDefined()

    state.setLocale('en-US')
    state.setNow(2_000)
    collectionTimer?.handler()
    await flush()

    expect(state.createNotification).toHaveBeenCalledWith(
      {
        title: 'Task completed',
        body: '“制定单元设备技术报告方案” completed.',
        silent: false
      },
      'batch-1'
    )
    state.coordinator.stop()
  })

  it('acknowledges ordinary increments without creating notification storms on Electron 39', async () => {
    const state = harness()
    await flush()
    state.setNow(4_000)
    const updated = batch({
      revision: 2,
      itemCount: 2,
      items: [item(), item({ eventId: 'event-2', runId: 'run-2' })],
      counts: {
        completed: 2,
        failed: 0,
        cancelled: 0,
        approvalRequired: 0,
        importantUpdate: 0,
        configurationBlocked: 0
      },
      isUpdate: true,
      deliveredRevision: 1,
      deliveredPriority: 'completed',
      soundLevelPlayed: 'initial'
    })
    state.setBatch(updated)
    state.batches.push([updated])
    state.coordinator.requestDrain()
    await flush()

    expect(state.createNotification).toHaveBeenCalledOnce()
    expect(state.acknowledge).toHaveBeenLastCalledWith(
      expect.objectContaining({
        batchId: 'batch-1',
        disposition: 'delivered',
        soundLevelPlayed: 'initial',
        nativeRevision: 2
      })
    )
    // The original native click remains bound to the exact snapshot the user actually saw.
    state.notifications[0]?.listeners.get('click')?.()
    await flush()
    expect(state.onOpenRequested).toHaveBeenLastCalledWith(
      {
        schemaVersion: 1,
        batchId: 'batch-1',
        eventIds: ['event-1'],
        destination: {
          kind: 'conversation',
          conversationId: 'conversation-1',
          messageId: 'assistant-message-1',
          approvalActionId: null
        }
      },
      ['event-1']
    )
    state.coordinator.stop()
  })

  it('throttles a native batch increment before using the Electron 39 silent fallback', async () => {
    const state = harness()
    await flush()
    const updated = batch({
      revision: 2,
      itemCount: 2,
      items: [item(), item({ eventId: 'event-2', runId: 'run-2' })],
      counts: {
        completed: 2,
        failed: 0,
        cancelled: 0,
        approvalRequired: 0,
        importantUpdate: 0,
        configurationBlocked: 0
      },
      isUpdate: true,
      deliveredRevision: 1,
      deliveredPriority: 'completed',
      soundLevelPlayed: 'initial'
    })
    state.setBatch(updated)
    state.batches.push([updated])
    state.coordinator.requestDrain()
    await flush()

    expect(state.createNotification).toHaveBeenCalledOnce()
    expect(state.acknowledge).toHaveBeenCalledOnce()
    const throttle = state.timers.find((timer) => timer.timeoutMs === 2_500)
    expect(throttle).toBeDefined()

    state.setNow(3_500)
    throttle?.handler()
    await flush()
    expect(state.createNotification).toHaveBeenCalledOnce()
    expect(state.acknowledge).toHaveBeenLastCalledWith(
      expect.objectContaining({ nativeRevision: 2, soundLevelPlayed: 'initial' })
    )
    state.coordinator.stop()
  })

  it('permits one priority-upgrade sound while replacing the previous Electron 39 entry', async () => {
    const state = harness()
    await flush()
    state.setNow(5_000)
    const failed = batch({
      revision: 2,
      highestPriority: 'failed',
      counts: {
        completed: 1,
        failed: 1,
        cancelled: 0,
        approvalRequired: 0,
        importantUpdate: 0,
        configurationBlocked: 0
      },
      itemCount: 2,
      items: [item(), item({ eventId: 'event-2', kind: 'task_failed', priority: 'failed' })],
      isUpdate: true,
      deliveredRevision: 1,
      deliveredPriority: 'completed',
      soundLevelPlayed: 'initial'
    })
    state.setBatch(failed)
    state.batches.push([failed])
    state.coordinator.requestDrain()
    await flush()

    expect(state.notifications[0]?.close).toHaveBeenCalledOnce()
    expect(state.createNotification).toHaveBeenCalledTimes(2)
    expect(state.createNotification).toHaveBeenLastCalledWith(
      expect.objectContaining({ silent: false }),
      'batch-1'
    )
    expect(state.acknowledge).toHaveBeenLastCalledWith(
      expect.objectContaining({ soundLevelPlayed: 'upgrade', nativeRevision: 2 })
    )
    state.coordinator.stop()
  })

  it('preserves a priority upgrade across a Main restart using the durable delivered priority', async () => {
    const failedAfterRestart = batch({
      revision: 2,
      highestPriority: 'failed',
      counts: {
        completed: 1,
        failed: 1,
        cancelled: 0,
        approvalRequired: 0,
        importantUpdate: 0,
        configurationBlocked: 0
      },
      itemCount: 2,
      items: [item(), item({ eventId: 'event-2', kind: 'task_failed', priority: 'failed' })],
      isUpdate: true,
      deliveredRevision: 1,
      deliveredPriority: 'completed',
      soundLevelPlayed: 'initial'
    })
    const state = harness(failedAfterRestart)
    await flush()

    expect(state.createNotification).toHaveBeenCalledOnce()
    expect(state.createNotification).toHaveBeenCalledWith(
      expect.objectContaining({ silent: false }),
      'batch-1'
    )
    expect(state.acknowledge).toHaveBeenCalledWith(
      expect.objectContaining({
        nativePriority: 'failed',
        soundLevelPlayed: 'upgrade',
        nativeRevision: 2
      })
    )
    state.coordinator.stop()
  })

  it('shows a silent visual upgrade when sound is disabled', async () => {
    const state = harness(batch({ soundEnabled: false }))
    await flush()
    expect(state.createNotification).toHaveBeenCalledWith(
      expect.objectContaining({ silent: true }),
      'batch-1'
    )

    state.setNow(5_000)
    const approval = batch({
      revision: 2,
      highestPriority: 'approval_required',
      counts: {
        completed: 1,
        failed: 0,
        cancelled: 0,
        approvalRequired: 1,
        importantUpdate: 0,
        configurationBlocked: 0
      },
      itemCount: 2,
      items: [
        item(),
        item({
          eventId: 'event-2',
          kind: 'approval_required',
          priority: 'approval_required'
        })
      ],
      isUpdate: true,
      deliveredRevision: 1,
      deliveredPriority: 'completed',
      soundEnabled: false,
      soundLevelPlayed: 'none'
    })
    state.setBatch(approval)
    state.batches.push([approval])
    state.coordinator.requestDrain()
    await flush()

    expect(state.createNotification).toHaveBeenCalledTimes(2)
    expect(state.createNotification).toHaveBeenLastCalledWith(
      expect.objectContaining({ silent: true }),
      'batch-1'
    )
    expect(state.acknowledge).toHaveBeenLastCalledWith(
      expect.objectContaining({ nativePriority: 'approval_required', soundLevelPlayed: 'none' })
    )
    state.coordinator.stop()
  })

  it('plays the upgrade once when sound was enabled after the initial silent delivery', async () => {
    const failed = batch({
      revision: 2,
      highestPriority: 'failed',
      counts: {
        completed: 1,
        failed: 1,
        cancelled: 0,
        approvalRequired: 0,
        importantUpdate: 0,
        configurationBlocked: 0
      },
      itemCount: 2,
      items: [item(), item({ eventId: 'event-2', kind: 'task_failed', priority: 'failed' })],
      isUpdate: true,
      deliveredRevision: 1,
      deliveredPriority: 'completed',
      soundEnabled: true,
      soundLevelPlayed: 'none'
    })
    const state = harness(failed)
    await flush()

    expect(state.createNotification).toHaveBeenCalledWith(
      expect.objectContaining({ silent: false }),
      'batch-1'
    )
    expect(state.acknowledge).toHaveBeenCalledWith(
      expect.objectContaining({ nativePriority: 'failed', soundLevelPlayed: 'upgrade' })
    )
    state.coordinator.stop()
  })

  it('treats an important automation update as the one visual priority upgrade', async () => {
    const important = batch({
      revision: 2,
      highestPriority: 'important_update',
      counts: {
        completed: 1,
        failed: 0,
        cancelled: 0,
        approvalRequired: 0,
        importantUpdate: 1,
        configurationBlocked: 0
      },
      itemCount: 2,
      items: [
        item(),
        item({
          eventId: 'event-2',
          kind: 'automation_important_update',
          sourceKind: 'automation',
          automationId: 'automation-1',
          priority: 'important_update'
        })
      ],
      isUpdate: true,
      deliveredRevision: 1,
      deliveredPriority: 'completed',
      soundLevelPlayed: 'initial'
    })
    const state = harness(important)
    await flush()

    expect(state.createNotification).toHaveBeenCalledOnce()
    expect(state.acknowledge).toHaveBeenCalledWith(
      expect.objectContaining({ nativePriority: 'important_update', soundLevelPlayed: 'upgrade' })
    )
    state.coordinator.stop()
  })

  it('does not create another banner for routine increments after the one upgrade was played', async () => {
    const state = harness()
    await flush()
    state.setNow(5_000)
    const failed = batch({
      revision: 2,
      highestPriority: 'failed',
      counts: {
        completed: 1,
        failed: 1,
        cancelled: 0,
        approvalRequired: 0,
        importantUpdate: 0,
        configurationBlocked: 0
      },
      itemCount: 2,
      items: [item(), item({ eventId: 'event-2', kind: 'task_failed', priority: 'failed' })],
      isUpdate: true,
      deliveredRevision: 1,
      deliveredPriority: 'completed',
      soundLevelPlayed: 'initial'
    })
    state.setBatch(failed)
    state.batches.push([failed])
    state.coordinator.requestDrain()
    await flush()
    expect(state.createNotification).toHaveBeenCalledTimes(2)

    state.setNow(9_000)
    const routineIncrement = batch({
      revision: 3,
      highestPriority: 'failed',
      counts: {
        completed: 2,
        failed: 1,
        cancelled: 0,
        approvalRequired: 0,
        importantUpdate: 0,
        configurationBlocked: 0
      },
      itemCount: 3,
      items: [
        item(),
        item({ eventId: 'event-2', kind: 'task_failed', priority: 'failed' }),
        item({ eventId: 'event-3', sourceId: 'run-3', runId: 'run-3' })
      ],
      isUpdate: true,
      deliveredRevision: 2,
      deliveredPriority: 'failed',
      soundLevelPlayed: 'upgrade'
    })
    state.setBatch(routineIncrement)
    state.batches.push([routineIncrement])
    state.coordinator.requestDrain()
    await flush()

    expect(state.createNotification).toHaveBeenCalledTimes(2)
    expect(state.acknowledge).toHaveBeenLastCalledWith(
      expect.objectContaining({
        nativePriority: 'failed',
        soundLevelPlayed: 'upgrade',
        nativeRevision: 3
      })
    )
    state.coordinator.stop()
  })

  it('shows at most one visual escalation as a merged batch rises through attention priorities', async () => {
    const state = harness()
    await flush()

    const upgrades = [
      batch({
        revision: 2,
        highestPriority: 'failed',
        deliveredRevision: 1,
        deliveredPriority: 'completed',
        soundLevelPlayed: 'initial',
        isUpdate: true,
        counts: {
          completed: 1,
          failed: 1,
          cancelled: 0,
          approvalRequired: 0,
          importantUpdate: 0,
          configurationBlocked: 0
        },
        itemCount: 2,
        items: [item(), item({ eventId: 'event-2', kind: 'task_failed', priority: 'failed' })]
      }),
      batch({
        revision: 3,
        highestPriority: 'configuration_blocked',
        deliveredRevision: 2,
        deliveredPriority: 'failed',
        soundLevelPlayed: 'upgrade',
        isUpdate: true
      }),
      batch({
        revision: 4,
        highestPriority: 'approval_required',
        deliveredRevision: 3,
        deliveredPriority: 'configuration_blocked',
        soundLevelPlayed: 'upgrade',
        isUpdate: true
      })
    ]

    for (const [index, update] of upgrades.entries()) {
      state.setNow(5_000 + index * 4_000)
      state.setBatch(update)
      state.batches.push([update])
      state.coordinator.requestDrain()
      await flush()
    }

    expect(state.createNotification).toHaveBeenCalledTimes(2)
    expect(state.acknowledge).toHaveBeenLastCalledWith(
      expect.objectContaining({
        nativePriority: 'configuration_blocked',
        soundLevelPlayed: 'upgrade',
        nativeRevision: 4
      })
    )
    state.coordinator.stop()
  })

  it('still escalates a displayed batch when its initial durable acknowledgement was transiently lost', async () => {
    const state = harness(batch(), true)
    state.acknowledge.mockRejectedValueOnce(new Error('sqlite temporarily busy'))
    state.coordinator.start()
    await flush()
    expect(state.createNotification).toHaveBeenCalledOnce()

    const failed = batch({
      revision: 2,
      highestPriority: 'failed',
      counts: {
        completed: 1,
        failed: 1,
        cancelled: 0,
        approvalRequired: 0,
        importantUpdate: 0,
        configurationBlocked: 0
      },
      itemCount: 2,
      items: [item(), item({ eventId: 'event-2', kind: 'task_failed', priority: 'failed' })],
      isUpdate: true,
      deliveredRevision: null,
      deliveredPriority: null,
      soundLevelPlayed: 'none'
    })
    state.setNow(5_000)
    state.setBatch(failed)
    state.batches.push([failed])
    state.coordinator.requestDrain()
    await flush()

    expect(state.createNotification).toHaveBeenCalledTimes(2)
    expect(state.createNotification).toHaveBeenLastCalledWith(
      expect.objectContaining({ silent: false }),
      'batch-1'
    )
    expect(state.acknowledge).toHaveBeenLastCalledWith(
      expect.objectContaining({
        nativePriority: 'failed',
        soundLevelPlayed: 'upgrade',
        nativeRevision: 2
      })
    )
    state.coordinator.stop()
  })

  it('does not clear an already displayed native item when the app merely regains focus', async () => {
    const state = harness()
    await flush()
    const shown = state.notifications[0]
    const updated = batch({
      revision: 2,
      isUpdate: true,
      deliveredRevision: 1,
      deliveredPriority: 'completed',
      soundLevelPlayed: 'initial'
    })
    state.setBatch(updated)
    state.batches.push([updated])
    state.setForeground(true)
    state.coordinator.requestDrain()
    await flush()

    expect(state.createNotification).toHaveBeenCalledOnce()
    expect(shown?.close).not.toHaveBeenCalled()
    expect(state.acknowledge).toHaveBeenLastCalledWith(
      expect.objectContaining({ disposition: 'suppressed_foreground' })
    )
    state.coordinator.stop()
  })

  it('withdraws an invalidated native card and refuses its stale navigation target', async () => {
    const state = harness()
    await flush()
    const shown = state.notifications[0]
    state.setBatch(batch({ items: [], itemCount: 0 }))

    state.coordinator.handleNotificationEvent({
      schemaVersion: 1,
      sequence: 2,
      eventId: 'change-2',
      kind: 'resolved',
      notificationId: 'event-1',
      batchId: 'batch-1',
      resourceRevision: null,
      occurredAt: 2_000
    })
    await flush()

    expect(shown?.close).toHaveBeenCalledOnce()
    shown?.listeners.get('click')?.()
    await flush()
    expect(state.onOpenRequested).not.toHaveBeenCalled()
    state.coordinator.stop()
  })

  it('withdraws a merged native card when one displayed member is seen', async () => {
    const second = item({ eventId: 'event-2', sourceId: 'run-2', runId: 'run-2' })
    const initial = batch({
      items: [item(), second],
      itemCount: 2,
      counts: {
        completed: 2,
        failed: 0,
        cancelled: 0,
        approvalRequired: 0,
        importantUpdate: 0,
        configurationBlocked: 0
      }
    })
    const state = harness(initial)
    await flush()
    const shown = state.notifications[0]
    state.setBatch(batch({ items: [second], itemCount: 1 }))

    state.coordinator.handleNotificationEvent({
      schemaVersion: 1,
      sequence: 2,
      eventId: 'change-2',
      kind: 'seen',
      notificationId: 'event-1',
      batchId: 'batch-1',
      resourceRevision: null,
      occurredAt: 2_000
    })
    await flush()

    expect(shown?.close).toHaveBeenCalledOnce()
    expect(state.createNotification).toHaveBeenCalledOnce()
    state.coordinator.stop()
  })

  it('suppresses native delivery when the platform does not support notifications', async () => {
    const state = harness()
    state.setSupported(false)
    await flush()

    expect(state.createNotification).not.toHaveBeenCalled()
    expect(state.acknowledge).toHaveBeenCalledWith(
      expect.objectContaining({ disposition: 'suppressed_disabled' })
    )
    state.coordinator.stop()
  })

  it('fences a validation that finishes after shutdown', async () => {
    type ValidationResult = { schemaVersion: number; batchId: string; batch: NotificationBatch }
    let resolveValidation: (value: ValidationResult) => void = () => undefined
    const state = harness()
    state.validate.mockImplementationOnce(
      () =>
        new Promise<ValidationResult>((resolve) => {
          resolveValidation = resolve
        })
    )
    await flush()
    state.coordinator.stop()
    resolveValidation({ schemaVersion: 1, batchId: 'batch-1', batch: batch() })
    await flush()

    expect(state.createNotification).not.toHaveBeenCalled()
    expect(state.acknowledge).not.toHaveBeenCalled()
  })
})
