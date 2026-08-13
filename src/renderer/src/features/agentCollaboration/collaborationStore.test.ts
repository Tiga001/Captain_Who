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

function event(rootConversationId: string, sequence: number): CollaborationEventEnvelope {
  return {
    schemaVersion: 1,
    eventId: `${rootConversationId}:${sequence}`,
    sequence,
    workspaceId: 'project-a',
    projectId: 'project-a',
    rootAgentId: `root:${rootConversationId}`,
    rootConversationId,
    agentId: `root:${rootConversationId}`,
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
      error: false,
      loading: false,
      rootConversationId: 'legacy-conversation',
      tree: null
    })
  })
})
