import { beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { AgentManualContextCompactionOperation } from '@mycopilot/protocol'

const service = vi.hoisted(() => ({
  load: vi.fn(),
  start: vi.fn(),
  subscribe: vi.fn(),
  translate: (key: string) => key
}))
vi.mock('../../features/agent/agentClient', () => ({
  getManualContextCompactionStatus: service.load,
  startManualContextCompaction: service.start,
  onManualContextCompaction: service.subscribe
}))
vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: service.translate })
}))
import { useManualContextCompaction } from '../../features/chat/useManualContextCompaction'
import { ConversationManualCompactionDivider } from '../../features/chat/components/ConversationModelTransition'

const running: AgentManualContextCompactionOperation = {
  schemaVersion: 1,
  operationId: 'operation',
  requestId: 'request',
  conversationId: 'chat',
  status: 'running',
  phase: 'generating',
  startedAt: 10,
  updatedAt: 20,
  coveredThroughMessageId: 'assistant'
}
const completed: AgentManualContextCompactionOperation = {
  ...running,
  status: 'completed',
  phase: 'committing',
  updatedAt: 30,
  completedAt: 30,
  summaryId: 'summary'
}

it.each(['running', 'noop', 'cancelled', 'failed', 'interrupted'] as const)(
  'does not offer a fork or cancel action for %s compaction',
  async (status) => {
    const fork = vi.fn()
    const view = await render(
      <ConversationManualCompactionDivider
        operation={{ ...running, status }}
        onContinueInNewTask={fork}
      />
    )
    const divider = view.getByTestId('manual-compaction-divider').element()
    expect(divider.querySelectorAll('button')).toHaveLength(1)
    await expect.element(divider.querySelector('button')!).toBeDisabled()
  }
)

function Harness({ id = 'chat' }: { id?: string }) {
  const state = useManualContextCompaction(id)
  return (
    <>
      <output>{state.ready ? (state.isRunning ? 'running' : 'idle') : 'loading'}</output>
      <button
        onClick={() => {
          void state.start(id)
        }}
      >
        start
      </button>
      {state.operations.map((operation) => (
        <ConversationManualCompactionDivider key={operation.operationId} operation={operation} />
      ))}
    </>
  )
}
beforeEach(() => {
  service.load.mockReset().mockResolvedValue({ operations: [] })
  service.start.mockReset().mockResolvedValue(running)
  service.subscribe.mockReset().mockReturnValue(() => {})
})

it('hydrates the divider after reload and never regresses completed state on a late running event', async () => {
  service.load.mockResolvedValue({ operations: [completed] })
  const view = await render(<Harness />)
  await expect
    .element(view.getByRole('status').last())
    .toHaveTextContent('chat.manualCompaction.completed')
  service.subscribe.mock.calls[0]![0](running)
  await expect
    .element(view.getByRole('status').last())
    .toHaveTextContent('chat.manualCompaction.completed')
  expect(service.start).not.toHaveBeenCalled()
})

it('guards concurrent starts and keeps the progress divider passive until completion', async () => {
  let resolve!: (operation: AgentManualContextCompactionOperation) => void
  service.start.mockImplementation(
    () =>
      new Promise<AgentManualContextCompactionOperation>((done) => {
        resolve = done
      })
  )
  const view = await render(<Harness />)
  await view.getByRole('button', { name: 'start', exact: true }).click()
  await view.getByRole('button', { name: 'start', exact: true }).click()
  expect(service.start).toHaveBeenCalledTimes(1)
  resolve(running)
  await expect
    .element(view.getByRole('button', { name: 'chat.manualCompaction.running', exact: true }))
    .toBeVisible()
  const divider = view.container.querySelector('[data-testid="manual-compaction-divider"]')
  expect(divider?.querySelector('button:not(:disabled)')).toBeNull()
  service.subscribe.mock.calls[0]![0](completed)
  await expect
    .element(view.getByRole('button', { name: 'chat.manualCompaction.completed', exact: true }))
    .toBeVisible()
  expect(view.container.querySelectorAll('[data-testid="manual-compaction-divider"]')).toHaveLength(
    1
  )
  expect(view.container.querySelector('[data-testid="manual-compaction-divider"]')).toBe(divider)
  expect(divider?.querySelectorAll('button')).toHaveLength(1)
})

it('keeps operation state scoped to the original conversation when navigation changes', async () => {
  service.load.mockImplementation(({ conversationId }) =>
    Promise.resolve({ operations: conversationId === 'chat' ? [running] : [] })
  )
  const view = await render(<Harness />)
  await expect
    .element(view.getByRole('button', { name: 'chat.manualCompaction.running' }))
    .toBeVisible()
  await view.rerender(<Harness id="another-chat" />)
  expect(view.container.querySelector('[data-testid="manual-compaction-divider"]')).toBeNull()
  service.subscribe.mock.calls[0]![0](completed)
  await view.rerender(<Harness />)
  await expect
    .element(view.getByRole('button', { name: 'chat.manualCompaction.completed' }))
    .toBeVisible()
  expect(service.start).not.toHaveBeenCalled()
})

it('keeps conflicting actions busy until a cancelled provider request finishes draining', async () => {
  const cancelled: AgentManualContextCompactionOperation = {
    ...running,
    status: 'cancelled',
    isBusy: true,
    completedAt: 20
  }
  service.load.mockResolvedValue({ operations: [cancelled] })
  const view = await render(<Harness />)
  await expect.element(view.container.querySelector('output')!).toHaveTextContent('running')
  const receive = service.subscribe.mock.calls[0]![0]
  receive({ ...cancelled, isBusy: false })
  await expect.element(view.container.querySelector('output')!).toHaveTextContent('idle')
  receive(cancelled)
  await expect.element(view.container.querySelector('output')!).toHaveTextContent('idle')
})
