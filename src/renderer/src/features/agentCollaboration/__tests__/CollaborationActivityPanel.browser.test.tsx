import type { AgentDisplayStatusView, AgentSummary } from '@mycopilot/protocol'
import { page } from 'vitest/browser'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { CollaborationActivityPanel } from '../CollaborationActivityPanel'

const translations: Record<string, string> = {
  'collaboration.activity.title': '子智能体协作',
  'collaboration.activity.activeCount': '{count} 个进行中',
  'collaboration.activity.openAgent': '查看子智能体 {name}',
  'collaboration.activity.modelUnavailable': '模型不可用',
  'collaboration.activity.time.unknown': '时间未知',
  'collaboration.activity.time.now': '刚刚',
  'collaboration.activity.status.idle': '待命',
  'collaboration.activity.status.queued': '排队中',
  'collaboration.activity.status.running': '处理中',
  'collaboration.activity.status.waitingApproval': '等待审批',
  'collaboration.activity.status.completed': '已完成',
  'collaboration.activity.status.failed': '失败',
  'collaboration.activity.status.interrupted': '已中断',
  'collaboration.activity.status.outcomeUnknown': '结果待确认',
  'collaboration.activity.status.archived': '已归档',
  'collaboration.activity.status.disabled': '已停用'
}

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    t: (key: string) => translations[key] ?? key
  })
}))

function agent(
  agentId: string,
  displayStatus: AgentDisplayStatusView,
  latestActivityAt: number,
  parentAgentId: string | null = 'agent-root'
): AgentSummary {
  return {
    agentId,
    rootAgentId: 'agent-root',
    rootConversationId: 'conversation-root',
    parentAgentId,
    conversationId: `conversation-${agentId}`,
    projectId: 'project-a',
    taskName: agentId === 'agent-review' ? '审阅实现' : '编写测试',
    taskPath: `/root/${agentId}`,
    lifecycle: 'active',
    displayStatus,
    latestActivityAt,
    model: {
      modelConfigId: agentId === 'agent-review' ? 'model-a' : 'model-b',
      displayName: agentId === 'agent-review' ? 'Model A' : 'Model B'
    }
  }
}

afterEach(() => vi.restoreAllMocks())

describe('CollaborationActivityPanel', () => {
  it('projects one stable row per child Agent and opens the exact Agent identity', async () => {
    vi.spyOn(Date, 'now').mockReturnValue(120_000)
    const onOpenAgent = vi.fn()
    const screen = await render(
      <CollaborationActivityPanel
        agents={[
          agent('agent-root', 'idle', 1, null),
          agent('agent-review', 'queued', 30_000),
          agent('agent-tests', 'latest_completed', 20_000),
          // A duplicate durable invalidation projection must update, not create another card.
          agent('agent-review', 'running', 90_000)
        ]}
        onOpenAgent={onOpenAgent}
      />
    )

    const rows = screen.container.querySelectorAll<HTMLButtonElement>(
      '.collaboration-activity__agent'
    )
    expect(rows).toHaveLength(2)
    expect(rows[0]?.dataset.agentId).toBe('agent-review')
    expect(rows[0]?.dataset.status).toBe('running')
    expect(rows[0]?.textContent).toContain('处理中')
    expect(rows[0]?.textContent).toContain('Model A')
    expect(rows[1]?.textContent).toContain('已完成')
    expect(screen.container.textContent).toContain('1 个进行中')
    expect(screen.container.textContent).not.toContain('conversation-agent-review')
    await expect.element(screen.getByRole('region', { name: '子智能体协作' })).toBeInTheDocument()
    expect(rows[0]?.querySelector('[aria-live="polite"]')?.textContent).toBe('处理中')

    await screen.getByRole('button', { name: '查看子智能体 审阅实现' }).click()
    expect(onOpenAgent).toHaveBeenCalledTimes(1)
    expect(onOpenAgent).toHaveBeenCalledWith('agent-review')
    await page.screenshot({
      element: screen.getByRole('region', { name: '子智能体协作' }).element(),
      path: '__screenshots__/CollaborationActivityPanel.browser.test.tsx/root-collaboration-activity.png'
    })
  })

  it('leaves the root chat DOM unchanged when the tree has no child', async () => {
    const screen = await render(
      <CollaborationActivityPanel
        agents={[agent('agent-root', 'idle', 1, null)]}
        onOpenAgent={vi.fn()}
      />
    )

    expect(screen.container.querySelector('[data-testid="collaboration-activity"]')).toBeNull()
    expect(screen.container.textContent).toBe('')
  })
})
