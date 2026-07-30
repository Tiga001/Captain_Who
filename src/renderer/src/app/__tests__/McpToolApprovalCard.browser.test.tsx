import type {
  AgentMcpApprovalMode,
  AgentMcpToolInvocationState,
  AgentMcpToolRisk,
  AgentProposedAction
} from '@mycopilot/protocol'
import { act, useState } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { AgentApprovalDialog } from '../../features/chat/components/AgentApprovalDialog'

const RAW_ARGUMENT_CANARY = 'raw-argument-secret-canary'
const PROPERTY_NAME_CANARY = 'neutral-secret-property-canary'
const DIGEST_CANARY = 'f'.repeat(64)

const translations: Record<string, string> = {
  'agent.mcp.approval.title': 'Approve external MCP tool?',
  'agent.mcp.approval.externalBadge': 'External MCP',
  'agent.mcp.approval.serverLabel': 'Server',
  'agent.mcp.approval.serverIdLabel': 'Server ID',
  'agent.mcp.approval.scopeLabel': 'Scope',
  'agent.mcp.approval.scope.user': 'User',
  'agent.mcp.approval.rawToolLabel': 'Raw tool',
  'agent.mcp.approval.modelToolLabel': 'Model tool',
  'agent.mcp.approval.riskLabel': 'Risk',
  'agent.mcp.approval.risk.readOnlyClaimed': 'Server claims read-only; approval is still required',
  'agent.mcp.approval.argumentsTitle': 'Argument structure',
  'agent.mcp.approval.encodedBytes': 'Encoded bytes',
  'agent.mcp.approval.topLevelProperties': 'Top-level properties',
  'agent.mcp.approval.maxDepth': 'Maximum depth',
  'agent.mcp.approval.strings': 'Strings',
  'agent.mcp.approval.numbers': 'Numbers',
  'agent.mcp.approval.booleans': 'Booleans',
  'agent.mcp.approval.nulls': 'Nulls',
  'agent.mcp.approval.objects': 'Objects',
  'agent.mcp.approval.arrays': 'Arrays',
  'agent.mcp.approval.truncated': 'Summary was truncated',
  'agent.mcp.approval.createdAtLabel': 'Created',
  'agent.mcp.approval.expiresAtLabel': 'Expires',
  'agent.mcp.approval.payloadLabel': 'Recovery',
  'agent.mcp.approval.payload.processOnly': 'Available only in this app process',
  'agent.mcp.approval.payload.durable': 'Protected recovery is available',
  'agent.mcp.approval.policyPrompt': 'This external call requires a separate approval every time.',
  'agent.mcp.approval.state.pendingApproval': 'Waiting for approval',
  'agent.mcp.approval.state.expired': 'Expired',
  'agent.mcp.approval.state.payloadUnavailable': 'Approval payload unavailable',
  'agent.mcp.approval.state.policyDenied': 'Blocked by policy',
  'agent.mcp.approval.approve': 'Approve once',
  'agent.mcp.approval.cancel': 'Cancel call',
  'agent.mcp.approval.rejectingPlaceholder': 'Optional rejection guidance',
  'agent.mcp.approval.reject': 'Reject'
}

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'en-US',
    t: (key: string) => translations[key] ?? key
  })
}))

const { formatToolDetails } = vi.hoisted(() => ({
  formatToolDetails: vi.fn(() => {
    throw new Error('MCP approval must never use generic argument formatting')
  })
}))

vi.mock('../../features/chat/components/toolActivities/toolActivityUtils', () => ({
  formatToolDetails,
  getToolDisplayName: () => 'Generic tool'
}))

interface McpActionOptions {
  approvalMode?: AgentMcpApprovalMode
  expiresAt?: number
  risk?: AgentMcpToolRisk
}

