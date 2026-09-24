import type { AgentSummary } from '@mycopilot/protocol'
import { expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'

const mocks = vi.hoisted(() => ({
  reload: vi.fn(),
  renderConversationSurface: vi.fn(),
  useObserverConversation: vi.fn()
}))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

vi.mock('../useObserverConversation', () => ({
  useObserverConversation: mocks.useObserverConversation
}))

vi.mock('../../chat/ConversationSurface', () => ({
  ConversationSurface: (props: unknown) => {
    mocks.renderConversationSurface(props)
    return <div data-testid="observer-surface" />
  }
}))

const { AgentObserverConversationSurface } = await import('../AgentObserverConversationSurface')

it('exposes the production retry action when initial observer hydration cannot safely converge', async () => {
  mocks.reload.mockReset()
  mocks.useObserverConversation.mockReturnValue({
    conversation: null,
    error: 'Observer hydration was overtaken by the live stream. Retry to recover.',
    loading: false,
    reload: mocks.reload
  })
  const screen = await render(
    <AgentObserverConversationSurface
      agent={agent()}
      directChildAgentIds={['grandchild']}
      agentLabelsById={{}}
      invalidationVersion="1:1"
      rootConversationId="root-conversation"
      showTokenUsageDetails={false}
    />
  )

  await expect.element(screen.getByRole('alert')).toBeVisible()
  await screen.getByRole('button', { name: 'agentCenter.retry' }).click()
  expect(mocks.reload).toHaveBeenCalledOnce()
})

it('keeps a previously authorized observer surface visible with a refresh recovery alert', async () => {
  mocks.reload.mockReset()
  mocks.useObserverConversation.mockReturnValue({
    conversation: {
      id: 'child-conversation',
      projectId: 'project',
      modelId: null,
      title: 'Child',
      messages: [],
      createdAt: 1,
      updatedAt: 1
    },
    error: 'temporary refresh failure',
    loading: false,
    reload: mocks.reload
  })
  const screen = await render(
    <AgentObserverConversationSurface
      agent={agent()}
      directChildAgentIds={['grandchild']}
      agentLabelsById={{}}
      invalidationVersion="1:2"
      rootConversationId="root-conversation"
      showTokenUsageDetails={false}
    />
  )

  expect(screen.container.querySelector('[data-testid="observer-surface"]')).not.toBeNull()
  await expect.element(screen.getByRole('alert')).toBeVisible()
  await screen.getByRole('button', { name: 'agentCenter.retry' }).click()
  expect(mocks.reload).toHaveBeenCalledOnce()
})

it('passes scoped child status activity and navigation to the shared observer conversation', async () => {
  mocks.renderConversationSurface.mockClear()
  mocks.useObserverConversation.mockReturnValue({
    conversation: {
      id: 'child-conversation',
      projectId: 'project',
      modelId: null,
      title: 'Child',
      messages: [],
      createdAt: 1,
      updatedAt: 1
    },
    error: null,
    loading: false,
    reload: mocks.reload
  })
  const activities = [
    {
      activityId: 'child-started',
      agentId: 'grandchild',
      occurredAt: 2,
      parentAgentId: 'child',
      parentConversationId: 'child-conversation',
      anchorMessageId: null,
      traceBoundarySequence: null,
      runId: null,
      semantic: 'started' as const,
      sequence: 1,
      taskNameSnapshot: 'grandchild',
      turnId: null
    }
  ]
  const onOpenAgent = vi.fn()
  await render(
    <AgentObserverConversationSurface
      agent={agent()}
      directChildAgentIds={['grandchild']}
      agentLabelsById={{ grandchild: 'grandchild' }}
      activities={activities}
      invalidationVersion="1:1"
      onOpenAgent={onOpenAgent}
      rootConversationId="root-conversation"
      showTokenUsageDetails={false}
    />
  )

  expect(mocks.renderConversationSurface).toHaveBeenCalledWith(
    expect.objectContaining({
      collaborationTimelineActivities: activities,
      directChildAgentIds: ['grandchild'],
      mode: 'observer',
      onOpenCollaborationAgent: onOpenAgent,
      rootConversationId: 'root-conversation'
    })
  )
})

function agent(): AgentSummary {
  return {
    agentId: 'agent-child',
    rootAgentId: 'agent-root',
    rootConversationId: 'root-conversation',
    parentAgentId: 'agent-root',
    conversationId: 'child-conversation',
    projectId: 'project',
    taskName: 'child',
    taskPath: '/root/child',
    lifecycle: 'active',
    displayStatus: 'running',
    latestActivityAt: 1,
    model: null
  }
}
