// Renderer UI regression test: command reasons belong in the persistent activity title.
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { userEvent } from 'vitest/browser'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatAgentRunView } from '../../features/chat/chatTypes'
import { AgentToolActivity } from '../../features/chat/components/toolActivities/AgentToolActivity'
import { RunCommandToolActivity } from '../../features/chat/components/toolActivities/RunCommandToolActivity'
import '../../styles/global.css'
import '../../features/chat/ChatConversationPage.agent.css'

const { copyTextSpy } = vi.hoisted(() => ({
  copyTextSpy: vi.fn().mockResolvedValue(undefined)
}))

const translations: Record<string, string> = {
  'agent.command.completed': '已运行命令',
  'agent.command.noOutput': '无输出',
  'agent.command.shell': 'Shell',
  'agent.command.successStatus': '成功',
  'agent.command.runningStatus': '运行中',
  'agent.command.waitingForOutput': '等待命令输出…',
  'agent.command.copyOutput': '复制命令输出',
  'agent.command.outputCopied': '命令输出已复制'
}

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => translations[key] ?? key })
}))
vi.mock('../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../features/chat/components/chatMessageItemUtils', async (importOriginal) => {
  const actual =
    await importOriginal<typeof import('../../features/chat/components/chatMessageItemUtils')>()
  return { ...actual, copyTextToClipboard: copyTextSpy }
})

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

  it('renders bounded live output before the final ToolResult arrives', async () => {
    const call: AgentToolCall = {
      approvalStatus: 'approved',
      args: {
        command: 'pnpm test',
        reason: '运行测试'
      },
      id: 'live-command',
      tool: 'run_command'
    }
    const screen = await render(
      <RunCommandToolActivity
        call={call}
        liveOutput={{
          callId: call.id,
          chunks: [
            { sequence: 1, stream: 'stdout', output: 'suite started\n' },
            { sequence: 2, stream: 'stderr', output: 'one warning\n' }
          ]
        }}
      />
    )

    const output = screen.container.querySelector<HTMLElement>('.run-command-shell__output')

    expect(output?.textContent).toBe('suite started\none warning\n')
    expect(window.getComputedStyle(output!).maxHeight).toBe('260px')
    expect(window.getComputedStyle(output!).overflowY).toBe('auto')
    expect(window.getComputedStyle(output!).whiteSpace).toBe('pre')
    const copyButton =
      screen.container.querySelector<HTMLButtonElement>('[aria-label="复制命令输出"]')
    expect(copyButton).not.toBeNull()
    await userEvent.click(screen.container.querySelector('summary')!)
    await userEvent.click(copyButton!)
    expect(copyTextSpy).toHaveBeenCalledWith('suite started\none warning\n')
    await expect.element(screen.getByRole('button', { name: '命令输出已复制' })).toBeVisible()
    expect(screen.container.querySelector('.run-command-shell__status')?.textContent).toContain(
      '运行中'
    )
  })

  it('shows an explicit running shell before the command emits its first byte', async () => {
    const call: AgentToolCall = {
      approvalStatus: 'not_required',
      args: { command: 'pnpm check', reason: '运行检查' },
      id: 'silent-command',
      tool: 'run_command'
    }
    const screen = await render(<RunCommandToolActivity call={call} />)

    expect(screen.container.querySelector('.run-command-shell__output')?.textContent).toBe(
      '等待命令输出…'
    )
    expect(screen.container.querySelector('.run-command-shell__status')?.textContent).toContain(
      '运行中'
    )
  })

  it('passes transient output through the generic timeline activity path', async () => {
    const call: AgentToolCall = {
      approvalStatus: 'not_required',
      args: { command: 'pnpm check', reason: '运行检查' },
      id: 'timeline-command',
      tool: 'run_command'
    }
    const run: ChatAgentRunView = {
      runId: 'run-1',
      status: 'running',
      toolDefinitions: [],
      toolCalls: [call],
      toolResults: [],
      approvals: [],
      diffs: [],
      timeline: [{ id: 'tool-timeline-command', type: 'tool_call', callId: call.id }],
      commandOutputPreviews: {
        [call.id]: {
          callId: call.id,
          chunks: [{ sequence: 1, stream: 'stdout', output: 'checking types\n' }]
        }
      }
    }
    const screen = await render(
      <AgentToolActivity
        call={call}
        run={run}
        settledStatus={undefined}
        showImageGenerationPreview={false}
      />
    )

    expect(screen.container.querySelector('.run-command-shell__output')?.textContent).toContain(
      'checking types'
    )
  })
})
