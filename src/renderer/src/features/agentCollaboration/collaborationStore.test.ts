import { describe, expect, it, vi } from 'vitest'
import type {
  AgentTreeSnapshot,
  CollaborationEventEnvelope,
  CollaborationEventsPage
} from '@mycopilot/protocol'
import type { CollaborationDataSource } from './collaborationClient'
import { CollaborationStore } from './collaborationStore'

vi.mock('../../host/hostClient', () => ({ hostClient: { agent: {} } }))

function tree(rootConversationId: string, lastSequence: number): AgentTreeSnapshot {
  return {
    schemaVersion: 1,
    workspaceId: 'project-a',
    projectId: 'project-a',
    rootAgentId: `root:${rootConversationId}`,
    rootConversationId,
    agents: [],
    lastSequence
  }
}

function event(
  rootConversationId: string,
  sequence: number,
  agentId = `root:${rootConversationId}`
): CollaborationEventEnvelope {
  return {
    schemaVersion: 1,
    eventId: `${rootConversationId}:${sequence}`,
    sequence,
    workspaceId: 'project-a',
    projectId: 'project-a',
    rootAgentId: `root:${rootConversationId}`,
    rootConversationId,
    agentId,
    conversationId: rootConversationId,
    turnId: null,
    runId: null,
    messageId: null,
    kind: 'agent_updated',
    resourceRevision: sequence,
    occurredAt: sequence
  }
}

function page(
  rootConversationId: string,
  events: CollaborationEventEnvelope[],
  hasMore = false
): CollaborationEventsPage {
  return {
    schemaVersion: 1,
    rootAgentId: `root:${rootConversationId}`,
    rootConversationId,
    events,
    lastSequence: events.at(-1)?.sequence ?? 0,
    hasMore
  }
}

async function settle(): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, 0))
}

