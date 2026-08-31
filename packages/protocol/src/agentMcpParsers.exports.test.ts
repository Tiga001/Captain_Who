import { describe, expect, it } from 'vitest'
import * as facade from './agentMcpParsers'
import * as rootProtocol from './index'

const EXPECTED_AGENT_MCP_PARSER_EXPORTS = [
  'parseAgentActionExecutionOutputForHost',
  'parseAgentBrowserRiskApproval',
  'parseAgentBrowserRiskProposedAction',
  'parseAgentBuiltinCapabilityActivationApproval',
  'parseAgentBuiltinCapabilityActivationProposedAction',
  'parseAgentBuiltinMcpToolApproval',
  'parseAgentBuiltinMcpToolApprovalProposedAction',
  'parseAgentEventForHost',
  'parseAgentFileChangeContentPageForHost',
  'parseAgentFileChangeDiffPageForHost',
  'parseAgentFileChangeHistoryDiffPageForHost',
  'parseAgentFileChangeResultForHost',
  'parseAgentMcpProposedAction',
  'parseAgentMcpToolApproval',
  'parseAgentMcpToolInvocationEvent',
  'parseAgentToolIdentityForHost',
  'parsePendingAgentActionSnapshotsForHost'
] as const

describe('agent MCP parser compatibility facade', () => {
  it('keeps the exact legacy-path runtime export surface', () => {
    expect(Object.keys(facade).sort()).toEqual([...EXPECTED_AGENT_MCP_PARSER_EXPORTS].sort())
  })

  it('keeps every legacy parser reachable through the package root', () => {
    const rootExports = rootProtocol as Record<string, unknown>
    const facadeExports = facade as Record<string, unknown>

    for (const name of EXPECTED_AGENT_MCP_PARSER_EXPORTS) {
      expect(rootExports[name], name).toBe(facadeExports[name])
    }
  })
})
