import { describe, expect, it, vi } from 'vitest'
import type {
  AgentTreeSnapshot,
  CollaborationEventEnvelope,
  CollaborationEventsPage
} from '@mycopilot/protocol'
import type { CollaborationDataSource } from './collaborationClient'
import { CollaborationStore } from './collaborationStore'
import { MAX_COLLABORATION_TIMELINE_ACTIVITIES } from './collaborationStore'

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
    schemaVersion: 2,
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
    activity: null,
    occurredAt: sequence
  }
}

function activityEvent(
  rootConversationId: string,
  sequence: number,
  agentId: string,
  semantic: NonNullable<CollaborationEventEnvelope['activity']>['semantic'],
  taskNameSnapshot = agentId,
  anchored = true
): CollaborationEventEnvelope {
  const outerAgentId = semantic === 'updated' ? `root:${rootConversationId}` : agentId
  const kind =
    semantic === 'started'
      ? 'wake_created'
      : semantic === 'updated'
        ? 'mailbox_enqueued'
        : semantic === 'waiting_approval'
          ? 'approval_projected'
          : 'wake_updated'
  return {
    ...event(rootConversationId, sequence, outerAgentId),
    kind,
    activity: {
      schemaVersion: 2,
      agentId,
      rootAnchorMessageId: anchored ? 'root-assistant-1' : null,
      rootTraceBoundarySequence: anchored ? sequence : null,
      semantic,
      taskNameSnapshot
    }
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

function durableEventsThrough(
  rootConversationId: string,
  afterSequence: number,
  lastSequence: number,
  agentId?: string
): CollaborationEventEnvelope[] {
  return Array.from({ length: Math.max(0, lastSequence - afterSequence) }, (_, index) =>
    event(rootConversationId, afterSequence + index + 1, agentId)
  )
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
          durableEventsThrough(
            'root-conversation',
            afterSequence,
            exposeEvents ? 4 : authoritative.lastSequence
          )
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
          durableEventsThrough('root-conversation', afterSequence, authoritative.lastSequence)
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
      listEvents: vi.fn(async ({ afterSequence }) =>
        page('root-conversation', durableEventsThrough('root-conversation', afterSequence, 1))
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
      listEvents: vi.fn(async ({ afterSequence }) =>
        page(
          'root-conversation',
          durableEventsThrough('root-conversation', afterSequence, authoritative.lastSequence)
        )
      ),
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
      activities: [],
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
      listEvents: vi.fn(async ({ afterSequence }) =>
        page('root-conversation', durableEventsThrough('root-conversation', afterSequence, 2))
      ),
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
          exposeEvents && afterSequence === 2
            ? [event('root-conversation', 3, childB)]
            : durableEventsThrough('root-conversation', afterSequence, authoritative.lastSequence)
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
      listEvents: vi.fn(async ({ afterSequence }) =>
        page(
          'root-conversation',
          exposeGap && afterSequence === 2
            ? [event('root-conversation', 4, childB)]
            : durableEventsThrough(
                'root-conversation',
                afterSequence,
                authoritative.lastSequence,
                exposeGap ? childB : undefined
              )
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

  it('continues durable replay when the authoritative snapshot advances during catch-up', async () => {
    let handler: ((value: CollaborationEventEnvelope) => void) | undefined
    const childA = 'agent-child-a'
    const childB = 'agent-child-b'
    let exposeEvents = false
    let servedInitialIncrement = false
    let authoritative: AgentTreeSnapshot = {
      ...tree('root-conversation', 2),
      agents: [childSummary(childA, 'child-a'), childSummary(childB, 'child-b')]
    }
    const source: CollaborationDataSource = {
      getTree: vi.fn(async () => authoritative),
      listEvents: vi.fn(async ({ afterSequence }) =>
        page(
          'root-conversation',
          exposeEvents && afterSequence === 2 && !servedInitialIncrement
            ? ((servedInitialIncrement = true), [event('root-conversation', 3, childA)])
            : exposeEvents
              ? durableEventsThrough(
                  'root-conversation',
                  afterSequence,
                  authoritative.lastSequence,
                  childB
                )
              : durableEventsThrough('root-conversation', afterSequence, authoritative.lastSequence)
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
      agentInvalidationSequences: { [childA]: 3, [childB]: 5 },
      hydrationRevision,
      tree: { lastSequence: 5 }
    })
  })

  it('rebuilds semantic activity after restart and ignores duplicate or out-of-order notices', async () => {
    let handler: ((value: CollaborationEventEnvelope) => void) | undefined
    let durable = [
      activityEvent('root-conversation', 1, 'agent-a', 'started', 'Researcher'),
      activityEvent('root-conversation', 2, 'agent-a', 'updated', 'Researcher')
    ]
    let authoritative = tree('root-conversation', durable.length)
    const source: CollaborationDataSource = {
      getTree: vi.fn(async () => authoritative),
      listEvents: vi.fn(async ({ afterSequence, limit }) => {
        const events = durable.slice(afterSequence, afterSequence + limit)
        return page('root-conversation', events, afterSequence + events.length < durable.length)
      }),
      subscribe(next) {
        handler = next
        return () => undefined
      },
      subscribeResync: () => () => undefined
    }

    const first = new CollaborationStore('root-conversation', source)
    first.start()
    await vi.waitFor(() => expect(first.getSnapshot().loading).toBe(false))
    expect(first.getSnapshot().activities.map((activity) => activity.semantic)).toEqual([
      'started',
      'updated'
    ])
    first.destroy()

    durable = [
      ...durable,
      activityEvent('root-conversation', 3, 'agent-a', 'completed', 'Researcher'),
      activityEvent('root-conversation', 4, 'agent-a', 'started', 'Researcher')
    ]
    authoritative = tree('root-conversation', durable.length)
    const reloaded = new CollaborationStore('root-conversation', source)
    reloaded.start()
    await vi.waitFor(() => expect(reloaded.getSnapshot().loading).toBe(false))
    handler?.(durable[3]!)
    handler?.(durable[1]!)
    handler?.(durable[3]!)
    await settle()
    expect(reloaded.getSnapshot().activities.map((activity) => activity.sequence)).toEqual([
      1, 2, 3, 4
    ])
  })

  it('retains the latest deterministic 2,048 semantic activities while advancing the full log', async () => {
    const total = MAX_COLLABORATION_TIMELINE_ACTIVITIES + 17
    const durable = Array.from({ length: total }, (_, index) =>
      activityEvent(
        'root-conversation',
        index + 1,
        `agent-${index % 3}`,
        index % 2 === 0 ? 'started' : 'updated'
      )
    )
    const source: CollaborationDataSource = {
      getTree: vi.fn(async () => tree('root-conversation', total)),
      listEvents: vi.fn(async ({ afterSequence, limit }) => {
        const events = durable.slice(afterSequence, afterSequence + limit)
        return page('root-conversation', events, afterSequence + events.length < total)
      }),
      subscribe: () => () => undefined,
      subscribeResync: () => () => undefined
    }
    const store = new CollaborationStore('root-conversation', source)
    store.start()
    await vi.waitFor(() => expect(store.getSnapshot().loading).toBe(false))

    expect(store.getSnapshot().tree?.lastSequence).toBe(total)
    expect(store.getSnapshot().activities).toHaveLength(MAX_COLLABORATION_TIMELINE_ACTIVITIES)
    expect(store.getSnapshot().activities[0]?.sequence).toBe(18)
    expect(store.getSnapshot().activities.at(-1)?.sequence).toBe(total)
  })

  it('does not let post-terminal unanchored events evict the frozen anchored window', async () => {
    const anchored = Array.from({ length: MAX_COLLABORATION_TIMELINE_ACTIVITIES }, (_, index) =>
      activityEvent('root-conversation', index + 1, 'agent-a', 'started')
    )
    const unanchored = Array.from({ length: 17 }, (_, index) =>
      activityEvent(
        'root-conversation',
        MAX_COLLABORATION_TIMELINE_ACTIVITIES + index + 1,
        'agent-a',
        'completed',
        'agent-a',
        false
      )
    )
    const durable = [...anchored, ...unanchored]
    const authoritative = {
      ...tree('root-conversation', durable.length),
      agents: [childSummary('agent-a', 'conversation-agent-a')]
    }
    const source: CollaborationDataSource = {
      getTree: vi.fn(async () => authoritative),
      listEvents: vi.fn(async ({ afterSequence, limit }) => {
        const events = durable.slice(afterSequence, afterSequence + limit)
        return page('root-conversation', events, afterSequence + events.length < durable.length)
      }),
      subscribe: () => () => undefined,
      subscribeResync: () => () => undefined
    }
    const store = new CollaborationStore('root-conversation', source)
    store.start()
    await vi.waitFor(() => expect(store.getSnapshot().loading).toBe(false))

    expect(store.getSnapshot().tree?.lastSequence).toBe(durable.length)
    expect(store.getSnapshot().agentInvalidationSequences['agent-a']).toBe(durable.length)
    expect(store.getSnapshot().activities).toHaveLength(MAX_COLLABORATION_TIMELINE_ACTIVITIES)
    expect(store.getSnapshot().activities[0]?.sequence).toBe(1)
    expect(store.getSnapshot().activities.at(-1)?.sequence).toBe(
      MAX_COLLABORATION_TIMELINE_ACTIVITIES
    )
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
