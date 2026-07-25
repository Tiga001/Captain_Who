import { describe, expect, it } from 'vitest'
import type { AgentEvent, AgentToolDefinition } from './agent'

const dynamicTool: AgentToolDefinition = {
  name: 'office_document',
  description: 'Create and edit Word documents.',
  inputSchema: {
    type: 'object',
    required: ['operation', 'reason']
  },
  safety: 'requires_approval',
  requiresWorkspace: false,
  requiresApproval: true,
  approvalMode: 'dynamic'
}

describe('tool_set_changed protocol event', () => {
  it('keeps the revision tuple and effective definitions in the wire payload', () => {
    const event: Extract<AgentEvent, { type: 'tool_set_changed' }> = {
      type: 'tool_set_changed',
      runId: 'run-tool-set',
      stableRevision: 'tool-set-sha256-v1:stable',
      dynamicRevision: 'tool-set-sha256-v1:documents',
      effectiveRevision: 'tool-set-sha256-v1:effective',
      toolDefinitions: [dynamicTool]
    }

    expect(JSON.parse(JSON.stringify(event))).toEqual({
      type: 'tool_set_changed',
      runId: 'run-tool-set',
      stableRevision: 'tool-set-sha256-v1:stable',
      dynamicRevision: 'tool-set-sha256-v1:documents',
      effectiveRevision: 'tool-set-sha256-v1:effective',
      toolDefinitions: [dynamicTool]
    })
  })
})
