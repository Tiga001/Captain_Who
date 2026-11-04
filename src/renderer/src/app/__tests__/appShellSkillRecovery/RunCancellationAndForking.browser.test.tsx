import { HostInvocationError } from '@mycopilot/host-api'
import type { AgentConversationTurnInput, AgentConversationTurnOutput } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import type { ChatConversation } from '../../../features/chat/chatTypes'
import { testState } from './state'
import { successfulTurnOutput, mockSuccessfulTurnStarts } from './turnFixtures'
import { emitAgentEvent, deferred } from './eventFixtures'
import {
  storedConversation,
  storedConversationWithRun,
  storedConversationWithCommandRun,
  commandSessionSnapshot,
  commandSessionGetOutput
} from './conversationFixtures'
import { createComposerDraft, renderSelectedConversation } from './setup'

describe('authoritative run cancellation and conversation forking', () => {
  it('retains a suspended Run binding until the same Run resumes and completes', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    const readsBeforeWaiting = testState.loadConversation.mock.calls.length
    await expect.element(screen.getByRole('button', { name: 'command-capabilities' })).toBeEnabled()

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'waiting_for_user_input',
      usage: { totalTokens: 12, billableRequestCount: 1 }
    })
    await expect
      .element(screen.getByTestId('agent-run-status'))
      .toHaveTextContent('waiting_for_user_input')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('pending')
    await expect.element(screen.getByRole('button', { name: 'command-compact' })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: 'command-fork' })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: 'command-capabilities' })).toBeEnabled()
    expect(testState.loadConversation.mock.calls.length).toBe(readsBeforeWaiting)

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await screen.getByRole('button', { name: 'continue-in-new-task' }).click()
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    expect(testState.forkConversation).not.toHaveBeenCalled()

    emitAgentEvent({ type: 'started', runId: 'run-1', toolDefinitions: [] })
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'Continued with the submitted answer.',
      usage: { totalTokens: 25, billableRequestCount: 2 }
    })
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
  })

  it('rehydrates a waiting Run binding and lets Stop settle it authoritatively', async () => {
    const stored = storedConversation()
    const assistant = stored.messages.at(-1)
    if (!assistant?.agentRun) throw new Error('missing human-input Run fixture')
    assistant.status = 'pending'
    assistant.agentRun.status = 'waiting_for_user_input'
    testState.persistedConversations.set(stored.id, stored)
    const screen = await renderSelectedConversation()

    await expect.element(screen.getByRole('button', { name: 'command-compact' })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: 'command-fork' })).toBeDisabled()
    await screen.getByRole('button', { name: 'stop-generating' }).click()
    expect(testState.cancelAgentRun).toHaveBeenCalledWith('run-old')
    await expect
      .element(screen.getByTestId('agent-run-status'))
      .toHaveTextContent('waiting_for_user_input')
    emitAgentEvent({
      type: 'done',
      runId: 'run-old',
      success: false,
      status: 'cancelled',
      content: ''
    })
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('cancelled')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
  })

  it('keeps a stopped run command event routed to its original Timeline item', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    const turnInput = testState.startConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnInput
    const assistantMessageId = turnInput.assistantMessageId!
    const sessionId = 'cmd_1234567890abcdef1234567890abcdef'

    emitAgentEvent({
      type: 'tool_call',
      runId: 'run-1',
      traceSequence: 0,
      identity: { type: 'builtin', toolName: 'run_command' },
      call: {
        id: 'command-call',
        tool: 'run_command',
        args: { command: 'python3 snake_game/main.py' },
        approvalStatus: 'approved',
        reason: null
      }
    })
    emitAgentEvent({
      type: 'command_started',
      runId: 'run-1',
      conversationId: 'conversation-a',
      assistantMessageId,
      projectId: 'project-a',
      callId: 'command-call',
      sessionId,
      startedAt: 10
    })
    emitAgentEvent({
      type: 'tool_result',
      runId: 'run-1',
      result: {
        callId: 'command-call',
        tool: 'run_command',
        ok: true,
        result: { status: 'running', sessionId, output: '', startedAt: 10 }
      }
    })
    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"running"')

    await screen.getByRole('button', { name: 'stop-generating' }).click()
    expect(testState.cancelAgentRun).toHaveBeenCalledWith('run-1')

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: false,
      status: 'cancelled',
      content: ''
    })
    emitAgentEvent({
      type: 'command_interrupted',
      runId: 'run-1',
      conversationId: 'conversation-a',
      assistantMessageId,
      projectId: 'project-a',
      callId: 'command-call',
      sessionId,
      endedAt: 20,
      latestSequence: 0,
      outputTruncated: false
    })

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('cancelled')
    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"interrupted"')
  })

  it('keeps a stopped run pending until the backend terminal event is received', async () => {
    mockSuccessfulTurnStarts()
    const cancellation = deferred<boolean>()
    testState.cancelAgentRun.mockReturnValueOnce(cancellation.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('pending')
    await screen.getByRole('button', { name: 'stop-generating' }).click()

    expect(testState.cancelAgentRun).toHaveBeenCalledWith('run-1')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('pending')

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: false,
      status: 'cancelled',
      content: ''
    })
    cancellation.resolve(true)

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('cancelled')
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
  })

  it('binds a run before cancelling when stop is requested during the start RPC', async () => {
    const start = deferred<AgentConversationTurnOutput>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'stop-generating' }).click()
    expect(testState.cancelAgentRun).not.toHaveBeenCalled()

    const input = testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput
    start.resolve(successfulTurnOutput(input, 1))
    await expect.poll(() => testState.cancelAgentRun.mock.calls.length).toBe(1)
    expect(testState.cancelAgentRun).toHaveBeenCalledWith('run-1')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('pending')

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: false,
      status: 'cancelled',
      content: ''
    })
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
  })

  it('reconciles authoritative storage even when cancel reports that no active registration exists', async () => {
    mockSuccessfulTurnStarts()
    testState.cancelAgentRun.mockResolvedValueOnce(false)
    const screen = await renderSelectedConversation()
    const initialLoadCount = testState.loadConversation.mock.calls.length

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    const input = testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput
    testState.loadConversation.mockResolvedValueOnce(
      storedConversationWithRun(input.assistantMessageId!, 'run-1', 'cancelled')
    )

    vi.useFakeTimers()
    await screen.getByRole('button', { name: 'stop-generating' }).click()
    await vi.advanceTimersByTimeAsync(400)
    vi.useRealTimers()

    await expect.poll(() => testState.loadConversation.mock.calls.length).toBe(initialLoadCount + 1)
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    expect(testState.showToast).not.toHaveBeenCalled()
  })

  it('ends the spinner with an explicit unknown status when cancellation hangs and storage checks fail', async () => {
    mockSuccessfulTurnStarts()
    testState.cancelAgentRun.mockReturnValueOnce(deferred<boolean>().promise)
    const screen = await renderSelectedConversation()
    const initialLoadCount = testState.loadConversation.mock.calls.length
    testState.loadConversation.mockReturnValue(deferred<ChatConversation | null>().promise)

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)

    vi.useFakeTimers()
    await screen.getByRole('button', { name: 'stop-generating' }).click()
    await vi.advanceTimersByTimeAsync(20_000)
    vi.useRealTimers()

    await expect.poll(() => testState.loadConversation.mock.calls.length).toBe(initialLoadCount + 3)
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('error')
    expect(testState.showToast).toHaveBeenCalledWith('chat.stopStatusUnknown')
  })

  it('does not let a stale storage response overwrite a terminal event received during the read', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()
    const initialLoadCount = testState.loadConversation.mock.calls.length
    const storageRead = deferred<ChatConversation | null>()
    testState.loadConversation.mockReturnValueOnce(storageRead.promise)

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    const input = testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput

    vi.useFakeTimers()
    await screen.getByRole('button', { name: 'stop-generating' }).click()
    await vi.advanceTimersByTimeAsync(400)
    expect(testState.loadConversation).toHaveBeenCalledTimes(initialLoadCount + 1)

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: false,
      status: 'cancelled',
      content: ''
    })
    storageRead.resolve(storedConversationWithRun(input.assistantMessageId!, 'run-1', 'running'))
    await vi.advanceTimersByTimeAsync(10_000)
    vi.useRealTimers()

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    expect(testState.loadConversation).toHaveBeenCalledTimes(initialLoadCount + 2)
  })

  it('routes workspace commands to explicit scoped panel opens without sending a model message', async () => {
    testState.gitStatus = 'available'
    testState.loadComposerDrafts.mockResolvedValue({
      'conversation-a': createComposerDraft({
        message: 'keep my draft',
        modelId: 'model-1',
        projectId: 'project-a'
      })
    })
    const screen = await renderSelectedConversation()
    const request = (target: string) =>
      JSON.parse(screen.getByTestId(target).element().textContent!)
    const commandButtons = Array.from(screen.container.querySelectorAll('button')).filter(
      (button) => button.textContent?.startsWith('command-')
    )
    expect(commandButtons[0]?.textContent).toBe('command-model')
    await screen.getByRole('button', { name: 'command-model', exact: true }).click()
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    await screen.getByRole('button', { name: 'command-terminal', exact: true }).click()
    expect(testState.openBottomPanel).toHaveBeenCalledTimes(1)
    expect(request('bottom-module-request')).toEqual({
      moduleId: 'terminal',
      requestId: 1,
      workspaceKey: 'project-a',
      workspacePath: '/workspace/a',
      conversationId: 'conversation-a'
    })
    await screen.getByRole('button', { name: 'command-terminal', exact: true }).click()
    expect(request('bottom-module-request').requestId).toBe(2)
    expect(testState.openBottomPanel).toHaveBeenCalledTimes(2)
    await screen.getByRole('button', { name: 'command-browser', exact: true }).click()
    expect(request('right-module-request')).toEqual({
      moduleId: 'browser',
      requestId: 1,
      workspaceKey: 'project-a',
      workspacePath: '/workspace/a',
      conversationId: 'conversation-a'
    })
    await screen.getByRole('button', { name: 'command-review', exact: true }).click()
    expect(request('right-module-request')).toEqual({
      moduleId: 'git-review',
      requestId: 2,
      workspaceKey: 'project-a',
      workspacePath: '/workspace/a',
      conversationId: 'conversation-a'
    })
    expect(testState.openRightSidebar).toHaveBeenCalledTimes(2)
    await expect.element(screen.getByTestId('draft-message')).toHaveTextContent('keep my draft')
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
  })

  it('hides unavailable review and child-agent commands, and disables terminal when the bottom panel cannot fit', async () => {
    testState.canOpenBottomPanel = false
    const screen = await renderSelectedConversation()
    await expect
      .element(screen.getByRole('button', { name: 'command-review', exact: true }))
      .not.toBeInTheDocument()
    await expect
      .element(screen.getByRole('button', { name: 'command-agents', exact: true }))
      .not.toBeInTheDocument()
    await expect
      .element(screen.getByRole('button', { name: 'command-terminal', exact: true }))
      .toBeDisabled()
    await expect
      .element(screen.getByRole('button', { name: 'command-browser', exact: true }))
      .toBeEnabled()
    expect(testState.openBottomPanel).not.toHaveBeenCalled()
  })

  it('keeps repository checking commands disabled without requiring the agent to be idle', async () => {
    testState.gitStatus = 'checking'
    const screen = await renderSelectedConversation()
    await expect
      .element(screen.getByRole('button', { name: 'command-review', exact: true }))
      .toBeDisabled()
    await expect
      .element(screen.getByRole('button', { name: 'command-review', exact: true }))
      .toHaveAttribute('title', 'chat.commands.moduleChecking')
    mockSuccessfulTurnStarts()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByRole('button', { name: 'command-browser', exact: true }))
      .toBeEnabled()
    await expect
      .element(screen.getByRole('button', { name: 'command-terminal', exact: true }))
      .toBeEnabled()
  })

  it('routes the fork command to latest and deduplicates a concurrent timeline click before navigating', async () => {
    const request = deferred<ChatConversation>()
    testState.forkConversation.mockReturnValueOnce(request.promise)
    const screen = await renderSelectedConversation()
    const command = screen.getByRole('button', { name: 'command-fork' })
    await expect.element(command).toBeEnabled()
    await command.click()
    await expect.element(command).toBeDisabled()
    await expect.element(command).toHaveAttribute('title', 'chat.continueInNewTaskPending')
    await expect
      .element(screen.getByText('chat.continueInNewTaskPending', { exact: true }))
      .not.toBeInTheDocument()
    await screen.getByRole('button', { name: 'continue-in-new-task' }).click()
    await expect.poll(() => testState.forkConversation.mock.calls.length).toBe(1)
    expect(testState.forkConversation).toHaveBeenCalledWith({
      requestId: expect.any(String),
      sourceConversationId: 'conversation-a',
      forkPoint: { kind: 'latest' }
    })
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    request.resolve({ ...storedConversation(), id: 'fork-created', messagesLoaded: true })
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('fork-created')
    await expect.element(screen.getByTestId('draft-message')).toHaveTextContent('')
    await expect
      .element(screen.getByText('chat.continueInNewTaskPending', { exact: true }))
      .not.toBeInTheDocument()
    expect(testState.saveComposerDraft).toHaveBeenCalledWith(
      'fork-created',
      expect.objectContaining({
        message: '',
        attachments: [],
        skills: [],
        modelId: 'model-1',
        projectId: 'project-a'
      })
    )
  })

  it('disables the fork command during manual compaction response drain', async () => {
    testState.getManualContextCompactionStatus.mockResolvedValue({
      operations: [
        {
          schemaVersion: 1,
          operationId: 'manual',
          requestId: 'request',
          conversationId: 'conversation-a',
          status: 'cancelled',
          isBusy: true,
          phase: 'generating',
          startedAt: 10,
          updatedAt: 20,
          completedAt: 20
        }
      ]
    })
    const screen = await renderSelectedConversation()
    await expect.element(screen.getByRole('button', { name: 'command-fork' })).toBeDisabled()
    await expect
      .element(screen.getByRole('button', { name: 'command-fork' }))
      .toHaveAttribute('title', 'chat.commands.compacting')
    expect(testState.forkConversation).not.toHaveBeenCalled()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('false')
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    expect(testState.showToast).toHaveBeenCalledWith('chat.commands.compacting')
  })

  it('disables latest fork when the conversation ends with an unanswered user message', async () => {
    testState.loadConversation.mockResolvedValue({
      ...storedConversation(),
      messages: [storedConversation().messages[0]]
    })
    const screen = await renderSelectedConversation()
    await expect.element(screen.getByRole('button', { name: 'command-fork' })).toBeDisabled()
    await expect
      .element(screen.getByRole('button', { name: 'command-fork' }))
      .toHaveAttribute('title', 'chat.commands.noForkPoint')
    expect(testState.forkConversation).not.toHaveBeenCalled()
  })

  it('disables fork when a completed turn still owns a running background command', async () => {
    const stored = storedConversationWithCommandRun()
    const session = commandSessionSnapshot()
    stored.messages.at(-1)!.agentRun!.commandSessions = {
      'command-call': session
    }
    testState.loadConversation.mockResolvedValue(stored)
    testState.listAgentCommandSessions.mockResolvedValue({ sessions: [session] })
    testState.getAgentCommandSession.mockResolvedValue(commandSessionGetOutput(session))
    const screen = await renderSelectedConversation()
    await expect.element(screen.getByRole('button', { name: 'command-fork' })).toBeDisabled()
    await expect
      .element(screen.getByRole('button', { name: 'command-fork' }))
      .toHaveAttribute('title', 'chat.continueInNewTaskActiveCommand')
    await screen.getByRole('button', { name: 'continue-in-new-task' }).click()
    expect(testState.forkConversation).not.toHaveBeenCalled()
  })

  it('allows latest fork from a settled failed reply', async () => {
    testState.loadConversation.mockResolvedValue(
      storedConversationWithRun('failed-assistant', 'failed-run', 'failed')
    )
    testState.forkConversation.mockResolvedValue({
      ...storedConversation(),
      id: 'recovery-fork',
      messagesLoaded: true
    })
    const screen = await renderSelectedConversation()
    await expect.element(screen.getByRole('button', { name: 'command-fork' })).toBeEnabled()
    await screen.getByRole('button', { name: 'command-fork' }).click()
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('recovery-fork')
    expect(testState.forkConversation.mock.calls[0]?.[0].forkPoint).toEqual({ kind: 'latest' })
  })

  it('opens an existing continuation source at the original reply', async () => {
    const forked = {
      ...storedConversation(),
      continuationOrigin: {
        sourceConversationId: 'conversation-source',
        sourceMessageId: 'assistant-old',
        boundaryMessageId: 'assistant-old'
      }
    }
    const source = {
      ...storedConversation(),
      id: 'conversation-source',
      title: 'Source'
    }
    testState.loadConversationMetas.mockResolvedValueOnce([
      { ...forked, messages: [], messagesLoaded: false }
    ])
    testState.loadConversation
      .mockReset()
      .mockResolvedValueOnce(forked)
      .mockResolvedValueOnce(source)

    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'open-continuation-origin' }).click()

    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-source')
    expect(testState.loadConversation.mock.calls).toEqual([
      ['conversation-a'],
      ['conversation-source']
    ])
    expect(testState.showToast).not.toHaveBeenCalled()
  })

  it('keeps the current task when the continuation source is archived', async () => {
    const forked = {
      ...storedConversation(),
      continuationOrigin: {
        sourceConversationId: 'conversation-source',
        sourceMessageId: 'assistant-old',
        boundaryMessageId: 'assistant-old'
      }
    }
    testState.loadConversationMetas.mockResolvedValueOnce([
      { ...forked, messages: [], messagesLoaded: false }
    ])
    testState.loadConversation
      .mockReset()
      .mockResolvedValueOnce(forked)
      .mockResolvedValueOnce({
        ...storedConversation(),
        id: 'conversation-source',
        archivedAt: 10
      })

    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'open-continuation-origin' }).click()

    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    expect(testState.showToast).toHaveBeenCalledWith('chat.continuationOriginArchived')
  })

  it('keeps the current task when the continuation source was deleted', async () => {
    const forked = {
      ...storedConversation(),
      continuationOrigin: {
        sourceConversationId: 'conversation-source',
        sourceMessageId: 'assistant-old',
        boundaryMessageId: 'assistant-old'
      }
    }
    testState.loadConversationMetas.mockResolvedValueOnce([
      { ...forked, messages: [], messagesLoaded: false }
    ])
    testState.loadConversation.mockReset().mockResolvedValueOnce(forked).mockResolvedValueOnce(null)

    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'open-continuation-origin' }).click()

    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    expect(testState.showToast).toHaveBeenCalledWith('chat.continuationOriginMissing')
  })

  it('shows the localized active-command explanation for a typed fork rejection', async () => {
    testState.forkConversation.mockRejectedValueOnce(
      new HostInvocationError({
        message: 'Conversation fork was rejected.',
        code: -32000,
        data: {
          type: 'conversation_fork',
          code: 'active_command_session',
          conversationId: 'conversation-a',
          activeSessionCount: 1
        }
      })
    )
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'continue-in-new-task' }).click()

    await expect.poll(() => testState.forkConversation.mock.calls.length).toBe(1)
    expect(testState.showToast).toHaveBeenCalledWith('chat.continueInNewTaskActiveCommand')
  })

  it('keeps the fork request identity when retrying an uncertain response and clears pending state', async () => {
    const retry = deferred<ChatConversation>()
    testState.forkConversation
      .mockRejectedValueOnce(new Error('response connection closed'))
      .mockReturnValueOnce(retry.promise)
    const screen = await renderSelectedConversation()
    const command = screen.getByRole('button', { name: 'command-fork' })
    await command.click()
    await expect.poll(() => testState.forkConversation.mock.calls.length).toBe(1)
    await expect.element(command).toBeEnabled()
    await expect
      .element(screen.getByText('chat.continueInNewTaskPending', { exact: true }))
      .not.toBeInTheDocument()
    const initialRequest = testState.forkConversation.mock.calls[0][0]
    await command.click()
    await expect.poll(() => testState.forkConversation.mock.calls.length).toBe(2)
    expect(testState.forkConversation.mock.calls[1][0]).toEqual(initialRequest)
    retry.resolve({ ...storedConversation(), id: 'fork-retried', messagesLoaded: true })
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('fork-retried')
  })

  it('uses a new fork identity when latest refers to a newer source history', async () => {
    mockSuccessfulTurnStarts()
    testState.forkConversation
      .mockRejectedValueOnce(new Error('response connection closed'))
      .mockResolvedValueOnce({ ...storedConversation(), id: 'fork-new-head', messagesLoaded: true })
    const screen = await renderSelectedConversation()
    const command = screen.getByRole('button', { name: 'command-fork' })
    await command.click()
    await expect.poll(() => testState.forkConversation.mock.calls.length).toBe(1)
    await expect.element(command).toBeEnabled()
    const initialRequestId = testState.forkConversation.mock.calls[0][0].requestId
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'New reply.'
    })
    await expect.element(command).toBeEnabled()
    await command.click()
    await expect.poll(() => testState.forkConversation.mock.calls.length).toBe(2)
    expect(testState.forkConversation.mock.calls[1][0].requestId).not.toBe(initialRequestId)
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('fork-new-head')
  })

  it('shows a localized busy explanation when fork admission is full', async () => {
    testState.forkConversation.mockRejectedValueOnce(
      new HostInvocationError({ message: 'fork queue is full', code: -32001 })
    )
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'command-fork' }).click()
    await expect
      .poll(() => testState.showToast.mock.calls.at(-1)?.[0])
      .toBe('chat.continueInNewTaskBusy')
    await expect.element(screen.getByRole('button', { name: 'command-fork' })).toBeEnabled()
  })

  it('does not expose an unknown Core, IPC, or database failure in the fork toast', async () => {
    testState.forkConversation.mockRejectedValueOnce(
      new Error(
        "Error invoking remote method 'host:storage.forkConversation': CoreJsonRpcError: UNIQUE constraint failed"
      )
    )
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'continue-in-new-task' }).click()

    await expect.poll(() => testState.forkConversation.mock.calls.length).toBe(1)
    expect(testState.showToast).toHaveBeenCalledWith('chat.continueInNewTaskFailed')
  })
})
