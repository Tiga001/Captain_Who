import type {
  AgentConversationTurnInput,
  AgentConversationTurnOutput,
  AgentConversationTurnRewriteInput
} from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import { testState } from './state'
import {
  skillSelection,
  explicitSkillSummary,
  modelSkillSummary,
  successfulTurnOutput,
  mockSuccessfulTurnStarts
} from './turnFixtures'
import { emitAgentEvent, deferred } from './eventFixtures'
import { storedConversation, storedConversationWithRun } from './conversationFixtures'
import { renderSelectedConversation } from './setup'

describe('unified activated Skill inventory', () => {
  it('restores the unified inventory from persisted message state', async () => {
    const restored = storedConversation()
    const assistantMessage = restored.messages.at(-1)
    if (!assistantMessage?.agentRun) throw new Error('missing assistant run fixture')
    assistantMessage.agentRun.activatedSkills = [explicitSkillSummary, modelSkillSummary]
    assistantMessage.agentRun.explicitSkillSelections = [skillSelection]
    assistantMessage.agentRun.skillActivationRevision = 'activation-sha256-v1:explicit-and-model'
    testState.loadConversation.mockResolvedValueOnce(restored)

    const screen = await renderSelectedConversation()

    await expect
      .element(screen.getByTestId('activated-skill-ids'))
      .toHaveTextContent(`${explicitSkillSummary.id},${modelSkillSummary.id}`)
  })

  it('merges Core model activation without writing it back or making it an explicit edit selection', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-with-skill' }).click()
    await expect
      .element(screen.getByTestId('activated-skill-ids'))
      .toHaveTextContent(explicitSkillSummary.id)

    emitAgentEvent({
      type: 'skill_activated',
      runId: 'run-1',
      activationRevision: 'activation-sha256-v1:explicit-and-model',
      activatedBy: 'model',
      skill: modelSkillSummary
    })

    await expect
      .element(screen.getByTestId('activated-skill-ids'))
      .toHaveTextContent(`${explicitSkillSummary.id},${modelSkillSummary.id}`)
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    await expect
      .element(screen.getByTestId('agent-run-projection'))
      .toHaveTextContent('activation-sha256-v1:explicit-and-model')

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'done'
    })
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)

    expect(
      (testState.rewriteConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnRewriteInput)
        .turn.skills
    ).toEqual([skillSelection])
  })

  it('keeps a model-only activation out of an edited turn explicit selections', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    emitAgentEvent({
      type: 'skill_activated',
      runId: 'run-1',
      activationRevision: 'activation-sha256-v1:model-only',
      activatedBy: 'model',
      skill: modelSkillSummary
    })
    await expect
      .element(screen.getByTestId('activated-skill-ids'))
      .toHaveTextContent(modelSkillSummary.id)

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'done'
    })
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)

    expect(
      (testState.rewriteConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnRewriteInput)
        .turn.skills
    ).toBeUndefined()
  })

  it('replays a model activation buffered before the turn start response', async () => {
    const start = deferred<AgentConversationTurnOutput>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-with-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    const input = testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput
    emitAgentEvent({
      type: 'skill_activated',
      runId: 'run-1',
      activationRevision: 'activation-sha256-v1:explicit-and-model',
      activatedBy: 'model',
      skill: modelSkillSummary
    })
    start.resolve(successfulTurnOutput(input, 1))

    await expect
      .element(screen.getByTestId('activated-skill-ids'))
      .toHaveTextContent(`${explicitSkillSummary.id},${modelSkillSummary.id}`)
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    await expect
      .element(screen.getByTestId('agent-run-projection'))
      .toHaveTextContent('activation-sha256-v1:explicit-and-model')
  })

  it('restores early workspace Skill resource activity from the authoritative terminal message', async () => {
    const start = deferred<AgentConversationTurnOutput>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    const input = testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput
    const assistantMessageId = input.assistantMessageId!
    const resourceUri = `skill://package/${encodeURIComponent(explicitSkillSummary.id)}/${encodeURIComponent(explicitSkillSummary.revision)}/references/workflows.md`

    emitAgentEvent({
      type: 'skill_activated',
      runId: 'run-1',
      activationRevision: 'activation-sha256-v1:workspace-resource',
      activatedBy: 'model',
      skill: explicitSkillSummary
    })
    emitAgentEvent({
      type: 'tool_call',
      runId: 'run-1',
      traceSequence: 1,
      identity: { type: 'builtin', toolName: 'skills_read_resource' },
      call: {
        id: 'read-workspace-resource',
        tool: 'skills_read_resource',
        args: { uri: resourceUri },
        approvalStatus: 'not_required',
        reason: null
      }
    })
    emitAgentEvent({
      type: 'tool_result',
      runId: 'run-1',
      result: {
        callId: 'read-workspace-resource',
        tool: 'skills_read_resource',
        ok: true,
        result: { uri: resourceUri }
      }
    })
    for (let index = 0; index < 128; index += 1) {
      emitAgentEvent({ type: 'message', runId: 'run-1', content: `buffered-${index}` })
    }

    start.resolve(successfulTurnOutput(input, 1))
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    await expect.element(screen.getByTestId('activated-skill-ids')).toHaveTextContent('')

    const authoritative = storedConversationWithRun(assistantMessageId, 'run-1', 'completed')
    const authoritativeAssistant = authoritative.messages.at(-1)
    if (!authoritativeAssistant?.agentRun) throw new Error('missing authoritative run fixture')
    authoritativeAssistant.agentRun.activatedSkills = [explicitSkillSummary]
    authoritativeAssistant.agentRun.skillActivationRevision =
      'activation-sha256-v1:workspace-resource'
    authoritativeAssistant.agentRun.toolCalls = [
      {
        id: 'read-workspace-resource',
        tool: 'skills_read_resource',
        args: { uri: resourceUri },
        approvalStatus: 'not_required',
        reason: null
      }
    ]
    authoritativeAssistant.agentRun.toolResults = [
      {
        callId: 'read-workspace-resource',
        tool: 'skills_read_resource',
        ok: true,
        result: { uri: resourceUri }
      }
    ]
    authoritativeAssistant.agentRun.timeline = [
      {
        id: 'tool-call-read-workspace-resource',
        type: 'tool_call',
        callId: 'read-workspace-resource'
      }
    ]
    testState.persistedConversations.set('conversation-a', authoritative)
    testState.loadConversation.mockResolvedValueOnce(authoritative)

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'authoritative answer'
    })

    await expect
      .element(screen.getByTestId('activated-skill-ids'))
      .toHaveTextContent(explicitSkillSummary.id)
    await expect
      .element(screen.getByTestId('agent-run-projection'))
      .toHaveTextContent('read-workspace-resource')
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
  })
})
