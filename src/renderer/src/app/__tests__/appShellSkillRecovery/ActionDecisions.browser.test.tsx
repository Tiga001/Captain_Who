import type { AgentActionExecutionOutput, AgentProposedAction } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { testState } from './state'
import { mockSuccessfulTurnStarts } from './turnFixtures'
import { emitAgentEvent, deferred } from './eventFixtures'
import { storedConversationWithRun } from './conversationFixtures'
import { renderSelectedConversation } from './setup'

describe('Core-owned action decisions', () => {
  const action: AgentProposedAction = {
    type: 'command',
    command: {
      id: 'round5-approval',
      command: 'echo fake',
      riskLevel: 'read_only',
      observe: null,
      cwd: null,
      timeoutMs: 1000,
      approvalStatus: 'required',
      reason: null
    }
  }
  function execution(status: 'approved' | 'rejected'): AgentActionExecutionOutput {
    return {
      actionId: 'round5-approval',
      actionType: 'command',
      toolName: 'run_command',
      status,
      agentOutput: {
        runId: 'run-1',
        status: 'running',
        content: '',
        events: [],
        toolDefinitions: [],
        proposedActions: []
      }
    }
  }
  it('upgrades a legacy pending run only after an accepted decision ACK', async () => {
    const stored = storedConversationWithRun('assistant-legacy', 'run-1', 'running')
    stored.messages.at(-1)!.agentRun!.status = 'waiting_for_approval'
    stored.messages.at(-1)!.agentRun!.approvals = [action]
    testState.persistedConversations.set(stored.id, stored)
    testState.cancelAgentAction.mockResolvedValueOnce(false).mockResolvedValueOnce(true)
    const consoleWarn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    const screen = await renderSelectedConversation()
    await expect.element(screen.getByTestId('approval-count')).toHaveTextContent('1')
    await screen.getByRole('button', { name: 'cancel-pending-action' }).click()
    await expect.poll(() => testState.cancelAgentAction.mock.calls.length).toBe(1)
    await expect.poll(() => consoleWarn.mock.calls.length).toBe(1)
    emitAgentEvent({
      type: 'message',
      runId: 'run-1',
      content: 'Legacy still owns this checkpoint.'
    })
    await expect.poll(() => testState.saveChatMessageState.mock.calls.length).toBeGreaterThan(0)
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    testState.saveChatMessageState.mockClear()
    await screen.getByRole('button', { name: 'cancel-pending-action' }).click()
    await expect.element(screen.getByTestId('approval-count')).toHaveTextContent('0')
    emitAgentEvent({ type: 'message', runId: 'run-1', content: 'Core owns the accepted decision.' })
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('Core owns the accepted decision.')
    window.dispatchEvent(new Event('pagehide'))
    await new Promise((resolve) => window.setTimeout(resolve, 350))
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    consoleWarn.mockRestore()
  })

  it.each(['approve', 'reject', 'cancel'] as const)(
    'sends %s without a message save and settles only after the authoritative ACK',
    async (operation) => {
      mockSuccessfulTurnStarts()
      const decision = deferred<AgentActionExecutionOutput | boolean>()
      const invoke =
        operation === 'approve'
          ? testState.approveAgentAction
          : operation === 'reject'
            ? testState.rejectAgentAction
            : testState.cancelAgentAction
      invoke.mockReturnValueOnce(decision.promise)
      testState.saveChatMessageState.mockImplementation(() => new Promise<void>(() => undefined))
      const screen = await renderSelectedConversation()
      await screen.getByRole('button', { name: 'submit-without-skill' }).click()
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
      emitAgentEvent({ type: 'approval_required', runId: 'run-1', action })
      await expect.element(screen.getByTestId('approval-count')).toHaveTextContent('1')
      await screen.getByRole('button', { name: `${operation}-pending-action` }).click()
      await expect.poll(() => invoke.mock.calls.length).toBe(1)
      await expect.element(screen.getByTestId('approval-count')).toHaveTextContent('1')
      expect(testState.saveChatMessageState).not.toHaveBeenCalled()
      decision.resolve(
        operation === 'cancel' ? true : execution(operation === 'approve' ? 'approved' : 'rejected')
      )
      await expect.element(screen.getByTestId('approval-count')).toHaveTextContent('0')
      expect(testState.saveChatMessageState).not.toHaveBeenCalled()
      expect(testState.saveConversationMeta).not.toHaveBeenCalled()
      expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    }
  )

  it.each(['lost_ack', 'not_accepted'] as const)(
    'keeps the original pending identity after %s and allows the exact decision to retry',
    async (outcome) => {
      mockSuccessfulTurnStarts()
      const decision = deferred<boolean>()
      testState.cancelAgentAction.mockReturnValueOnce(decision.promise).mockResolvedValueOnce(true)
      const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)
      const consoleWarn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
      const screen = await renderSelectedConversation()
      await screen.getByRole('button', { name: 'submit-without-skill' }).click()
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
      emitAgentEvent({ type: 'approval_required', runId: 'run-1', action })
      await expect.element(screen.getByTestId('approval-count')).toHaveTextContent('1')
      await screen.getByRole('button', { name: 'cancel-pending-action' }).click()
      await expect.poll(() => testState.cancelAgentAction.mock.calls.length).toBe(1)
      if (outcome === 'lost_ack') decision.reject(new Error('decision ACK lost'))
      else decision.resolve(false)
      await expect
        .poll(() => consoleError.mock.calls.length + consoleWarn.mock.calls.length)
        .toBe(1)
      await expect.element(screen.getByTestId('approval-count')).toHaveTextContent('1')
      await screen.getByRole('button', { name: 'cancel-pending-action' }).click()
      await expect.element(screen.getByTestId('approval-count')).toHaveTextContent('0')
      expect(testState.cancelAgentAction.mock.calls).toEqual([
        ['run-1', 'round5-approval'],
        ['run-1', 'round5-approval']
      ])
      expect(testState.saveChatMessageState).not.toHaveBeenCalled()
      consoleError.mockRestore()
      consoleWarn.mockRestore()
    }
  )
})
