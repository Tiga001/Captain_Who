import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatAgentRunView, ChatMcpToolInvocationView } from '../../features/chat/chatTypes'
import { AgentToolActivity } from '../../features/chat/components/toolActivities/AgentToolActivity'
import { McpToolActivity } from '../../features/chat/components/toolActivities/McpToolActivity'

const translations: Record<string, string> = {
  'agent.detail.args': 'Arguments',
  'agent.detail.result': 'Result',
  'agent.mcp.activity.detailUnavailable': 'Detailed status is unavailable',
  'agent.mcp.activity.duration': 'Duration',
  'agent.mcp.activity.errorCode': 'Error code',
  'agent.mcp.activity.expired': 'External MCP Tool approval expired',
  'agent.mcp.activity.externalTool': 'External MCP Tool',
  'agent.mcp.activity.outcomeUnknown': 'External MCP Tool outcome unknown',
  'agent.mcp.activity.outcomeUnknownWarning':
    'The result is unknown and side effects may have occurred. Check the authoritative system before deciding what to do next.',
  'agent.mcp.activity.outputTruncated': 'Some output was omitted',
  'agent.mcp.activity.payloadUnavailable': 'External MCP Tool payload unavailable',
  'agent.mcp.activity.policyDenied': 'External MCP Tool denied by policy',
  'agent.mcp.activity.rejected': 'External MCP Tool rejected',
  'agent.mcp.activity.rejectionReason': 'Rejection reason',
  'agent.mcp.activity.server': 'Server',
  'agent.mcp.activity.tool': 'Tool',
  'agent.mcp.activity.toolError': 'External MCP Tool returned an error',
  'agent.tool.cancelled': 'Stopped {tool}',
  'agent.tool.completed': 'Completed {tool}',
  'agent.tool.failed': '{tool} failed',
  'agent.tool.running': 'Running {tool}',
  'agent.tool.waitingApproval': 'Waiting for approval: {tool}'
}

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => translations[key] ?? key })
}))
vi.mock('../../host/hostClient', () => ({ hostClient: {} }))

const INVOCATION_ID = '22222222-2222-4222-8222-222222222222'
const CALL_ID = `tc1_${'a'.repeat(43)}`
const SECRET_CANARY = 'MCP_RAW_RESULT_CANARY_DO_NOT_RENDER'

function invocation(overrides: Partial<ChatMcpToolInvocationView> = {}): ChatMcpToolInvocationView {
  return {
    actionId: '11111111-1111-4111-8111-111111111111',
    invocationId: INVOCATION_ID,
    callId: CALL_ID,
    serverId: '33333333-3333-4333-8333-333333333333',
    serverDisplayName: 'Owned fixture',
    scope: { type: 'user' },
    rawToolName: 'echo_text',
    modelToolName: 'provider-safe-name',
    external: true,
    state: 'running',
    dispatchCertainty: 'possibly_dispatched',
    outputTruncated: false,
    ...overrides
  }
}

function run(): ChatAgentRunView {
  return {
    runId: 'run-mcp-activity',
    status: 'running',
    toolDefinitions: [],
    toolCalls: [],
    toolResults: [],
    approvals: [],
    diffs: [],
    mcpInvocations: [],
    timeline: []
  }
}

