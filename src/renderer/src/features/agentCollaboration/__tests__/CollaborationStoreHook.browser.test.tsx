import { StrictMode } from 'react'
import { beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { AgentTreeRequest, CollaborationEventsRequest } from '@mycopilot/protocol'
import { useOptionalCollaborationStore } from '../useCollaborationStore'

const mocks = vi.hoisted(() => ({
  account: 'first-account',
  getTree: vi.fn(),
  listEvents: vi.fn(),
  subscriptions: new Set<unknown>(),
  resyncHandlers: new Set<() => void>()
}))

vi.mock('../../auth/AccountAuthContext', () => ({
  useAccountAuth: () => ({
    state: { status: 'signedIn', profile: { userId: mocks.account } }
  })
}))

vi.mock('../collaborationClient', () => ({
  hostCollaborationDataSource: {
    getTree: mocks.getTree,
    listEvents: mocks.listEvents,
    subscribe: (handler: unknown) => {
      mocks.subscriptions.add(handler)
      return () => mocks.subscriptions.delete(handler)
    },
    subscribeResync: (handler: () => void) => {
      mocks.resyncHandlers.add(handler)
      return () => mocks.resyncHandlers.delete(handler)
    }
  }
}))

beforeEach(() => {
  mocks.account = 'first-account'
  mocks.subscriptions.clear()
  mocks.resyncHandlers.clear()
  mocks.getTree
    .mockReset()
    .mockImplementation(async ({ rootConversationId }: AgentTreeRequest) => ({
      schemaVersion: 1,
      workspaceId: 'project',
      projectId: 'project',
      rootAgentId: `root:${rootConversationId}`,
      rootConversationId,
      agents: [],
      lastSequence: 1
    }))
  mocks.listEvents
    .mockReset()
    .mockImplementation(
      async ({ rootConversationId, afterSequence }: CollaborationEventsRequest) => ({
        schemaVersion: 1,
        rootAgentId: `root:${rootConversationId}`,
        rootConversationId,
        events:
          afterSequence === 0
            ? [
                {
                  sequence: 1,
                  agentId: `root:${rootConversationId}`,
                  rootConversationId,
                  rootAgentId: `root:${rootConversationId}`,
                  activities: []
                }
              ]
            : [],
        lastSequence: 1,
        hasMore: false
      })
    )
})

function Harness({ root }: { root: string | null }) {
  const state = useOptionalCollaborationStore(root)
  return (
    <output>
      {state
        ? `${state.tree?.rootConversationId ?? 'loading'}:${state.hydrationRevision}`
        : 'draft'}
    </output>
  )
}

it('reuses roots within an account, drops them on account change, and unsubscribes for drafts', async () => {
  const screen = await render(<Harness root="a" />)
  await expect.element(screen.getByRole('status')).toHaveTextContent('a:1')
  await screen.rerender(<Harness root="b" />)
  await expect.element(screen.getByRole('status')).toHaveTextContent('b:1')
  await screen.rerender(<Harness root="a" />)
  await expect.element(screen.getByRole('status')).toHaveTextContent('a:2')
  expect(mocks.subscriptions.size).toBe(1)
  mocks.account = 'second-account'
  await screen.rerender(<Harness root="a" />)
  await expect.element(screen.getByRole('status')).toHaveTextContent('a:1')
  expect(mocks.subscriptions.size).toBe(1)
  await screen.rerender(<Harness root={null} />)
  await expect.element(screen.getByRole('status')).toHaveTextContent('draft')
  expect(mocks.subscriptions.size).toBe(0)
})

it('survives effect cleanup and setup without a dead store or duplicate live subscriptions', async () => {
  const screen = await render(
    <StrictMode>
      <Harness root="a" />
    </StrictMode>
  )
  await expect.element(screen.getByRole('status')).toHaveTextContent('a:1')
  expect(mocks.subscriptions.size).toBe(1)
  await screen.unmount()
  expect(mocks.subscriptions.size).toBe(0)
  expect(mocks.resyncHandlers.size).toBe(0)
})

it('invalidates idle root checkpoints on a global resync without loading those roots in the background', async () => {
  const screen = await render(<Harness root="a" />)
  await expect.element(screen.getByRole('status')).toHaveTextContent('a:1')
  await screen.rerender(<Harness root="b" />)
  await expect.element(screen.getByRole('status')).toHaveTextContent('b:1')
  mocks.getTree.mockClear()
  for (const handler of mocks.resyncHandlers) handler()
  await expect.element(screen.getByRole('status')).toHaveTextContent('b:2')
  expect(mocks.getTree.mock.calls.every(([input]) => input.rootConversationId === 'b')).toBe(true)
  mocks.listEvents.mockClear()
  await screen.rerender(<Harness root="a" />)
  await expect.element(screen.getByRole('status')).toHaveTextContent('a:2')
  expect(mocks.listEvents).toHaveBeenCalledWith({
    rootConversationId: 'a',
    afterSequence: 0,
    limit: 256
  })
})
