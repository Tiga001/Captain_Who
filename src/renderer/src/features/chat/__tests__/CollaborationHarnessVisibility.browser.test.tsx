import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import type { CSSProperties } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import { classicDarkTheme, classicLightTheme } from '../../../config/themes/classic'
import type { CollaborationTimelineActivity } from '../../agentCollaboration/CollaborationTimelineActivity'
import type { ChatMessage } from '../chatTypes'
import { ChatMessageItem } from '../components/ChatMessageItem'
import {
  SendMessageToolActivity,
  SendMessageToolActivityGroup
} from '../components/toolActivities/SendMessageToolActivity'
import '../../../styles/global.css'
import '../ChatConversationPage.css'

const translations: Record<string, string> = {
  'agent.processed': '已处理 {duration}',
  'agent.thinking': '正在思考',
  'agent.command.waitingForCompletion': '正在等待命令完成',
  'agent.sendMessage.sending': '正在向智能体「{name}」发送消息',
  'agent.sendMessage.sent': '向智能体「{name}」发送了消息',
  'agent.sendMessage.failed': '向智能体「{name}」发送消息失败',
  'agent.sendMessage.cancelled': '已取消向智能体「{name}」发送消息',
  'agent.sendMessage.unknownTarget': '未知智能体',
  'agent.sendMessage.unknown': '向智能体「{name}」发送消息的结果待确认',
  'agent.sendMessage.group.sending': '正在发送 {count} 条消息',
  'agent.sendMessage.group.sent': '已发送 {count} 条消息',
  'agent.sendMessage.group.failed': '消息发送失败',
  'agent.sendMessage.group.cancelled': '消息发送已取消',
  'agent.sendMessage.group.unknown': '消息发送结果待确认',
  'collaboration.activity.copyAgentStatus': '{name}: {status}',
  'collaboration.activity.moreAgents': '另有 {count} 个',
  'collaboration.activity.openAgentActivity': '查看子智能体 {name}：{status}',
  'collaboration.activity.statusListSeparator': '；',
  'collaboration.activity.status.started': '已开始工作'
}

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    t: (key: string) => translations[key] ?? key
  })
}))

vi.mock('../../storage/storageClient', () => ({
  loadAttachmentImage: vi.fn(),
  loadImageFile: vi.fn(),
  revealStoredProjectFile: vi.fn()
}))

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))

const harnessTools = [
  'spawn_agent',
  'followup_task',
  'wait_agent',
  'list_agents',
  'interrupt_agent'
] as const

function call(tool: string, index: number): AgentToolCall {
  return {
    id: `call-${index}`,
    tool,
    args: { secretDiagnosticMarker: `RAW-HARNESS-${tool}` },
    approvalStatus: 'not_required',
    reason: `internal ${tool}`
  }
}

function message(toolCalls: AgentToolCall[]): ChatMessage {
  return {
    id: 'assistant-message',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending',
    agentRun: {
      runId: 'run-root',
      status: 'running',
      startedAt: 1,
      firstResponseAt: 2,
      toolDefinitions: [],
      toolCalls,
      toolResults: [],
      approvals: [],
      fileChangeProposals: [],
      timeline: toolCalls.map((toolCall) => ({
        id: `timeline-${toolCall.id}`,
        type: 'tool_call' as const,
        callId: toolCall.id
      }))
    }
  }
}

function settledMessage(toolCalls: AgentToolCall[]): ChatMessage {
  const settled = message(toolCalls)
  settled.content = '主智能体最终正文'
  settled.status = 'sent'
  if (!settled.agentRun) throw new Error('missing root run fixture')
  settled.agentRun.status = 'completed'
  settled.agentRun.completedAt = 10
  return settled
}

function activity(
  anchorMessageId: string | null,
  traceBoundarySequence: number | null
): CollaborationTimelineActivity {
  return {
    activityId: 'activity-reviewer-started',
    agentId: 'agent-reviewer',
    occurredAt: 5,
    ownerAgentId: 'root:root-conversation',
    ownerConversationId: 'root-conversation',
    anchorMessageId,
    traceBoundarySequence,
    runId: 'run-reviewer',
    semantic: 'started',
    sequence: 1,
    taskNameSnapshot: 'Reviewer',
    turnId: 'turn-reviewer',
    taskMessageId: 'task-agent-reviewer'
  }
}

