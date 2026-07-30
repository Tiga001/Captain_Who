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
  'agent.approval.dialog.reject': 'No',
  'agent.approval.dialog.rejectPlaceholder': 'No, tell me how to adjust',
  'agent.approval.dialog.approve': 'Yes',
  'agent.mcp.approval.title': 'Approve external MCP tool?',
  'agent.mcp.approval.serverLabel': 'Server',
  'agent.mcp.approval.rawToolLabel': 'Raw tool',
  'agent.mcp.approval.state.expired': 'Expired',
  'agent.mcp.approval.state.payloadUnavailable': 'Approval payload unavailable',
  'agent.mcp.approval.state.policyDenied': 'Blocked by policy'
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
        displayReason: 'Read the workspace inventory',
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
  const button = [...container.querySelectorAll('button')].find((candidate) =>
    candidate.textContent?.trim().endsWith(label)
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
  it('routes typed MCP actions through the native approval shell and renders only reason and identity', async () => {
    const action = createMcpAction()
    const onApprove = vi.fn()
    const screen = await render(
      <AgentApprovalDialog
        target={{ action, messageId: 'assistant-message' }}
        onApprove={onApprove}
      />
    )

    expect(screen.container.querySelector('.agent-approval-dialog')).not.toBeNull()
    expect(
      screen.container.querySelector('.agent-approval-dialog')?.getAttribute('data-approval-kind')
    ).toBe('mcp')
    expect(formatToolDetails).not.toHaveBeenCalled()
    expect(screen.container.querySelector('img')).toBeNull()
    expect(screen.container.textContent).toContain('Read the workspace inventory')
    expect(screen.container.textContent).toContain('<img src=x onerror=alert(1)>Safe server')
    expect(screen.container.textContent).toContain('Raw tool: echo_text')
    expect(screen.container.textContent).not.toContain('Server claims read-only')
    expect(screen.container.textContent).not.toContain('Server ID')
    expect(screen.container.textContent).not.toContain('Encoded bytes')
    expect(screen.container.textContent).not.toContain('91')
    expect(screen.container.textContent).not.toContain(RAW_ARGUMENT_CANARY)
    expect(screen.container.textContent).not.toContain(PROPERTY_NAME_CANARY)
    expect(screen.container.textContent).not.toContain(DIGEST_CANARY)
    expect(screen.container.querySelector('details')).toBeNull()
    expect(screen.container.querySelector('[data-choice="remember"]')).toBeNull()
    expect(screen.container.textContent).not.toContain('Always Allow')

    const hiddenMarkup = [...screen.container.querySelectorAll('*')]
      .flatMap((element) => [...element.attributes])
      .map((attribute) => attribute.value)
      .join(' ')
    expect(hiddenMarkup).not.toContain(RAW_ARGUMENT_CANARY)
    expect(hiddenMarkup).not.toContain(PROPERTY_NAME_CANARY)
    expect(hiddenMarkup).not.toContain(DIGEST_CANARY)

    const approveButton = getButton(screen.container, 'Yes')
    approveButton.click()
    approveButton.click()
    expect(onApprove).toHaveBeenCalledTimes(1)
    expect(onApprove).toHaveBeenCalledWith('assistant-message', action)
    screen.unmount()
  })

  it('submits optional rejection guidance separately from approve and cancel', async () => {
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

    await screen
      .getByRole('textbox', { name: 'No, tell me how to adjust' })
      .fill('Use a different server')
    await screen.getByRole('button', { name: 'No' }).click()
    expect(onReject).toHaveBeenCalledWith('assistant-message', action, 'Use a different server')
    expect(onReject).toHaveBeenCalledTimes(1)
    expect(onCancel).not.toHaveBeenCalled()
    screen.unmount()
  })

  it('keeps cancellation separate on Escape without adding a third visible choice', async () => {
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
    const dialog = screen.container.querySelector('[data-approval-kind="mcp"]')
    if (!(dialog instanceof HTMLElement)) throw new Error('Expected MCP approval dialog')
    dialog.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'Escape' }))
    expect(onCancel).toHaveBeenCalledWith('assistant-message', action)
    expect(onCancel).toHaveBeenCalledOnce()
    expect(onReject).not.toHaveBeenCalled()
    expect([...screen.container.querySelectorAll('button')]).toHaveLength(2)
    screen.unmount()
  })

  it('disables approval at expiry using the local timer while leaving Host validation authoritative', async () => {
    const realNow = Date.now()
    const screen = await render(<ExpiryHarness now={realNow} />)

    vi.useFakeTimers()
    vi.setSystemTime(realNow)
    getButton(screen.container, 'Arm expiry').click()

    const approveButton = getButton(screen.container, 'Yes')
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

    expect(getButton(screen.container, 'Yes').disabled).toBe(true)
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

    expect(getButton(screen.container, 'Yes').disabled).toBe(true)
    expect(screen.container.textContent).toContain('Blocked by policy')
    expect(screen.container.textContent).not.toContain('Server claims read-only')
    screen.unmount()
  })
})
