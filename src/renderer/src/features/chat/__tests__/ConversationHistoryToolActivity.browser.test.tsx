import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { ConversationHistoryToolActivity } from '../components/toolActivities/ConversationHistoryToolActivity'

const translations: Record<string, string> = {
  'agent.history.running': '正在回忆',
  'agent.history.completed': '回忆了一下',
  'agent.history.failed': '回忆时遇到问题',
  'agent.history.cancelled': '已停止回忆'
}

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => translations[key] ?? key })
}))

const call: AgentToolCall = {
  id: 'history-call',
  tool: 'conversation_history',
  args: { query: 'Exact History Archive' },
  approvalStatus: 'not_required',
  reason: null
}

describe('ConversationHistoryToolActivity', () => {
  it('reuses the running activity treatment while the model is recalling history', async () => {
    const screen = await render(<ConversationHistoryToolActivity items={[{ call }]} />)

    await expect.element(screen.getByText('正在回忆')).toBeVisible()
    expect(screen.getByText('正在回忆').element()).toHaveClass('agent-running-text')
  })

  it('settles into the existing completed status after a successful recall', async () => {
    const result: AgentToolResult = {
      callId: call.id,
      tool: 'conversation_history',
      ok: true,
      result: { view: 'search_results', returnedTurns: 1, returnedMatches: 2 }
    }
    const screen = await render(<ConversationHistoryToolActivity items={[{ call, result }]} />)

    await expect.element(screen.getByText('回忆了一下')).toBeVisible()
    expect(screen.getByText('回忆了一下').element()).not.toHaveClass('agent-running-text')
  })
})