describe('collaboration Harness timeline projection', () => {
  it('keeps the other five raw coordination calls out of the chat timeline', async () => {
    const calls = harnessTools.map(call)
    const screen = await render(
      <ChatMessageItem message={message(calls)} showTokenUsageDetails={false} />
    )

    expect(screen.container.querySelector('.agent-activity')).toBeNull()
    for (const tool of harnessTools) {
      expect(screen.container.textContent).not.toContain(tool)
      expect(screen.container.textContent).not.toContain(`RAW-HARNESS-${tool}`)
    }
    expect(screen.container.textContent).toContain('正在思考')
    expect(screen.container.textContent).not.toContain('正在等待命令完成')
  })

  it.each(['interactive', 'observer'] as const)(
    'shows send_message at its sender-local timeline position in %s mode',
    async (mode) => {
      const sending = call('send_message', 10)
      sending.args = { target: '主智能体', message: 'PRIVATE-MESSAGE-BODY' }
      const pending = message([sending])
      pending.agentRun!.timeline = [
        { id: 'before', type: 'message', content: '汇报之前', traceSequence: 0 },
        ...pending.agentRun!.timeline,
        { id: 'after', type: 'message', content: '汇报之后', traceSequence: 2 }
      ]
      const screen = await render(
        <ChatMessageItem message={pending} mode={mode} showTokenUsageDetails={false} />
      )
      const row = screen.container.querySelector('.agent-activity--send-message')!
      expect(row.textContent).toContain('正在向智能体「主智能体」发送消息')
      expect(row.querySelector('.agent-running-text')).not.toBeNull()
      expect(row.querySelector('details, summary, button, .agent-activity__chevron')).toBeNull()

      const sent = structuredClone(pending)
      sent.agentRun!.toolResults = [sendResult(sending, true)]
      await screen.rerender(
        <ChatMessageItem message={sent} mode={mode} showTokenUsageDetails={false} />
      )
      const text = screen.container.textContent ?? ''
      const receipt = '向智能体「浙大调研」发送了消息'
      expect(text).toContain(receipt)
      expect(text.indexOf('汇报之前')).toBeLessThan(text.indexOf(receipt))
      expect(text.indexOf(receipt)).toBeLessThan(text.indexOf('汇报之后'))
      expect(text).not.toContain('PRIVATE-MESSAGE-BODY')
      expect(text).not.toContain('send_message')
      expect(screen.container.querySelectorAll('.agent-activity--send-message')).toHaveLength(1)
      expect(row.querySelector('.agent-running-text')).toBeNull()

      // Cancelling the sender afterwards must not rewrite the already accepted delivery.
      sent.agentRun!.status = 'cancelled'
      sent.agentRun!.completedAt = 10
      sent.uiState = { timelineCollapsed: false }
      await screen.rerender(
        <ChatMessageItem message={sent} mode={mode} showTokenUsageDetails={false} />
      )
      expect(screen.container.textContent).toContain(receipt)
    }
  )

  it.each(['failed', 'cancelled'] as const)(
    'settles a send_message as %s without exposing arguments or diagnostics',
    async (status) => {
      const sending = call('send_message', 11)
      sending.args = { target: '主智能体', message: 'PRIVATE-MESSAGE-BODY' }
      const candidate = message([sending])
      candidate.agentRun!.status = status
      candidate.agentRun!.completedAt = 10
      candidate.uiState = { timelineCollapsed: false }
      if (status === 'failed') candidate.agentRun!.toolResults = [sendResult(sending, false)]
      const screen = await render(
        <ChatMessageItem message={candidate} mode="observer" showTokenUsageDetails={false} />
      )
      const row = screen.container.querySelector('.agent-activity--send-message')!
      expect(row.textContent).toContain(
        status === 'failed'
          ? '向智能体「主智能体」发送消息失败'
          : '已取消向智能体「主智能体」发送消息'
      )
      expect(row.querySelector('.agent-running-text, details, button')).toBeNull()
      expect(screen.container.textContent).not.toContain('PRIVATE-')
    }
  )

  it.each(['interactive', 'observer'] as const)(
    'folds adjacent sends by default and preserves recipient order in %s mode',
    async (mode) => {
      const calls = [21, 22].map((index) => ({
        ...call('send_message', index),
        args: { target: `收件智能体 ${index}`, message: 'PRIVATE-MESSAGE-BODY' }
      }))
      const candidate = message(calls)
      candidate.agentRun!.toolResults = calls.map((entry, index) => ({
        ...sendResult(entry, true),
        result: { taskName: `收件智能体 ${index + 21}`, deliveryState: 'queued' }
      }))
      const screen = await render(
        <ChatMessageItem message={candidate} mode={mode} showTokenUsageDetails={false} />
      )
      const group = screen.container.querySelector<HTMLDetailsElement>(
        '.agent-activity--send-message-group'
      )!
      expect(group.open).toBe(false)
      expect(group.querySelector('summary')?.textContent).toBe('已发送 2 条消息')
      expect(group.querySelectorAll('.agent-activity--send-message')).toHaveLength(2)
      expect(screen.container.textContent).not.toContain('PRIVATE-')

      await userEvent.click(group.querySelector('summary')!)
      expect(group.open).toBe(true)
      expect(
        [...group.querySelectorAll('.agent-activity--send-message')].map((row) => row.textContent)
      ).toEqual(['向智能体「收件智能体 21」发送了消息', '向智能体「收件智能体 22」发送了消息'])

      // A later receipt updates the same disclosure without collapsing it again.
      const third = { ...call('send_message', 23), args: { target: '收件智能体 23' } }
      candidate.agentRun!.toolCalls.push(third)
      candidate.agentRun!.timeline.push({ id: 'third-send', type: 'tool_call', callId: third.id })
      await screen.rerender(
        <ChatMessageItem
          message={structuredClone(candidate)}
          mode={mode}
          showTokenUsageDetails={false}
        />
      )
      expect(group.open).toBe(true)
      expect(group.querySelector('summary')?.textContent).toBe('正在发送 3 条消息')
      expect(group.querySelectorAll('.agent-activity--send-message')).toHaveLength(3)
    }
  )

  it.each([
    { state: 'failed', label: '消息发送失败', row: '向智能体「主智能体」发送消息失败' },
    { state: 'cancelled', label: '消息发送已取消', row: '已取消向智能体「主智能体」发送消息' },
    {
      state: 'completed',
      label: '消息发送结果待确认',
      row: '向智能体「主智能体」发送消息的结果待确认'
    }
  ] as const)(
    'does not count a $state send without a successful receipt as sent',
    async ({ state, label, row }) => {
      const calls = [31, 32].map((index) => ({
        ...call('send_message', index),
        args: { target: '主智能体' }
      }))
      const screen = await render(
        <SendMessageToolActivityGroup
          items={[
            { call: calls[0], result: sendResult(calls[0], true), settledStatus: state },
            { call: calls[1], settledStatus: state }
          ]}
        />
      )
      const group = screen.container.querySelector<HTMLDetailsElement>('details')!
      expect(group.querySelector('summary')?.textContent).toBe(label)
      expect(group.querySelector('summary')?.textContent).not.toBe('已发送 2 条消息')
      await userEvent.click(group.querySelector('summary')!)
      expect(group.textContent).toContain('向智能体「浙大调研」发送了消息')
      expect(group.textContent).toContain(row)
    }
  )

  it('keeps sends on either side of a child status as separate timeline rows', async () => {
    const calls = [41, 42].map((index) => ({
      ...call('send_message', index),
      args: { target: '主智能体' }
    }))
    const candidate = message(calls)
    candidate.agentRun!.timeline = calls.map((entry, index) => ({
      id: `timeline-${entry.id}`,
      type: 'tool_call',
      callId: entry.id,
      traceSequence: index * 2
    }))
    const started = activity('assistant-message', 1)
    const screen = await render(
      <ChatMessageItem
        conversationId="root-conversation"
        collaborationTimelineActivities={[started]}
        collaborationTreeAgentIds={['root:root-conversation', 'agent-reviewer']}
        message={candidate}
        mode="observer"
        onOpenCollaborationAgent={vi.fn()}
        showTokenUsageDetails={false}
      />
    )
    expect(screen.container.querySelector('.agent-activity--send-message-group')).toBeNull()
    const orderedRows = screen.container.querySelectorAll(
      '.agent-activity--send-message, [data-semantic="started"]'
    )
    expect(orderedRows).toHaveLength(3)
    expect(orderedRows[1]).toHaveAttribute('data-semantic', 'started')
  })

  it('keeps a dark narrow send group compact and readable when expanded', async () => {
    const calls = [51, 52, 53].map((index) => ({
      ...call('send_message', index),
      args: { target: index === 52 ? '国际学术研究协作与资料核验智能体' : '主智能体' }
    }))
    const screen = await render(
      <div
        style={
          {
            ...getFrontendCssVariables(frontendConfig, classicDarkTheme),
            width: 280,
            padding: 20,
            background: 'var(--mc-color-surface-main-panel)',
            fontFamily: 'var(--mc-font-family)'
          } as CSSProperties
        }
      >
        <SendMessageToolActivityGroup
          items={calls.map((entry) => ({
            call: entry,
            result: {
              ...sendResult(entry, true),
              result: {
                taskName: entry.args && (entry.args as { target: string }).target,
                deliveryState: 'queued'
              }
            }
          }))}
        />
      </div>
    )
    const container = screen.container.firstElementChild as HTMLElement
    const group = container.querySelector<HTMLDetailsElement>('details')!
    expect(group.open).toBe(false)
    await page.screenshot({
      element: container,
      path: '__screenshots__/CollaborationHarnessVisibility.browser.test.tsx/send-message-group-dark-collapsed.png'
    })
    await userEvent.click(group.querySelector('summary')!)
    expect(group.open).toBe(true)
    expect(container.scrollWidth).toBeLessThanOrEqual(container.clientWidth + 1)
    for (const row of container.querySelectorAll<HTMLElement>('.agent-activity')) {
      expect(row.scrollWidth).toBeLessThanOrEqual(row.clientWidth + 1)
    }
    await page.screenshot({
      element: container,
      path: '__screenshots__/CollaborationHarnessVisibility.browser.test.tsx/send-message-group-dark-expanded.png'
    })
  })

  it.each([
    { name: 'light', theme: classicLightTheme, width: 620 },
    { name: 'dark', theme: classicDarkTheme, width: 280 }
  ])(
    'wraps a long recipient in the $name theme without overflow',
    async ({ name, theme, width }) => {
      const sending = call('send_message', 12)
      sending.args = { target: '浙大调研', message: 'PRIVATE-MESSAGE-BODY' }
      const longName = `${'国际学术研究协作'.repeat(4)}-research-group`
      const style = {
        ...getFrontendCssVariables(frontendConfig, theme),
        width,
        padding: 20,
        background: 'var(--mc-color-surface-main-panel)',
        fontFamily: 'var(--mc-font-family)',
        display: 'grid',
        gap: 18
      } as CSSProperties
      const screen = await render(
        <div style={style}>
          <SendMessageToolActivity call={sending} />
          <SendMessageToolActivity call={sending} result={sendResult(sending, true)} />
          <SendMessageToolActivity call={sending} result={sendResult(sending, false)} />
          <SendMessageToolActivity call={sending} cancelled />
          <SendMessageToolActivity call={{ ...sending, args: { target: longName } }} />
        </div>
      )
      const container = screen.container.firstElementChild as HTMLElement
      expect(container.scrollWidth).toBeLessThanOrEqual(container.clientWidth + 1)
      for (const row of container.querySelectorAll<HTMLElement>('.agent-activity')) {
        expect(row.scrollWidth).toBeLessThanOrEqual(row.clientWidth + 1)
        expect(row.querySelector('details, summary, button, .agent-activity__chevron')).toBeNull()
      }
      await page.screenshot({
        element: container,
        path: `__screenshots__/CollaborationHarnessVisibility.browser.test.tsx/send-message-${name}.png`
      })
    }
  )

  it('continues to render an ordinary Tool beside hidden collaboration bookkeeping', async () => {
    const visible = call('custom_visible_tool', harnessTools.length)
    visible.args = { marker: 'VISIBLE-TOOL-DETAIL' }
    const screen = await render(
      <ChatMessageItem
        message={message([...harnessTools.map(call), visible])}
        showTokenUsageDetails={false}
      />
    )

    expect(screen.container.textContent).toContain('VISIBLE-TOOL-DETAIL')
    expect(screen.container.textContent).not.toContain('RAW-HARNESS-wait_agent')
  })

  it('folds trusted anchored activity for a settled root run without exposing raw Harness calls', async () => {
    const settled = settledMessage(harnessTools.map(call))
    const started = activity('assistant-message', 1)
    if (!settled.agentRun) throw new Error('missing root run fixture')
    settled.agentRun.collaborationTimelineActivities = [started]
    const onUiStateChange = vi.fn()
    const props = {
      conversationId: 'root-conversation',
      collaborationTimelineActivities: [started],
      message: settled,
      mode: 'interactive' as const,
      onOpenCollaborationAgent: vi.fn(),
      onUiStateChange,
      showTokenUsageDetails: false
    }
    const screen = await render(<ChatMessageItem {...props} />)

    const disclosure = screen.container.querySelector<HTMLButtonElement>(
      '.agent-run__elapsed-button'
    )
    expect(disclosure).not.toBeNull()
    expect(disclosure).toHaveAttribute('aria-expanded', 'false')
    expect(screen.container.querySelector('[data-semantic="started"]')).toBeNull()
    expect(screen.getByText('主智能体最终正文').elements()).toHaveLength(1)

    await userEvent.click(disclosure!)
    expect(onUiStateChange).toHaveBeenCalledWith('assistant-message', {
      timelineCollapsed: false
    })

    const expanded = structuredClone(settled)
    expanded.uiState = { timelineCollapsed: false }
    await screen.rerender(<ChatMessageItem {...props} message={expanded} />)

    expect(screen.container.querySelector('[data-semantic="started"]')).not.toBeNull()
    expect(screen.getByText('主智能体最终正文').elements()).toHaveLength(1)
    for (const tool of harnessTools) {
      expect(screen.container.textContent).not.toContain(tool)
      expect(screen.container.textContent).not.toContain(`RAW-HARNESS-${tool}`)
    }
  })

  it.each([
    {
      activities: [] as CollaborationTimelineActivity[],
      label: 'no child activity',
      mode: 'interactive' as const,
      status: 'completed' as const
    },
    {
      activities: [activity('assistant-message', null)],
      label: 'an incomplete durable anchor',
      mode: 'interactive' as const,
      status: 'completed' as const
    },
    {
      activities: [activity('another-message', 1)],
      label: 'another message anchor',
      mode: 'interactive' as const,
      status: 'completed' as const
    },
    {
      activities: [activity('assistant-message', 1)],
      label: 'an unsettled root run',
      mode: 'interactive' as const,
      status: 'running' as const
    }
  ])('does not add a collaboration-only disclosure for $label', async (variant) => {
    const candidate = settledMessage(harnessTools.map(call))
    if (!candidate.agentRun) throw new Error('missing root run fixture')
    candidate.agentRun.status = variant.status
    if (variant.status === 'running') candidate.agentRun.completedAt = undefined
    else {
      candidate.agentRun.collaborationTimelineActivities = variant.activities.filter(
        (candidateActivity) =>
          candidateActivity.anchorMessageId === 'assistant-message' &&
          candidateActivity.traceBoundarySequence !== null
      )
    }

    const screen = await render(
      <ChatMessageItem
        conversationId="root-conversation"
        collaborationTimelineActivities={variant.activities}
        message={candidate}
        mode={variant.mode}
        onOpenCollaborationAgent={vi.fn()}
        showTokenUsageDetails={false}
      />
    )

    expect(screen.container.querySelector('.agent-run__elapsed-button')).toBeNull()
    expect(screen.getByText('主智能体最终正文').elements()).toHaveLength(1)
  })
})

function sendResult(toolCall: AgentToolCall, ok: boolean): AgentToolResult {
  return {
    callId: toolCall.id,
    tool: 'send_message',
    ok,
    result: ok ? { taskName: '浙大调研', deliveryState: 'queued' } : undefined,
    error: ok ? undefined : 'PRIVATE-INTERNAL-DIAGNOSTIC'
  }
}
