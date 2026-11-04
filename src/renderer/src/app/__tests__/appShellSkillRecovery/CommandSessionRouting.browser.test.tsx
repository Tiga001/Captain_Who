import type { AgentConversationTurnInput, PendingAgentActionSnapshot } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatMessage } from '../../../features/chat/chatTypes'
import { testState } from './state'
import { mockSuccessfulTurnStarts } from './turnFixtures'
import { emitAgentEvent, deferred } from './eventFixtures'
import {
  storedConversation,
  storedConversationWithCommandRun,
  commandSessionSnapshot,
  commandSessionGetOutput
} from './conversationFixtures'
import { AppShell, renderSelectedConversation } from './setup'

describe('managed command Session lifecycle routing', () => {
  it('keeps copied command history immutable when a root fork lists matching terminal Sessions', async () => {
    const stored = storedConversationWithCommandRun()
    stored.continuationOrigin = {
      sourceConversationId: 'source-conversation',
      sourceMessageId: 'source-assistant',
      boundaryMessageId: 'assistant-command'
    }
    const snapshot = commandSessionSnapshot({ status: 'exited', endedAt: 30, exitCode: 0 })
    stored.messages.at(-1)!.agentRun!.commandSessions = { 'command-call': snapshot }
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValue({ sessions: [snapshot] })
    testState.getAgentCommandSession.mockResolvedValue(commandSessionGetOutput(snapshot))
    testState.saveChatMessageState.mockRejectedValue(
      new Error('Child context snapshot message is immutable')
    )
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)

    const screen = await renderSelectedConversation()

    await expect.poll(() => testState.listAgentCommandSessions.mock.calls.length).toBeGreaterThan(0)
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"exited"')
    expect(testState.getAgentCommandSession).not.toHaveBeenCalled()
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    expect(consoleError).not.toHaveBeenCalled()
    consoleError.mockRestore()
  })

  it('refreshes new fork-owned Sessions without reconciling missing copied-history receipts', async () => {
    const stored = storedConversationWithCommandRun()
    const copiedAssistant = stored.messages.at(-1)!
    copiedAssistant.agentRun!.toolResults = [
      {
        callId: 'command-call',
        tool: 'run_command',
        ok: true,
        result: {
          status: 'running',
          sessionId: 'cmd_1234567890abcdef1234567890abcdef',
          output: 'copied receipt'
        }
      }
    ]
    stored.continuationOrigin = {
      sourceConversationId: 'source-conversation',
      sourceMessageId: 'source-assistant',
      boundaryMessageId: copiedAssistant.id
    }
    const currentAssistant = structuredClone(copiedAssistant)
    currentAssistant.id = 'assistant-after-fork'
    currentAssistant.agentRun!.runId = 'run-after-fork'
    currentAssistant.agentRun!.toolResults = []
    stored.messages.push(currentAssistant)
    const snapshot = commandSessionSnapshot({
      sessionId: 'cmd_abcdef1234567890abcdef1234567890',
      assistantMessageId: currentAssistant.id,
      originRunId: 'run-after-fork',
      status: 'exited',
      endedAt: 30,
      exitCode: 0
    })
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValue({ sessions: [snapshot] })
    testState.getAgentCommandSession.mockResolvedValue(
      commandSessionGetOutput(snapshot, 'after the fork\n')
    )
    testState.saveChatMessageState.mockImplementation(async (_conversationId, message) => {
      if ((message as ChatMessage).id === copiedAssistant.id) {
        throw new Error('Child context snapshot message is immutable')
      }
    })
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)

    const screen = await renderSelectedConversation()

    await expect.element(screen.getByTestId('command-output')).toHaveTextContent('after the fork')
    await expect.poll(() => testState.saveChatMessageState.mock.calls.length).toBeGreaterThan(0)
    expect(
      testState.saveChatMessageState.mock.calls.every(
        ([, message]) => (message as ChatMessage).id === currentAssistant.id
      )
    ).toBe(true)
    expect(testState.getAgentCommandSession).toHaveBeenCalledTimes(1)
    expect(consoleError).not.toHaveBeenCalled()
    consoleError.mockRestore()
  })

  it('refreshes Host-owned Sessions whenever an already loaded conversation is reopened', async () => {
    const conversationA = storedConversation()
    const conversationB = {
      ...storedConversation(),
      id: 'conversation-b',
      title: 'Second conversation'
    }
    testState.loadConversationMetas.mockResolvedValueOnce([
      { ...conversationA, messages: [], messagesLoaded: false },
      { ...conversationB, messages: [], messagesLoaded: false }
    ])
    testState.loadConversation.mockImplementation(async (conversationId: string) =>
      conversationId === 'conversation-a' ? conversationA : conversationB
    )

    const screen = await render(<AppShell />)
    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await expect
      .poll(
        () =>
          testState.listAgentCommandSessions.mock.calls.filter(
            ([input]) => input.conversationId === 'conversation-a'
          ).length
      )
      .toBe(1)

    await screen.getByRole('button', { name: 'select-conversation-b', exact: true }).click()
    await expect
      .poll(
        () =>
          testState.listAgentCommandSessions.mock.calls.filter(
            ([input]) => input.conversationId === 'conversation-b'
          ).length
      )
      .toBe(1)

    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await expect
      .poll(
        () =>
          testState.listAgentCommandSessions.mock.calls.filter(
            ([input]) => input.conversationId === 'conversation-a'
          ).length
      )
      .toBe(2)
  })

  it('keeps routing background output and exit to the original completed assistant message', async () => {
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
    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'The game is running.'
    })
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')

    emitAgentEvent({
      type: 'command_output',
      runId: 'run-1',
      conversationId: 'conversation-a',
      assistantMessageId,
      projectId: 'project-a',
      callId: 'command-call',
      sessionId,
      sequence: 1,
      stream: 'stdout',
      output: 'window closed\n'
    })
    emitAgentEvent({
      type: 'command_exited',
      runId: 'run-1',
      conversationId: 'conversation-a',
      assistantMessageId,
      projectId: 'project-a',
      callId: 'command-call',
      sessionId,
      status: 'exited',
      exitCode: 0,
      endedAt: 20,
      latestSequence: 1,
      outputTruncated: false
    })

    await expect.element(screen.getByTestId('command-output')).toHaveTextContent('window closed')
    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"exited"')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
  })

  it('restores a terminal Session and bounded transcript into its existing command item', async () => {
    const stored = storedConversationWithCommandRun()
    const snapshot = commandSessionSnapshot({
      status: 'interrupted',
      endedAt: 30,
      latestSequence: 1
    })
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValueOnce({ sessions: [snapshot] })
    testState.getAgentCommandSession.mockResolvedValueOnce(
      commandSessionGetOutput(snapshot, 'recovered output\n')
    )

    const screen = await renderSelectedConversation()

    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"interrupted"')
    await expect.element(screen.getByTestId('command-output')).toHaveTextContent('recovered output')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    expect(testState.listAgentCommandSessions).toHaveBeenCalledWith({
      conversationId: 'conversation-a'
    })
    expect(testState.getAgentCommandSession).toHaveBeenCalledWith({
      conversationId: 'conversation-a',
      sessionId: snapshot.sessionId,
      afterSequence: 0,
      maxBytes: 256 * 1024
    })
    await expect.poll(() => testState.saveChatMessageState.mock.calls.length).toBeGreaterThan(0)
  })

  it('does not let a stale pending-action snapshot restore approval after Session authority', async () => {
    const stored = storedConversationWithCommandRun()
    const assistant = stored.messages.at(-1)
    if (!assistant?.agentRun) throw new Error('missing command run fixture')
    assistant.status = 'pending'
    assistant.agentRun.status = 'waiting_for_approval'
    assistant.agentRun.toolCalls = [
      {
        ...assistant.agentRun.toolCalls[0],
        approvalStatus: 'required',
        reason: null
      }
    ]
    assistant.agentRun.approvals = [
      {
        type: 'command',
        command: {
          id: 'command-call',
          command: 'python3 snake_game/main.py',
          cwd: null,
          timeoutMs: null,
          approvalStatus: 'required',
          riskLevel: null,
          reason: null,
          observe: null
        }
      }
    ]
    const session = commandSessionSnapshot()
    const pendingActions = deferred<PendingAgentActionSnapshot[]>()
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValueOnce({ sessions: [session] })
    testState.getAgentCommandSession.mockResolvedValueOnce(commandSessionGetOutput(session))
    testState.listPendingAgentActions.mockReturnValueOnce(pendingActions.promise)

    const screen = await renderSelectedConversation()

    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"running"')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await expect.element(screen.getByTestId('approval-count')).toHaveTextContent('0')

    pendingActions.resolve([
      {
        actionId: 'command-call',
        actionType: 'command',
        toolName: 'run_command',
        toolCallId: 'command-call',
        runId: 'run-command',
        conversationId: 'conversation-a',
        assistantMessageId: 'assistant-command',
        action: {
          type: 'command',
          command: {
            id: 'command-call',
            command: 'python3 snake_game/main.py',
            cwd: null,
            timeoutMs: null,
            approvalStatus: 'required',
            riskLevel: null,
            reason: null,
            observe: null
          }
        },
        createdAt: 5,
        status: 'pending'
      }
    ])

    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await expect.element(screen.getByTestId('approval-count')).toHaveTextContent('0')
  })

  it('retries a rejected transcript read without rolling back the applied Session status', async () => {
    const stored = storedConversationWithCommandRun()
    const session = commandSessionSnapshot()
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValue({ sessions: [session] })
    testState.getAgentCommandSession
      .mockRejectedValueOnce(new Error('temporary transcript failure'))
      .mockResolvedValue(commandSessionGetOutput(session, 'recovered after retry\n'))
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)

    const screen = await renderSelectedConversation()

    await expect.poll(() => testState.getAgentCommandSession.mock.calls.length).toBeGreaterThan(0)
    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"running"')
    await expect.poll(() => testState.listAgentCommandSessions.mock.calls.length).toBeGreaterThan(1)
    await expect.poll(() => testState.getAgentCommandSession.mock.calls.length).toBeGreaterThan(1)
    await expect
      .element(screen.getByTestId('command-output'))
      .toHaveTextContent('recovered after retry')
    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"running"')
    consoleError.mockRestore()
  })

  it('keeps the current running projection when Host Session refresh fails', async () => {
    const stored = storedConversationWithCommandRun()
    const assistant = stored.messages.at(-1)
    if (!assistant?.agentRun) throw new Error('missing command run fixture')
    const snapshot = commandSessionSnapshot()
    assistant.agentRun.toolResults = [
      {
        callId: 'command-call',
        tool: 'run_command',
        ok: true,
        result: {
          status: 'running',
          sessionId: snapshot.sessionId,
          output: 'application ready\n',
          startedAt: snapshot.startedAt,
          latestSequence: 1
        }
      }
    ]
    assistant.agentRun.commandSessions = {
      'command-call': {
        callId: 'command-call',
        sessionId: snapshot.sessionId,
        status: 'running',
        startedAt: snapshot.startedAt,
        latestSequence: 1,
        outputTruncated: false
      }
    }
    assistant.agentRun.commandOutputPreviews = {
      'command-call': {
        callId: 'command-call',
        chunks: [{ sequence: 1, stream: 'stdout', output: 'application ready\n' }]
      }
    }
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockRejectedValue(new Error('temporary Host failure'))
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)

    const screen = await renderSelectedConversation()

    await expect.poll(() => testState.listAgentCommandSessions.mock.calls.length).toBeGreaterThan(0)
    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"running"')
    await expect
      .element(screen.getByTestId('command-output'))
      .toHaveTextContent('application ready')
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    consoleError.mockRestore()
  })

  it('settles a pre-refresh active projection missing from a successful Host list', async () => {
    const stored = storedConversationWithCommandRun()
    const assistant = stored.messages.at(-1)
    if (!assistant?.agentRun) throw new Error('missing command run fixture')
    const snapshot = commandSessionSnapshot()
    assistant.agentRun.toolResults = [
      {
        callId: 'command-call',
        tool: 'run_command',
        ok: true,
        result: {
          status: 'running',
          sessionId: snapshot.sessionId,
          output: '',
          startedAt: snapshot.startedAt
        }
      }
    ]
    assistant.agentRun.commandSessions = {
      'command-call': {
        callId: 'command-call',
        sessionId: snapshot.sessionId,
        status: 'running',
        startedAt: snapshot.startedAt,
        latestSequence: 0,
        outputTruncated: false
      }
    }
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValueOnce({ sessions: [] })

    const screen = await renderSelectedConversation()

    await expect
      .element(screen.getByTestId('command-sessions'))
      .toHaveTextContent('"outcome_unknown"')
    await expect.poll(() => testState.saveChatMessageState.mock.calls.length).toBeGreaterThan(0)
    expect(testState.getAgentCommandSession).not.toHaveBeenCalled()
  })

  it('settles a pre-refresh running receipt missing from a successful Host list', async () => {
    const stored = storedConversationWithCommandRun()
    const assistant = stored.messages.at(-1)
    if (!assistant?.agentRun) throw new Error('missing command run fixture')
    const snapshot = commandSessionSnapshot()
    assistant.agentRun.toolResults = [
      {
        callId: 'command-call',
        tool: 'run_command',
        ok: true,
        result: {
          status: 'running',
          sessionId: snapshot.sessionId,
          output: '',
          startedAt: snapshot.startedAt
        }
      }
    ]
    delete assistant.agentRun.commandSessions
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValueOnce({ sessions: [] })

    const screen = await renderSelectedConversation()

    await expect
      .element(screen.getByTestId('command-sessions'))
      .toHaveTextContent('"outcome_unknown"')
    await expect.poll(() => testState.saveChatMessageState.mock.calls.length).toBeGreaterThan(0)
    expect(testState.getAgentCommandSession).not.toHaveBeenCalled()
  })

  it('keeps durable terminal metadata when the Host retention window no longer lists the Session', async () => {
    const stored = storedConversationWithCommandRun()
    const assistant = stored.messages.at(-1)
    if (!assistant?.agentRun) throw new Error('missing command run fixture')
    assistant.agentRun.toolResults = [
      {
        callId: 'command-call',
        tool: 'run_command',
        ok: true,
        result: {
          status: 'running',
          sessionId: 'cmd_1234567890abcdef1234567890abcdef',
          output: '',
          startedAt: 10
        }
      }
    ]
    assistant.agentRun.commandSessions = {
      'command-call': {
        callId: 'command-call',
        status: 'exited',
        startedAt: 10,
        endedAt: 30,
        exitCode: 0,
        latestSequence: 0,
        outputTruncated: false
      }
    }
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValueOnce({ sessions: [] })

    const screen = await renderSelectedConversation()

    await expect.poll(() => testState.listAgentCommandSessions.mock.calls.length).toBeGreaterThan(0)
    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"exited"')
    expect(testState.getAgentCommandSession).not.toHaveBeenCalled()
  })

  it('rejects foreign list entries and mismatched transcript responses during hydration', async () => {
    const stored = storedConversationWithCommandRun()
    const listed = commandSessionSnapshot()
    const foreign = commandSessionSnapshot({
      conversationId: 'conversation-foreign',
      assistantMessageId: 'assistant-foreign',
      originRunId: 'run-foreign'
    })
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValueOnce({ sessions: [foreign, listed] })
    testState.getAgentCommandSession.mockResolvedValueOnce(
      commandSessionGetOutput(
        commandSessionSnapshot({
          sessionId: listed.sessionId,
          callId: 'different-call',
          status: 'exited',
          endedAt: 40,
          exitCode: 0
        }),
        'must not be merged\n'
      )
    )

    const screen = await renderSelectedConversation()

    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"running"')
    await expect.element(screen.getByTestId('command-output')).toHaveTextContent('')
    expect(testState.getAgentCommandSession).toHaveBeenCalledTimes(1)
  })
})
