import { afterEach, describe, expect, it, vi } from 'vitest'
import type { BrowserRiskFailure } from '../browser/BrowserRiskCoordinator'
import { type ManagedMcpClient } from './ManagedPlaywrightMcpHost'
import { createManagedPlaywrightHostTestFixture } from './ManagedPlaywrightMcpHost.test-fixtures'

const {
  closeTrackedHosts,
  RISK_CONTEXT,
  PARENT_REQUEST_ID,
  fakeBrowserContext,
  fakeHost,
  riskLease
} = createManagedPlaywrightHostTestFixture()

afterEach(closeTrackedHosts)

describe('ManagedPlaywrightMcpHost', () => {
  it('acknowledges the exact dispatch boundary and authoritative response in order', async () => {
    const order: string[] = []
    const upstream = vi.fn<ManagedMcpClient['callTool']>(async () => {
      order.push('official-call')
      return { content: [{ type: 'text', text: 'snapshot' }], isError: false }
    })
    const onDispatchPhase = vi.fn(async (phase: string) => {
      order.push(phase)
      return true
    })
    const host = fakeHost({ callTool: upstream })

    await expect(
      host.callTool(
        'browser_snapshot',
        { call_reason: 'Verify the local dispatch acknowledgement.' },
        { onDispatchPhase }
      )
    ).resolves.toMatchObject({ isError: false })

    expect(order).toEqual(['possibly_dispatched', 'official-call', 'response_received'])
  })

  it('fails closed before the official handler when dispatch acknowledgement is rejected', async () => {
    const upstream = vi.fn<ManagedMcpClient['callTool']>(async () => ({
      content: [{ type: 'text', text: 'must not run' }],
      isError: false
    }))
    const host = fakeHost({ callTool: upstream })

    await expect(
      host.callTool(
        'browser_snapshot',
        { call_reason: 'Verify rejected dispatch acknowledgement.' },
        { onDispatchPhase: vi.fn(async () => false) }
      )
    ).rejects.toMatchObject({
      code: 'mcp.builtin_playwright.protocol_error',
      dispatchCertainty: 'definitely_not_dispatched'
    })
    expect(upstream).not.toHaveBeenCalled()
  })

  it('isolates persistent network state to one run and cleans it at run completion', async () => {
    const setOffline = vi.fn(async () => undefined)
    const route = vi.fn(async () => undefined)
    const unroute = vi.fn(async () => undefined)
    const context = fakeBrowserContext({ route, setOffline, unroute })
    const callTool = vi.fn(async () => ({
      content: [{ type: 'text', text: 'ok' }],
      isError: false
    }))
    const host = fakeHost({ callTool, getBrowserContext: async () => context })
    const runA = { ...RISK_CONTEXT, runId: 'run-a', callId: 'call-a' }
    const runB = { ...RISK_CONTEXT, runId: 'run-b', callId: 'call-b' }

    await host.callTool(
      'browser_network_state_set',
      { state: 'offline', call_reason: 'Test offline state.' },
      { authorizationContext: runA }
    )
    expect(setOffline).toHaveBeenLastCalledWith(true)
    await expect(
      host.callTool(
        'browser_network_state_set',
        { state: 'online', call_reason: 'Restore another run.' },
        { authorizationContext: runB }
      )
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.busy' })
    expect(setOffline).not.toHaveBeenCalledWith(false)

    await host.releaseRun('run-a')
    expect(setOffline).toHaveBeenLastCalledWith(false)
    await expect(
      host.callTool(
        'browser_navigate',
        { url: 'http://127.0.0.1/fixture', call_reason: 'Navigate after cleanup.' },
        { authorizationContext: runB }
      )
    ).resolves.toMatchObject({ isError: false })
  })

  it('preserves the fixed 0.0.79 route fulfill, header rewrite, list, and unroute semantics', async () => {
    type FixtureRouteHandler = (route: {
      fulfill(input: unknown): Promise<void>
      continue(input: unknown): Promise<void>
      request(): { headers(): Record<string, string> }
    }) => Promise<void>
    const contextRoute = vi.fn<(pattern: string, handler: FixtureRouteHandler) => Promise<void>>(
      async () => undefined
    )
    const contextUnroute = vi.fn<(pattern: string, handler: FixtureRouteHandler) => Promise<void>>(
      async () => undefined
    )
    const context = fakeBrowserContext({
      route: contextRoute as unknown as (...args: unknown[]) => Promise<void>,
      unroute: contextUnroute as unknown as (...args: unknown[]) => Promise<void>
    })
    const host = fakeHost({ getBrowserContext: async () => context })
    const authorizationContext = { ...RISK_CONTEXT, runId: 'route-run' }

    await expect(
      host.callTool(
        'browser_route',
        {
          pattern: '**/fixture-response',
          status: 201,
          body: 'super-secret-response-body',
          contentType: 'application/json',
          headers: ['X-Added: added-value'],
          removeHeaders: 'Authorization, X-Remove',
          call_reason: 'Mock the local fixture response.'
        },
        { authorizationContext }
      )
    ).resolves.toMatchObject({ isError: false })

    expect(contextRoute).toHaveBeenCalledTimes(1)
    expect(contextRoute.mock.calls[0][0]).toBe('**/fixture-response')
    const fulfillHandler = contextRoute.mock.calls[0][1]
    const fulfill = vi.fn(async () => undefined)
    const continueRequest = vi.fn(async () => undefined)
    await fulfillHandler({
      fulfill,
      continue: continueRequest,
      request: () => ({ headers: () => ({ authorization: 'original' }) })
    })
    expect(fulfill).toHaveBeenCalledWith({
      status: 201,
      contentType: 'application/json',
      body: 'super-secret-response-body'
    })
    expect(continueRequest).not.toHaveBeenCalled()

    const listed = await host.callTool(
      'browser_route_list',
      { call_reason: 'List the local fixture routes.' },
      { authorizationContext: { ...authorizationContext, callId: 'route-list-call' } }
    )
    expect(listed).toEqual({
      content: [
        {
          type: 'text',
          text: '1. **/fixture-response (status=201, body=super-secret-response-body, contentType=application/json, addHeaders={"X-Added":"added-value"}, removeHeaders=Authorization,X-Remove)'
        }
      ],
      isError: false
    })

    await expect(
      host.callTool(
        'browser_unroute',
        { pattern: '**/fixture-response', call_reason: 'Remove the response route.' },
        { authorizationContext: { ...authorizationContext, callId: 'route-unroute-call' } }
      )
    ).resolves.toMatchObject({ isError: false })
    expect(contextUnroute).toHaveBeenCalledWith('**/fixture-response', fulfillHandler)

    await host.callTool(
      'browser_route',
      {
        pattern: '**/fixture-request',
        headers: ['X-Added: replacement'],
        removeHeaders: 'Authorization, X-Remove',
        call_reason: 'Rewrite local fixture request headers.'
      },
      { authorizationContext: { ...authorizationContext, callId: 'route-header-call' } }
    )
    const rewriteHandler = contextRoute.mock.calls.at(-1)![1]
    const rewriteFulfill = vi.fn(async () => undefined)
    const rewriteContinue = vi.fn(async () => undefined)
    await rewriteHandler({
      fulfill: rewriteFulfill,
      continue: rewriteContinue,
      request: () => ({
        headers: () => ({ authorization: 'credential', 'x-remove': 'remove-me', keep: 'yes' })
      })
    })
    expect(rewriteFulfill).not.toHaveBeenCalled()
    expect(rewriteContinue).toHaveBeenCalledWith({
      headers: { keep: 'yes', 'X-Added': 'replacement' }
    })

    await expect(
      host.callTool(
        'browser_unroute',
        { call_reason: 'Remove every local fixture route.' },
        { authorizationContext: { ...authorizationContext, callId: 'route-unroute-all-call' } }
      )
    ).resolves.toEqual({
      content: [{ type: 'text', text: 'Removed all 1 route(s)' }],
      isError: false
    })
    expect(contextUnroute).toHaveBeenLastCalledWith('**/fixture-request', rewriteHandler)
    await expect(
      host.callTool(
        'browser_route_list',
        { call_reason: 'Confirm all local fixture routes were removed.' },
        { authorizationContext: { ...authorizationContext, callId: 'route-empty-list-call' } }
      )
    ).resolves.toEqual({
      content: [{ type: 'text', text: 'No active routes' }],
      isError: false
    })
  })

  it('preflights browser_navigate before upstream dispatch', async () => {
    const order: string[] = []
    const check = vi.fn(async () => {
      order.push('preflight')
    })
    const callTool = vi.fn(async () => {
      order.push('upstream')
      return { content: [{ type: 'text', text: 'ok' }], isError: false }
    })
    const risk = riskLease({ check })
    risk.markDispatched.mockImplementation(() => order.push('dispatched'))
    const host = fakeHost({
      callTool,
      beginNetworkOperation: vi.fn(async () => risk.lease)
    })

    await expect(
      host.callTool(
        'browser_navigate',
        { url: 'http://127.0.0.1:3000/', call_reason: 'Open the fixture.' },
        { authorizationContext: RISK_CONTEXT, parentRequestId: PARENT_REQUEST_ID }
      )
    ).resolves.toMatchObject({ isError: false })

    expect(order).toEqual(['preflight', 'dispatched', 'upstream'])
    expect(check).toHaveBeenCalledWith('http://127.0.0.1:3000/')
    expect(risk.markDispatched).toHaveBeenCalledOnce()
    expect(risk.finish).toHaveBeenCalledOnce()
  })

  it('preflights browser_navigate for a local file URL', async () => {
    const check = vi.fn(async () => undefined)
    const callTool = vi.fn(async () => ({
      content: [{ type: 'text', text: 'ok' }],
      isError: false
    }))
    const risk = riskLease({ check })
    const host = fakeHost({
      callTool,
      beginNetworkOperation: vi.fn(async () => risk.lease)
    })
    const url = 'file:///Users/docs/Predici%20.pdf'

    await expect(
      host.callTool(
        'browser_navigate',
        { url, call_reason: 'Open the local PDF in the managed browser.' },
        { authorizationContext: RISK_CONTEXT, parentRequestId: PARENT_REQUEST_ID }
      )
    ).resolves.toMatchObject({ isError: false })

    expect(check).toHaveBeenCalledWith(url)
    expect(callTool).toHaveBeenCalledWith(
      expect.objectContaining({
        name: 'browser_navigate',
        arguments: expect.objectContaining({ url })
      }),
      undefined,
      expect.anything()
    )
  })

  it('returns a rejected risk decision as a normal tool-level result without dispatch', async () => {
    const failure: BrowserRiskFailure = {
      code: 'browser.risk_rejected',
      dispatchCertainty: 'definitely_not_dispatched',
      reason: 'Use a public page instead.'
    }
    const risk = riskLease({
      check: vi.fn(async () => {
        throw new Error('blocked before dispatch')
      }),
      failure
    })
    const callTool = vi.fn(async () => ({ content: [], isError: false }))
    const host = fakeHost({
      callTool,
      beginNetworkOperation: vi.fn(async () => risk.lease)
    })

    await expect(
      host.callTool(
        'browser_navigate',
        { url: 'http://127.0.0.1:3000/', call_reason: 'Open the fixture.' },
        { authorizationContext: RISK_CONTEXT, parentRequestId: PARENT_REQUEST_ID }
      )
    ).resolves.toEqual({
      content: [
        {
          type: 'text',
          text: 'The user declined this browser destination or operation. Reason: Use a public page instead.'
        }
      ],
      structuredContent: {
        status: 'browser.risk_rejected',
        dispatchCertainty: 'definitely_not_dispatched',
        rejectionReason: 'Use a public page instead.'
      },
      isError: true
    })
    expect(callTool).not.toHaveBeenCalled()
    expect(risk.markDispatched).not.toHaveBeenCalled()
  })

  it('maps a post-dispatch webRequest refusal to outcome unknown', async () => {
    const failure: BrowserRiskFailure = {
      code: 'browser.risk_rejected',
      dispatchCertainty: 'possibly_dispatched'
    }
    const risk = riskLease({ failure })
    const host = fakeHost({
      callTool: vi.fn(async () => {
        throw new Error('net::ERR_BLOCKED_BY_CLIENT including an untrusted URL')
      }),
      beginNetworkOperation: vi.fn(async () => risk.lease)
    })

    await expect(
      host.callTool(
        'browser_click',
        { target: 'button', call_reason: 'Open the link.' },
        { authorizationContext: RISK_CONTEXT, parentRequestId: PARENT_REQUEST_ID }
      )
    ).rejects.toMatchObject({ code: 'browser.risk_outcome_unknown' })
    expect(risk.markDispatched).toHaveBeenCalledOnce()
  })

  it('does not settle a tool before an asynchronous boundary decision finishes', async () => {
    let release!: () => void
    let failure: BrowserRiskFailure | undefined
    const risk = riskLease({
      failure: () => failure,
      settle: async () =>
        await new Promise<void>((resolve) => {
          release = () => {
            failure = {
              code: 'browser.risk_rejected',
              dispatchCertainty: 'possibly_dispatched'
            }
            resolve()
          }
        })
    })
    const upstream = vi.fn(async () => ({
      content: [{ type: 'text', text: 'ok' }],
      isError: false
    }))
    const host = fakeHost({
      callTool: upstream,
      beginNetworkOperation: vi.fn(async () => risk.lease)
    })

    const pending = host.callTool(
      'browser_click',
      { target: 'risky link', call_reason: 'Open the link.' },
      { authorizationContext: RISK_CONTEXT, parentRequestId: PARENT_REQUEST_ID }
    )
    let settled = false
    void pending.then(
      () => {
        settled = true
      },
      () => {
        settled = true
      }
    )
    await vi.waitFor(() => expect(risk.settle).toHaveBeenCalledOnce())
    expect(upstream).toHaveBeenCalledOnce()
    expect(settled).toBe(false)

    release()
    await expect(pending).rejects.toMatchObject({ code: 'browser.risk_outcome_unknown' })
  })

  it('never auto-replays a possibly-dispatched browser operation', async () => {
    const risk = riskLease({
      failure: {
        code: 'browser.risk_outcome_unknown',
        dispatchCertainty: 'possibly_dispatched'
      }
    })
    const callTool = vi.fn(async () => ({ content: [], isError: false }))
    const host = fakeHost({
      callTool,
      beginNetworkOperation: vi.fn(async () => risk.lease)
    })

    await expect(
      host.callTool(
        'browser_click',
        { target: 'download', call_reason: 'Download the fixture.' },
        { authorizationContext: RISK_CONTEXT, parentRequestId: PARENT_REQUEST_ID }
      )
    ).rejects.toMatchObject({ code: 'browser.risk_outcome_unknown' })
    expect(callTool).toHaveBeenCalledOnce()
  })

  it('reports phase-accurate certainty before and after upstream dispatch', async () => {
    const surfaceUnavailable = fakeHost({
      beginNetworkOperation: vi.fn(async () => {
        throw Object.assign(new Error('safe fixture error'), {
          code: 'browser.surface_unavailable'
        })
      })
    })
    await expect(
      surfaceUnavailable.callTool(
        'browser_snapshot',
        { call_reason: 'Inspect the page.' },
        { authorizationContext: RISK_CONTEXT, parentRequestId: PARENT_REQUEST_ID }
      )
    ).rejects.toMatchObject({
      code: 'browser.surface_unavailable',
      dispatchCertainty: 'definitely_not_dispatched'
    })

    const controller = new AbortController()
    const upstream = vi.fn(
      async (_input, _schema, options) =>
        await new Promise((_resolve, reject) => {
          options?.signal?.addEventListener('abort', () => reject(new Error('aborted')), {
            once: true
          })
        })
    )
    const dispatched = fakeHost({
      beginNetworkOperation: vi.fn(async () => riskLease({}).lease),
      callTool: upstream
    })
    const pending = dispatched.callTool(
      'browser_snapshot',
      { call_reason: 'Inspect the page.' },
      {
        authorizationContext: RISK_CONTEXT,
        parentRequestId: PARENT_REQUEST_ID,
        signal: controller.signal
      }
    )
    await vi.waitFor(() => expect(upstream).toHaveBeenCalledOnce())
    controller.abort('task_cancelled')
    await expect(pending).rejects.toMatchObject({
      dispatchCertainty: 'possibly_dispatched'
    })
  })
})