describe('McpToolActivity', () => {
  it('renders untrusted Server and Tool names as sanitized plain text', async () => {
    const screen = await render(
      <McpToolActivity
        invocation={invocation({
          serverDisplayName: '<img src=x onerror=alert(1)>\u202Eserver',
          rawToolName: '[click](javascript:alert(1))\u0000tool'
        })}
      />
    )

    expect(screen.container.querySelector('img')).toBeNull()
    expect(screen.container.querySelector('a')).toBeNull()
    expect(screen.container.textContent).toContain('<img src=x onerror=alert(1)>server')
    expect(screen.container.textContent).toContain('[click](javascript:alert(1))tool')
    expect(screen.container.textContent).not.toContain('\u202E')
    expect(screen.container.textContent).not.toContain('\u0000')
  })

  it('shows OutcomeUnknown as a prominent warning with no retry action', async () => {
    const screen = await render(
      <McpToolActivity
        invocation={invocation({
          state: 'outcome_unknown',
          outcome: 'outcome_unknown',
          errorCode: 'response_lost',
          durationMs: 1250
        })}
      />
    )

    const alert = screen.container.querySelector('[role="alert"]')
    expect(alert?.textContent).toContain('side effects may have occurred')
    expect(screen.container.textContent).toContain('response_lost')
    expect(screen.container.textContent).toContain('1,250 ms')
    expect(screen.container.querySelector('button')).toBeNull()
    expect(screen.container.textContent?.toLowerCase()).not.toContain('retry')
  })

  it('presents isError as a completed Tool-level error without any result body', async () => {
    const screen = await render(
      <McpToolActivity
        invocation={invocation({
          state: 'completed',
          dispatchCertainty: 'response_received',
          outcome: 'tool_error',
          isError: true,
          errorCode: 'server_tool_error',
          durationMs: 20,
          outputTruncated: true
        })}
      />
    )

    expect(screen.container.textContent).toContain('returned an error')
    expect(screen.container.textContent).toContain('Some output was omitted')
    expect(screen.container.textContent).toContain('server_tool_error')
    expect(screen.container.textContent).not.toContain(SECRET_CANARY)
  })

  it('shows only Server, Tool and optional user guidance for a rejected call', async () => {
    const withReason = await render(
      <McpToolActivity
        invocation={invocation({
          state: 'rejected',
          outcome: 'rejected',
          errorCode: 'mcp.approval_rejected',
          rejectionReason: 'Use a different directory',
          durationMs: 80
        })}
      />
    )

    expect(withReason.container.textContent).toContain('Server')
    expect(withReason.container.textContent).toContain('Owned fixture')
    expect(withReason.container.textContent).toContain('Tool')
    expect(withReason.container.textContent).toContain('echo_text')
    expect(withReason.container.textContent).toContain('Rejection reason')
    expect(withReason.container.textContent).toContain('Use a different directory')
    expect(withReason.container.textContent).not.toContain('Error code')
    expect(withReason.container.textContent).not.toContain('mcp.approval_rejected')
    expect(withReason.container.textContent).not.toContain('80 ms')

    const withoutReason = await render(
      <McpToolActivity
        invocation={invocation({
          state: 'rejected',
          outcome: 'rejected',
          errorCode: 'mcp.approval_rejected'
        })}
      />
    )
    expect(withoutReason.container.textContent).not.toContain('Rejection reason')
    expect(withoutReason.container.textContent).not.toContain('mcp.approval_rejected')
  })

  it('fails closed when typed MCP lifecycle details are unavailable', async () => {
    const screen = await render(<McpToolActivity />)

    expect(screen.container.textContent).toContain('External MCP Tool')
    expect(screen.container.textContent).toContain('Detailed status is unavailable')
    expect(screen.container.querySelector('pre')).toBeNull()
  })

  it('routes typed MCP identity before GenericToolActivity and never renders args or results', async () => {
    const call: AgentToolCall = {
      id: CALL_ID,
      tool: 'provider-safe-name',
      args: { rawArguments: SECRET_CANARY },
      approvalStatus: 'approved',
      reason: SECRET_CANARY
    }
    const result: AgentToolResult = {
      callId: CALL_ID,
      tool: call.tool,
      ok: false,
      error: SECRET_CANARY,
      result: { structuredContent: SECRET_CANARY }
    }
    const screen = await render(
      <AgentToolActivity
        call={call}
        mcpInvocation={invocation()}
        result={result}
        run={run()}
        showImageGenerationPreview={false}
      />
    )

    expect(screen.container.textContent).toContain('External MCP Tool')
    expect(screen.container.textContent).not.toContain(SECRET_CANARY)
    expect(screen.container.querySelector('pre')).toBeNull()
  })

  it('does not infer MCP identity from an mcp__ Tool name', async () => {
    const call: AgentToolCall = {
      id: 'ordinary-call',
      tool: 'mcp__spoofed__name',
      args: { ordinaryArgument: 'visible-generic-value' },
      approvalStatus: 'not_required'
    }
    const screen = await render(
      <AgentToolActivity call={call} run={run()} showImageGenerationPreview={false} />
    )

    expect(screen.container.textContent).not.toContain('External MCP Tool')
    expect(screen.container.textContent).toContain('visible-generic-value')
  })
})
