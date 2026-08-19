import { HostInvocationError } from '@mycopilot/host-api'
import type {
  McpBuiltinCapabilityListItem,
  McpBuiltinCapabilityListOutput,
  McpBuiltinCapabilityMutationOutput
} from '@mycopilot/protocol'
import { StrictMode } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'

const service = vi.hoisted(() => ({
  list: vi.fn(),
  setAllowed: vi.fn()
}))

vi.mock('../mcpManagementClient', () => ({
  listMcpBuiltinCapabilities: service.list,
  setMcpBuiltinCapabilityAllowed: service.setAllowed
}))

const { useBuiltinMcpCapabilities } = await import('../useBuiltinMcpCapabilities')

function capability(
  overrides: Partial<McpBuiltinCapabilityListItem> = {}
): McpBuiltinCapabilityListItem {
  return {
    schemaVersion: 1,
    kind: 'builtinCapability',
    capabilityId: 'browser_automation',
    displayName: 'Browser automation',
    description: 'Request browser automation access.',
    userAllowed: false,
    policyVersion: 1,
    policyRevision: 0,
    ...overrides
  }
}

function output(
  item = capability(),
  revision = item.policyRevision
): McpBuiltinCapabilityListOutput {
  return { schemaVersion: 1, revision, capabilities: [item] }
}

function mutation(
  item: McpBuiltinCapabilityListItem,
  revision = item.policyRevision
): McpBuiltinCapabilityMutationOutput {
  return { schemaVersion: 1, revision, capability: item }
}

function Harness() {
  const management = useBuiltinMcpCapabilities()
  const item = management.state.output?.capabilities[0]
  return (
    <div>
      <output data-testid="status">{management.state.status}</output>
      <output data-testid="allowed">{item?.userAllowed ? 'allowed' : 'denied'}</output>
      <output data-testid="revision">{item?.policyRevision ?? -1}</output>
      <output data-testid="pending">{management.pendingCapabilities.size}</output>
      <button
        disabled={!item}
        onClick={() => {
          if (item) void management.setAllowed(item, true).catch(() => undefined)
        }}
        type="button"
      >
        allow
      </button>
      <button
        disabled={!item}
        onClick={() => {
          if (!item) return
          void management.setAllowed(item, true).catch(() => undefined)
          void management.setAllowed(item, true).catch(() => undefined)
        }}
        type="button"
      >
        allow twice
      </button>
      <button onClick={() => void management.refresh(true)} type="button">
        refresh
      </button>
    </div>
  )
}

beforeEach(() => {
  service.list.mockReset()
  service.setAllowed.mockReset()
  service.list.mockResolvedValue(output())
})

describe('useBuiltinMcpCapabilities', () => {
  it('does not retain focus refresh listeners after StrictMode unmount', async () => {
    const screen = await render(
      <StrictMode>
        <Harness />
      </StrictMode>
    )
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')
    const callsBeforeUnmount = service.list.mock.calls.length

    screen.unmount()
    window.dispatchEvent(new Event('focus'))
    await Promise.resolve()

    expect(service.list).toHaveBeenCalledTimes(callsBeforeUnmount)
  })

  it('loads the Host-owned capability policy without deriving runtime state', async () => {
    const screen = await render(<Harness />)

    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')
    await expect.element(screen.getByTestId('allowed')).toHaveTextContent('denied')
    expect(service.list).toHaveBeenCalledTimes(1)
  })

  it('uses the exact policy revision as the CAS precondition and applies the response', async () => {
    const allowed = capability({ userAllowed: true, policyRevision: 1 })
    service.setAllowed.mockResolvedValue(mutation(allowed, 1))
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')

    await screen.getByRole('button', { name: 'allow', exact: true }).click()

    await expect.element(screen.getByTestId('allowed')).toHaveTextContent('allowed')
    await expect.element(screen.getByTestId('revision')).toHaveTextContent('1')
    expect(service.setAllowed).toHaveBeenCalledWith({
      schemaVersion: 1,
      capabilityId: 'browser_automation',
      allowed: true,
      expectedPolicyRevision: 0
    })
  })

  it('admits only one in-flight mutation per capability', async () => {
    const pending = deferred<McpBuiltinCapabilityMutationOutput>()
    service.setAllowed.mockReturnValue(pending.promise)
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')

    await screen.getByRole('button', { name: 'allow twice' }).click()

    expect(service.setAllowed).toHaveBeenCalledTimes(1)
    await expect.element(screen.getByTestId('pending')).toHaveTextContent('1')
    pending.resolve(mutation(capability({ userAllowed: true, policyRevision: 1 }), 1))
    await expect.element(screen.getByTestId('pending')).toHaveTextContent('0')
  })

  it('fails a stale CAS closed and re-fetches authoritative policy state', async () => {
    service.setAllowed.mockRejectedValue(
      new HostInvocationError({
        message: 'Policy changed.',
        data: {
          schemaVersion: 1,
          type: 'mcpManagement',
          operation: 'setBuiltinCapabilityAllowed',
          code: 'conflict',
          recovery: 'refresh',
          message: 'Policy changed.',
          currentRegistryRevision: 1
        }
      })
    )
    service.list
      .mockResolvedValueOnce(output())
      .mockResolvedValueOnce(output(capability({ userAllowed: true, policyRevision: 1 }), 1))
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')

    await screen.getByRole('button', { name: 'allow', exact: true }).click()

    await expect.element(screen.getByTestId('allowed')).toHaveTextContent('allowed')
    expect(service.setAllowed).toHaveBeenCalledTimes(1)
    expect(service.list).toHaveBeenCalledTimes(2)
  })

  it('does not let an older focus refresh overwrite a committed mutation', async () => {
    const staleRefresh = deferred<McpBuiltinCapabilityListOutput>()
    const committedMutation = deferred<McpBuiltinCapabilityMutationOutput>()
    service.list.mockResolvedValueOnce(output()).mockReturnValueOnce(staleRefresh.promise)
    service.setAllowed.mockReturnValue(committedMutation.promise)
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')

    await screen.getByRole('button', { name: 'allow', exact: true }).click()
    window.dispatchEvent(new Event('focus'))
    await expect.poll(() => service.list.mock.calls.length).toBe(2)
    committedMutation.resolve(mutation(capability({ userAllowed: true, policyRevision: 1 }), 1))
    await expect.element(screen.getByTestId('allowed')).toHaveTextContent('allowed')
    staleRefresh.resolve(output())
    await expect.element(screen.getByTestId('allowed')).toHaveTextContent('allowed')
    await expect.element(screen.getByTestId('revision')).toHaveTextContent('1')
  })
})

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, reject, resolve }
}
