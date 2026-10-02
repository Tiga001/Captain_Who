import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type {
  WorkflowNodeMessage,
  WorkflowRuntimeEvent,
  WorkflowRuntimeSnapshot
} from '@mycopilot/protocol'
import { createWorkflowNode } from '../../features/workflows/workflowAuthoring'
import { WorkflowTransmissionLayer } from '../../features/workflows/project/WorkflowTransmissionLayer'
import { useWorkflowExecution } from '../../features/workflows/project/useWorkflowExecution'
import { workflowNodeQueue } from '../../features/workflows/project/workflowNodeQueue'
import { WorkflowNodePanel } from '../../features/workflows/project/WorkflowNodePanel'
const mocks = vi.hoisted(() => ({
  request: vi.fn(),
  listeners: new Set<(value: WorkflowRuntimeSnapshot) => void>()
}))
vi.mock('../../features/workflows/workflowClient', () => ({ requestWorkflows: mocks.request }))
vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'en-US' })
}))
vi.mock('../../host/hostClient', () => ({
  hostClient: {
    agent: {
      onWorkflowRuntimeChanged: (listener: (value: WorkflowRuntimeSnapshot) => void) => {
        mocks.listeners.add(listener)
        return () => mocks.listeners.delete(listener)
      }
    }
  }
}))
const sender = { ...createWorkflowNode('Sender', 80, 90), id: 'sender' }
const receiver = { ...createWorkflowNode('Receiver', 420, 200), id: 'receiver' }
function event(sequence: number, kind = 'sent'): WorkflowRuntimeEvent {
  return {
    sequence,
    instanceId: 'network',
    inputId: null,
    messageId: `mail-${sequence}`,
    sourceNodeId: kind === 'recalled' ? 'receiver' : 'sender',
    targetNodeId: kind === 'recalled' ? 'sender' : 'receiver',
    kind,
    createdAt: Date.now()
  }
}
function snapshot(sequence: number, kind = 'sent'): WorkflowRuntimeSnapshot {
  return { instanceId: 'network', sequence, inputs: [], events: [event(sequence, kind)] }
}
function Harness({ foreground = true }: { foreground?: boolean }) {
  const { snapshot: value, transmissions } = useWorkflowExecution('network', foreground)
  return (
    <>
      <output data-testid="sequence">{value?.sequence}</output>
      <WorkflowTransmissionLayer
        nodes={[sender, receiver]}
        events={transmissions}
        origin={{ x: 0, y: 0 }}
        width={800}
        height={400}
      />
    </>
  )
}
async function emit(value: WorkflowRuntimeSnapshot) {
  await act(() => mocks.listeners.forEach((listener) => listener(value)))
}
beforeEach(() => {
  mocks.request.mockReset().mockResolvedValue({ records: [], issues: [], runtime: snapshot(1) })
  mocks.listeners.clear()
})
afterEach(() => vi.restoreAllMocks())
describe('mail network activity', () => {
  it('starts a newly received pulse at its source even after the shared SVG timeline has advanced', async () => {
    const view = await render(<Harness />)
    await expect.element(view.getByTestId('sequence')).toHaveTextContent('1')
    const svg = view.container.querySelector('svg')!
    // Control the real SVG clock so concurrent browser tabs cannot throttle it.
    svg.pauseAnimations()
    svg.setCurrentTime(30)
    await emit(snapshot(2))
    const pulse = view.container.querySelector('circle')!
    const start = pulse.getBoundingClientRect().x
    expect(start).toBeLessThan(receiver.x)
    svg.setCurrentTime(30.8)
    await expect
      .poll(() => pulse.getBoundingClientRect().x, { timeout: 1500 })
      .toBeGreaterThan(start + 15)
  })
  it('draws only actual send/recall events, respecting authoritative direction and expiry', async () => {
    const view = await render(<Harness />)
    await expect.element(view.getByTestId('sequence')).toHaveTextContent('1')
    expect(view.container.querySelectorAll('[data-transmission-sequence]')).toHaveLength(0)
    await emit(snapshot(2))
    await emit(snapshot(2))
    expect(view.container.querySelectorAll('[data-transmission-sequence]')).toHaveLength(1)
    expect(
      view.container
        .querySelector('[data-transmission-sequence="2"]')
        ?.getAttribute('data-source-node')
    ).toBe('sender')
    await emit(snapshot(3, 'recalled'))
    expect(
      view.container
        .querySelector('[data-transmission-sequence="3"]')
        ?.getAttribute('data-source-node')
    ).toBe('receiver')
    await emit(snapshot(4, 'completed'))
    expect(view.container.querySelectorAll('[data-transmission-sequence]')).toHaveLength(2)
    await expect
      .poll(() => view.container.querySelectorAll('[data-transmission-sequence]').length, {
        timeout: 4000
      })
      .toBe(0)
  })
  it('updates hidden state without accumulating or replaying animations', async () => {
    const view = await render(<Harness />)
    await expect.element(view.getByTestId('sequence')).toHaveTextContent('1')
    await emit(snapshot(2))
    await view.rerender(<Harness foreground={false} />)
    expect(view.container.querySelectorAll('[data-transmission-sequence]')).toHaveLength(0)
    await emit(snapshot(3))
    await expect.element(view.getByTestId('sequence')).toHaveTextContent('3')
    mocks.request.mockResolvedValue({ records: [], issues: [], runtime: snapshot(3) })
    await view.rerender(<Harness />)
    expect(view.container.querySelectorAll('[data-transmission-sequence]')).toHaveLength(0)
    await emit(snapshot(4))
    expect(view.container.querySelectorAll('[data-transmission-sequence]')).toHaveLength(1)
  })
  it('establishes a fresh baseline after the document was hidden without notifications', async () => {
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible')
    const view = await render(<Harness />)
    await expect.element(view.getByTestId('sequence')).toHaveTextContent('1')
    await emit(snapshot(2))
    expect(view.container.querySelectorAll('[data-transmission-sequence]')).toHaveLength(1)
    visibility.mockReturnValue('hidden')
    await act(() => document.dispatchEvent(new Event('visibilitychange')))
    expect(view.container.querySelectorAll('[data-transmission-sequence]')).toHaveLength(0)
    mocks.request.mockResolvedValue({ records: [], issues: [], runtime: snapshot(5) })
    visibility.mockReturnValue('visible')
    await act(() => document.dispatchEvent(new Event('visibilitychange')))
    await expect.element(view.getByTestId('sequence')).toHaveTextContent('5')
    expect(view.container.querySelectorAll('[data-transmission-sequence]')).toHaveLength(0)
    await emit(snapshot(6))
    expect(view.container.querySelectorAll('[data-transmission-sequence]')).toHaveLength(1)
  })
  it('counts mail by authoritative state rather than a later run failure', () => {
    const value = {
      ...snapshot(8),
      inputs: [
        { id: 'pending', nodeId: 'receiver', mailStatus: 'pending', messages: [{ id: 'p' }] },
        { id: 'done', nodeId: 'receiver', mailStatus: 'processed', messages: [{ id: 'd' }] },
        { id: 'active', nodeId: 'receiver', mailStatus: 'processing', messages: [{ id: 'a' }] }
      ],
      inputRuns: [{ inputId: 'done', status: 'failed' }]
    } as unknown as WorkflowRuntimeSnapshot
    const queue = workflowNodeQueue(value, 'receiver')
    expect(queue.waiting.map((input) => input.id)).toEqual(['pending'])
    expect(queue.processing.map((input) => input.id)).toEqual(['active'])
  })
  it('refreshes older displayed mail from storage after it leaves the compact runtime snapshot', async () => {
    const row = (sequence: number): WorkflowNodeMessage => ({
      sequence,
      inputId: `input-${sequence}`,
      status: sequence === 1 ? 'pending' : 'processed',
      runStatus: null,
      error: null,
      message: {
        id: `mail-${sequence}`,
        instanceId: 'network',
        workflowName: 'Network',
        sourceNodeId: sender.id,
        sourceNodeName: sender.name,
        sourceConversationId: 'sender-chat',
        sourceConversationTitle: 'Sender chat',
        targetNodeId: receiver.id,
        targetNodeName: receiver.name,
        targetConversationId: 'receiver-chat',
        targetConversationTitle: 'Receiver chat',
        replyToMessageId: null,
        content: sequence === 1 ? 'Old letter body' : `Letter ${sequence}`,
        createdAt: sequence
      }
    })
    let rows = [...Array.from({ length: 20 }, (_, index) => row(200 - index)), row(1)]
    mocks.request.mockImplementation(async ({ beforeSequence }: { beforeSequence?: number }) => {
      const matching = rows.filter(
        (message) => beforeSequence === undefined || message.sequence < beforeSequence
      )
      const messages = matching.slice(0, 20)
      return {
        records: [],
        issues: [],
        nodeMessages: {
          instanceId: 'network',
          nodeId: receiver.id,
          messages,
          nextBeforeSequence: matching.length > 20 ? messages.at(-1)!.sequence : null
        }
      }
    })
    const pending: WorkflowRuntimeSnapshot = {
      ...snapshot(200),
      inputs: [
        {
          id: 'input-1',
          instanceId: 'network',
          nodeId: receiver.id,
          conversationId: 'receiver-chat',
          executionVersion: 'version',
          content: '',
          messages: [row(1).message],
          mailStatus: 'pending',
          status: 'pending',
          runId: null,
          deliveryId: null,
          createdAt: 1,
          error: null
        }
      ]
    }
    const panel = (value: WorkflowRuntimeSnapshot) => (
      <WorkflowNodePanel
        instanceId="network"
        node={receiver}
        snapshot={value}
        title="Receiver"
        status="Standby"
        onClose={() => {}}
        onOpenConversation={() => {}}
      />
    )
    const view = await render(panel(pending))
    await view.getByRole('button', { name: 'Load older messages' }).click()
    await expect.element(view.getByText('Old letter body')).toBeVisible()
    const oldStatus = () =>
      [...view.container.querySelectorAll('article')]
        .find((article) => article.textContent?.includes('Old letter body'))
        ?.querySelector('header span')?.textContent
    expect(oldStatus()).toBe('Pending')
    rows = [
      ...Array.from({ length: 12 }, (_, index) => row(212 - index)),
      ...rows.map((message) => ({ ...message, status: 'processed' }))
    ]
    await view.rerender(panel({ ...snapshot(213), inputs: [] }))
    await expect.poll(oldStatus).toBe('Processed')
    await expect.element(view.getByText('Old letter body')).toBeVisible()
    await expect.element(view.getByText('Letter 212')).toBeVisible()
    expect(view.container.querySelectorAll('article')).toHaveLength(33)
    expect(view.getByRole('button', { name: 'Load older messages' }).elements()).toHaveLength(0)
  })
})
