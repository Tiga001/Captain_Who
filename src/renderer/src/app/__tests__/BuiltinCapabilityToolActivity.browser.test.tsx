import type { AgentToolCall, AgentToolIdentity, AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatAgentRunView, ChatMcpToolInvocationView } from '../../features/chat/chatTypes'
import { AgentToolActivity } from '../../features/chat/components/toolActivities/AgentToolActivity'

const translations: Record<string, string> = {
  'mcp.builtin.browserAutomation.name': 'Browser automation',
  'agent.builtinCapability.activation.waiting': 'Waiting for approval to use {capability}',
  'agent.builtinCapability.activation.completed': 'Enabled {capability}',
  'agent.builtinCapability.activity.reason': 'Call reason',
  'agent.builtinCapability.browser.navigate.running': 'Opening page',
  'agent.builtinCapability.browser.navigate.completed': 'Opened page',
  'agent.builtinCapability.browser.navigate.outcomeUnknown':
    'Page navigation outcome uncertain; the action may have occurred',
  'agent.builtinCapability.browser.click.running': 'Clicking page',
  'agent.builtinCapability.browser.click.completed': 'Clicked page',
  'agent.builtinCapability.browser.click.failed': 'Failed to click page',
  'agent.builtinCapability.browser.click.cancelled': 'Cancelled clicking page',
  'agent.builtinCapability.browser.click.outcomeUnknown':
    'Page click outcome uncertain; the action may have occurred',
  'agent.builtinCapability.browser.snapshot.completed': 'Read page',
  'agent.builtinCapability.browser.find.completed': 'Searched page',
  'agent.builtinCapability.browser.type.completed': 'Typed into page',
  'agent.builtinCapability.browser.fillForm.completed': 'Filled form',
  'agent.builtinCapability.browser.pressKey.completed': 'Sent key press',
  'agent.builtinCapability.browser.tabs.completed': 'Viewed browser tabs',
  'agent.builtinCapability.browser.waitFor.completed': 'Finished waiting for page',
  'agent.builtinCapability.browser.close.completed': 'Closed page',
  'agent.builtinCapability.browser.fallback.running': 'Running browser action',
  'agent.builtinCapability.browser.fallback.completed': 'Completed browser action',
  'agent.builtinCapability.browser.fallback.outcomeUnknown':
    'Browser action outcome uncertain; the action may have occurred',
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

function call(tool = 'managed-visible-name', reason: string | null = null): AgentToolCall {
  return {
    id: 'call-browser-capability',
    tool,
    args: { url: CANARY, cdpEndpoint: CANARY },
    approvalStatus: 'approved',
    reason
  }
}

function identity(modelName = 'browser_navigate', toolId = modelName): AgentToolIdentity {
  return {
    type: 'builtin_capability',
    capabilityId: 'browser_automation',
    managedMcpId: 'builtin.browser_automation.mcp',
    manifestDigest: `sha256:${'a'.repeat(64)}`,
    toolId,
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
  it('renders activate_capability as localized product copy without generic details', async () => {
    const activationCall: AgentToolCall = {
      id: 'call-browser-capability',
      tool: 'activate_capability',
      args: { capability: 'browser_automation', reason: CANARY },
      approvalStatus: 'required',
      reason: CANARY
    }
    const runtimeIdentity: AgentToolIdentity = {
      type: 'runtime_extension',
      extensionId: 'builtin.capabilities',
      toolName: 'activate_capability'
    }
    const waiting = await render(
      <AgentToolActivity
        call={activationCall}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={runtimeIdentity}
      />
    )

    expect(waiting.container.textContent).toContain(
      'Waiting for approval to use Browser automation'
    )
    expect(waiting.container.textContent).not.toContain('activate_capability')
    expect(waiting.container.textContent).not.toContain(CANARY)
    expect(waiting.container.querySelector('details')).toBeNull()
    await waiting.unmount()

    const completed = await render(
      <AgentToolActivity
        call={activationCall}
        result={{
          callId: activationCall.id,
          tool: activationCall.tool,
          ok: true,
          result: { status: 'active', capability: 'browser_automation' }
        }}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={runtimeIdentity}
      />
    )
    expect(completed.container.textContent).toContain('Enabled Browser automation')
    expect(completed.container.textContent).not.toContain('active')
    await completed.unmount()
  })

  it('routes only from typed identity and exposes only the bounded display reason', async () => {
    const result: AgentToolResult = {
      callId: 'call-browser-capability',
      tool: 'managed-visible-name',
      ok: true,
      result: { structuredContent: CANARY },
      error: CANARY
    }
    const screen = await render(
      <AgentToolActivity
        call={call('managed-visible-name', 'Open the requested page.')}
        result={result}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={identity()}
      />
    )

    expect(screen.container.textContent).toContain('Opened page')
    expect(screen.container.textContent).toContain('Call reason')
    expect(screen.container.textContent).toContain('Open the requested page.')
    expect(screen.container.textContent).not.toContain(CANARY)
    expect(screen.container.querySelector('details')).not.toBeNull()
    expect(screen.container.querySelector('pre')).toBeNull()
  })

  it('recognizes a managed browser Tool whose model name has no browser prefix', async () => {
    const screen = await render(
      <AgentToolActivity
        call={call('opaque-model-name')}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={identity('opaque-model-name', 'browser_click')}
      />
    )

    expect(screen.container.textContent).toContain('Clicking page')
    expect(screen.container.textContent).not.toContain('opaque-model-name')
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

    expect(screen.container.textContent).toContain('Opening page')
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
      'Page navigation outcome uncertain; the action may have occurred'
    )
    expect(screen.container.textContent).not.toContain(CANARY)
    expect(screen.container.querySelector('button')).toBeNull()
    expect(screen.container.querySelector('a')).toBeNull()
  })

  it.each([
    ['running', undefined, undefined, 'Clicking page'],
    [
      'failed',
      {
        callId: 'call-browser-capability',
        tool: 'browser_click',
        ok: false,
        result: {
          schemaVersion: 1,
          type: 'builtin_capability_tool',
          status: 'failed',
          contentOmitted: true
        }
      } satisfies AgentToolResult,
      undefined,
      'Failed to click page'
    ],
    ['cancelled', undefined, 'cancelled', 'Cancelled clicking page']
  ] as const)(
    'renders the browser click %s state with dedicated copy',
    async (_name, result, settledStatus, expected) => {
      const screen = await render(
        <AgentToolActivity
          call={call('browser_click')}
          result={result}
          run={run()}
          settledStatus={settledStatus}
          showImageGenerationPreview={false}
          toolIdentity={identity('browser_click', 'browser_click')}
        />
      )
      expect(screen.container.textContent).toContain(expected)
      expect(screen.container.textContent).not.toContain('browser_click')
    }
  )

  it('renders the display reason as text rather than HTML', async () => {
    const reason = '<img src=x onerror=alert(1)>Click the requested result'
    const screen = await render(
      <AgentToolActivity
        call={call('browser_click', reason)}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={identity('browser_click', 'browser_click')}
      />
    )

    expect(screen.container.textContent).toContain(reason)
    expect(screen.container.querySelector('img')).toBeNull()
    expect(screen.container.querySelector('pre')).toBeNull()
  })

  it.each([
    ['browser_navigate', 'Opened page'],
    ['browser_snapshot', 'Read page'],
    ['browser_find', 'Searched page'],
    ['browser_click', 'Clicked page'],
    ['browser_type', 'Typed into page'],
    ['browser_fill_form', 'Filled form'],
    ['browser_press_key', 'Sent key press'],
    ['browser_tabs', 'Viewed browser tabs'],
    ['browser_wait_for', 'Finished waiting for page'],
    ['browser_close', 'Closed page']
  ])('renders reviewed tool %s with product copy', async (toolId, expected) => {
    const screen = await render(
      <AgentToolActivity
        call={call(toolId)}
        result={{
          callId: 'call-browser-capability',
          tool: toolId,
          ok: true,
          result: {
            schemaVersion: 1,
            type: 'builtin_capability_tool',
            status: 'completed',
            contentOmitted: true
          }
        }}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={identity(toolId, toolId)}
      />
    )

    expect(screen.container.textContent).toContain(expected)
    expect(screen.container.textContent).not.toContain(toolId)
    expect(screen.container.querySelector('pre')).toBeNull()
  })

  it('uses a safe fallback for a future reviewed Tool without exposing its raw name', async () => {
    const screen = await render(
      <AgentToolActivity
        call={call('opaque-model-name')}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={identity('opaque-model-name', 'future.reviewed.tool')}
      />
    )

    expect(screen.container.textContent).toContain('Running browser action')
    expect(screen.container.textContent).not.toContain('opaque-model-name')
    expect(screen.container.textContent).not.toContain('future.reviewed.tool')
  })
})
