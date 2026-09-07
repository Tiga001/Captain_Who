import type {
  AgentProposedAction,
  CollaborationApprovalDecisionResult,
  CollaborationApprovalProjection,
  CollaborationApprovalStatus
} from '@mycopilot/protocol'
import { useState } from 'react'
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
  const [decisionError, setDecisionError] = useState<string | null>(null)
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
      <output data-testid="pending-count">
        {controller.approvals.filter((entry) => entry.status === 'pending').length}
      </output>
      <output data-testid="load-error">{controller.error ?? 'none'}</output>
      <output data-testid="decision-error">{decisionError ?? 'none'}</output>
      <button
        onClick={() => {
          setDecisionError(null)
          void controller
            .decide('approval-stable', 'approve', null)
            .catch((error) =>
              setDecisionError(error instanceof Error ? error.message : String(error))
            )
        }}
        type="button"
      >
        decide
      </button>
    </div>
  )
}

function decisionResult(
  overrides: Partial<CollaborationApprovalDecisionResult> = {}
): CollaborationApprovalDecisionResult {
  return {
    schemaVersion: 1,
    approvalId: 'approval-stable',
    accepted: true,
    alreadySettled: false,
    status: 'approved',
    ...overrides
  }
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
    clients.decide.mockResolvedValue(decisionResult())
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

  it('keeps an accepted decision authoritative when the projection refresh fails', async () => {
    clients.list
      .mockReset()
      .mockResolvedValueOnce({ schemaVersion: 1, approvals: [approval('pending')] })
      .mockRejectedValueOnce(new Error('projection temporarily unavailable'))
    clients.decide.mockReset().mockResolvedValue(decisionResult())
    const screen = await render(<Harness sequence={1} />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('pending')

    await screen.getByRole('button', { name: 'decide' }).click()

    await expect.element(screen.getByTestId('status')).toHaveTextContent('approved')
    await expect
      .element(screen.getByTestId('load-error'))
      .toHaveTextContent('projection temporarily unavailable')
    await expect.element(screen.getByTestId('decision-error')).toHaveTextContent('none')
  })

  it('returns an unaccepted pending result for the decision card to reopen', async () => {
    clients.list
      .mockReset()
      .mockResolvedValueOnce({ schemaVersion: 1, approvals: [approval('pending')] })
      .mockResolvedValueOnce({ schemaVersion: 1, approvals: [approval('pending')] })
    clients.decide
      .mockReset()
      .mockResolvedValue(
        decisionResult({ accepted: false, alreadySettled: true, status: 'pending' })
      )
    const screen = await render(<Harness sequence={1} />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('pending')

    await screen.getByRole('button', { name: 'decide' }).click()

    await expect.element(screen.getByTestId('status')).toHaveTextContent('pending')
    await expect.element(screen.getByTestId('decision-error')).toHaveTextContent('none')
    expect(clients.list).toHaveBeenCalledTimes(2)
  })

  it('projects an already-settled authoritative status without reopening the decision', async () => {
    clients.list
      .mockReset()
      .mockResolvedValueOnce({ schemaVersion: 1, approvals: [approval('pending')] })
      .mockResolvedValueOnce({ schemaVersion: 1, approvals: [approval('completed')] })
    clients.decide
      .mockReset()
      .mockResolvedValue(
        decisionResult({ accepted: false, alreadySettled: true, status: 'completed' })
      )
    const screen = await render(<Harness sequence={1} />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('pending')

    await screen.getByRole('button', { name: 'decide' }).click()

    await expect.element(screen.getByTestId('status')).toHaveTextContent('completed')
    await expect.element(screen.getByTestId('decision-error')).toHaveTextContent('none')
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

  it('removes each acknowledged approval from the pending count before refresh finishes and never reopens it from stale lists', async () => {
    const refresh = deferred<{ schemaVersion: 1; approvals: CollaborationApprovalProjection[] }>()
    const projections = [approval('pending'), approval('pending', 'conversation-root', 'other')]
    clients.list
      .mockReset()
      .mockResolvedValueOnce({ schemaVersion: 1, approvals: projections })
      .mockReturnValueOnce(refresh.promise)
      .mockResolvedValueOnce({ schemaVersion: 1, approvals: projections })
    clients.decide.mockReset().mockResolvedValue(decisionResult())
    const screen = await render(<Harness sequence={1} />)
    await expect.element(screen.getByTestId('pending-count')).toHaveTextContent('2')

    await screen.getByRole('button', { name: 'decide' }).click()
    await expect.element(screen.getByTestId('status')).toHaveTextContent('loading')
    await expect.element(screen.getByTestId('pending-count')).toHaveTextContent('1')

    // A notification can start another list before the decision-triggered reload completes.
    await screen.rerender(<Harness sequence={2} />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('approved')
    await expect.element(screen.getByTestId('pending-count')).toHaveTextContent('1')
    refresh.resolve({ schemaVersion: 1, approvals: projections })
    await expect.element(screen.getByTestId('pending-count')).toHaveTextContent('1')
  })

  it('allows a list-only approved state to recover to pending without a local decision acknowledgement', async () => {
    clients.list
      .mockReset()
      .mockResolvedValueOnce({ schemaVersion: 1, approvals: [approval('approved')] })
      .mockResolvedValueOnce({ schemaVersion: 1, approvals: [approval('pending')] })
    const screen = await render(<Harness sequence={1} />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('approved')

    await screen.rerender(<Harness sequence={2} />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('pending')
  })

  it('does not let a late decision refresh from the previous root replace the active root or invalidate its load', async () => {
    const decision = deferred<CollaborationApprovalDecisionResult>()
    const nextRoot = deferred<{ schemaVersion: 1; approvals: CollaborationApprovalProjection[] }>()
    clients.list
      .mockReset()
      .mockResolvedValueOnce({ schemaVersion: 1, approvals: [approval('pending', 'root-a')] })
      .mockReturnValueOnce(nextRoot.promise)
    clients.decide.mockReset().mockReturnValueOnce(decision.promise)
    const screen = await render(<Harness rootConversationId="root-a" sequence={1} />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('pending')
    await screen.getByRole('button', { name: 'decide' }).click()

    await screen.rerender(<Harness rootConversationId="root-b" sequence={1} />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('loading')
    decision.resolve(decisionResult())
    nextRoot.resolve({ schemaVersion: 1, approvals: [approval('pending', 'root-b')] })

    await expect.element(screen.getByTestId('status')).toHaveTextContent('pending')
    await expect.element(screen.getByTestId('pending-count')).toHaveTextContent('1')
    expect(clients.list).toHaveBeenCalledTimes(2)
  })

  it('deduplicates decisions within one root while allowing the same approval identity in another root', async () => {
    const firstDecision = deferred<CollaborationApprovalDecisionResult>()
    const secondDecision = deferred<CollaborationApprovalDecisionResult>()
    clients.list
      .mockReset()
      .mockResolvedValueOnce({ schemaVersion: 1, approvals: [approval('pending', 'root-a')] })
      .mockResolvedValueOnce({ schemaVersion: 1, approvals: [approval('pending', 'root-b')] })
      .mockResolvedValueOnce({ schemaVersion: 1, approvals: [approval('pending', 'root-b')] })
    clients.decide
      .mockReset()
      .mockReturnValueOnce(firstDecision.promise)
      .mockReturnValueOnce(secondDecision.promise)
    const screen = await render(<Harness rootConversationId="root-a" sequence={1} />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('pending')
    await screen.getByRole('button', { name: 'decide' }).click()
    await screen.getByRole('button', { name: 'decide' }).click()
    await expect
      .element(screen.getByTestId('decision-error'))
      .toHaveTextContent('already in progress')
    expect(clients.decide).toHaveBeenCalledTimes(1)

    await screen.rerender(<Harness rootConversationId="root-b" sequence={1} />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('pending')
    await screen.getByRole('button', { name: 'decide' }).click()
    expect(clients.decide).toHaveBeenNthCalledWith(2, {
      approvalId: 'approval-stable',
      decision: 'approve',
      message: null,
      rootConversationId: 'root-b'
    })
    secondDecision.resolve(decisionResult())
    firstDecision.resolve(decisionResult({ status: 'completed' }))
    await expect.element(screen.getByTestId('status')).toHaveTextContent('approved')
    await expect.element(screen.getByTestId('decision-error')).toHaveTextContent('none')
    expect(clients.list).toHaveBeenCalledTimes(3)
  })
})

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((finish) => {
    resolve = finish
  })
  return { promise, resolve }
}