function createMcpAction(
  options: McpActionOptions = {}
): Extract<AgentProposedAction, { type: 'mcp_tool_call' }> {
  const approvalMode = options.approvalMode ?? 'prompt'
  const expiresAt = options.expiresAt ?? Date.now() + 15 * 60 * 1000
  const risk = options.risk ?? 'read_only_claimed'
  return {
    type: 'mcp_tool_call',
    approval: {
      identity: {
        actionId: 'c2fd7f32-2ca4-4d34-a4c3-177d8fcb7457',
        invocationId: '5849f9ae-f7cb-4697-a56f-3622b823b08d',
        runId: 'run-safe-id',
        callId: 'tc1_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
        provenance: {
          serverId: '3ac3ec2b-3549-4dca-bf7d-298511b42523',
          scope: { type: 'user' },
          rawToolName: 'echo_text',
          modelToolName: 'mcp_safe_echo',
          configEpoch: '44b009b5-adc5-4f85-8e52-b362528101cc',
          registryRevision: 7,
          configDigest: DIGEST_CANARY,
          catalogGeneration: 4,
          catalogDigest: DIGEST_CANARY,
          catalogSchemaDigest: DIGEST_CANARY,
          schemaDigest: DIGEST_CANARY,
          schemaNormalizerVersion: 1
        }
      },
      call: {
        id: 'tc1_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
        tool: 'mcp_safe_echo',
        args: {
          [PROPERTY_NAME_CANARY]: RAW_ARGUMENT_CANARY
        },
        approvalStatus: 'required'
      },
      summary: {
        serverId: '3ac3ec2b-3549-4dca-bf7d-298511b42523',
        serverDisplayName: '<img src=x onerror=alert(1)>\u0000Safe server',
        scope: { type: 'user' },
        rawToolName: 'echo_text',
        modelToolName: 'mcp_safe_echo',
        arguments: {
          encodedBytes: 91,
          topLevelPropertyCount: 1,
          stringValueCount: 1,
          numberValueCount: 0,
          booleanValueCount: 0,
          nullValueCount: 0,
          objectValueCount: 1,
          arrayValueCount: 0,
          maxDepth: 2,
          truncated: true
        },
        risk,
        external: true
      },
      approvalMode,
      payloadPersistence: 'process_only',
      createdAt: expiresAt - 15 * 60 * 1000,
      expiresAt
    }
  }
}

function getButton(container: HTMLElement, label: string): HTMLButtonElement {
  const button = [...container.querySelectorAll('button')].find(
    (candidate) => candidate.textContent === label
  )
  if (!(button instanceof HTMLButtonElement)) {
    const labels = [...container.querySelectorAll('button')].map(
      (candidate) => candidate.textContent
    )
    throw new Error(`Expected button: ${label}; found ${labels.join(', ')}`)
  }
  return button
}

function ExpiryHarness({ now }: { now: number }) {
  const [expiresAt, setExpiresAt] = useState(now + 60_000)
  return (
    <>
      <button onClick={() => setExpiresAt(now + 1_000)} type="button">
        Arm expiry
      </button>
      <AgentApprovalDialog
        target={{
          action: createMcpAction({ expiresAt }),
          messageId: 'assistant-message'
        }}
        onApprove={vi.fn()}
      />
    </>
  )
}

afterEach(() => {
  vi.useRealTimers()
  vi.clearAllMocks()
})

