import type { AgentToolCall, AgentToolIdentity, AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { enUSTranslations } from '../../config/frontendTranslations.enUS'
import { zhCNTranslations } from '../../config/frontendTranslations.zhCN'
import type { ChatAgentRunView, ChatMcpToolInvocationView } from '../../features/chat/chatTypes'
import { AgentToolActivity } from '../../features/chat/components/toolActivities/AgentToolActivity'

const translations: Record<string, string> = {
  'mcp.builtin.browserAutomation.name': 'Browser automation',
  'agent.builtinCapability.activation.waiting': 'Waiting for approval to use {capability}',
  'agent.builtinCapability.activation.completed': 'Enabled {capability}',
  'agent.builtinCapability.activity.reason': 'Call reason',
  'agent.builtinCapability.artifact.list': 'Browser artifacts',
  'agent.builtinCapability.artifact.export': 'Export',
  'agent.builtinCapability.artifact.exported': 'Exported',
  'agent.builtinCapability.artifact.exportUnavailable': 'Export unavailable',
  'agent.builtinCapability.artifact.preview': 'Preview',
  'agent.builtinCapability.artifact.previewTruncated': 'Preview truncated',
  'agent.builtinCapability.artifact.previewUnavailable': 'Preview unavailable',
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
  'agent.builtinCapability.browser.tabs.completed': 'Managed browser tabs',
  'agent.builtinCapability.browser.waitFor.completed': 'Finished waiting for page',
  'agent.builtinCapability.browser.close.completed': 'Closed page',
  'agent.builtinCapability.browser.pageInteraction.completed': 'Interacted with page',
  'agent.builtinCapability.browser.hover.running': 'Hovering over page element',
  'agent.builtinCapability.browser.hover.completed': 'Hovered over page element',
  'agent.builtinCapability.browser.hover.failed': 'Failed to hover over page element',
  'agent.builtinCapability.browser.hover.cancelled': 'Cancelled hovering over page element',
  'agent.builtinCapability.browser.hover.outcomeUnknown':
    'Hover outcome uncertain; the action may have occurred',
  'agent.builtinCapability.browser.selectOption.running': 'Selecting page option',
  'agent.builtinCapability.browser.selectOption.completed': 'Selected page option',
  'agent.builtinCapability.browser.selectOption.failed': 'Failed to select page option',
  'agent.builtinCapability.browser.selectOption.cancelled': 'Cancelled selecting page option',
  'agent.builtinCapability.browser.selectOption.outcomeUnknown':
    'Option selection outcome uncertain; the selection may have changed',
  'agent.builtinCapability.browser.drag.running': 'Dragging page item',
  'agent.builtinCapability.browser.drag.completed': 'Dragged page item',
  'agent.builtinCapability.browser.drag.failed': 'Failed to drag page item',
  'agent.builtinCapability.browser.drag.cancelled': 'Cancelled dragging page item',
  'agent.builtinCapability.browser.drag.outcomeUnknown':
    'Drag outcome uncertain; the item may have moved',
  'agent.builtinCapability.browser.dialog.completed': 'Handled page dialog',
  'agent.builtinCapability.browser.navigateBack.completed': 'Went back a page',
  'agent.builtinCapability.browser.resize.completed': 'Resized browser page',
  'agent.builtinCapability.browser.screenshot.completed': 'Captured page screenshot',
  'agent.builtinCapability.browser.upload.completed': 'Uploaded file to page',
  'agent.builtinCapability.browser.upload.outcomeUnknown':
    'File upload outcome uncertain; the upload may have occurred',
  'agent.builtinCapability.browser.script.running': 'Running browser script',
  'agent.builtinCapability.browser.script.completed': 'Ran browser script',
  'agent.builtinCapability.browser.script.failed': 'Failed to run browser script',
  'agent.builtinCapability.browser.script.cancelled': 'Cancelled browser script',
  'agent.builtinCapability.browser.script.outcomeUnknown':
    'Browser script outcome uncertain; effects may have occurred',
  'agent.builtinCapability.browser.config.completed': 'Read browser configuration',
  'agent.builtinCapability.browser.console.completed': 'Read page console',
  'agent.builtinCapability.browser.networkRead.completed': 'Read page network activity',
  'agent.builtinCapability.browser.networkConfigure.completed': 'Configured page network',
  'agent.builtinCapability.browser.storageRead.completed': 'Read browser storage',
  'agent.builtinCapability.browser.storageWrite.completed': 'Updated browser storage',
  'agent.builtinCapability.browser.devtools.completed': 'Updated browser diagnostics',
  'agent.builtinCapability.browser.resume.completed': 'Resumed browser page',
  'agent.builtinCapability.browser.networkRules.completed': 'Read page network rules',
  'agent.builtinCapability.browser.recordStart.completed': 'Started browser recording',
  'agent.builtinCapability.browser.recordStop.completed': 'Stopped browser recording',
  'agent.builtinCapability.browser.recordEdit.completed': 'Updated browser recording',
  'agent.builtinCapability.browser.pdf.completed': 'Saved page as PDF',
  'agent.builtinCapability.browser.locator.completed': 'Generated page locator',
  'agent.builtinCapability.browser.verify.completed': 'Checked page state',
  'agent.builtinCapability.browser.fallback.running': 'Running browser action',
  'agent.builtinCapability.browser.fallback.completed': 'Completed browser action',
  'agent.builtinCapability.browser.fallback.failed': 'Browser action failed',
  'agent.builtinCapability.browser.fallback.cancelled': 'Cancelled browser action',
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
const hostMocks = vi.hoisted(() => ({
  exportArtifact: vi.fn(),
  readArtifactPreview: vi.fn()
}))
vi.mock('../../host/hostClient', () => ({
  hostClient: {
    browser: {
      exportArtifact: hostMocks.exportArtifact,
      readArtifactPreview: hostMocks.readArtifactPreview
    }
  }
}))

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

function identity(
  modelName = 'browser_navigate',
  toolId = modelName
): Extract<AgentToolIdentity, { type: 'builtin_capability' }> {
  return {
    type: 'builtin_capability',
    capabilityId: 'browser_automation',
    managedMcpId: 'builtin.browser_automation.mcp',
    packageName: '@playwright/mcp',
    packageVersion: '0.0.79',
    upstreamCatalogDigest: `sha256:${'1'.repeat(64)}`,
    policyDigest: `sha256:${'2'.repeat(64)}`,
    manifestDigest: `sha256:${'a'.repeat(64)}`,
    toolId,
    rawName: toolId,
    modelName,
    upstreamSchemaDigest: `sha256:${'3'.repeat(64)}`,
    hostOverlayDigest: `sha256:${'4'.repeat(64)}`,
    hostInputSchemaDigest: `sha256:${'5'.repeat(64)}`
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

  it('localizes activate_capability while its runtime identity is still missing', async () => {
    const activationCall: AgentToolCall = {
      id: 'call-browser-capability',
      tool: 'activate_capability',
      args: { capability: 'browser_automation', reason: CANARY },
      approvalStatus: 'required',
      reason: CANARY
    }
    const waiting = await render(
      <AgentToolActivity call={activationCall} run={run()} showImageGenerationPreview={false} />
    )

    expect(waiting.container.textContent).toContain(
      'Waiting for approval to use Browser automation'
    )
    expect(waiting.container.textContent).not.toContain('activate_capability')
    expect(waiting.container.textContent).not.toContain(CANARY)
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
      />
    )

    expect(completed.container.textContent).toContain('Enabled Browser automation')
    expect(completed.container.textContent).not.toContain('activate_capability')
    expect(completed.container.textContent).not.toContain(CANARY)
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

  it('uses the canonical raw name when the durable builtin toolId is namespaced', async () => {
    const toolIdentity = {
      ...identity('browser_evaluate', 'browser.evaluate'),
      rawName: 'browser_evaluate'
    } satisfies AgentToolIdentity
    const screen = await render(
      <AgentToolActivity
        call={call('browser_evaluate')}
        result={{
          callId: 'call-browser-capability',
          tool: 'browser_evaluate',
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
        toolIdentity={toolIdentity}
      />
    )

    expect(screen.container.textContent).toContain('Ran browser script')
    expect(screen.container.textContent).not.toContain('browser_evaluate')
    expect(screen.container.textContent).not.toContain('browser.evaluate')
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

  it('keeps external MCP lifecycle authoritative over the browser_* presentation fallback', async () => {
    const screen = await render(
      <AgentToolActivity
        call={call('browser_evaluate')}
        mcpInvocation={staleExternalInvocation()}
        run={run()}
        showImageGenerationPreview={false}
      />
    )

    expect(screen.container.querySelector('.mcp-tool-activity')).not.toBeNull()
    expect(screen.container.querySelector('.builtin-capability-tool-activity')).toBeNull()
  })

  it.each([
    ['running', undefined, undefined, 'Running browser script'],
    ['completed', { ok: true, status: 'completed' }, undefined, 'Ran browser script'],
    ['failed', { ok: false, status: 'failed' }, undefined, 'Failed to run browser script'],
    ['cancelled', undefined, 'cancelled', 'Cancelled browser script'],
    [
      'outcomeUnknown',
      { ok: false, status: 'outcome_unknown' },
      undefined,
      'Browser script outcome uncertain; effects may have occurred'
    ]
  ] as const)(
    'uses specific browser script copy for unregistered browser_evaluate in the %s state',
    async (_statusName, projected, settledStatus, expected) => {
      const toolId = 'browser_evaluate'
      const result = projected
        ? ({
            callId: 'call-browser-capability',
            tool: toolId,
            ok: projected.ok,
            result: {
              schemaVersion: 1,
              type: 'builtin_capability_tool',
              status: projected.status,
              contentOmitted: true
            }
          } satisfies AgentToolResult)
        : undefined
      const screen = await render(
        <AgentToolActivity
          call={call(toolId)}
          result={result}
          run={run()}
          settledStatus={settledStatus}
          showImageGenerationPreview={false}
          toolIdentity={{ type: 'unregistered', toolName: toolId }}
        />
      )

      expect(screen.container.textContent).toContain(expected)
      expect(screen.container.textContent).not.toContain(toolId)
    }
  )

  it.each([
    ['running', undefined, undefined, 'Running browser action'],
    ['completed', { ok: true, status: 'completed' }, undefined, 'Completed browser action'],
    ['failed', { ok: false, status: 'failed' }, undefined, 'Browser action failed'],
    ['cancelled', undefined, 'cancelled', 'Cancelled browser action'],
    [
      'outcomeUnknown',
      { ok: false, status: 'outcome_unknown' },
      undefined,
      'Browser action outcome uncertain; the action may have occurred'
    ]
  ] as const)(
    'uses generic browser copy for identity-less future browser Tool in the %s state',
    async (_statusName, projected, settledStatus, expected) => {
      const toolId = 'browser_future_tool'
      const result = projected
        ? ({
            callId: 'call-browser-capability',
            tool: toolId,
            ok: projected.ok,
            result: {
              schemaVersion: 1,
              type: 'builtin_capability_tool',
              status: projected.status,
              contentOmitted: true
            }
          } satisfies AgentToolResult)
        : undefined
      const screen = await render(
        <AgentToolActivity
          call={call(toolId)}
          result={result}
          run={run()}
          settledStatus={settledStatus}
          showImageGenerationPreview={false}
        />
      )

      expect(screen.container.textContent).toContain(expected)
      expect(screen.container.textContent).not.toContain(toolId)
    }
  )

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

  it('renders only safe Artifact metadata and reads a text preview on explicit request', async () => {
    const artifact = {
      schemaVersion: 1,
      artifactId: 'browser-artifact:123e4567-e89b-42d3-a456-426614174000',
      kind: 'snapshot',
      displayName: 'page-snapshot.txt',
      mimeType: 'text/plain',
      sizeBytes: 13,
      createdAt: 1_000,
      expiresAt: 2_000,
      lifecycle: 'run',
      owner: 'browser_automation',
      preview: 'text'
    } as const
    hostMocks.readArtifactPreview.mockResolvedValueOnce({
      ok: true,
      value: {
        schemaVersion: 1,
        artifact,
        bytes: new TextEncoder().encode('safe snapshot')
      }
    })
    const screen = await render(
      <AgentToolActivity
        call={call('browser_snapshot')}
        result={{
          callId: 'call-browser-capability',
          tool: 'browser_snapshot',
          ok: true,
          result: {
            schemaVersion: 1,
            type: 'builtin_capability_tool',
            status: 'completed',
            contentOmitted: true,
            artifacts: [artifact]
          }
        }}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={identity('browser_snapshot', 'browser_snapshot')}
      />
    )
    screen.container.querySelector('summary')?.click()
    expect(screen.container.textContent).toContain('page-snapshot.txt')
    expect(screen.container.querySelector('b')).toBeNull()
    expect(screen.container.textContent).toContain('text/plain · 13 B')
    expect(hostMocks.readArtifactPreview).not.toHaveBeenCalled()
    const previewButton = [...screen.container.querySelectorAll('button')].find(
      (button) => button.textContent === 'Preview'
    )
    expect(previewButton).toBeDefined()
    previewButton?.click()
    await expect.poll(() => screen.container.textContent).toContain('safe snapshot')
    expect(hostMocks.readArtifactPreview).toHaveBeenCalledWith({ schemaVersion: 1, artifact })
    expect(screen.container.textContent).not.toContain('managedPath')
  })

  it('exports a preview-disabled Artifact explicitly and treats dialog cancellation normally', async () => {
    const artifact = {
      schemaVersion: 1,
      artifactId: 'browser-artifact:123e4567-e89b-42d3-a456-426614174000',
      kind: 'json',
      displayName: 'storage-state.json',
      mimeType: 'application/json',
      sizeBytes: 42,
      createdAt: 1_000,
      expiresAt: 2_000,
      lifecycle: 'run',
      owner: 'browser_automation',
      preview: 'none'
    } as const
    let resolveCancellation!: (value: {
      ok: true
      value: { schemaVersion: 1; status: 'cancelled' }
    }) => void
    hostMocks.exportArtifact
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveCancellation = resolve
          })
      )
      .mockResolvedValueOnce({
        ok: true,
        value: { schemaVersion: 1, status: 'exported', displayName: 'user-copy.json' }
      })
      .mockResolvedValueOnce({
        ok: false,
        error: { message: '/private/secret/export-path' }
      })
    const screen = await render(
      <AgentToolActivity
        call={call('browser_storage_state')}
        result={{
          callId: 'call-browser-capability',
          tool: 'browser_storage_state',
          ok: true,
          result: {
            schemaVersion: 1,
            type: 'builtin_capability_tool',
            status: 'completed',
            contentOmitted: true,
            artifacts: [artifact]
          }
        }}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={identity('browser_storage_state', 'browser_storage_state')}
      />
    )
    screen.container.querySelector('summary')?.click()
    expect(screen.container.textContent).not.toContain('Preview')

    const exportButton = [...screen.container.querySelectorAll('button')].find(
      (button) => button.textContent === 'Export'
    )
    expect(exportButton).toBeDefined()
    const callsBeforeExport = hostMocks.exportArtifact.mock.calls.length
    exportButton?.click()
    exportButton?.click()
    await expect.poll(() => hostMocks.exportArtifact.mock.calls.length).toBe(callsBeforeExport + 1)
    expect(hostMocks.exportArtifact).toHaveBeenLastCalledWith({ schemaVersion: 1, artifact })
    resolveCancellation({ ok: true, value: { schemaVersion: 1, status: 'cancelled' } })
    await expect.poll(() => exportButton?.textContent).toBe('Export')
    expect(screen.container.textContent).not.toContain('Export unavailable')

    exportButton?.click()
    await expect.poll(() => exportButton?.textContent).toBe('Exported')
    expect(screen.container.textContent).not.toContain('user-copy.json')

    exportButton?.click()
    await expect.poll(() => screen.container.textContent).toContain('Export unavailable')
    expect(screen.container.textContent).not.toContain('/private/secret/export-path')
  })

  it.each([`<img-${CANARY}>.png`, `../${CANARY}.png`])(
    'drops an unsafe Artifact display name instead of adding it to the DOM',
    async (displayName) => {
      const screen = await render(
        <AgentToolActivity
          call={call('browser_take_screenshot')}
          result={{
            callId: 'call-browser-capability',
            tool: 'browser_take_screenshot',
            ok: true,
            result: {
              schemaVersion: 1,
              type: 'builtin_capability_tool',
              status: 'completed',
              contentOmitted: true,
              artifacts: [
                {
                  schemaVersion: 1,
                  artifactId: 'browser-artifact:123e4567-e89b-42d3-a456-426614174000',
                  kind: 'image',
                  displayName,
                  mimeType: 'image/png',
                  sizeBytes: 1,
                  createdAt: 1_000,
                  expiresAt: 2_000,
                  lifecycle: 'run',
                  owner: 'browser_automation',
                  preview: 'image'
                }
              ]
            }
          }}
          run={run()}
          showImageGenerationPreview={false}
          toolIdentity={identity('browser_take_screenshot', 'browser_take_screenshot')}
        />
      )
      expect(screen.container.querySelector('.browser-artifact-list')).toBeNull()
      expect(screen.container.textContent).not.toContain(CANARY)
    }
  )

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
    [
      'rejected',
      {
        callId: 'call-browser-capability',
        tool: 'browser_click',
        ok: true,
        result: {
          schemaVersion: 1,
          type: 'builtin_capability_tool',
          status: 'rejected',
          contentOmitted: true
        }
      } satisfies AgentToolResult,
      undefined,
      'Cancelled clicking page'
    ],
    [
      'expired',
      {
        callId: 'call-browser-capability',
        tool: 'browser_click',
        ok: false,
        result: {
          schemaVersion: 1,
          type: 'builtin_capability_tool',
          status: 'expired',
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

  it('removes control characters and bounds the only expanded call_reason field', async () => {
    const visiblePrefix = 'R'.repeat(512)
    const reason = `${visiblePrefix}\u202e${CANARY}`
    const screen = await render(
      <AgentToolActivity
        call={call('browser_click', reason)}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={identity('browser_click', 'browser_click')}
      />
    )

    expect(screen.container.textContent).toContain(visiblePrefix)
    expect(screen.container.textContent).not.toContain('\u202e')
    expect(screen.container.textContent).not.toContain(CANARY)
    expect(screen.container.querySelector('details')).not.toBeNull()
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
    ['browser_tabs', 'Managed browser tabs'],
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

  it.each([
    ['browser_annotate', 'Updated browser diagnostics'],
    ['browser_console_messages', 'Read page console'],
    ['browser_cookie_clear', 'Updated browser storage'],
    ['browser_cookie_delete', 'Updated browser storage'],
    ['browser_cookie_get', 'Read browser storage'],
    ['browser_cookie_list', 'Read browser storage'],
    ['browser_cookie_set', 'Updated browser storage'],
    ['browser_drag', 'Dragged page item'],
    ['browser_drop', 'Dragged page item'],
    ['browser_evaluate', 'Ran browser script'],
    ['browser_file_upload', 'Uploaded file to page'],
    ['browser_generate_locator', 'Generated page locator'],
    ['browser_get_config', 'Read browser configuration'],
    ['browser_handle_dialog', 'Handled page dialog'],
    ['browser_hide_highlight', 'Updated browser diagnostics'],
    ['browser_highlight', 'Updated browser diagnostics'],
    ['browser_hover', 'Hovered over page element'],
    ['browser_localstorage_clear', 'Updated browser storage'],
    ['browser_localstorage_delete', 'Updated browser storage'],
    ['browser_localstorage_get', 'Read browser storage'],
    ['browser_localstorage_list', 'Read browser storage'],
    ['browser_localstorage_set', 'Updated browser storage'],
    ['browser_mouse_click_xy', 'Interacted with page'],
    ['browser_mouse_down', 'Interacted with page'],
    ['browser_mouse_drag_xy', 'Interacted with page'],
    ['browser_mouse_move_xy', 'Interacted with page'],
    ['browser_mouse_up', 'Interacted with page'],
    ['browser_mouse_wheel', 'Interacted with page'],
    ['browser_navigate_back', 'Went back a page'],
    ['browser_network_request', 'Read page network activity'],
    ['browser_network_requests', 'Read page network activity'],
    ['browser_network_state_set', 'Configured page network'],
    ['browser_pdf_save', 'Saved page as PDF'],
    ['browser_resize', 'Resized browser page'],
    ['browser_resume', 'Resumed browser page'],
    ['browser_route', 'Configured page network'],
    ['browser_route_list', 'Read page network rules'],
    ['browser_run_code_unsafe', 'Ran browser script'],
    ['browser_select_option', 'Selected page option'],
    ['browser_sessionstorage_clear', 'Updated browser storage'],
    ['browser_sessionstorage_delete', 'Updated browser storage'],
    ['browser_sessionstorage_get', 'Read browser storage'],
    ['browser_sessionstorage_list', 'Read browser storage'],
    ['browser_sessionstorage_set', 'Updated browser storage'],
    ['browser_set_storage_state', 'Updated browser storage'],
    ['browser_start_tracing', 'Started browser recording'],
    ['browser_start_video', 'Started browser recording'],
    ['browser_stop_tracing', 'Stopped browser recording'],
    ['browser_stop_video', 'Stopped browser recording'],
    ['browser_storage_state', 'Read browser storage'],
    ['browser_take_screenshot', 'Captured page screenshot'],
    ['browser_unroute', 'Configured page network'],
    ['browser_verify_element_visible', 'Checked page state'],
    ['browser_verify_list_visible', 'Checked page state'],
    ['browser_verify_text_visible', 'Checked page state'],
    ['browser_verify_value', 'Checked page state'],
    ['browser_video_chapter', 'Updated browser recording'],
    ['browser_video_hide_actions', 'Updated browser recording'],
    ['browser_video_show_actions', 'Updated browser recording']
  ])('renders expanded tool %s through its typed toolId family', async (toolId, expected) => {
    const screen = await render(
      <AgentToolActivity
        call={call('opaque-model-name')}
        result={{
          callId: 'call-browser-capability',
          tool: 'opaque-model-name',
          ok: true,
          result: {
            schemaVersion: 1,
            type: 'builtin_capability_tool',
            status: 'completed',
            contentOmitted: true,
            rawResult: CANARY
          }
        }}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={identity('opaque-model-name', toolId)}
      />
    )

    expect(screen.container.textContent).toContain(expected)
    expect(screen.container.textContent).not.toContain(toolId)
    expect(screen.container.textContent).not.toContain(CANARY)
    expect(screen.container.querySelector('pre')).toBeNull()
  })

  it.each([
    ['browser_hover', 'running', undefined, undefined, 'Hovering over page element'],
    [
      'browser_hover',
      'failed',
      { ok: false, status: 'failed' },
      undefined,
      'Failed to hover over page element'
    ],
    ['browser_hover', 'cancelled', undefined, 'cancelled', 'Cancelled hovering over page element'],
    [
      'browser_hover',
      'outcomeUnknown',
      { ok: false, status: 'outcome_unknown' },
      undefined,
      'Hover outcome uncertain; the action may have occurred'
    ],
    ['browser_select_option', 'running', undefined, undefined, 'Selecting page option'],
    [
      'browser_select_option',
      'failed',
      { ok: false, status: 'failed' },
      undefined,
      'Failed to select page option'
    ],
    [
      'browser_select_option',
      'cancelled',
      undefined,
      'cancelled',
      'Cancelled selecting page option'
    ],
    [
      'browser_select_option',
      'outcomeUnknown',
      { ok: false, status: 'outcome_unknown' },
      undefined,
      'Option selection outcome uncertain; the selection may have changed'
    ],
    ['browser_drag', 'running', undefined, undefined, 'Dragging page item'],
    [
      'browser_drag',
      'failed',
      { ok: false, status: 'failed' },
      undefined,
      'Failed to drag page item'
    ],
    ['browser_drag', 'cancelled', undefined, 'cancelled', 'Cancelled dragging page item'],
    [
      'browser_drag',
      'outcomeUnknown',
      { ok: false, status: 'outcome_unknown' },
      undefined,
      'Drag outcome uncertain; the item may have moved'
    ]
  ] as const)(
    'renders %s %s with dedicated copy',
    async (toolId, _statusName, projected, settledStatus, expected) => {
      const result = projected
        ? ({
            callId: 'call-browser-capability',
            tool: 'opaque-model-name',
            ok: projected.ok,
            result: {
              schemaVersion: 1,
              type: 'builtin_capability_tool',
              status: projected.status,
              contentOmitted: true
            }
          } satisfies AgentToolResult)
        : undefined
      const screen = await render(
        <AgentToolActivity
          call={call('opaque-model-name')}
          result={result}
          run={run()}
          settledStatus={settledStatus}
          showImageGenerationPreview={false}
          toolIdentity={identity('opaque-model-name', toolId)}
        />
      )

      expect(screen.container.textContent).toContain(expected)
      expect(screen.container.textContent).not.toContain(toolId)
      expect(screen.container.textContent).not.toContain(CANARY)
      if (_statusName === 'outcomeUnknown') {
        expect(screen.container.querySelector('button')).toBeNull()
        expect(screen.container.querySelector('a')).toBeNull()
      }
    }
  )

  it('localizes representative expanded browser categories in Chinese and English', () => {
    expect(zhCNTranslations['agent.builtinCapability.browser.screenshot.completed']).toBe(
      '已截取网页'
    )
    expect(enUSTranslations['agent.builtinCapability.browser.screenshot.completed']).toBe(
      'Captured page screenshot'
    )
    expect(zhCNTranslations['agent.builtinCapability.browser.storageWrite.completed']).toBe(
      '已更新浏览器存储'
    )
    expect(enUSTranslations['agent.builtinCapability.browser.storageWrite.completed']).toBe(
      'Updated browser storage'
    )
    expect(zhCNTranslations['agent.builtinCapability.browser.upload.completed']).toBe(
      '已向网页上传文件'
    )
    expect(enUSTranslations['agent.builtinCapability.browser.upload.completed']).toBe(
      'Uploaded file to page'
    )
  })

  it('renders an expanded OutcomeUnknown state without exposing a retry control or payload', async () => {
    const screen = await render(
      <AgentToolActivity
        call={call('opaque-model-name', 'Upload the selected file.')}
        result={{
          callId: 'call-browser-capability',
          tool: 'opaque-model-name',
          ok: false,
          result: {
            schemaVersion: 1,
            type: 'builtin_capability_tool',
            status: 'outcome_unknown',
            contentOmitted: true,
            rawUrl: `https://${CANARY}.invalid`,
            cdpEndpoint: CANARY
          },
          error: CANARY
        }}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={identity('opaque-model-name', 'browser_file_upload')}
      />
    )

    expect(screen.container.textContent).toContain(
      'File upload outcome uncertain; the upload may have occurred'
    )
    expect(screen.container.textContent).toContain('Upload the selected file.')
    expect(screen.container.textContent).not.toContain(CANARY)
    expect(screen.container.querySelector('button')).toBeNull()
    expect(screen.container.querySelector('a')).toBeNull()
    expect(screen.container.querySelector('pre')).toBeNull()
  })

  it.each([
    ['running', undefined, undefined, 'Running browser action'],
    ['completed', { ok: true, status: 'completed' }, undefined, 'Completed browser action'],
    ['failed', { ok: false, status: 'failed' }, undefined, 'Browser action failed'],
    ['cancelled', undefined, 'cancelled', 'Cancelled browser action'],
    [
      'outcomeUnknown',
      { ok: false, status: 'outcome_unknown' },
      undefined,
      'Browser action outcome uncertain; the action may have occurred'
    ]
  ] as const)(
    'uses a safe fallback for an unknown browser_* Tool in the %s state',
    async (_statusName, projected, settledStatus, expected) => {
      const toolId = 'browser_future_reviewed_tool'
      const result = projected
        ? ({
            callId: 'call-browser-capability',
            tool: 'opaque-model-name',
            ok: projected.ok,
            result: {
              schemaVersion: 1,
              type: 'builtin_capability_tool',
              status: projected.status,
              contentOmitted: true
            }
          } satisfies AgentToolResult)
        : undefined
      const internalIdentity = {
        ...identity('opaque-model-name', toolId),
        packageName: '@private/cdp-canary',
        rawName: 'private_cdp_canary'
      } satisfies AgentToolIdentity
      const screen = await render(
        <AgentToolActivity
          call={call('opaque-model-name')}
          result={result}
          run={run()}
          settledStatus={settledStatus}
          showImageGenerationPreview={false}
          toolIdentity={internalIdentity}
        />
      )

      expect(screen.container.textContent).toContain(expected)
      expect(screen.container.textContent).not.toContain('opaque-model-name')
      expect(screen.container.textContent).not.toContain(toolId)
      expect(screen.container.textContent).not.toContain('private_cdp_canary')
      expect(screen.container.textContent).not.toContain('@private/cdp-canary')
    }
  )

  it('ships the generic browser fallback copy in every locale for every state', () => {
    const fallbackKeys = [
      'agent.builtinCapability.browser.fallback.running',
      'agent.builtinCapability.browser.fallback.completed',
      'agent.builtinCapability.browser.fallback.failed',
      'agent.builtinCapability.browser.fallback.cancelled',
      'agent.builtinCapability.browser.fallback.outcomeUnknown'
    ] as const

    for (const translations of [zhCNTranslations, enUSTranslations]) {
      for (const key of fallbackKeys) {
        expect(translations[key]).toBeTruthy()
        expect(translations[key]).not.toBe(key)
        expect(translations[key]).not.toContain('browser_')
      }
    }
    expect(zhCNTranslations['agent.builtinCapability.browser.fallback.completed']).toBe(
      '已执行浏览器操作'
    )
  })

  it('uses a safe fallback for a future reviewed non-browser Tool without exposing its raw name', async () => {
    const internalIdentity = {
      ...identity('opaque-model-name', 'future.reviewed.tool'),
      packageName: '@private/cdp-canary',
      rawName: 'private_cdp_canary'
    } satisfies AgentToolIdentity
    const screen = await render(
      <AgentToolActivity
        call={call('opaque-model-name')}
        run={run()}
        showImageGenerationPreview={false}
        toolIdentity={internalIdentity}
      />
    )

    expect(screen.container.textContent).toContain('Running browser action')
    expect(screen.container.textContent).not.toContain('opaque-model-name')
    expect(screen.container.textContent).not.toContain('future.reviewed.tool')
    expect(screen.container.textContent).not.toContain('private_cdp_canary')
    expect(screen.container.textContent).not.toContain('@private/cdp-canary')
  })
})
