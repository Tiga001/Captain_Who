import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { WorkflowRuntimeEvent, WorkflowRuntimeSnapshot } from '@mycopilot/protocol'
import { createWorkflowNode } from '../../features/workflows/workflowAuthoring'
import { WorkflowTransmissionLayer } from '../../features/workflows/project/WorkflowTransmissionLayer'
import { useWorkflowExecution } from '../../features/workflows/project/useWorkflowExecution'
import { workflowNodeQueue } from '../../features/workflows/project/workflowNodeQueue'
const mocks = vi.hoisted(() => ({
  request: vi.fn(),
  listeners: new Set<(value: WorkflowRuntimeSnapshot) => void>()
}))
vi.mock('../../features/workflows/workflowClient', () => ({ requestWorkflows: mocks.request }))
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
  })
})
