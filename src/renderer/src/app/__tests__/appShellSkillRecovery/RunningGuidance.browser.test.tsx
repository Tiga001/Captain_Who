import type {
  AgentConversationTurnInput,
  AgentEvent,
  AgentSteerRunOutput
} from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import type { ChatComposerDraft, ChatQueuedMessage } from '../../../features/chat/chatTypes'
import { testState } from './state'
import { skillSelection, mockSuccessfulTurnStarts } from './turnFixtures'
import {
  emitAgentEvent,
  completedProviderTransition,
  runningProviderTransition,
  deferred
} from './eventFixtures'
import { storedConversation, queuedMessage } from './conversationFixtures'
import { createComposerDraft, renderSelectedConversation } from './setup'

describe('running conversation guidance queue', () => {
  it('restores acknowledged guidance abandoned by a core restart into the editable queue', async () => {
    const interrupted = storedConversation()
    const assistant = interrupted.messages.at(-1)
    if (!assistant?.agentRun) throw new Error('missing assistant run fixture')
    assistant.agentRun.timeline.push({
      id: 'user-guidance-client-interrupted',
      type: 'user_guidance',
      guidanceId: 'guidance-interrupted',
      clientMessageId: 'client-interrupted',
      content: 'Recover after restart.\n\nAttachments: recovery.txt',
      attachments: [
        {
          id: 'attachment-interrupted',
          kind: 'file',
          name: 'recovery.txt',
          mimeType: 'text/plain',
          sizeBytes: 7
        }
      ],
      status: 'rejected',
      rejectionCode: 'run_interrupted',
      error: 'Core process restarted.',
      recoverable: true,
      createdAt: 10
    })
    testState.loadConversation.mockResolvedValueOnce(interrupted)
    testState.loadInputAttachments.mockResolvedValueOnce([
      {
        id: 'attachment-interrupted',
        kind: 'file',
        name: 'recovery.txt',
        mimeType: 'text/plain',
        sizeBytes: 7,
        encoding: 'managed',
        data: 'managed-test-cmVjb3Zlcg=='
      }
    ])

    const screen = await renderSelectedConversation()

    await expect
      .element(screen.getByTestId('queued-message-ids'))
      .toHaveTextContent('recovered-guidance-guidance-interrupted')
    expect(testState.loadInputAttachments).toHaveBeenCalledWith(['attachment-interrupted'])
    await expect
      .poll(() => {
        const draft = testState.saveComposerDraft.mock.calls.at(-1)?.[1] as
          ChatComposerDraft | undefined
        return draft?.queuedMessages[0]
      })
      .toEqual(
        expect.objectContaining({
          clientMessageId: 'client-interrupted',
          content: 'Recover after restart.\n\nAttachments: recovery.txt',
          status: 'error',
          attachments: [expect.objectContaining({ id: 'attachment-interrupted' })]
        })
      )
  })

  it('guides any selected item in the active run and auto-sends the remaining head item', async () => {
    const first = queuedMessage('queue-first', 'send me next', 10)
    const second = queuedMessage('queue-second', 'guide this run', 20)
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({
        modelId: 'model-1',
        projectId: 'project-a',
        queuedMessages: [first, second]
      })
    })
    mockSuccessfulTurnStarts()
    const steering = deferred<AgentSteerRunOutput>()
    testState.steerAgentRun.mockReturnValueOnce(steering.promise)
    const screen = await renderSelectedConversation()

    await expect
      .element(screen.getByTestId('queued-message-ids'))
      .toHaveTextContent('queue-first,queue-second')
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()

    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await screen.getByRole('button', { name: 'select-custom-permissions' }).click()
    const preflightsBeforeGuidance = testState.preflightProviderTransition.mock.calls.length
    await screen.getByRole('button', { name: 'guide-second-message' }).click()
    await expect.poll(() => testState.steerAgentRun.mock.calls.length).toBe(1)
    expect(testState.steerAgentRun).toHaveBeenCalledWith({
      conversationId: 'conversation-a',
      expectedRunId: 'run-1',
      clientMessageId: 'client-queue-second',
      content: 'guide this run',
      attachments: [],
      folderReferences: []
    })
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(preflightsBeforeGuidance)
    expect(testState.startConversationTurn.mock.calls[0]?.[0].modelId).toBe('model-1')
    await expect
      .element(screen.getByTestId('guidance-timeline'))
      .toHaveTextContent('client-queue-second:submitting')

    steering.resolve({ guidanceId: 'guidance-1', status: 'queued' })
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-first')
    await expect
      .element(screen.getByTestId('guidance-timeline'))
      .toHaveTextContent('client-queue-second:queued')

    // A synchronous human-input pause must keep the already inserted guidance in the
    // assistant timeline. The backend handoff intentionally emits no guidance_rejected event.
    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'waiting_for_user_input'
    })
    await expect
      .element(screen.getByTestId('guidance-timeline'))
      .toHaveTextContent('client-queue-second:queued')

    emitAgentEvent({
      type: 'guidance_applied',
      runId: 'run-1',
      guidanceId: 'guidance-1',
      clientMessageId: 'client-queue-second',
      content: 'guide this run',
      attachments: [],
      createdAt: 20,
      sequence: 1
    })
    await expect
      .element(screen.getByTestId('guidance-timeline'))
      .toHaveTextContent('client-queue-second:applied')

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'finished current run'
    })

    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    expect(
      (testState.startConversationTurn.mock.calls[1]?.[0] as AgentConversationTurnInput).content
    ).toBe('send me next')
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
  })

  it('keeps an auto-send queue item until its required Provider transition succeeds', async () => {
    const queued: ChatQueuedMessage = {
      ...queuedMessage('queue-preserved', 'send after compaction', 10),
      attachments: [
        {
          id: 'attachment-queued',
          kind: 'file',
          name: 'queued.txt',
          mimeType: 'text/plain',
          sizeBytes: 6,
          encoding: 'managed',
          data: 'managed-test-cXVldWVk'
        }
      ],
      permissionMode: 'custom',
      skills: [skillSelection]
    }
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({
        modelId: 'model-1',
        projectId: 'project-a',
        queuedMessages: [queued]
      })
    })
    mockSuccessfulTurnStarts()
    const running = runningProviderTransition('model-1')
    testState.preflightProviderTransition
      .mockResolvedValueOnce({
        conversationId: 'conversation-a',
        targetModelId: 'model-1',
        decision: 'compatible',
        reason: 'same_protocol',
        operationId: 'provider-transition-2',
        transitionToken: 'token-initial-run'
      })
      .mockResolvedValueOnce({
        conversationId: 'conversation-a',
        targetModelId: 'model-1',
        decision: 'requires_compaction',
        reason: 'provider_protocol_changed',
        operationId: running.operationId,
        transitionToken: 'token-queued-run'
      })
    testState.startProviderTransition
      .mockResolvedValueOnce(completedProviderTransition('model-1'))
      .mockResolvedValueOnce(running)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'finished current run'
    })

    await expect
      .element(screen.getByTestId('model-transition-confirmation'))
      .toHaveTextContent('provider_protocol_changed')
    await expect
      .element(screen.getByTestId('queued-message-ids'))
      .toHaveTextContent('queue-preserved')
    expect(
      JSON.parse(screen.getByTestId('queued-message-payloads').element().textContent ?? '[]')
    ).toEqual([queued])
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)

    await screen.getByRole('button', { name: 'cancel-model-transition' }).click()
    await expect
      .element(screen.getByTestId('queued-message-ids'))
      .toHaveTextContent('queue-preserved')
    expect(
      JSON.parse(screen.getByTestId('queued-message-payloads').element().textContent ?? '[]')
    ).toEqual([queued])
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
  })

  it('accepts guidance when approval arrives before the queued message is submitted', async () => {
    const queued = queuedMessage('approval-race', 'keep this instruction', 10)
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({ modelId: 'model-1', queuedMessages: [queued] })
    })
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    emitAgentEvent({
      type: 'approval_required',
      runId: 'run-1',
      action: {
        type: 'command',
        command: {
          id: 'approval-command',
          command: 'sleep 5',
          cwd: null,
          timeoutMs: null,
          approvalStatus: 'required',
          riskLevel: null,
          reason: 'check approval',
          observe: null
        }
      }
    })
    await expect
      .element(screen.getByTestId('agent-run-status'))
      .toHaveTextContent('waiting_for_approval')
    await screen.getByRole('button', { name: 'guide-first-message' }).click()
    await expect.poll(() => testState.steerAgentRun.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
    await expect
      .element(screen.getByTestId('guidance-timeline'))
      .toHaveTextContent(`${queued.clientMessageId}:queued`)
    expect(testState.steerAgentRun.mock.calls[0][0].expectedRunId).toBe('run-1')
    await expect
      .element(screen.getByTestId('agent-run-status'))
      .toHaveTextContent('waiting_for_approval')
  })

  it('manually resends a confirmed rejected guidance identity once and ignores its late notifications', async () => {
    const queued = {
      ...queuedMessage('rejected-row', 'retry this instruction', 10),
      attachments: [
        {
          id: 'guidance-attachment',
          kind: 'file' as const,
          name: 'notes.txt',
          mimeType: 'text/plain',
          sizeBytes: 5,
          encoding: 'managed' as const,
          data: 'managed-test-aGVsbG8='
        }
      ],
      status: 'error' as const,
      error: 'The previous run stopped accepting guidance.'
    }
    const next = queuedMessage('next-row', 'preserve this position', 20)
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({ modelId: 'model-1', queuedMessages: [queued, next] })
    })
    mockSuccessfulTurnStarts()
    const original = deferred<AgentSteerRunOutput>()
    const replacement = deferred<AgentSteerRunOutput>()
    testState.steerAgentRun
      .mockReturnValueOnce(original.promise)
      .mockReturnValueOnce(replacement.promise)
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'guide-first-message' }).click()
    await expect.poll(() => testState.steerAgentRun.mock.calls.length).toBe(1)
    const rejection: AgentEvent = {
      type: 'guidance_rejected',
      runId: 'run-1',
      guidanceId: 'old-guidance',
      clientMessageId: queued.clientMessageId,
      content: queued.content,
      rejectionCode: 'run_not_steerable',
      message: queued.error,
      createdAt: queued.createdAt
    }
    // Notifications may restore the error row before the original RPC has acknowledged it.
    emitAgentEvent(rejection)
    await screen.getByRole('button', { name: 'guide-first-message' }).click()
    expect(testState.steerAgentRun).toHaveBeenCalledTimes(1)
    original.resolve({
      guidanceId: 'old-guidance',
      status: 'rejected',
      rejectionCode: 'run_not_steerable'
    })
    await expect.poll(() => testState.steerAgentRun.mock.calls.length).toBe(2)
    const replacementInput = testState.steerAgentRun.mock.calls[1][0]
    expect(replacementInput.clientMessageId).not.toBe(queued.clientMessageId)
    expect(replacementInput).toEqual({
      conversationId: 'conversation-a',
      expectedRunId: 'run-1',
      clientMessageId: expect.any(String),
      content: queued.content,
      attachments: queued.attachments,
      folderReferences: []
    })
    emitAgentEvent(rejection)
    await expect
      .element(screen.getByTestId('queued-message-ids'))
      .toHaveTextContent('rejected-row,next-row')
    replacement.resolve({ guidanceId: 'new-guidance', status: 'queued' })
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('next-row')
    emitAgentEvent(rejection)
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('next-row')
    await expect
      .element(screen.getByTestId('guidance-timeline'))
      .toHaveTextContent(`${replacementInput.clientMessageId}:queued`)
    expect(screen.getByTestId('guidance-timeline').element().textContent).not.toContain(
      queued.clientMessageId
    )
    expect(testState.steerAgentRun).toHaveBeenCalledTimes(2)
  })

  it.each(['queued', 'applied', 'identity_conflict', 'transport_error'] as const)(
    'preserves the original guidance identity after an error when Host reports %s',
    async (outcome) => {
      const queued = {
        ...queuedMessage('uncertain-row', 'do not duplicate this message', 10),
        status: 'error' as const,
        error: 'No acknowledgement received'
      }
      testState.loadComposerDrafts.mockResolvedValueOnce({
        'conversation-a': createComposerDraft({ modelId: 'model-1', queuedMessages: [queued] })
      })
      mockSuccessfulTurnStarts()
      if (outcome === 'transport_error') {
        testState.steerAgentRun.mockRejectedValueOnce(new Error('Transport unavailable'))
      } else {
        testState.steerAgentRun.mockResolvedValueOnce({
          guidanceId: 'original-guidance',
          status: outcome === 'identity_conflict' ? 'rejected' : outcome,
          ...(outcome === 'identity_conflict' ? { rejectionCode: 'identity_conflict' } : {})
        })
      }
      const screen = await renderSelectedConversation()
      await screen.getByRole('button', { name: 'submit-without-skill' }).click()
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
      await screen.getByRole('button', { name: 'guide-first-message' }).click()
      if (outcome === 'queued' || outcome === 'applied') {
        await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
        await expect
          .element(screen.getByTestId('guidance-timeline'))
          .toHaveTextContent(`${queued.clientMessageId}:${outcome}`)
      } else {
        await expect
          .poll(
            () =>
              JSON.parse(
                screen.getByTestId('queued-message-payloads').element().textContent ?? '[]'
              )[0]?.status
          )
          .toBe('error')
        expect(
          JSON.parse(screen.getByTestId('queued-message-payloads').element().textContent ?? '[]')[0]
            .clientMessageId
        ).toBe(queued.clientMessageId)
      }
      expect(testState.steerAgentRun).toHaveBeenCalledExactlyOnceWith({
        conversationId: 'conversation-a',
        expectedRunId: 'run-1',
        clientMessageId: queued.clientMessageId,
        content: queued.content,
        attachments: [],
        folderReferences: []
      })
    }
  )

  it.each(['guidance_queued', 'guidance_applied'] as const)(
    'keeps the authoritative %s receipt when the RPC acknowledgement is lost',
    async (type) => {
      const queued = queuedMessage('lost-reply', 'accepted once', 10)
      testState.loadComposerDrafts.mockResolvedValueOnce({
        'conversation-a': createComposerDraft({ modelId: 'model-1', queuedMessages: [queued] })
      })
      mockSuccessfulTurnStarts()
      const reply = deferred<AgentSteerRunOutput>()
      testState.steerAgentRun.mockReturnValueOnce(reply.promise)
      const screen = await renderSelectedConversation()
      await screen.getByRole('button', { name: 'submit-without-skill' }).click()
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
      await screen.getByRole('button', { name: 'guide-first-message' }).click()
      await expect.poll(() => testState.steerAgentRun.mock.calls.length).toBe(1)
      emitAgentEvent({
        type,
        runId: 'run-1',
        guidanceId: 'accepted-guidance',
        clientMessageId: queued.clientMessageId,
        content: queued.content,
        attachments: [],
        createdAt: queued.createdAt,
        ...(type === 'guidance_applied' ? { sequence: 1 } : {})
      } as AgentEvent)
      await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
      reply.reject(new Error('Acknowledgement lost'))
      await new Promise((resolve) => window.setTimeout(resolve, 0))
      await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
      await expect
        .element(screen.getByTestId('guidance-timeline'))
        .toHaveTextContent(
          `${queued.clientMessageId}:${type === 'guidance_queued' ? 'queued' : 'applied'}`
        )
      expect(testState.steerAgentRun).toHaveBeenCalledTimes(1)
    }
  )

  it('auto-sends the next row when the steer acknowledgement arrives after run completion', async () => {
    const first = queuedMessage('queue-first', 'late guidance', 10)
    const second = queuedMessage('queue-second', 'send after the race', 20)
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({
        modelId: 'model-1',
        projectId: 'project-a',
        queuedMessages: [first, second]
      })
    })
    mockSuccessfulTurnStarts()
    const steering = deferred<AgentSteerRunOutput>()
    testState.steerAgentRun.mockReturnValueOnce(steering.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await screen.getByRole('button', { name: 'guide-first-message' }).click()
    await expect.poll(() => testState.steerAgentRun.mock.calls.length).toBe(1)

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'finished before steering acknowledgement'
    })
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)

    steering.resolve({ guidanceId: 'guidance-late', status: 'queued' })
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    expect(
      (testState.startConversationTurn.mock.calls[1]?.[0] as AgentConversationTurnInput).content
    ).toBe('send after the race')
  })
})
