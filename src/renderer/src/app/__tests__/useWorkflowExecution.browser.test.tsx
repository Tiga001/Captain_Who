import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { WorkflowRuntimeSnapshot } from '@mycopilot/protocol'
import { useWorkflowExecution } from '../../features/workflows/project/useWorkflowExecution'

const mocks = vi.hoisted(() => ({
  request: vi.fn(),
  listeners: new Set<(snapshot: WorkflowRuntimeSnapshot) => void>()
}))
vi.mock('../../features/workflows/workflowClient', () => ({ requestWorkflows: mocks.request }))
vi.mock('../../host/hostClient', () => ({
  hostClient: {
    agent: {
      onWorkflowRuntimeChanged: (listener: (snapshot: WorkflowRuntimeSnapshot) => void) => {
        mocks.listeners.add(listener)
        return () => mocks.listeners.delete(listener)
      }
    }
  }
}))

function snapshot(
  sequence: number,
  kind = 'sent',
  instanceId = 'workflow'
): WorkflowRuntimeSnapshot {
  return {
    instanceId,
    sequence,
    inputs: [],
    events: [
      { sequence, instanceId, inputId: null, flowIds: ['flow'], kind, createdAt: Date.now() }
    ]
  }
}
function Harness({ instanceId = 'workflow' }: { instanceId?: string }) {
  const execution = useWorkflowExecution(instanceId)
  return (
    <>
      <output data-testid="sequence">{execution.snapshot?.sequence ?? 'none'}</output>
      <output data-testid="transmissions">
        {execution.transmissions.map((event) => event.sequence).join(',') || 'none'}
      </output>
      <button onClick={() => void execution.completeUserInput('input')}>Complete</button>
    </>
  )
}
const emit = async (value: WorkflowRuntimeSnapshot) =>
  act(() => {
    mocks.listeners.forEach((listener) => listener(value))
  })
beforeEach(() => {
  mocks.request.mockReset().mockResolvedValue({ records: [], issues: [], runtime: snapshot(4) })
  mocks.listeners.clear()
})
afterEach(() => vi.restoreAllMocks())

describe('workflow execution projection', () => {
  it('never revives old transmissions when switching away and back to an instance', async () => {
    mocks.request.mockImplementation(async (request) => ({
      records: [],
      issues: [],
      runtime: snapshot(4, 'sent', request.instanceId)
    }))
    const view = await render(<Harness />)
    await expect.element(view.getByTestId('sequence')).toHaveTextContent('4')
    await emit(snapshot(5))
    await expect.element(view.getByTestId('transmissions')).toHaveTextContent('5')
    await view.rerender(<Harness instanceId="other" />)
    await expect.element(view.getByTestId('transmissions')).toHaveTextContent('none')
    await view.rerender(<Harness />)
    await expect.element(view.getByTestId('sequence')).toHaveTextContent('4')
    await expect.element(view.getByTestId('transmissions')).toHaveTextContent('none')
  })
  it('starts from a quiet baseline, animates real new sends once, and rejects old or foreign snapshots', async () => {
    const view = await render(<Harness />)
    await expect.element(view.getByTestId('sequence')).toHaveTextContent('4')
    await expect.element(view.getByTestId('transmissions')).toHaveTextContent('none')
    await emit(snapshot(5))
    await emit(snapshot(5))
    await emit(snapshot(3))
    await emit(snapshot(99, 'sent', 'other'))
    await expect.element(view.getByTestId('sequence')).toHaveTextContent('5')
    await expect.element(view.getByTestId('transmissions')).toHaveTextContent('5')
    await emit(snapshot(6, 'failed'))
    await expect.element(view.getByTestId('sequence')).toHaveTextContent('6')
    await expect.element(view.getByTestId('transmissions')).toHaveTextContent('5')
    await view.unmount()
    expect(mocks.listeners.size).toBe(0)
  })
  it('confirms the exact input and accepts its saved state without initiating a conversation', async () => {
    const view = await render(<Harness />)
    await expect.element(view.getByTestId('sequence')).toHaveTextContent('4')
    mocks.request.mockResolvedValue({ records: [], issues: [], runtime: snapshot(5, 'completed') })
    await view.getByRole('button', { name: 'Complete' }).click()
    expect(mocks.request).toHaveBeenLastCalledWith({
      operation: 'completeUserInput',
      instanceId: 'workflow',
      inputId: 'input'
    })
    await expect.element(view.getByTestId('sequence')).toHaveTextContent('5')
    await expect.element(view.getByTestId('transmissions')).toHaveTextContent('none')
  })
})
