// Clock ticks must update elapsed/activity labels without re-parsing settled timeline Markdown.
import { Profiler } from 'react'
import { flushSync } from 'react-dom'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { remarkNormalizeCjkAutolinkBoundaries } from '../components/chatMarkdownAutolinks'
import { AgentRunView } from '../components/AgentRunView'
import type { ChatMessage } from '../chatTypes'
import { applyAgentEventToChatMessage, ensureAgentRun } from '../../agentRun/agentEventReducer'

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: (key: string) =>
      ({
        'agent.processed': 'Processed {duration}',
        'agent.stoppedAfter': 'Stopped after {duration}',
        'agent.thinking': 'Thinking',
        'agent.waitingForNextAction': 'Waiting for next action',
        'agent.command.waitingForCompletion': 'Waiting for command completion',
        'agent.llmRetry.reconnecting': 'Reconnecting {attempt}/{maxAttempts}'
      })[key] ?? key
  })
}))
vi.mock('../components/ImagePreview', () => ({
  useImagePreview: () => vi.fn(),
  useImagePreviewNotice: () => vi.fn()
}))
vi.mock('../components/chatMarkdownAutolinks', { spy: true })

const markdownParseCount = () => vi.mocked(remarkNormalizeCjkAutolinkBoundaries).mock.calls.length

function runningMessage(): ChatMessage {
  return {
    id: 'assistant',
    role: 'assistant',
    content: '',
    createdAt: Date.now(),
    status: 'pending',
    agentRun: {
      runId: 'run',
      status: 'waiting_for_approval',
      startedAt: Date.now(),
      firstResponseAt: Date.now(),
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      approvals: [],
      fileChangeProposals: [],
      timeline: Array.from({ length: 50 }, (_, index) => ({
        id: `message-${index}`,
        type: 'message',
        content: `**Static segment ${index}**: $x^2+y^2=z^2$`,
        createdAt: Date.now()
      }))
    }
  }
}

beforeEach(() => {
  vi.mocked(remarkNormalizeCjkAutolinkBoundaries).mockClear()
  vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'clearInterval'] })
  vi.setSystemTime(new Date('2026-09-28T00:00:00Z'))
})

afterEach(() => vi.useRealTimers())

it('keeps 50 static Markdown segments untouched across 30 seconds of elapsed updates', async () => {
  const message = runningMessage()
  let commits = 0
  let renderDurationMs = 0
  const renderRun = (currentMessage: ChatMessage) => (
    <Profiler
      id="waiting-run"
      onRender={(_, __, duration) => {
        commits += 1
        renderDurationMs += duration
      }}
    >
      <AgentRunView message={currentMessage} mode="observer" />
    </Profiler>
  )
  const screen = await render(renderRun(message))
  const initialParses = markdownParseCount()
  expect(initialParses).toBe(50)
  commits = 0
  renderDurationMs = 0

  for (let tick = 0; tick < 60; tick += 1) {
    flushSync(() => vi.advanceTimersByTime(500))
  }
  console.info('30s static timeline probe', {
    commits,
    renderDurationMs,
    additionalMarkdownParses: markdownParseCount() - initialParses
  })
  expect(screen.container.querySelector('.agent-run__elapsed')?.textContent).toBe('Processed 30s')
  expect(markdownParseCount()).toBe(initialParses)

  const updated: ChatMessage = {
    ...message,
    agentRun: {
      ...message.agentRun!,
      timeline: message.agentRun!.timeline.map((item, index) =>
        index === 49 && item.type === 'message' ? { ...item, content: '**New text**' } : item
      )
    }
  }
  await screen.rerender(renderRun(updated))
  expect(markdownParseCount()).toBe(initialParses + 1)
  expect(screen.container.textContent).toContain('New text')

  const completed: ChatMessage = {
    ...updated,
    status: 'sent',
    agentRun: { ...updated.agentRun!, status: 'completed', completedAt: Date.now() }
  }
  await screen.rerender(renderRun(completed))
  expect(vi.getTimerCount()).toBe(0)
  flushSync(() => vi.advanceTimersByTime(2000))
  expect(screen.container.querySelector('.agent-run__elapsed')?.textContent).toBe('Processed 30s')
  await screen.unmount()
  expect(vi.getTimerCount()).toBe(0)
})

