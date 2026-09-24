import { afterEach, describe, expect, it, vi } from 'vitest'
import type { AgentEvent, AgentTreeSnapshot, CollaborationEventEnvelope } from '@mycopilot/protocol'
import type { CollaborationDataSource } from './collaborationClient'
import { CollaborationStore, TREE_TRANSMISSION_LIFETIME_MS } from './collaborationStore'

vi.mock('../../host/hostClient', () => ({ hostClient: { agent: {} } }))

const ROOT = 'root-agent'
const CHILD = 'child-agent'
const GRANDCHILD = 'grandchild-agent'
const CONVERSATION = 'root-conversation'

function fixture(initial: CollaborationEventEnvelope[] = []) {
  let notify: (event: CollaborationEventEnvelope) => void = () => undefined
  let agentEvent: (event: AgentEvent) => void = () => undefined
  let resync: () => void = () => undefined
  const events = [...initial]
  const unsubscribeAgent = vi.fn()
  const snapshot: AgentTreeSnapshot = {
    schemaVersion: 1,
    rootAgentId: ROOT,
    rootConversationId: CONVERSATION,
    workspaceId: null,
    projectId: null,
    lastSequence: 0,
    agents: [ROOT, CHILD, GRANDCHILD].map((id, index) => ({
      agentId: id,
      rootAgentId: ROOT,
      rootConversationId: CONVERSATION,
      parentAgentId: [null, ROOT, CHILD][index]!,
      conversationId: index === 0 ? CONVERSATION : `conversation-${id}`,
      projectId: null,
      taskName: id,
      taskPath: `/root/${id}`,
      lifecycle: 'active',
      displayStatus: 'running',
      latestActivityAt: 1,
      model: null
    }))
  }
  const source: CollaborationDataSource = {
    getTree: async () => ({ ...snapshot, lastSequence: events.length }),
    listEvents: async ({ afterSequence, limit }) => ({
      schemaVersion: 1,
      rootAgentId: ROOT,
      rootConversationId: CONVERSATION,
      events: events.slice(afterSequence, afterSequence + limit),
      hasMore: afterSequence + limit < events.length,
      lastSequence: events.length
    }),
    subscribe: (handler) => {
      notify = handler
      return () => undefined
    },
    subscribeResync: (handler) => {
      resync = handler
      return () => undefined
    },
    subscribeAgentEvents: (handler) => {
      agentEvent = handler
      return unsubscribeAgent
    }
  }
  const store = new CollaborationStore(CONVERSATION, source)
  return {
    store,
    events,
    unsubscribeAgent,
    notify: (event: CollaborationEventEnvelope) => notify(event),
    append: (...next: CollaborationEventEnvelope[]) => {
      events.push(...next)
      notify(next.at(-1)!)
    },
    agentEvent: (event: AgentEvent) => agentEvent(event),
    resync: () => resync()
  }
}

function event(
  sequence: number,
  changes: Partial<CollaborationEventEnvelope> = {}
): CollaborationEventEnvelope {
  return {
    schemaVersion: 2,
    eventId: `event-${sequence}`,
    sequence,
    workspaceId: null,
    projectId: null,
    rootAgentId: ROOT,
    rootConversationId: CONVERSATION,
    agentId: CHILD,
    conversationId: 'child-conversation',
    turnId: null,
    runId: null,
    messageId: `message-${sequence}`,
    kind: 'mailbox_enqueued',
    resourceRevision: sequence,
    activity: null,
    occurredAt: Date.now(),
    transmission: {
      id: `mailbox:message-${sequence}`,
      kind: 'message',
      sourceAgentId: ROOT,
      targetAgentId: CHILD
    },
    ...changes
  }
}

function guidance(
  runId: string,
  clientMessageId: string
): Extract<AgentEvent, { type: 'guidance_applied' }> {
  return {
    type: 'guidance_applied',
    runId,
    clientMessageId,
    guidanceId: `guidance-${clientMessageId}`,
    content: 'private input',
    attachments: [],
    createdAt: Date.now(),
    sequence: 1
  }
}

async function settle() {
  await vi.advanceTimersByTimeAsync(0)
}

afterEach(() => vi.useRealTimers())

