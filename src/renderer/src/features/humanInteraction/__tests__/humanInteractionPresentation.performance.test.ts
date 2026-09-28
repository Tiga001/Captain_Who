import { describe, expect, it } from 'vitest'
import type { HumanInteractionRequestSnapshot } from '@mycopilot/protocol'
import { ensureAgentRun } from '../../agentRun/agentEventReducer'
import type { ChatConversation, ChatMessage } from '../../chat/chatTypes'
import {
  createHumanInteractionConversationSelector,
  getUnanchoredHumanInteractionRequests,
  projectHumanInteractionConversation
} from '../humanInteractionPresentation'
import { humanInteractionResponseDisplay } from '../humanInteractionState'
import { projectHumanInteractionConversation as reference } from './humanInteractionPresentation.reference'
import { question, submitted } from './humanInteractionFixtures'

function chatWithCalls(count: number): ChatConversation {
  const run = ensureAgentRun(undefined, 'run-chat')
  for (let i = 0; i < count; i++) {
    run.toolCalls.push({
      id: `call-${i}`,
      tool: 'exec_command',
      args: {},
      approvalStatus: 'not_required',
      reason: null
    })
    run.timeline.push({ id: `timeline-${i}`, type: 'tool_call', callId: `call-${i}` })
  }
  return {
    id: 'chat',
    title: 'Projection',
    projectId: null,
    modelId: null,
    createdAt: 1,
    updatedAt: 1,
    messages: [
      { id: 'assistant-chat', role: 'assistant', content: '', createdAt: 1, agentRun: run }
    ]
  }
}

function mixedHistory(count: number) {
  const chat = chatWithCalls(0)
  chat.messages = []
  const requests: HumanInteractionRequestSnapshot[] = []
  for (let i = 0; i < count; i++) {
    const source = chatWithCalls(8).messages[0]
    const run = source.agentRun!
    source.id = `assistant-${i}`
    run.runId = `run-${i}`
    const pending = {
      ...question(`question-${i}`, i),
      assistantMessageId: source.id,
      runId: run.runId,
      mode: (i % 2 ? 'async' : 'sync') as 'sync' | 'async'
    }
    const request =
      i % 7 === 0
        ? pending
        : i % 7 === 1
          ? { ...pending, status: 'cancelled' as const, revision: 1 }
          : submitted(pending)
    if (request.response && i % 3 !== 0) {
      request.response.answers = [
        {
          kind: 'option',
          questionId: pending.questions[0].id,
          optionId: pending.questions[0].options![0].id
        },
        { kind: 'text', questionId: pending.questions[1].id, text: `Answer ${i}` }
      ]
    }
    requests.push(request)
    run.toolCalls.push({
      id: request.toolCallId,
      tool: request.mode === 'sync' ? 'request_user_input' : 'request_user_input_async',
      args: {},
      approvalStatus: 'not_required',
      reason: null
    })
    run.timeline.splice(
      3,
      0,
      { id: `narration-${i}`, type: 'message', content: 'Before the question' },
      { id: `boundary-${i}`, type: 'tool_call', callId: request.toolCallId, traceSequence: i },
      { id: `repeated-${i}`, type: 'tool_call', callId: request.toolCallId }
    )
    chat.messages.push(source)
    const display = humanInteractionResponseDisplay(request)
    if (!display) continue
    if (i % 4 === 0) {
      run.toolResults.push({
        callId: request.toolCallId,
        tool: 'request_user_input',
        ok: true,
        result: display
      })
    } else if (i % 4 === 1) {
      for (const status of ['rejected', 'queued', 'applied'] as const)
        run.timeline.push({
          id: `${status}-${i}`,
          type: 'user_guidance',
          clientMessageId: `human-answer-${i}`,
          guidanceId: `guidance-${status}-${i}`,
          content: JSON.stringify(display),
          status,
          attachments: [],
          createdAt: 5
        })
    } else if (i % 4 === 2) {
      const user: ChatMessage = {
        id: `user-answer-${i}`,
        role: 'user',
        content: JSON.stringify(display),
        createdAt: 5
      }
      request.delivery = { ...request.delivery!, userMessageId: user.id }
      chat.messages.push(user)
    }
  }
  return { chat, requests }
}