it('expires streaming grace, preserves retry labels, and clears clocks on cancellation and unmount', async () => {
  const message = runningMessage()
  message.agentRun!.status = 'running'
  message.agentRun!.lastResponseAt = Date.now()
  const screen = await render(<AgentRunView message={message} mode="observer" />)
  expect(screen.container.querySelector('.agent-thinking')).toBeNull()
  flushSync(() => vi.advanceTimersByTime(1500))
  expect(screen.container.querySelector('.agent-thinking')?.textContent).toBe(
    'Waiting for next action'
  )

  const retry: ChatMessage = {
    ...message,
    agentRun: {
      ...message.agentRun!,
      llmRetry: {
        category: 'network',
        delayMs: 1000,
        retryAt: Date.now() + 1000,
        attempt: 2,
        maxAttempts: 4
      }
    }
  }
  await screen.rerender(<AgentRunView message={retry} mode="observer" />)
  expect(screen.container.querySelector('.agent-thinking')?.textContent).toBe('Reconnecting 1/3')

  const cancelled: ChatMessage = {
    ...message,
    status: 'sent',
    agentRun: { ...message.agentRun!, status: 'cancelled', completedAt: Date.now() }
  }
  await screen.rerender(<AgentRunView message={cancelled} mode="observer" />)
  expect(vi.getTimerCount()).toBe(0)
  expect(screen.container.querySelector('.agent-thinking')).toBeNull()
  expect(screen.container.querySelector('.agent-run__elapsed')?.textContent).toContain(
    'Stopped after'
  )

  const next = runningMessage()
  next.id = 'other-conversation-assistant'
  next.agentRun!.runId = 'next-run'
  await screen.rerender(<AgentRunView message={next} mode="observer" />)
  expect(vi.getTimerCount()).toBeGreaterThan(0)
  await screen.unmount()
  expect(vi.getTimerCount()).toBe(0)
})

it('uses the existing header slot for waiting and evidence-based reasoning, then removes it on completion', async () => {
  let message: ChatMessage = {
    id: 'assistant-activity',
    role: 'assistant',
    content: '',
    createdAt: Date.now(),
    status: 'pending',
    agentRun: ensureAgentRun(undefined, 'run-activity', 'running')
  }
  const screen = await render(<AgentRunView message={message} mode="interactive" />)
  const header = () => screen.container.querySelector('.agent-run__elapsed .agent-running-text')
  expect(header()?.textContent).toBe('Waiting for next action')
  expect(screen.container.querySelector('.agent-thinking')).toBeNull()
  const activity = {
    type: 'model_activity_changed' as const,
    runId: 'run-activity',
    streamId: 'stream-activity',
    attempt: 1,
    activity: 'waiting' as const
  }
  message = applyAgentEventToChatMessage(message, activity)
  message = applyAgentEventToChatMessage(message, { ...activity, activity: 'reasoning' })
  await screen.rerender(<AgentRunView message={message} mode="interactive" />)
  expect(header()?.textContent).toBe('Thinking')
  expect(screen.container.querySelector('.agent-thinking')).toBeNull()
  message = applyAgentEventToChatMessage(message, activity)
  await screen.rerender(<AgentRunView message={message} mode="interactive" />)
  expect(header()?.textContent).toBe('Waiting for next action')
  message = applyAgentEventToChatMessage(message, {
    type: 'done',
    runId: activity.runId,
    success: true,
    status: 'completed',
    content: 'Final answer.'
  })
  await screen.rerender(<AgentRunView message={message} mode="interactive" />)
  expect(header()).toBeNull()
  expect(screen.container.querySelector('.agent-thinking')).toBeNull()
  expect(screen.container.textContent).toContain('Final answer.')
  await screen.unmount()
})

it('switches the footer only on model evidence while text streaming keeps the slot suppressed', async () => {
  let message = runningMessage()
  message.agentRun!.status = 'running'
  const activity = {
    type: 'model_activity_changed' as const,
    runId: 'run',
    streamId: 'stream-footer',
    attempt: 1,
    activity: 'waiting' as const
  }
  message = applyAgentEventToChatMessage(message, activity)
  const screen = await render(<AgentRunView message={message} mode="interactive" />)
  const footer = () => screen.container.querySelector('.agent-thinking .agent-running-text')
  expect(footer()?.textContent).toBe('Waiting for next action')
  message = applyAgentEventToChatMessage(message, { ...activity, activity: 'reasoning' })
  await screen.rerender(<AgentRunView message={message} mode="interactive" />)
  expect(footer()?.textContent).toBe('Thinking')
  message = applyAgentEventToChatMessage(message, {
    type: 'message_delta',
    runId: activity.runId,
    streamId: activity.streamId,
    delta: 'Visible text.'
  })
  await screen.rerender(<AgentRunView message={message} mode="interactive" />)
  expect(footer()).toBeNull()
  flushSync(() => vi.advanceTimersByTime(1500))
  expect(footer()?.textContent).toBe('Waiting for next action')
  message = applyAgentEventToChatMessage(message, {
    type: 'message_stream_reset',
    runId: activity.runId,
    streamId: activity.streamId,
    reason: 'retry'
  })
  message = applyAgentEventToChatMessage(message, { ...activity, activity: 'reasoning' })
  await screen.rerender(<AgentRunView message={message} mode="interactive" />)
  expect(footer()?.textContent).toBe('Waiting for next action')
  await screen.unmount()
})

