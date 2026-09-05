import { describe, expect, it } from 'vitest'
import type { ChatConversation, ChatGuidanceTimelineItem } from '../../chat/chatTypes'
import { ensureAgentRun } from '../../agentRun/agentEventReducer'
import {
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
})