describe('human interaction projection caching and linear work', () => {
  it('matches the complete previous projection for mixed batches, persisted facts and receipt order', () => {
    const { chat, requests } = mixedHistory(80)
    const frozen = structuredClone(chat)
    const select = createHumanInteractionConversationSelector()
    for (const snapshot of [
      [],
      requests,
      [...requests].reverse(),
      requests.slice(0, 30),
      requests
    ]) {
      expect(select(chat, snapshot)).toEqual(reference(chat, snapshot))
      const reloaded = structuredClone(chat)
      expect(select(reloaded, snapshot)).toEqual(reference(reloaded, snapshot))
    }
    expect(chat).toEqual(frozen)
  })

  it('refreshes request and delivery revisions while the message and request objects stay the same', () => {
    const chat = chatWithCalls(0)
    const request = { ...question(), mode: 'sync' as const }
    const select = createHumanInteractionConversationSelector()
    const requests = [request]
    expect(select(chat, requests)).toEqual(reference(chat, requests))
    Object.assign(request, submitted(request))
    expect(select(chat, requests)).toEqual(reference(chat, requests))
    expect(select(chat, requests).messages[0].agentRun!.timeline[0].type).toBe('user_guidance')
    const next: ChatMessage = {
      ...chatWithCalls(0).messages[0],
      id: 'next-assistant',
      agentRun: { ...ensureAgentRun(undefined, 'next-run') }
    }
    chat.messages.push(next)
    request.delivery!.targetRunId = 'next-run'
    request.delivery!.revision++
    const retargeted = select(chat, requests)
    expect(retargeted).toEqual(reference(chat, requests))
    expect(retargeted.messages[0].agentRun!.timeline).toHaveLength(0)
    expect(retargeted.messages[1].agentRun!.timeline).toHaveLength(1)
  })

  it('keeps unchanged projected history references through live updates and only rebuilds changed runs', () => {
    const { chat, requests } = mixedHistory(100)
    let reads = 0
    for (const message of chat.messages)
      for (const call of message.agentRun?.toolCalls ?? []) {
        const id = call.id
        Object.defineProperty(call, 'id', {
          get() {
            reads++
            return id
          }
        })
      }
    const select = createHumanInteractionConversationSelector()
    const initial = select(chat, requests)
    expect(reads).toBeGreaterThan(0)
    reads = 0
    for (let i = 0; i < 100; i++) {
      const live = { ...chatWithCalls(1).messages[0], id: 'live', content: `token-${i}` }
      const view = select({ ...chat, messages: [...chat.messages, live] }, requests)
      for (let j = 0; j < chat.messages.length; j++)
        expect(view.messages[j]).toBe(initial.messages[j])
    }
    expect(reads).toBe(0)
    const replacement = structuredClone({ ...chat.messages[0], content: 'same ID, new content' })
    const changed = { ...chat, messages: [replacement, ...chat.messages.slice(1)] }
    expect(select(changed, requests)).toEqual(reference(changed, requests))
    expect(select(changed, requests).messages[0].content).toBe('same ID, new content')
  })

  it('does not share cached answers across conversations, even with colliding message/run IDs', () => {
    const chat = chatWithCalls(0)
    const answered = submitted(question())
    const select = createHumanInteractionConversationSelector()
    expect(select(chat, [answered]).messages[0].agentRun!.timeline).toHaveLength(1)
    const other = { ...chat, id: 'other' }
    expect(select(other, [answered])).toBe(other)
    expect(getUnanchoredHumanInteractionRequests(other, [question()])).toEqual([])
    expect(select(chat, [answered])).toEqual(reference(chat, [answered]))
  })

  it('invalidates cached temporary anchors when trace arrays arrive before or after request settlement', () => {
    const chat = chatWithCalls(0)
    const pending = { ...question(), mode: 'sync' as const }
    const select = createHumanInteractionConversationSelector()
    expect(select(chat, [pending])).toEqual(reference(chat, [pending]))
    const accepted = submitted(pending)
    expect(select(chat, [accepted])).toEqual(reference(chat, [accepted]))
    const run = chat.messages[0].agentRun!
    run.timeline.push({ id: 'durable-boundary', type: 'tool_call', callId: pending.toolCallId })
    expect(select(chat, [accepted])).toEqual(reference(chat, [accepted]))
    run.toolResults.push({
      callId: pending.toolCallId,
      tool: 'request_user_input',
      ok: true,
      result: humanInteractionResponseDisplay(accepted)!
    })
    expect(select(chat, [])).toEqual(reference(chat, []))
    // A same-ID replacement with equal array lengths must also invalidate the old run indexes.
    const replacement = structuredClone(chat)
    replacement.messages[0].agentRun!.timeline[0] = {
      id: 'new-narration',
      type: 'message',
      content: 'changed'
    }
    expect(select(replacement, [])).toEqual(reference(replacement, []))
  })

  it('indexes message anchors rather than searching all messages per admitted response', () => {
    const counts: number[] = []
    for (const size of [100, 500, 1000]) {
      const { chat, requests } = mixedHistory(size)
      let reads = 0
      for (const message of chat.messages) {
        const id = message.id
        Object.defineProperty(message, 'id', {
          get() {
            reads++
            return id
          }
        })
      }
      const select = createHumanInteractionConversationSelector()
      select(chat, requests)
      expect(reads).toBeLessThanOrEqual(size * 25)
      counts.push(reads)
    }
    expect(counts[2]).toBeLessThan(counts[0] * 11)
  })

  it('does not scan ordinary tool payloads, while persisted answers still work without live requests', () => {
    const chat = chatWithCalls(10)
    chat.messages[0].agentRun!.toolResults = [
      {
        callId: 'call-1',
        tool: 'exec_command',
        ok: true,
        get result() {
          throw new Error('ordinary result payload must not be inspected')
        }
      }
    ]
    expect(projectHumanInteractionConversation(chat, [])).toBe(chat)
    const persisted = mixedHistory(40).chat
    expect(projectHumanInteractionConversation(persisted, [])).toEqual(reference(persisted, []))
  })

  it.each([100, 500, 1000, 2000, 5000])(
    'indexes %i calls with a bounded number of ID reads',
    (count) => {
      const chat = chatWithCalls(count)
      let reads = 0
      for (const call of chat.messages[0].agentRun!.toolCalls) {
        const id = call.id
        Object.defineProperty(call, 'id', {
          get() {
            reads++
            return id
          }
        })
      }
      const select = createHumanInteractionConversationSelector()
      expect(select(chat, [])).toBe(chat)
      expect(reads).toBeLessThanOrEqual(count * 6)
      reads = 0
      expect(select({ ...chat }, [])).toMatchObject({ messages: chat.messages })
      expect(reads).toBe(0)
    }
  )
})

// Opt-in timing: deterministic work-count and equivalence checks above always run.
if (process.env.BENCH_HUMAN_INTERACTION_PROJECTION === '1') {
  it('reports same-process before/cold/hot medians for tool-heavy snapshots', () => {
    const median = (action: () => unknown) => {
      const samples: number[] = []
      for (let i = 0; i < 20; i++) {
        const start = performance.now()
        action()
        samples.push(performance.now() - start)
      }
      return samples.sort((a, b) => a - b)[10].toFixed(3)
    }
    for (const calls of [100, 500, 1000, 2000, 5000]) {
      const chat = chatWithCalls(calls)
      const select = createHumanInteractionConversationSelector()
      select(chat, [])
      console.info(
        JSON.stringify({
          calls,
          beforeMs: median(() => reference(chat, [])),
          coldMs: median(() => projectHumanInteractionConversation(chat, [])),
          hotMs: median(() => select({ ...chat }, []))
        })
      )
    }
  })
}
