// Clock ticks must update elapsed/activity labels without re-parsing settled timeline Markdown.
import { Profiler } from 'react'
import { flushSync } from 'react-dom'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { remarkNormalizeCjkAutolinkBoundaries } from '../components/chatMarkdownAutolinks'
import { AgentRunView } from '../components/AgentRunView'
import type { ChatMessage } from '../chatTypes'

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: (key: string) =>
      ({
        'agent.processed': 'Processed {duration}',
        'agent.stoppedAfter': 'Stopped after {duration}',
        'agent.thinking': 'Thinking',
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
  expect(screen.container.querySelector('.agent-thinking')?.textContent).toBe('Thinking')

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
