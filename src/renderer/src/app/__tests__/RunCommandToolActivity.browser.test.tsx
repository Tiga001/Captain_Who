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
  'agent.command.failed': '命令失败',
  'agent.command.waitingApproval': '等待审批运行命令',
  'agent.command.waitingApprovalStatus': '等待审批',
  'agent.command.starting': '正在启动命令',
  'agent.command.startingStatus': '正在启动',
  'agent.command.running': '正在运行命令',
  'agent.command.interrupted': '已中断',
  'agent.command.interruptedStatus': '已中断',
  'agent.command.timedOutLabel': '已超时',
  'agent.command.timedOut': '已超时',
  'agent.command.failedStatus': '失败',
  'agent.command.exitCode': '退出码 {code}',
  'agent.command.noOutput': '无输出',
  'agent.command.shell': 'Shell',
  'agent.command.successStatus': '成功',
  'agent.command.runningStatus': '运行中',
  'agent.command.runningElapsed': '已运行 {duration}',
  'agent.command.waitingForOutput': '等待命令输出…',
  'agent.command.copyOutput': '复制命令输出',
  'agent.command.outputCopied': '命令输出已复制',
  'agent.command.cancelled': '已取消',
  'agent.separator': ' · ',
  'agent.tool.running': '正在{tool}',
  'tool.runCommand': '运行命令'
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
        status: 'exited',
        exitCode: 0,
        stderr: '',
        stdout: 'OK',
        durationMs: 25
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

  it('keeps a long heredoc as one folded, scrollable, preformatted Timeline activity', async () => {
    const command = [
      "python3 <<'PY'",
      ...Array.from({ length: 80 }, (_, index) => `    print('page ${index + 1}')`),
      'PY',
      ''
    ].join('\n')
    const call: AgentToolCall = {
      approvalStatus: 'approved',
      args: { command, reason: '批量检查 PDF 页面' },
      id: 'multiline-command',
      tool: 'run_command'
    }
    const result: AgentToolResult = {
      callId: call.id,
      ok: true,
      result: { status: 'exited', exitCode: 0, stdout: 'done\n', stderr: '' },
      tool: 'run_command'
    }
    const screen = await render(<RunCommandToolActivity call={call} result={result} />)
    const disclosure = screen.container.querySelector<HTMLDetailsElement>(
      '.agent-activity--run-command'
    )

    expect(disclosure?.open).toBe(false)
    expect(screen.container.querySelectorAll('.agent-activity--run-command')).toHaveLength(1)
    await userEvent.click(screen.container.querySelector('summary')!)

    const commandBlock = screen.container.querySelector<HTMLElement>('.run-command-shell__command')
    expect(commandBlock?.textContent).toBe(`$ ${command}`)
    expect(window.getComputedStyle(commandBlock!).maxHeight).toBe('260px')
    expect(window.getComputedStyle(commandBlock!).overflow).toBe('auto')
    expect(window.getComputedStyle(commandBlock!).whiteSpace).toBe('pre')
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
    expect(copyButton?.parentElement).toHaveClass('run-command-shell')
    expect(window.getComputedStyle(copyButton!).top).toBe('8px')
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

  it('distinguishes approval waiting from a process that is starting', async () => {
    const waitingCall: AgentToolCall = {
      approvalStatus: 'required',
      args: { command: 'python3 app.py', reason: '启动应用' },
      id: 'approval-command',
      tool: 'run_command'
    }
    const waiting = await render(<RunCommandToolActivity call={waitingCall} />)
    expect(waiting.container.querySelector('summary')?.textContent).toContain('等待审批运行命令')
    expect(waiting.container.querySelector('.run-command-shell__status')?.textContent).toContain(
      '等待审批'
    )

    const starting = await render(
      <RunCommandToolActivity
        call={{ ...waitingCall, approvalStatus: 'approved' }}
        session={{
          callId: waitingCall.id,
          status: 'starting',
          latestSequence: 0,
          outputTruncated: false
        }}
      />
    )
    expect(starting.container.querySelector('summary')?.textContent).toContain('正在启动命令')
    expect(starting.container.querySelector('.run-command-shell__status')?.textContent).toContain(
      '正在启动'
    )
  })

  it('renders a managed non-zero exit as failed with exit code and duration', async () => {
    const call: AgentToolCall = {
      approvalStatus: 'approved',
      args: { command: 'pnpm test', reason: '运行测试' },
      id: 'failed-command',
      tool: 'run_command'
    }
    const result: AgentToolResult = {
      callId: call.id,
      tool: 'run_command',
      ok: true,
      result: {
        status: 'running',
        sessionId: 'cmd_1234567890abcdef1234567890abcdef',
        output: '',
        startedAt: 1,
        latestSequence: 0,
        outputTruncated: false
      }
    }
    const screen = await render(
      <RunCommandToolActivity
        call={call}
        result={result}
        session={{
          callId: call.id,
          sessionId: 'cmd_1234567890abcdef1234567890abcdef',
          status: 'exited',
          startedAt: 1_000,
          endedAt: 3_500,
          exitCode: 2,
          latestSequence: 0,
          outputTruncated: false
        }}
      />
    )

    expect(screen.container.querySelector('summary')?.textContent).toContain('命令失败')
    expect(screen.container.querySelector('.run-command-shell__status')).toHaveAttribute(
      'data-status',
      'failed'
    )
    expect(screen.container.querySelector('.run-command-shell__status')?.textContent).toContain(
      '失败 · 退出码 2 · 2s'
    )
  })

  it('renders a running receipt as started instead of completed and keeps later output', async () => {
    const call: AgentToolCall = {
      approvalStatus: 'approved',
      args: { command: 'python3 app.py', reason: '启动应用' },
      id: 'managed-command',
      tool: 'run_command'
    }
    const result: AgentToolResult = {
      callId: call.id,
      ok: true,
      result: {
        status: 'running',
        sessionId: 'cmd_1234567890abcdef1234567890abcdef',
        output: 'booting\n',
        startedAt: 10,
        latestSequence: 2,
        outputTruncated: false
      },
      tool: 'run_command'
    }
    const screen = await render(
      <RunCommandToolActivity
        call={call}
        liveOutput={{
          callId: call.id,
          chunks: [
            { sequence: 2, stream: 'stdout', output: 'booting\n' },
            { sequence: 3, stream: 'stdout', output: 'ready\n' }
          ]
        }}
        result={result}
        session={{
          callId: call.id,
          sessionId: 'cmd_1234567890abcdef1234567890abcdef',
          status: 'running',
          startedAt: 10,
          latestSequence: 3,
          outputTruncated: false
        }}
        settledStatus="completed"
      />
    )

    expect(screen.container.querySelector('summary')?.textContent).toContain(
      '正在运行命令 启动应用'
    )
    expect(screen.container.querySelector('summary')?.textContent).not.toContain('已运行命令')
    expect(screen.container.querySelector('.run-command-shell__output')?.textContent).toBe(
      'booting\nready\n'
    )
    expect(screen.container.querySelector('.run-command-shell__status')).toHaveAttribute(
      'data-status',
      'running'
    )
    expect(screen.container.querySelector('.run-command-shell__status')?.textContent).toContain(
      '运行中'
    )
  })

  it('shows live elapsed time only while a managed command is running', async () => {
    const call: AgentToolCall = {
      approvalStatus: 'approved',
      args: { command: 'pnpm check', reason: '运行检查' },
      id: 'timed-command',
      tool: 'run_command'
    }
    const startedAt = Date.now() - 5_500
    const screen = await render(
      <RunCommandToolActivity
        call={call}
        session={{
          callId: call.id,
          sessionId: 'cmd_1234567890abcdef1234567890abcdef',
          status: 'running',
          startedAt,
          latestSequence: 0,
          outputTruncated: false
        }}
      />
    )

    expect(screen.container.querySelector('.run-command-shell__status')?.textContent).toContain(
      '已运行 5s · 运行中'
    )
  })

  it('keeps a durable handoff receipt as the last-known running state until Host refreshes it', async () => {
    const call: AgentToolCall = {
      approvalStatus: 'approved',
      args: { command: 'python3 app.py', reason: '启动应用' },
      id: 'stale-managed-command',
      tool: 'run_command'
    }
    const result: AgentToolResult = {
      callId: call.id,
      ok: true,
      result: {
        status: 'running',
        sessionId: 'cmd_1234567890abcdef1234567890abcdef',
        output: 'booting\n',
        startedAt: 10,
        latestSequence: 1,
        outputTruncated: false
      },
      tool: 'run_command'
    }
    const screen = await render(<RunCommandToolActivity call={call} result={result} />)

    expect(screen.container.querySelector('summary')?.textContent).toContain('正在运行命令')
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
