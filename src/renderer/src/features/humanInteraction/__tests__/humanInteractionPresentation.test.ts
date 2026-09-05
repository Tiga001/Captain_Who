import { describe, expect, it } from 'vitest'
import type { ChatConversation, ChatGuidanceTimelineItem } from '../../chat/chatTypes'
import { applyAgentEventToChatMessage, ensureAgentRun } from '../../agentRun/agentEventReducer'
import {
  getUnanchoredHumanInteractionRequests,
  projectHumanInteractionConversation,
  readHumanInteractionDisplay
} from '../humanInteractionPresentation'
import { humanInteractionResponseDisplay } from '../humanInteractionState'
import { question, submitted } from './humanInteractionFixtures'

function conversation(): ChatConversation {
  return {
    id: 'chat',
    title: 'Questions',
    projectId: null,
    modelId: null,
    createdAt: 1,
    updatedAt: 1,
    messages: [
      {
        id: 'assistant-chat',
        role: 'assistant',
        content: '',
        createdAt: 1,
        agentRun: { ...ensureAgentRun(undefined, 'run-chat'), status: 'running' }
      }
    ]
  }
}
const request = submitted({ ...question(), mode: 'sync' })
const display = humanInteractionResponseDisplay(request)!
function guidance(
  id: string,
  status: ChatGuidanceTimelineItem['status'] = 'applied'
): ChatGuidanceTimelineItem {
  return {
    id,
    type: 'user_guidance',
    clientMessageId: `human-answer-${id}`,
    guidanceId: id,
    content: JSON.stringify(display),
    attachments: [],
    status,
    createdAt: 2
  }
}
function allResponses(chat: ChatConversation) {
  return chat.messages
    .flatMap((message) => [
      ...(message.role === 'user' ? [readHumanInteractionDisplay(message.content)] : []),
      ...(message.agentRun?.timeline ?? []).flatMap((item) =>
        item.type === 'user_guidance' ? [readHumanInteractionDisplay(item.content)] : []
      )
    ])
    .filter(Boolean)
}
describe('human interaction history projection', () => {
  it('keeps each open batch at its original tool call position and preserves durable chronology', () => {
    const chat = conversation(),
      run = chat.messages[0].agentRun!,
      pending = question()
    run.toolCalls.push({
      id: pending.toolCallId,
      tool: 'request_user_input_async',
      args: {},
      approvalStatus: 'not_required',
      reason: null
    })
    run.timeline.push(
      { id: 'introduction', type: 'message', content: '兴趣爱好：', traceSequence: 12 },
      { id: 'question-call', type: 'tool_call', callId: pending.toolCallId, traceSequence: 13 },
      { id: 'followup', type: 'message', content: '问题已发出。', traceSequence: 14 }
    )
    const before = structuredClone(chat)
    const projected = projectHumanInteractionConversation(chat, [pending])
    expect(projected.messages[0].agentRun!.timeline).toEqual(run.timeline)
    expect(getUnanchoredHumanInteractionRequests(chat, [pending])).toEqual([])
    expect(chat).toEqual(before)
    const reloaded = projectHumanInteractionConversation(structuredClone(chat), [pending])
    expect(reloaded.messages[0].agentRun!.timeline).toEqual(run.timeline)
    const finished = projectHumanInteractionConversation(chat, [{ ...pending, status: 'ignored' }])
    expect(finished.messages[0].agentRun!.timeline.map((item) => item.id)).toEqual([
      'introduction',
      'followup'
    ])
  })
  it('keeps early durable requests visible until trace arrives without duplicating call anchors', () => {
    const chat = conversation(),
      pending = question()
    const early = projectHumanInteractionConversation(chat, [pending])
    expect(early.messages[0].agentRun!.timeline).toEqual([
      { id: 'human-request:question', type: 'tool_call', callId: pending.toolCallId }
    ])
    expect(chat.messages[0].agentRun!.toolCalls).toEqual([])
    chat.messages[0].agentRun!.timeline.push({
      id: 'durable-call',
      type: 'tool_call',
      callId: pending.toolCallId,
      traceSequence: 3
    })
    const anchored = projectHumanInteractionConversation(chat, [pending])
    expect(anchored.messages[0].agentRun!.timeline).toEqual(chat.messages[0].agentRun!.timeline)
    expect(anchored.messages[0].agentRun!.toolCalls).toHaveLength(1)
    expect(getUnanchoredHumanInteractionRequests({ ...chat, messages: [] }, [pending])).toEqual([
      pending
    ])
    const answeredBeforeTrace = conversation()
    answeredBeforeTrace.messages[0].agentRun!.toolResults.push({
      callId: request.toolCallId,
      tool: 'request_user_input',
      ok: true,
      result: display
    })
    const answered = projectHumanInteractionConversation(answeredBeforeTrace, [
      { ...pending, mode: 'sync' }
    ])
    expect(answered.messages[0].agentRun!.timeline.map((item) => item.type)).toEqual([
      'user_guidance'
    ])
    expect(allResponses(answered)).toEqual([display])
  })
  it('projects a synchronous ToolResult once at its call boundary without altering authoritative history', () => {
    const chat = conversation(),
      run = chat.messages[0].agentRun!
    run.toolCalls.push({
      id: request.toolCallId,
      tool: 'request_user_input',
      args: {},
      approvalStatus: 'not_required',
      reason: null
    })
    run.toolResults.push({
      callId: request.toolCallId,
      tool: 'request_user_input',
      ok: true,
      result: display
    })
    run.timeline.push(
      { id: 'before', type: 'message', content: 'Before' },
      { id: 'question-tool', type: 'tool_call', callId: request.toolCallId },
      { id: 'after', type: 'message', content: 'After' }
    )
    const frozen = structuredClone(chat)
    const view = projectHumanInteractionConversation(chat, [request])
    expect(allResponses(view)).toEqual([display])
    expect(view.messages[0].agentRun?.timeline.map((item) => item.type)).toEqual([
      'message',
      'user_guidance',
      'message'
    ])
    expect(view.messages.filter((message) => message.role === 'user')).toHaveLength(0)
    expect(chat).toEqual(frozen)
  })
  it.each(['before', 'after'] as const)(
    'anchors an admitted sync answer at its call when its receipt arrives %s resumed streaming',
    (receiptOrder) => {
      const chat = conversation(),
        run = chat.messages[0].agentRun!
      run.status = 'waiting_for_user_input'
      run.toolCalls.push({
        id: request.toolCallId,
        tool: 'request_user_input',
        args: {},
        approvalStatus: 'not_required',
        reason: null
      })
      run.timeline.push(
        { id: 'introduction', type: 'message', content: '请告诉我兴趣爱好。', traceSequence: 10 },
        { id: 'question-call', type: 'tool_call', callId: request.toolCallId, traceSequence: 11 }
      )
      if (receiptOrder === 'before') {
        expect(
          projectHumanInteractionConversation(chat, [request]).messages[0].agentRun!.timeline.map(
            (item) => item.type
          )
        ).toEqual(['message', 'user_guidance'])
      }
      for (const delta of ['你跳过了这个问题，', '没关系。']) {
        chat.messages[0] = applyAgentEventToChatMessage(chat.messages[0], {
          type: 'message_delta',
          runId: 'run-chat',
          streamId: 'resumed-stream',
          delta
        })
        const view = projectHumanInteractionConversation(chat, [request])
        expect(view.messages[0].agentRun!.timeline.map((item) => item.type)).toEqual([
          'message',
          'user_guidance',
          'message'
        ])
        expect(view.messages[0].agentRun!.timeline[1]).toMatchObject({
          id: 'question-call',
          traceSequence: 11
        })
        expect(allResponses(view)).toEqual([display])
        // The Renderer projection cannot fabricate a live model result or persist a display User.
        expect(chat.messages[0].agentRun!.toolResults).toEqual([])
        expect(
          chat.messages[0].agentRun!.timeline.some((item) => item.type === 'user_guidance')
        ).toBe(false)
      }
      const projectedIds = projectHumanInteractionConversation(chat, [
        request
      ]).messages[0].agentRun!.timeline.map((item) => item.id)
      chat.messages[0].agentRun!.toolResults.push({
        callId: request.toolCallId,
        tool: 'request_user_input',
        ok: true,
        result: display
      })
      const durable = projectHumanInteractionConversation(structuredClone(chat), [request])
      expect(durable.messages[0].agentRun!.timeline.map((item) => item.id)).toEqual(projectedIds)
      expect(allResponses(durable)).toEqual([display])
    }
  )

  it('deduplicates retargeted guidance and late snapshots by response ID', () => {
    const chat = conversation(),
      run = chat.messages[0].agentRun!
    run.timeline.push(guidance('old', 'rejected'), guidance('accepted'))
    const view = projectHumanInteractionConversation(chat, [{ ...request, mode: 'async' }])
    expect(allResponses(view)).toEqual([display])
    expect(view.messages[0].agentRun?.timeline.map((item) => item.id)).toEqual(['accepted'])
  })
  it('lets the committed normal User row replace a rejected old route without creating another message', () => {
    const chat = conversation()
    chat.messages[0].agentRun!.timeline.push(guidance('old', 'rejected'))
    chat.messages.push({
      id: 'answer-user',
      role: 'user',
      content: JSON.stringify(display),
      createdAt: 3
    })
    const view = projectHumanInteractionConversation(chat, [
      {
        ...request,
        mode: 'async',
        delivery: { ...request.delivery!, userMessageId: 'answer-user' }
      }
    ])
    expect(allResponses(view)).toEqual([display])
    expect(view.messages).toHaveLength(2)
    expect(view.messages[0].agentRun?.timeline).toEqual([])
  })
  it('shows an accepted answer immediately before ToolResult/steering notification then merges it', () => {
    const chat = conversation()
    const pending = projectHumanInteractionConversation(chat, [request])
    expect(allResponses(pending)).toEqual([display])
    chat.messages[0].agentRun!.timeline.push(guidance('committed'))
    const settled = projectHumanInteractionConversation(chat, [request])
    expect(allResponses(settled)).toEqual([display])
    expect(settled.messages[0].agentRun?.timeline[0].id).toBe('committed')
  })
  it('can render forked frozen facts without live request rows; ignoring has no answer', () => {
    const chat = conversation()
    chat.messages[0].agentRun!.timeline.push({
      ...guidance('frozen'),
      guidanceId: 'fork-remapped-guidance'
    })
    expect(allResponses(projectHumanInteractionConversation(chat, []))).toEqual([display])
    expect(
      allResponses(
        projectHumanInteractionConversation(conversation(), [{ ...question(), status: 'ignored' }])
      )
    ).toEqual([])
    expect(readHumanInteractionDisplay('{"type":"human_interaction_response"}')).toBeNull()
  })
  it('does not hide or relabel ordinary User JSON or ordinary guidance with a matching shape', () => {
    const chat = conversation()
    chat.messages.push(
      { id: 'plain-one', role: 'user', content: JSON.stringify(display), createdAt: 2 },
      { id: 'plain-two', role: 'user', content: JSON.stringify(display), createdAt: 3 }
    )
    chat.messages[0].agentRun!.timeline.push({
      ...guidance('ordinary'),
      clientMessageId: 'ordinary'
    })
    const view = projectHumanInteractionConversation(chat, [])
    expect(view.messages).toHaveLength(3)
    expect(view.messages[1]).toBe(chat.messages[1])
    expect(view.messages[2]).toBe(chat.messages[2])
    expect(view.messages[1].humanInteractionDisplay).toBeUndefined()
  })
  it('keeps forked ordinary JSON ordinary while verified idle answer history survives without live requests', () => {
    const chat = conversation()
    const ordinary = {
      id: 'fork-plain',
      role: 'user' as const,
      content: JSON.stringify(display),
      createdAt: 2,
      inputOrigin: {
        kind: 'historical_snapshot' as const,
        senderAgentId: null,
        sourceAgentMessageId: null,
        snapshotSourceConversationId: 'deleted-source',
        snapshotSourceMessageId: 'original-plain'
      }
    }
    chat.messages.push(ordinary, {
      ...ordinary,
      id: 'fork-answer',
      humanInteractionDisplay: display
    })
    const view = projectHumanInteractionConversation(chat, [])
    expect(view.messages[1]).toBe(ordinary)
    expect(view.messages[1].humanInteractionDisplay).toBeUndefined()
    expect(view.messages[2].humanInteractionDisplay).toEqual(display)
    expect(view.messages).toHaveLength(3)
  })
  it('requires the complete answer to match the persisted binding, not only its response identifiers', () => {
    const chat = conversation()
    chat.messages.push({
      id: 'answer-user',
      role: 'user',
      content: JSON.stringify({
        ...display,
        answers: [{ ...display.answers[0], question: 'Forged title' }, display.answers[1]]
      }),
      createdAt: 3
    })
    const view = projectHumanInteractionConversation(chat, [
      { ...request, delivery: { ...request.delivery!, userMessageId: 'answer-user' } }
    ])
    expect(view.messages[1].humanInteractionDisplay).toBeUndefined()
    expect(view.messages[1]).toBe(chat.messages[1])
  })
})