describe('CollaborationStore', () => {
  it('releases 1,000 transient subscribers without duplicate delivery or residue', async () => {
    const source: CollaborationDataSource = {
      getTree: vi.fn(async () => null),
      listEvents: vi.fn(),
      subscribe: () => () => undefined,
      subscribeResync: () => () => undefined
    }
    const store = new CollaborationStore('root-conversation', source)
    let activeSubscriber = -1
    let activeDeliveries = 0
    let staleDeliveries = 0

    for (let subscriber = 0; subscriber < 1_000; subscriber += 1) {
      const unsubscribe = store.subscribe(() => {
        if (activeSubscriber === subscriber) activeDeliveries += 1
        else staleDeliveries += 1
      })
      activeSubscriber = subscriber
      await store.hydrate()
      activeSubscriber = -1
      unsubscribe()

      // A second publication in every round proves that the just-released listener and all
      // earlier listeners are absent, rather than merely checking a private Set size.
      await store.hydrate()
    }

    expect(activeDeliveries).toBe(2_000)
    expect(staleDeliveries).toBe(0)
    expect(source.getTree).toHaveBeenCalledTimes(2_000)
  })

  it('ignores duplicates and other roots, then closes an event gap from the durable log', async () => {
    let handler: ((value: CollaborationEventEnvelope) => void) | undefined
    let authoritative = tree('root-conversation', 2)
    let exposeEvents = false
    const source: CollaborationDataSource = {
      getTree: vi.fn(async () => authoritative),
      listEvents: vi.fn(async ({ afterSequence }) =>
        page(
          'root-conversation',
          exposeEvents && afterSequence === 2
            ? [event('root-conversation', 3), event('root-conversation', 4)]
            : []
        )
      ),
      subscribe: vi.fn((next) => {
        handler = next
        return () => undefined
      }),
      subscribeResync: () => () => undefined
    }
    const store = new CollaborationStore('root-conversation', source)
    store.start()
    await settle()

    expect(store.getSnapshot().tree?.lastSequence).toBe(2)
    handler?.(event('other-root', 99))
    handler?.(event('root-conversation', 2))
    expect(source.listEvents).toHaveBeenCalledTimes(1) // initial no-op catch-up

    authoritative = tree('root-conversation', 4)
    exposeEvents = true
    handler?.(event('root-conversation', 4))
    await settle()
    await settle()

    expect(store.getSnapshot().tree?.lastSequence).toBe(4)
    expect(source.listEvents).toHaveBeenLastCalledWith({
      afterSequence: 2,
      limit: 256,
      rootConversationId: 'root-conversation'
    })
  })

  it('replays a 520-event durable history in bounded pages despite duplicate and out-of-order notifications', async () => {
    let handler: ((value: CollaborationEventEnvelope) => void) | undefined
    const childA = 'agent-child-a'
    const childB = 'agent-child-b'
    const durableEvents = Array.from({ length: 520 }, (_, index) => {
      const sequence = index + 1
      return event('root-conversation', sequence, sequence % 2 === 0 ? childB : childA)
    })
    let exposeEvents = false
    let authoritative: AgentTreeSnapshot = {
      ...tree('root-conversation', 0),
      agents: [childSummary(childA, 'child-a'), childSummary(childB, 'child-b')]
    }
    const source: CollaborationDataSource = {
      getTree: vi.fn(async () => authoritative),
      listEvents: vi.fn(async ({ afterSequence, limit }) => {
        if (!exposeEvents) return page('root-conversation', [])
        const events = durableEvents.slice(afterSequence, afterSequence + limit)
        return page('root-conversation', events, afterSequence + events.length < 520)
      }),
      subscribe: (next) => {
        handler = next
        return () => undefined
      },
      subscribeResync: () => () => undefined
    }
    const store = new CollaborationStore('root-conversation', source)
    store.start()
    await settle()
    await settle()
    vi.mocked(source.listEvents).mockClear()

    exposeEvents = true
    authoritative = { ...authoritative, lastSequence: 520 }
    // Notifications only lower latency. They may duplicate or arrive out of order; the store
    // must validate and replay the monotonic durable log instead of reducing these envelopes.
    handler?.(durableEvents[519]!)
    handler?.(durableEvents[2]!)
    handler?.(durableEvents[519]!)

    await vi.waitFor(() => expect(store.getSnapshot().tree?.lastSequence).toBe(520))
    expect(store.getSnapshot()).toMatchObject({
      agentInvalidationSequences: { [childA]: 519, [childB]: 520 },
      error: false,
      loading: false,
      tree: { lastSequence: 520 }
    })
    expect(vi.mocked(source.listEvents).mock.calls.map(([input]) => input.afterSequence)).toEqual(
      expect.arrayContaining([0, 256, 512])
    )
    expect(vi.mocked(source.listEvents).mock.calls.every(([input]) => input.limit === 256)).toBe(
      true
    )
  })

  it('rehydrates independently after a window reload', async () => {
    const handlers = new Set<(value: CollaborationEventEnvelope) => void>()
    let authoritative = tree('root-conversation', 1)
    const source: CollaborationDataSource = {
      getTree: vi.fn(async () => authoritative),
      listEvents: vi.fn(async ({ afterSequence }) =>
        page(
          'root-conversation',
          authoritative.lastSequence > afterSequence
            ? [event('root-conversation', authoritative.lastSequence)]
            : []
        )
      ),
      subscribe(next) {
        handlers.add(next)
        return () => handlers.delete(next)
      },
      subscribeResync: () => () => undefined
    }

    const first = new CollaborationStore('root-conversation', source)
    first.start()
    await settle()
    first.destroy()

    authoritative = tree('root-conversation', 7)
    const reloaded = new CollaborationStore('root-conversation', source)
    reloaded.start()
    await settle()

    expect(reloaded.getSnapshot()).toMatchObject({
      error: false,
      loading: false,
      tree: { rootConversationId: 'root-conversation', lastSequence: 7 }
    })
  })

  it('recovers an initial failure when a durable root event arrives', async () => {
    let handler: ((value: CollaborationEventEnvelope) => void) | undefined
    let attempts = 0
    const source: CollaborationDataSource = {
      getTree: vi.fn(async () => {
        attempts += 1
        if (attempts === 1) throw new Error('temporarily unavailable')
        return tree('root-conversation', 1)
      }),
      listEvents: vi.fn(async () => page('root-conversation', [])),
      subscribe: (next) => {
        handler = next
        return () => undefined
      },
      subscribeResync: () => () => undefined
    }
    const store = new CollaborationStore('root-conversation', source)
    store.start()
    await settle()
    expect(store.getSnapshot()).toMatchObject({ error: true, tree: null })

    handler?.(event('root-conversation', 1))
    await settle()
    expect(store.getSnapshot()).toMatchObject({
      error: false,
      loading: false,
      tree: { rootConversationId: 'root-conversation', lastSequence: 1 }
    })
  })

  it('rehydrates an existing store when a restarted Core announces a global resync', async () => {
    let resync: (() => void) | undefined
    let authoritative = tree('root-conversation', 2)
    const source: CollaborationDataSource = {
      getTree: vi.fn(async () => authoritative),
      listEvents: vi.fn(async () => page('root-conversation', [])),
      subscribe: () => () => undefined,
      subscribeResync: (next) => {
        resync = next
        return () => undefined
      }
    }
    const store = new CollaborationStore('root-conversation', source)
    store.start()
    await settle()

    // Simulate a durable mutation committed after the old process' last notification and before
    // the replacement process established its notifier cursor.
    authoritative = tree('root-conversation', 3)
    resync?.()
    await settle()

    expect(store.getSnapshot()).toMatchObject({
      error: false,
      tree: { rootConversationId: 'root-conversation', lastSequence: 3 }
    })
  })

  it('treats a legacy unmaterialized root as a healthy empty collaboration state', async () => {
    const source: CollaborationDataSource = {
      getTree: vi.fn(async () => null),
      listEvents: vi.fn(),
      subscribe: () => () => undefined,
      subscribeResync: () => () => undefined
    }
    const store = new CollaborationStore('legacy-conversation', source)
    store.start()
    await settle()
    expect(store.getSnapshot()).toEqual({
      agentInvalidationSequences: {},
      error: false,
      hydrationRevision: 1,
      loading: false,
      rootConversationId: 'legacy-conversation',
      tree: null
    })
  })

  it('advances hydration revision on resync even when the durable sequence is unchanged', async () => {
    let resync: (() => void) | undefined
    const source: CollaborationDataSource = {
      getTree: vi.fn(async () => tree('root-conversation', 2)),
      listEvents: vi.fn(async () => page('root-conversation', [])),
      subscribe: () => () => undefined,
      subscribeResync: (next) => {
        resync = next
        return () => undefined
      }
    }
    const store = new CollaborationStore('root-conversation', source)
    store.start()
    await settle()
    const before = store.getSnapshot().hydrationRevision
    resync?.()
    await settle()
    expect(store.getSnapshot()).toMatchObject({
      hydrationRevision: before + 1,
      tree: { lastSequence: 2 }
    })
  })

  it('advances only the target Agent invalidation sequence after a validated catch-up', async () => {
    let handler: ((value: CollaborationEventEnvelope) => void) | undefined
    const childA = 'agent-child-a'
    const childB = 'agent-child-b'
    let exposeEvents = false
    let authoritative: AgentTreeSnapshot = {
      ...tree('root-conversation', 2),
      agents: [childSummary(childA, 'child-a'), childSummary(childB, 'child-b')]
    }
    const source: CollaborationDataSource = {
      getTree: vi.fn(async () => authoritative),
      listEvents: vi.fn(async ({ afterSequence }) =>
        page(
          'root-conversation',
          exposeEvents && afterSequence === 2 ? [event('root-conversation', 3, childB)] : []
        )
      ),
      subscribe: (next) => {
        handler = next
        return () => undefined
      },
      subscribeResync: () => () => undefined
    }
    const store = new CollaborationStore('root-conversation', source)
    store.start()
    await settle()
    const initial = store.getSnapshot().agentInvalidationSequences
    expect(initial).toEqual({ [childA]: 2, [childB]: 2 })

    authoritative = { ...authoritative, lastSequence: 3 }
    exposeEvents = true
    handler?.(event('root-conversation', 3, childB))
    await settle()
    await settle()

    expect(store.getSnapshot().agentInvalidationSequences).toEqual({
      [childA]: 2,
      [childB]: 3
    })
  })

  it('invalidates every selected observer after an event-log gap forces full hydration', async () => {
    let handler: ((value: CollaborationEventEnvelope) => void) | undefined
    const childA = 'agent-child-a'
    const childB = 'agent-child-b'
    let exposeGap = false
    let authoritative: AgentTreeSnapshot = {
      ...tree('root-conversation', 2),
      agents: [childSummary(childA, 'child-a'), childSummary(childB, 'child-b')]
    }
    const source: CollaborationDataSource = {
      getTree: vi.fn(async () => authoritative),
      listEvents: vi.fn(async () =>
        page('root-conversation', exposeGap ? [event('root-conversation', 4, childB)] : [])
      ),
      subscribe: (next) => {
        handler = next
        return () => undefined
      },
      subscribeResync: () => () => undefined
    }
    const store = new CollaborationStore('root-conversation', source)
    store.start()
    await settle()
    const hydrationRevision = store.getSnapshot().hydrationRevision

    authoritative = { ...authoritative, lastSequence: 4 }
    exposeGap = true
    handler?.(event('root-conversation', 4, childB))
    await settle()
    await settle()

    expect(store.getSnapshot()).toMatchObject({
      agentInvalidationSequences: { [childA]: 4, [childB]: 4 },
      hydrationRevision: hydrationRevision + 1
    })
  })

  it('fully invalidates observers when the authoritative snapshot advances beyond replay', async () => {
    let handler: ((value: CollaborationEventEnvelope) => void) | undefined
    const childA = 'agent-child-a'
    const childB = 'agent-child-b'
    let exposeEvents = false
    let authoritative: AgentTreeSnapshot = {
      ...tree('root-conversation', 2),
      agents: [childSummary(childA, 'child-a'), childSummary(childB, 'child-b')]
    }
    const source: CollaborationDataSource = {
      getTree: vi.fn(async () => authoritative),
      listEvents: vi.fn(async ({ afterSequence }) =>
        page(
          'root-conversation',
          exposeEvents && afterSequence === 2 ? [event('root-conversation', 3, childA)] : []
        )
      ),
      subscribe: (next) => {
        handler = next
        return () => undefined
      },
      subscribeResync: () => () => undefined
    }
    const store = new CollaborationStore('root-conversation', source)
    store.start()
    await settle()
    const hydrationRevision = store.getSnapshot().hydrationRevision

    exposeEvents = true
    authoritative = { ...authoritative, lastSequence: 5 }
    handler?.(event('root-conversation', 3, childA))
    handler?.(event('root-conversation', 4, childB))
    handler?.(event('root-conversation', 5, childB))
    await settle()
    await settle()

    expect(store.getSnapshot()).toMatchObject({
      agentInvalidationSequences: { [childA]: 5, [childB]: 5 },
      hydrationRevision: hydrationRevision + 1,
      tree: { lastSequence: 5 }
    })
  })
})

function childSummary(agentId: string, conversationId: string) {
  return {
    agentId,
    rootAgentId: 'root:root-conversation',
    rootConversationId: 'root-conversation',
    parentAgentId: 'root:root-conversation',
    conversationId,
    projectId: 'project-a',
    taskName: agentId,
    taskPath: `/root/${agentId}`,
    lifecycle: 'active' as const,
    displayStatus: 'running' as const,
    latestActivityAt: 1,
    model: null
  }
}
