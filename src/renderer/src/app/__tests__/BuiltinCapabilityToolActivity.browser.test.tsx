import type { AgentToolCall, AgentToolIdentity, AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatAgentRunView, ChatMcpToolInvocationView } from '../../features/chat/chatTypes'
import { AgentToolActivity } from '../../features/chat/components/toolActivities/AgentToolActivity'

const translations: Record<string, string> = {
  'agent.builtinCapability.activity.browserAutomation': 'Browser automation',
  'agent.builtinCapability.activity.running': 'Using {capability} · {tool}',
  'agent.builtinCapability.activity.completed': 'Used {capability} · {tool}',
  'agent.builtinCapability.activity.failed': '{capability} failed · {tool}',
  'agent.builtinCapability.activity.outcomeUnknown':
    '{capability} outcome unknown; effects may have occurred · {tool}',
  'agent.builtinCapability.activity.cancelled': 'Cancelled {capability} · {tool}',
  'agent.detail.args': 'Arguments',
  'agent.detail.error': 'Error',
  'agent.detail.result': 'Result',
  'agent.tool.running': 'Running {tool}'
}

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => translations[key] ?? key })
}))
vi.mock('../../host/hostClient', () => ({ hostClient: {} }))

const CANARY = 'PRIVATE_BROWSER_ARGUMENT_RESULT_CDP_CANARY'

function run(): ChatAgentRunView {
  return {
    runId: 'run-browser-capability',
    status: 'running',
    toolDefinitions: [],
    toolCalls: [],
    toolResults: [],
    approvals: [],
    diffs: [],
    timeline: []
  }
}

function call(tool = 'managed-visible-name'): AgentToolCall {
  return {
    id: 'call-browser-capability',
    tool,
    args: { url: CANARY, cdpEndpoint: CANARY },
    approvalStatus: 'approved',
    reason: CANARY
  }
}

function identity(modelName = 'browser_navigate'): AgentToolIdentity {
  return {
    type: 'builtin_capability',
    capabilityId: 'browser_automation',
    managedMcpId: 'builtin.browser_automation.mcp',
    manifestDigest: `sha256:${'a'.repeat(64)}`,
    toolId: 'browser.navigate',
    modelName
  }
}

function staleExternalInvocation(): ChatMcpToolInvocationView {
  return {
    actionId: '11111111-1111-4111-8111-111111111111',
    invocationId: '22222222-2222-4222-8222-222222222222',
    callId: 'call-browser-capability',
    serverId: '33333333-3333-4333-8333-333333333333',
    serverDisplayName: CANARY,
    rawToolName: CANARY,
    modelToolName: CANARY,
    external: true,
    state: 'running',
    dispatchCertainty: 'possibly_dispatched',
    outputTruncated: false
  }
}

describe('BuiltinCapabilityToolActivity', () => {
  it('routes only from typed identity and never renders Tool arguments or result bodies', async () => {
    const result: AgentToolResult = {
      callId: 'call-browser-capability',
      tool: 'managed-visible-name',
      ok: true,
      result: { structuredContent: CANARY },
      error: CANARY
    }
    const screen = await render(
      <AgentToolActivity
        call={call()}
        result={result}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={identity()}
      />
    )

    expect(screen.container.textContent).toContain('Used Browser automation · browser_navigate')
    expect(screen.container.textContent).not.toContain(CANARY)
    expect(screen.container.querySelector('details')).toBeNull()
    expect(screen.container.querySelector('pre')).toBeNull()
  })

  it('recognizes a managed browser Tool whose model name has no browser prefix', async () => {
    const screen = await render(
      <AgentToolActivity
        call={call('opaque-model-name')}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={identity('opaque-model-name')}
      />
    )

    expect(screen.container.textContent).toContain('Using Browser automation · opaque-model-name')
    expect(screen.container.textContent).not.toContain(CANARY)
  })

  it('lets typed built-in identity win over stale external MCP lifecycle data', async () => {
    const screen = await render(
      <AgentToolActivity
        call={call()}
        mcpInvocation={staleExternalInvocation()}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={identity()}
      />
    )

    expect(screen.container.textContent).toContain('Using Browser automation · browser_navigate')
    expect(screen.container.textContent).not.toContain(CANARY)
    expect(screen.container.querySelector('details')).toBeNull()
  })

  it('does not infer built-in capability identity from a browser-prefixed Tool name', async () => {
    const screen = await render(
      <AgentToolActivity
        call={call('browser_spoofed_tool')}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={{ type: 'unregistered', toolName: 'browser_spoofed_tool' }}
      />
    )

    expect(screen.container.textContent).not.toContain('Browser automation')
    expect(screen.container.textContent).toContain(CANARY)
  })

  it('renders an uncertain browser failure without exposing a retry control or raw result', async () => {
    const result: AgentToolResult = {
      callId: 'call-browser-capability',
      tool: 'managed-visible-name',
      ok: false,
      result: {
        schemaVersion: 1,
        type: 'builtin_capability_tool',
        status: 'outcome_unknown',
        contentOmitted: true,
        detail: CANARY
      },
      error: CANARY
    }
    const screen = await render(
      <AgentToolActivity
        call={call()}
        result={result}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={identity()}
      />
    )

    expect(screen.container.textContent).toContain(
      'Browser automation outcome unknown; effects may have occurred · browser_navigate'
    )
    expect(screen.container.textContent).not.toContain(CANARY)
    expect(screen.container.querySelector('button')).toBeNull()
    expect(screen.container.querySelector('a')).toBeNull()
  })
})