describe('McpToolApprovalCard', () => {
  it('routes typed MCP actions before generic formatting and renders only safe structure data', async () => {
    const action = createMcpAction()
    const onApprove = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onApprove={onApprove}
      />
    )

    expect(screen.container.querySelector('.mcp-tool-approval-card')).not.toBeNull()
    expect(screen.container.querySelector('.agent-approval-dialog')).toBeNull()
    expect(formatToolDetails).not.toHaveBeenCalled()
    expect(screen.container.querySelector('img')).toBeNull()
    expect(screen.container.textContent).toContain('<img src=x onerror=alert(1)>Safe server')
    expect(screen.container.textContent).toContain('Server claims read-only')
    expect(screen.container.textContent).toContain('Encoded bytes')
    expect(screen.container.textContent).toContain('91')
    expect(screen.container.textContent).not.toContain(RAW_ARGUMENT_CANARY)
    expect(screen.container.textContent).not.toContain(PROPERTY_NAME_CANARY)
    expect(screen.container.textContent).not.toContain(DIGEST_CANARY)
    expect(screen.container.querySelector('[data-choice="remember"]')).toBeNull()
    expect(screen.container.textContent).not.toContain('Always Allow')

    const hiddenMarkup = [...screen.container.querySelectorAll('*')]
      .flatMap((element) => [...element.attributes])
      .map((attribute) => attribute.value)
      .join(' ')
    expect(hiddenMarkup).not.toContain(RAW_ARGUMENT_CANARY)
    expect(hiddenMarkup).not.toContain(PROPERTY_NAME_CANARY)
    expect(hiddenMarkup).not.toContain(DIGEST_CANARY)

    const approveButton = getButton(screen.container, 'Approve once')
    approveButton.click()
    approveButton.click()
    expect(onApprove).toHaveBeenCalledTimes(1)
    expect(onApprove).toHaveBeenCalledWith('assistant-message', action)
    screen.unmount()
  })

  it('submits rejection separately from approve and cancel', async () => {
    const action = createMcpAction()
    const onReject = vi.fn()
    const onCancel = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onCancel={onCancel}
        onReject={onReject}
      />
    )

    const input = screen.container.querySelector('input')
    if (!(input instanceof HTMLInputElement)) throw new Error('Expected rejection input')
    input.value = 'Do not use this external server'
    input.dispatchEvent(new Event('input', { bubbles: true }))
    getButton(screen.container, 'Reject').click()
    expect(onReject).toHaveBeenCalledOnce()
    expect(onCancel).not.toHaveBeenCalled()
    screen.unmount()
  })

  it('submits cancellation separately from approve and reject', async () => {
    const action = createMcpAction()
    const onReject = vi.fn()
    const onCancel = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onCancel={onCancel}
        onReject={onReject}
      />
    )
    getButton(screen.container, 'Cancel call').click()
    expect(onCancel).toHaveBeenCalledWith('assistant-message', action)
    expect(onCancel).toHaveBeenCalledOnce()
    expect(onReject).not.toHaveBeenCalled()
    screen.unmount()
  })

  it('disables approval at expiry using the local timer while leaving Host validation authoritative', async () => {
    const realNow = Date.now()
    const screen = await render(<ExpiryHarness now={realNow} />)

    vi.useFakeTimers()
    vi.setSystemTime(realNow)
    getButton(screen.container, 'Arm expiry').click()

    const approveButton = getButton(screen.container, 'Approve once')
    expect(approveButton.disabled).toBe(false)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000)
    })
    expect(approveButton.disabled).toBe(true)
    expect(screen.container.textContent).toContain('Expired')
    screen.unmount()
    vi.useRealTimers()
  })

  it.each<[AgentMcpToolInvocationState, string]>([
    ['payload_unavailable', 'Approval payload unavailable'],
    ['policy_denied', 'Blocked by policy']
  ])('fails closed for %s', async (invocationState, statusLabel) => {
    const screen = await render(
      <AgentApprovalDialog
        mcpInvocationState={invocationState}
        target={{ action: createMcpAction(), messageId: 'assistant-message' }}
        onApprove={vi.fn()}
      />
    )

    expect(getButton(screen.container, 'Approve once').disabled).toBe(true)
    expect(screen.container.textContent).toContain(statusLabel)
    screen.unmount()
  })

  it('fails closed when the Host policy is deny even for a claimed read-only tool', async () => {
    const screen = await render(
      <AgentApprovalDialog
        target={{
          action: createMcpAction({ approvalMode: 'deny' }),
          messageId: 'assistant-message'
        }}
        onApprove={vi.fn()}
      />
    )

    expect(getButton(screen.container, 'Approve once').disabled).toBe(true)
    expect(screen.container.textContent).toContain('Blocked by policy')
    expect(screen.container.textContent).toContain('Server claims read-only')
    screen.unmount()
  })
})
