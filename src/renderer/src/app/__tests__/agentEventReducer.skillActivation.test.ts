import type { ActivatedSkillSummary, AgentEvent } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import type { ChatAgentRunView, ChatMessage } from '../../features/chat/chatTypes'
import { applyAgentEventToChatMessage } from '../../features/agentRun/agentEventReducer'

const explicitSkill: ActivatedSkillSummary = {
  id: 'bundled:application:documents',
  name: 'documents',
  revision: 'skill-package-sha256-v2:documents',
  source: { kind: 'bundled', id: 'bundled:application' }
}

const modelSkill: ActivatedSkillSummary = {
  id: 'bundled:application:spreadsheets',
  name: 'spreadsheets',
  revision: 'skill-package-sha256-v2:spreadsheets',
  source: { kind: 'bundled', id: 'bundled:application' }
}

function run(overrides: Partial<ChatAgentRunView> = {}): ChatAgentRunView {
  return {
    runId: 'run-skill-activation',
    status: 'running',
    toolDefinitions: [],
    toolCalls: [],
    toolResults: [],
    approvals: [],
    diffs: [],
    timeline: [],
    ...overrides
  }
}

function message(agentRun?: ChatAgentRunView): ChatMessage {
  return {
    id: 'assistant-message',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending',
    agentRun
  }
}

function activationEvent(
  skill: ActivatedSkillSummary = modelSkill,
  activationRevision = 'activation-sha256-v1:two-skills'
): Extract<AgentEvent, { type: 'skill_activated' }> {
  return {
    type: 'skill_activated',
    runId: 'run-skill-activation',
    activationRevision,
    activatedBy: 'model',
    skill
  }
}

describe('Skill activation runtime projection', () => {
  it('adds a model-activated Skill without terminating the run', () => {
    const result = applyAgentEventToChatMessage(message(), activationEvent())

    expect(result.status).toBe('pending')
    expect(result.agentRun?.status).toBe('running')
    expect(result.agentRun?.activatedSkills).toEqual([modelSkill])
    expect(result.agentRun?.skillActivationRevision).toBe('activation-sha256-v1:two-skills')
  })

  it('merges explicit and model activation into one ordered inventory', () => {
    const current = message(
      run({
        activatedSkills: [explicitSkill],
        skillActivationRevision: 'activation-sha256-v1:explicit',
        explicitSkillSelections: [{ id: explicitSkill.id, revision: explicitSkill.revision }]
      })
    )

    const result = applyAgentEventToChatMessage(current, activationEvent())

    expect(result.agentRun?.activatedSkills).toEqual([explicitSkill, modelSkill])
    expect(result.agentRun?.explicitSkillSelections).toEqual([
      { id: explicitSkill.id, revision: explicitSkill.revision }
    ])
  })

  it('replays idempotently and replaces a revised identity in place', () => {
    const duplicateOldSkill = { ...modelSkill, name: 'legacy duplicate' }
    const revisedSkill = {
      ...modelSkill,
      name: 'Spreadsheets',
      revision: 'skill-package-sha256-v2:spreadsheets-new'
    }
    const current = message(
      run({ activatedSkills: [modelSkill, duplicateOldSkill, explicitSkill] })
    )

    const first = applyAgentEventToChatMessage(
      current,
      activationEvent(revisedSkill, 'activation-sha256-v1:revised')
    )
    const replayed = applyAgentEventToChatMessage(
      first,
      activationEvent(revisedSkill, 'activation-sha256-v1:revised')
    )

    expect(replayed.agentRun?.activatedSkills).toEqual([revisedSkill, explicitSkill])
    expect(replayed.agentRun?.skillActivationRevision).toBe('activation-sha256-v1:revised')
  })

  it('does not claim activation from a failed skills_activate ToolResult', () => {
    const result = applyAgentEventToChatMessage(message(run()), {
      type: 'tool_result',
      runId: 'run-skill-activation',
      result: {
        callId: 'activate-failed',
        tool: 'skills_activate',
        ok: false,
        error: 'Skill activation failed.'
      }
    })

    expect(result.agentRun?.activatedSkills).toBeUndefined()
  })
})