it('keeps Waiting hidden after final-answer readiness until Done, including a run with earlier tools', async () => {
  let message = runningMessage()
  message.agentRun!.status = 'running'
  message.agentRun!.toolCalls = [
    {
      id: 'earlier-call',
      tool: 'read_file',
      args: {},
      approvalStatus: 'not_required',
      reason: null
    }
  ]
  message.agentRun!.toolResults = [
    { callId: 'earlier-call', tool: 'read_file', ok: true, result: 'Earlier result' }
  ]
  message = applyAgentEventToChatMessage(message, {
    type: 'message_stream_started',
    runId: 'run',
    streamId: 'stream-final',
    attempt: 1
  })
  message = applyAgentEventToChatMessage(message, {
    type: 'message_delta',
    runId: 'run',
    streamId: 'stream-final',
    delta: 'The complete final answer.'
  })
  const screen = await render(<AgentRunView message={message} mode="interactive" />)
  expect(screen.container.textContent).toContain('The complete final answer.')
  expect(screen.container.querySelector('.agent-thinking')).toBeNull()
  message = applyAgentEventToChatMessage(message, {
    type: 'message_stream_committed',
    runId: 'run',
    streamId: 'stream-final',
    traceSequence: null
  })
  await screen.rerender(<AgentRunView message={message} mode="interactive" />)
  flushSync(() => vi.advanceTimersByTime(1500))
  // A commit alone can still be followed by a tool or queued guidance.
  expect(screen.container.querySelector('.agent-thinking')?.textContent).toBe(
    'Waiting for next action'
  )
  message = applyAgentEventToChatMessage(message, { type: 'final_answer_ready', runId: 'run' })
  await screen.rerender(<AgentRunView message={message} mode="interactive" />)
  flushSync(() => vi.advanceTimersByTime(10_000))
  expect(screen.container.querySelector('.agent-thinking')).toBeNull()
  expect(screen.container.textContent).toContain('The complete final answer.')
  expect(message.agentRun?.status).toBe('running')
  expect(message.status).toBe('pending')
  message = applyAgentEventToChatMessage(message, {
    type: 'done',
    runId: 'run',
    success: true,
    status: 'completed',
    content: 'The complete final answer.'
  })
  await screen.rerender(<AgentRunView message={message} mode="interactive" />)
  expect(screen.container.querySelector('.agent-thinking')).toBeNull()
  expect(screen.container.textContent).toContain('The complete final answer.')
  expect(vi.getTimerCount()).toBe(0)
  await screen.unmount()
})

it.each(['reasoning', 'retry', 'command'] as const)(
  'preserves the specialized %s footer even with a final marker',
  async (kind) => {
    const message = runningMessage()
    const run = message.agentRun!
    run.status = 'running'
    run.finalAnswerReady = true
    if (kind === 'reasoning')
      run.modelActivity = { streamId: 'next', attempt: 1, activity: 'reasoning' }
    if (kind === 'retry')
      run.llmRetry = {
        category: 'network',
        delayMs: 1000,
        retryAt: Date.now() + 1000,
        attempt: 2,
        maxAttempts: 4
      }
    if (kind === 'command')
      run.toolCalls = [
        {
          id: 'command',
          tool: 'command_session',
          args: {},
          approvalStatus: 'not_required',
          reason: null
        }
      ]
    const screen = await render(<AgentRunView message={message} mode="interactive" />)
    expect(screen.container.querySelector('.agent-thinking')?.textContent).toBe(
      {
        reasoning: 'Thinking',
        retry: 'Reconnecting 1/3',
        command: 'Waiting for command completion'
      }[kind]
    )
    await screen.unmount()
  }
)

it('hides the generic header waiting label for metadata-only final readiness', async () => {
  const message: ChatMessage = {
    id: 'metadata-only-final',
    role: 'assistant',
    content: '',
    createdAt: Date.now(),
    status: 'pending',
    agentRun: { ...ensureAgentRun(undefined, 'metadata-run', 'running'), finalAnswerReady: true }
  }
  const screen = await render(<AgentRunView message={message} mode="observer" />)
  expect(screen.container.querySelector('.agent-run__elapsed')?.textContent).toContain('Processed')
  expect(screen.container.querySelector('.agent-running-text')).toBeNull()
  expect(screen.container.querySelector('.agent-thinking')).toBeNull()
  await screen.unmount()
})
