import type {
  AgentActionExecutionOutput,
  AgentProposedAction,
  AgentSkillInstallationPreview
} from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import {
  applyAgentActionDecisionToChatMessage,
  applyAgentActionExecutionToChatMessage,
  applyAgentEventToChatMessage
} from '../../agentRun/agentEventReducer'
import type { ChatMessage } from '../chatTypes'

const preview: AgentSkillInstallationPreview = {
  name: 'fixture-skill',
  description: 'Fixture description',
  sourceSummary: { kind: 'github', url: 'https://github.com/example/fixture' },
  resolvedRevision: '0123456789abcdef',
  fileCount: 3,
  totalBytes: 1_024,
  resourceSummary: { total: 2, references: 1, assets: 0, scripts: 1, bytes: 512 },
  containsScripts: true,
  warnings: [
    {
      code: 'containsScripts',
      message: 'The package contains scripts.',
      requiresAcknowledgement: true
    }
  ],
  compatibility: 'compatibleWithWarnings',
  operation: 'install',
  impact: 'addManagedSkill'
}

function action(): AgentProposedAction {
  return {
    type: 'skill_installation',
    installation: {
      schemaVersion: 1,
      id: 'install-action',
      installRef: `skill_install_${'a'.repeat(32)}`,
      preview,
      approvalStatus: 'required',
      expiresAt: Date.now() + 60_000
    }
  }
}

function assistantMessage(): ChatMessage {
  return {
    id: 'assistant-message',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending'
  }
}

describe('Skill installation Timeline lifecycle', () => {
  it('retains the frozen approval preview after approval and successful installation', () => {
    const approval = action()
    const waiting = applyAgentEventToChatMessage(assistantMessage(), {
      type: 'approval_required',
      runId: 'run-1',
      action: approval
    })
    expect(waiting.agentRun?.skillInstallations).toEqual([
      expect.objectContaining({ status: 'waiting_for_approval' })
    ])

    const installing = applyAgentActionDecisionToChatMessage(waiting, approval, 'approved')
    expect(installing.agentRun?.approvals).toEqual([])
    expect(installing.agentRun?.skillInstallations).toEqual([
      expect.objectContaining({ status: 'installing' })
    ])

    const execution: AgentActionExecutionOutput = {
      actionId: 'install-action',
      actionType: 'skill_installation',
      toolName: 'skills_commit_install',
      status: 'approved',
      toolResult: {
        callId: 'install-action',
        tool: 'skills_commit_install',
        ok: true,
        result: {
          status: 'installed',
          name: 'fixture-skill',
          availableFrom: 'nextRun'
        }
      },
      agentOutput: {
        status: 'completed',
        content: 'Installed.',
        runId: 'run-1',
        events: [],
        toolDefinitions: [],
        proposedActions: []
      }
    }
    const installed = applyAgentActionExecutionToChatMessage(installing, execution)
    expect(installed.agentRun?.skillInstallations).toEqual([
      expect.objectContaining({
        status: 'installed',
        action: expect.objectContaining({ preview })
      })
    ])
    expect(installed.agentRun?.timeline).toContainEqual(
      expect.objectContaining({ type: 'tool_call', callId: 'install-action' })
    )
  })

  it('retains rejection as a settled Timeline state without installing', () => {
    const approval = action()
    const waiting = applyAgentEventToChatMessage(assistantMessage(), {
      type: 'approval_required',
      runId: 'run-1',
      action: approval
    })
    const rejected = applyAgentActionDecisionToChatMessage(waiting, approval, 'rejected')

    expect(rejected.agentRun?.skillInstallations).toEqual([
      expect.objectContaining({ status: 'rejected' })
    ])
    expect(rejected.agentRun?.toolResults).toEqual([
      expect.objectContaining({
        callId: 'install-action',
        ok: true,
        result: expect.objectContaining({ status: 'rejected' })
      })
    ])
  })

  it('distinguishes an uncertain commit from an ordinary installation failure', () => {
    const approval = action()
    const waiting = applyAgentEventToChatMessage(assistantMessage(), {
      type: 'approval_required',
      runId: 'run-1',
      action: approval
    })
    const installing = applyAgentActionDecisionToChatMessage(waiting, approval, 'approved')
    const uncertain = applyAgentActionExecutionToChatMessage(installing, {
      actionId: 'install-action',
      actionType: 'skill_installation',
      toolName: 'skills_commit_install',
      status: 'failed',
      toolResult: {
        callId: 'install-action',
        tool: 'skills_commit_install',
        ok: false,
        result: {
          code: 'commitResultUncertain',
          commitMayHaveSucceeded: true,
          recovery: 'refreshCatalogNextRun'
        },
        error: 'The final commit acknowledgement was interrupted.'
      },
      agentOutput: {
        status: 'completed',
        content: 'The result is uncertain.',
        runId: 'run-1',
        events: [],
        toolDefinitions: [],
        proposedActions: []
      }
    })

    expect(uncertain.agentRun?.skillInstallations).toEqual([
      expect.objectContaining({ status: 'uncertain' })
    ])
  })
})
