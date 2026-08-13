import type {
  AgentProposedAction,
  CollaborationApprovalProjection,
  CollaborationApprovalStatus
} from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'

const clients = vi.hoisted(() => ({
  decide: vi.fn(),
  list: vi.fn()
}))

vi.mock('../collaborationClient', () => ({
  decideCollaborationApproval: clients.decide,
  listCollaborationApprovals: clients.list
}))

const { useCollaborationApprovals } = await import('../useCollaborationApprovals')

const action: AgentProposedAction = {
  type: 'command',
  command: {
    id: 'command-action',
    command: 'cargo test',
    cwd: null,
    timeoutMs: null,
    approvalStatus: 'required',
    riskLevel: null,
    reason: 'Run tests',
    observe: null
  }
}

function approval(
  status: CollaborationApprovalStatus,
  rootConversationId = 'conversation-root',
  approvalId = 'approval-stable'
): CollaborationApprovalProjection {
  return {
    schemaVersion: 1,
    approvalId,
    rootAgentId: 'agent-root',
    rootConversationId,
    sourceAgentId: 'agent-child',
    sourceTaskPath: '/root/review',
    sourceConversationId: 'conversation-child',
    runId: 'run-child',
    actionId: 'command-action',
    actionType: 'command',
    toolName: 'run_command',
    action,
    status,
    createdAt: 1,
    updatedAt: status === 'pending' ? 1 : 2
  }
}

function Harness({
  rootConversationId = 'conversation-root',
  sequence
}: {
  rootConversationId?: string
  sequence: number
}) {
  const controller = useCollaborationApprovals({
    invalidationSequence: sequence,
    rootConversationId
  })
  return (
    <div>
      <output data-testid="status">
        {controller.loading
          ? 'loading'
          : (controller.approvals[0]?.status ?? controller.error ?? 'empty')}
      </output>
      <output data-testid="approvals">
        {controller.approvals.map((entry) => entry.approvalId).join(',')}
      </output>
      <button
        onClick={() => void controller.decide('approval-stable', 'approve', null)}
        type="button"
      >
        decide
      </button>
    </div>
  )
}

describe('useCollaborationApprovals', () => {
  it('refreshes from an externally supplied durable sequence without creating a subscription', async () => {
    clients.list.mockReset()
    clients.decide.mockReset()
    clients.list
      .mockResolvedValueOnce({ schemaVersion: 1, approvals: [approval('pending')] })
      .mockResolvedValueOnce({ schemaVersion: 1, approvals: [approval('executing')] })
    const screen = await render(<Harness sequence={1} />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('pending')

    await screen.rerender(<Harness sequence={2} />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('executing')
    expect(clients.list).toHaveBeenNthCalledWith(1, {
      rootConversationId: 'conversation-root'
    })
    expect(clients.list).toHaveBeenNthCalledWith(2, {
      rootConversationId: 'conversation-root'
    })
  })

  it('routes a decision then reloads the durable projection', async () => {
    clients.list.mockReset()
    clients.decide.mockReset()
    clients.list
      .mockResolvedValueOnce({ schemaVersion: 1, approvals: [approval('pending')] })
      .mockResolvedValueOnce({ schemaVersion: 1, approvals: [approval('approved')] })
    clients.decide.mockResolvedValue({
      schemaVersion: 1,
      approvalId: 'approval-stable',
      accepted: true,
      alreadySettled: false,
      status: 'approved'
    })
    const screen = await render(<Harness sequence={1} />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('pending')

    await screen.getByRole('button', { name: 'decide' }).click()
    await expect.element(screen.getByTestId('status')).toHaveTextContent('approved')
    expect(clients.decide).toHaveBeenCalledWith({
      approvalId: 'approval-stable',
      decision: 'approve',
      message: null,
      rootConversationId: 'conversation-root'
    })
    expect(clients.list).toHaveBeenCalledTimes(2)
  })

  it('never exposes approvals from the previous root while the new root is loading', async () => {
    const nextRoot = deferred<{ schemaVersion: 1; approvals: CollaborationApprovalProjection[] }>()
    clients.list
      .mockReset()
      .mockResolvedValueOnce({
        schemaVersion: 1,
        approvals: [approval('pending', 'root-a', 'approval-a')]
      })
      .mockReturnValueOnce(nextRoot.promise)

    const screen = await render(<Harness rootConversationId="root-a" sequence={1} />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('pending')

    await screen.rerender(<Harness rootConversationId="root-b" sequence={1} />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('loading')
    expect(screen.container.textContent).not.toContain('approval-a')

    nextRoot.resolve({
      schemaVersion: 1,
      approvals: [approval('pending', 'root-b', 'approval-b')]
    })
    await expect.element(screen.getByTestId('status')).toHaveTextContent('pending')
  })

  it('keeps a same-root durable approval visible while an invalidation refresh is pending', async () => {
    const refresh = deferred<{
      schemaVersion: 1
      approvals: CollaborationApprovalProjection[]
    }>()
    clients.list
      .mockReset()
      .mockResolvedValueOnce({
        schemaVersion: 1,
        approvals: [approval('pending')]
      })
      .mockReturnValueOnce(refresh.promise)

    const screen = await render(<Harness sequence={1} />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('pending')

    await screen.rerender(<Harness sequence={2} />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('loading')
    await expect.element(screen.getByTestId('approvals')).toHaveTextContent('approval-stable')

    refresh.resolve({ schemaVersion: 1, approvals: [approval('executing')] })
    await expect.element(screen.getByTestId('status')).toHaveTextContent('executing')
  })
})

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((finish) => {
    resolve = finish
  })
  return { promise, resolve }
}
