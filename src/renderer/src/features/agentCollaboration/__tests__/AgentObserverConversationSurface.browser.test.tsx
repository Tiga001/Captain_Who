import type { AgentSummary } from '@mycopilot/protocol'
import { expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'

const mocks = vi.hoisted(() => ({
  reload: vi.fn(),
  useObserverConversation: vi.fn()
}))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

vi.mock('../useObserverConversation', () => ({
  useObserverConversation: mocks.useObserverConversation
}))

vi.mock('../../chat/ConversationSurface', () => ({
  ConversationSurface: () => <div data-testid="observer-surface" />
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
