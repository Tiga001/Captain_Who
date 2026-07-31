import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatAgentRunView, ChatMcpToolInvocationView } from '../../features/chat/chatTypes'
import { AgentToolActivity } from '../../features/chat/components/toolActivities/AgentToolActivity'
import {
  McpToolActivity,
  McpToolActivityGroup
} from '../../features/chat/components/toolActivities/McpToolActivity'

const translations: Record<string, string> = {
  'agent.detail.args': 'Arguments',
  'agent.detail.result': 'Result',
  'agent.separator': ' · ',
  'agent.mcp.activity.detailUnavailable': 'Detailed status is unavailable',
  'agent.mcp.activity.duration': 'Duration',
  'agent.mcp.activity.durationInProgress': 'In progress',
  'agent.mcp.activity.durationMs': '{duration} ms',
  'agent.mcp.activity.durationNotExecuted': 'Not executed',
  'agent.mcp.activity.durationNotStarted': 'Not started',
  'agent.mcp.activity.durationUnavailable': 'Not recorded',
  'agent.mcp.activity.externalTool': 'External MCP Tool',
  'agent.mcp.activity.group.cancelledCount': '{count} cancelled',
  'agent.mcp.activity.group.completed': 'Called {count} “{server}” tools',
  'agent.mcp.activity.group.failedCount': '{count} failed',
  'agent.mcp.activity.group.outcomeUnknownCount':
    '{count} outcome unknown; effects may have occurred',
  'agent.mcp.activity.group.outputTruncatedCount': '{count} output truncated',
  'agent.mcp.activity.group.processed': 'Processed {count} “{server}” tool calls',
  'agent.mcp.activity.group.rejectedCount': '{count} rejected',
  'agent.mcp.activity.group.running': 'Calling {count} “{server}” tools',
  'agent.mcp.activity.group.runningCount': '{count} running',
  'agent.mcp.activity.group.succeededCount': '{count} succeeded',
  'agent.mcp.activity.group.waiting': 'Waiting to call {count} “{server}” tools',
  'agent.mcp.activity.group.waitingCount': '{count} awaiting approval',
  'agent.mcp.activity.reason': 'Call reason',
  'agent.mcp.activity.reasonUnavailable': 'No call reason was provided',
  'agent.mcp.activity.summary.outputTruncated': 'output truncated',
  'agent.mcp.activity.single.cancelled': 'Cancelled “{server}” tool call · {tool}',
  'agent.mcp.activity.single.cancelled.compact': 'Cancelled tool call · {tool}',
  'agent.mcp.activity.single.completed': 'Called “{server}” tool · {tool}',
  'agent.mcp.activity.single.completed.compact': 'Called tool · {tool}',
  'agent.mcp.activity.single.expired': '“{server}” tool call expired · {tool}',
  'agent.mcp.activity.single.expired.compact': 'Tool call expired · {tool}',
  'agent.mcp.activity.single.failed': '“{server}” tool call failed · {tool}',
  'agent.mcp.activity.single.failed.compact': 'Tool call failed · {tool}',
  'agent.mcp.activity.single.outcomeUnknown':
    '“{server}” tool call result unknown; effects may have occurred · {tool}',
  'agent.mcp.activity.single.outcomeUnknown.compact':
    'Tool call result unknown; effects may have occurred · {tool}',
  'agent.mcp.activity.single.payloadUnavailable':
    '“{server}” tool call arguments unavailable · {tool}',
  'agent.mcp.activity.single.payloadUnavailable.compact':
    'Tool call arguments unavailable · {tool}',
  'agent.mcp.activity.single.policyDenied': 'Policy denied “{server}” tool call · {tool}',
  'agent.mcp.activity.single.policyDenied.compact': 'Policy denied tool call · {tool}',
  'agent.mcp.activity.single.rejected': 'Rejected “{server}” tool call · {tool}',
  'agent.mcp.activity.single.rejected.compact': 'Rejected tool call · {tool}',
  'agent.mcp.activity.single.running': 'Calling “{server}” tool · {tool}',
  'agent.mcp.activity.single.running.compact': 'Calling tool · {tool}',
  'agent.mcp.activity.single.waiting': 'Waiting to call “{server}” tool · {tool}',
  'agent.mcp.activity.single.waiting.compact': 'Waiting to call tool · {tool}',
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

  it('shows OutcomeUnknown prominently in the summary with exactly reason and duration details', async () => {
    const screen = await render(
      <McpToolActivity
        invocation={invocation({
          state: 'outcome_unknown',
          outcome: 'outcome_unknown',
          errorCode: 'response_lost',
          displayReason: 'Check the remote state',
          durationMs: 1250
        })}
      />
    )

    expect(screen.container.textContent).toContain(
      'tool call result unknown; effects may have occurred'
    )
    expect(screen.container.textContent).toContain('Check the remote state')
    expect(screen.container.textContent).toContain('1,250 ms')
    expect(screen.container.querySelectorAll('.mcp-tool-activity__metadata > div')).toHaveLength(2)
    expect(screen.container.textContent).not.toContain('response_lost')
    expect(screen.container.querySelector('button')).toBeNull()
    expect(screen.container.textContent?.toLowerCase()).not.toContain('retry')
  })

  it('presents isError as a failed call with truncation in the summary but no unsafe details', async () => {
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

    expect(screen.container.textContent).toContain('tool call failed')
    expect(screen.container.textContent).not.toContain('server_tool_error')
    expect(screen.container.textContent).toContain('output truncated')
    expect(screen.container.querySelectorAll('.mcp-tool-activity__metadata > div')).toHaveLength(2)
    expect(screen.container.textContent).not.toContain(SECRET_CANARY)
  })

  it('prioritizes rejection guidance in the reason slot and always renders only two detail rows', async () => {
    const withReason = await render(
      <McpToolActivity
        invocation={invocation({
          state: 'rejected',
          outcome: 'rejected',
          errorCode: 'mcp.approval_rejected',
          rejectionReason: 'Use a different directory',
          displayReason: 'Read the inventory',
          durationMs: 80
        })}
      />
    )

    expect(withReason.container.textContent).toContain('Owned fixture')
    expect(withReason.container.textContent).toContain('echo_text')
    expect(withReason.container.textContent).toContain('Call reason')
    expect(withReason.container.textContent).toContain('Use a different directory')
    expect(withReason.container.textContent).not.toContain('Read the inventory')
    expect(withReason.container.textContent).not.toContain('mcp.approval_rejected')
    expect(withReason.container.textContent).toContain('80 ms')
    expect(
      withReason.container.querySelectorAll('.mcp-tool-activity__metadata > div')
    ).toHaveLength(2)

    const withoutReason = await render(
      <McpToolActivity
        invocation={invocation({
          state: 'rejected',
          outcome: 'rejected',
          errorCode: 'mcp.approval_rejected'
        })}
      />
    )
    expect(withoutReason.container.textContent).toContain('No call reason was provided')
    expect(withoutReason.container.textContent).toContain('Not executed')
    expect(withoutReason.container.textContent).not.toContain('mcp.approval_rejected')
  })

  it('groups calls from one Server into nested disclosures with separate terminal counts', async () => {
    const screen = await render(
      <McpToolActivityGroup
        items={[
          invocation({
            invocationId: 'invocation-success',
            state: 'completed',
            outcome: 'succeeded',
            dispatchCertainty: 'response_received',
            displayReason: 'List files',
            durationMs: 8,
            outputTruncated: true
          }),
          invocation({
            invocationId: 'invocation-failed',
            rawToolName: 'read_text_file',
            state: 'failed',
            outcome: 'transport_error',
            dispatchCertainty: 'definitely_not_dispatched',
            displayReason: 'Read fruits',
            durationMs: 12
          }),
          invocation({
            invocationId: 'invocation-unknown',
            rawToolName: 'move_file',
            state: 'outcome_unknown',
            outcome: 'outcome_unknown',
            displayReason: 'Rename the result'
          })
        ]}
      />
    )

    expect(screen.container.textContent).toContain('Called 3 “Owned fixture” tools')
    expect(screen.container.textContent).toContain('1 succeeded')
    expect(screen.container.textContent).toContain('1 failed')
    expect(screen.container.textContent).toContain('1 outcome unknown; effects may have occurred')
    expect(screen.container.textContent).toContain('1 output truncated')
    expect(screen.container.querySelectorAll('details')).toHaveLength(4)
    expect(screen.container.textContent).toContain('Called tool · echo_text')
    expect(screen.container.textContent).toContain('Tool call failed · read_text_file')
    expect(screen.container.textContent).toContain(
      'Tool call result unknown; effects may have occurred · move_file'
    )
  })

  it('describes terminal calls that never crossed dispatch as processed, not called', async () => {
    const screen = await render(
      <McpToolActivityGroup
        items={[
          invocation({
            invocationId: 'invocation-rejected',
            state: 'rejected',
            outcome: 'rejected',
            dispatchCertainty: 'definitely_not_dispatched'
          }),
          invocation({
            invocationId: 'invocation-expired',
            state: 'expired',
            outcome: 'expired',
            dispatchCertainty: 'definitely_not_dispatched'
          }),
          invocation({
            invocationId: 'invocation-policy-denied',
            state: 'policy_denied',
            outcome: 'policy_denied',
            dispatchCertainty: 'definitely_not_dispatched'
          })
        ]}
      />
    )

    expect(screen.container.textContent).toContain('Processed 3 “Owned fixture” tool calls')
    expect(screen.container.textContent).toContain('2 rejected')
    expect(screen.container.textContent).toContain('1 failed')
    expect(screen.container.textContent).not.toContain('Called 3 “Owned fixture” tools')
  })

  it('keeps mixed waiting and running counts live under a running group summary', async () => {
    const screen = await render(
      <McpToolActivityGroup
        items={[
          invocation({
            invocationId: 'invocation-waiting',
            state: 'pending_approval',
            dispatchCertainty: 'definitely_not_dispatched'
          }),
          invocation({
            invocationId: 'invocation-running',
            state: 'running',
            dispatchCertainty: 'possibly_dispatched'
          })
        ]}
      />
    )

    expect(screen.container.textContent).toContain('Calling 2 “Owned fixture” tools')
    expect(screen.container.textContent).toContain('1 awaiting approval')
    expect(screen.container.textContent).toContain('1 running')
  })

  it('keeps a one-item group as a single disclosure with the Server in its summary', async () => {
    const screen = await render(
      <McpToolActivityGroup
        items={[
          invocation({
            state: 'completed',
            outcome: 'succeeded',
            dispatchCertainty: 'response_received',
            displayReason: 'List the directory',
            durationMs: 6
          })
        ]}
      />
    )

    expect(screen.container.querySelectorAll('details')).toHaveLength(1)
    expect(screen.container.textContent).toContain('Called “Owned fixture” tool · echo_text')
    expect(screen.container.querySelectorAll('.mcp-tool-activity__metadata > div')).toHaveLength(2)
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

    expect(screen.container.textContent).toContain('Owned fixture')
    expect(screen.container.textContent).toContain('echo_text')
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