describe('live tree transmissions', () => {
  it('does not replay history and preserves real cross-level endpoints, then deduplicates receipts', async () => {
    vi.useFakeTimers()
    const f = fixture([event(1)])
    f.store.start()
    await settle()
    expect(f.store.getSnapshot().transmissions).toEqual([])

    const crossLevel = event(2, {
      transmission: {
        id: 'mailbox:cross-level',
        kind: 'task',
        sourceAgentId: ROOT,
        targetAgentId: GRANDCHILD
      }
    })
    const reply = event(3, {
      transmission: {
        id: 'mailbox:reply',
        kind: 'message',
        sourceAgentId: GRANDCHILD,
        targetAgentId: ROOT
      }
    })
    f.append(crossLevel, reply)
    await settle()
    expect(f.store.getSnapshot().transmissions).toMatchObject([
      crossLevel.transmission!,
      reply.transmission!
    ])
    f.notify(crossLevel)
    f.append(event(4, { transmission: reply.transmission }))
    await settle()
    expect(f.store.getSnapshot().transmissions).toHaveLength(2)
    f.store.destroy()
  })

  it('ignores old catch-up events and routes outside the active tree', async () => {
    vi.useFakeTimers()
    const f = fixture()
    f.store.start()
    await settle()
    f.append(
      event(1, { occurredAt: Date.now() - TREE_TRANSMISSION_LIFETIME_MS - 1 }),
      event(2, {
        transmission: {
          id: 'foreign-agent',
          kind: 'message',
          sourceAgentId: ROOT,
          targetAgentId: 'other-tree-agent'
        }
      }),
      event(3, {
        transmission: {
          id: 'false-human-route',
          kind: 'user_message',
          sourceAgentId: null,
          targetAgentId: CHILD
        }
      })
    )
    f.notify(event(4, { rootConversationId: 'another-conversation' }))
    await settle()
    expect(f.store.getSnapshot().transmissions).toEqual([])
    f.store.destroy()
  })

  it('handles submitted root input and completion independently and never duplicates a completed run', async () => {
    vi.useFakeTimers()
    const f = fixture()
    f.store.start()
    await settle()
    const submission = event(1, {
      agentId: ROOT,
      kind: 'turn_started',
      runId: 'root-run',
      transmission: {
        id: 'user-message:human-1',
        kind: 'user_message',
        sourceAgentId: null,
        targetAgentId: ROOT
      }
    })
    f.append(submission)
    await settle()
    const completion = event(2, {
      agentId: ROOT,
      kind: 'turn_updated',
      runId: 'root-run',
      transmission: {
        id: 'completion:root-run',
        kind: 'completion',
        sourceAgentId: ROOT,
        targetAgentId: null
      }
    })
    f.append(completion, event(3, { ...completion, sequence: 3, eventId: 'event-3' }))
    await settle()
    expect(f.store.getSnapshot().transmissions?.map((pulse) => pulse.kind)).toEqual([
      'user_message',
      'completion'
    ])
    f.store.destroy()
  })

  it('only pulses applied guidance for its bound root run, including notification arrival races', async () => {
    vi.useFakeTimers()
    const f = fixture()
    f.store.start()
    await settle()
    // Stream delivery may win the race against durable collaboration catch-up.
    f.agentEvent(guidance('root-run', 'human-guidance'))
    f.agentEvent(guidance('another-root-run', 'foreign-guidance'))
    f.agentEvent({ ...guidance('root-run', 'queued-guidance'), type: 'guidance_queued' })
    f.append(
      event(1, { agentId: ROOT, kind: 'turn_started', runId: 'root-run', transmission: undefined })
    )
    await settle()
    expect(f.store.getSnapshot().transmissions).toMatchObject([
      { id: 'user-message:human-guidance', sourceAgentId: null, targetAgentId: ROOT }
    ])
    f.agentEvent(guidance('root-run', 'human-guidance'))
    f.agentEvent(guidance('root-run', 'next-guidance'))
    expect(f.store.getSnapshot().transmissions).toHaveLength(2)
    expect(JSON.stringify(f.store.getSnapshot().transmissions)).not.toContain('private input')
    f.store.destroy()
    expect(f.unsubscribeAgent).toHaveBeenCalledOnce()
  })

  it('clears pulses on resync without replaying and expires a bounded burst', async () => {
    vi.useFakeTimers()
    const f = fixture()
    f.store.start()
    await settle()
    f.append(...Array.from({ length: 40 }, (_, i) => event(i + 1)))
    await settle()
    expect(f.store.getSnapshot().transmissions).toHaveLength(24)
    f.resync()
    await settle()
    expect(f.store.getSnapshot().transmissions).toEqual([])
    f.append(event(41))
    await settle()
    expect(f.store.getSnapshot().transmissions).toHaveLength(1)
    await vi.advanceTimersByTimeAsync(TREE_TRANSMISSION_LIFETIME_MS)
    expect(f.store.getSnapshot().transmissions).toEqual([])
    f.store.destroy()
    expect(vi.getTimerCount()).toBe(0)
  })
})
