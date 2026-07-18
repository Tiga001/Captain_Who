// Renderer UI regression test: command reasons belong in the persistent activity title.
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { RunCommandToolActivity } from '../../features/chat/components/toolActivities/RunCommandToolActivity'

const translations: Record<string, string> = {
  'agent.command.completed': '已运行命令',
  'agent.command.noOutput': '无输出',
  'agent.command.shell': 'Shell',
  'agent.command.successStatus': '成功'
}

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => translations[key] ?? key })
}))

describe('RunCommandToolActivity', () => {
  it('keeps the reason in the title and the command in the expanded body', async () => {
    const call: AgentToolCall = {
      approvalStatus: 'approved',
      args: {
        command: 'python3 gen_budget.py',
        reason: '执行脚本生成预算工作簿'
      },
      id: 'command-call',
      tool: 'run_command'
    }
    const result: AgentToolResult = {
      callId: call.id,
      ok: true,
      result: {
        command: 'python3 gen_budget.py',
        exitCode: 0,
        stderr: '',
        stdout: 'OK'
      },
      tool: 'run_command'
    }
    const screen = await render(<RunCommandToolActivity call={call} result={result} />)
    const summary = screen.container.querySelector('summary')
    const details = screen.container.querySelector('.agent-activity__details')

    expect(summary?.textContent).toContain('已运行命令 执行脚本生成预算工作簿')
    expect(summary?.textContent).not.toContain('python3 gen_budget.py')
    expect(details?.textContent).toContain('python3 gen_budget.py')
    expect(details?.textContent).not.toContain('执行脚本生成预算工作簿')
  })
})
