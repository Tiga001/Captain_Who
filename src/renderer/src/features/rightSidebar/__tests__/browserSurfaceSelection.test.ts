import type { BrowserHostApi } from '@mycopilot/host-api'
import type { BrowserSurfaceSelectedInput, BrowserSurfaceSelectedOutput } from '@mycopilot/protocol'
import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  clearBrowserSurfaceSelection,
  synchronizeBrowserSurfaceSelection
} from '../../browser/browserSurface'

afterEach(() => {
  vi.useRealTimers()
})

describe('Renderer browser surface selection synchronization', () => {
  it('retries startup registration, probes the Main instance, then binds the same revision', async () => {
    vi.useFakeTimers()
    let current = true
    const surfaceSelected = vi
      .fn<(input: BrowserSurfaceSelectedInput) => Promise<BrowserSurfaceSelectedOutput>>()
      .mockImplementationOnce(async (input) => negativeAck(input, 'not_registered'))
      .mockImplementationOnce(async (input) => instanceRequired(input, 'instance-startup-0001'))
      .mockImplementationOnce(async (input) => applied(input))
    const cancel = synchronizeBrowserSurfaceSelection(
      browserWith(surfaceSelected),
      { surfaceId: 'startup-surface', isCurrent: () => current },
      vi.fn()
    )

    await vi.waitFor(() => expect(surfaceSelected).toHaveBeenCalledTimes(1))
    await vi.advanceTimersByTimeAsync(20)
    await vi.waitFor(() => expect(surfaceSelected).toHaveBeenCalledTimes(3))

    const inputs = surfaceSelected.mock.calls.map(([input]) => input)
    expect(inputs.map((input) => input.surfaceInstanceId)).toEqual([
      null,
      null,
      'instance-startup-0001'
    ])
    expect(new Set(inputs.map((input) => input.selectionRevision)).size).toBe(1)
    current = false
    cancel()
  })

  it('never echoes an old StrictMode probe token after that exact webview is disposed', async () => {
    let resolveOld!: (output: BrowserSurfaceSelectedOutput) => void
    let oldCurrent = true
    const oldSurfaceSelected = vi.fn(
      (input: BrowserSurfaceSelectedInput) =>
        new Promise<BrowserSurfaceSelectedOutput>((resolve) => {
          resolveOld = (output) => resolve(output)
          expect(input.surfaceInstanceId).toBeNull()
        })
    )
    const cancelOld = synchronizeBrowserSurfaceSelection(
      browserWith(oldSurfaceSelected),
      { surfaceId: 'strict-aba', isCurrent: () => oldCurrent },
      vi.fn()
    )
    await vi.waitFor(() => expect(oldSurfaceSelected).toHaveBeenCalledTimes(1))

    oldCurrent = false
    cancelOld()
    resolveOld(instanceRequired(oldSurfaceSelected.mock.calls[0][0], 'instance-old-00001'))
    await Promise.resolve()
    expect(oldSurfaceSelected).toHaveBeenCalledTimes(1)

    const replacementSelected = vi
      .fn<(input: BrowserSurfaceSelectedInput) => Promise<BrowserSurfaceSelectedOutput>>()
      .mockImplementationOnce(async (input) => instanceRequired(input, 'instance-new-00001'))
      .mockImplementationOnce(async (input) => applied(input))
    const cancelReplacement = synchronizeBrowserSurfaceSelection(
      browserWith(replacementSelected),
      { surfaceId: 'strict-aba', isCurrent: () => true },
      vi.fn()
    )
    await vi.waitFor(() => expect(replacementSelected).toHaveBeenCalledTimes(2))
    expect(replacementSelected.mock.calls[1][0].surfaceInstanceId).toBe('instance-new-00001')
    cancelReplacement()
  })

  it('bounds not-registered retries and exposes transport errors', async () => {
    vi.useFakeTimers()
    const surfaceSelected = vi.fn(async (input: BrowserSurfaceSelectedInput) =>
      negativeAck(input, 'not_registered')
    )
    const onError = vi.fn()
    const cancel = synchronizeBrowserSurfaceSelection(
      browserWith(surfaceSelected),
      { surfaceId: 'missing-surface', isCurrent: () => true },
      onError
    )

    await vi.waitFor(() => expect(surfaceSelected).toHaveBeenCalledTimes(1))
    await vi.advanceTimersByTimeAsync(5_000)
    expect(surfaceSelected).toHaveBeenCalledTimes(6)
    await vi.advanceTimersByTimeAsync(5_000)
    expect(surfaceSelected).toHaveBeenCalledTimes(6)
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({
        message: 'Browser surface registration did not become available'
      })
    )
    cancel()

    const failure = new Error('selection transport failed')
    const rejected = vi.fn(async () => {
      throw failure
    })
    synchronizeBrowserSurfaceSelection(
      browserWith(rejected),
      { surfaceId: 'error-surface', isCurrent: () => true },
      onError
    )
    await vi.waitFor(() => expect(onError).toHaveBeenCalledWith(failure))
  })

  it('reports an explicit null selection when Browser has no foreground surface', async () => {
    const surfaceSelected = vi.fn(async (input: BrowserSurfaceSelectedInput) => applied(input))

    await clearBrowserSurfaceSelection(browserWith(surfaceSelected))

    expect(surfaceSelected).toHaveBeenCalledWith({
      schemaVersion: 1,
      surfaceId: null,
      surfaceInstanceId: null,
      selectionRevision: expect.any(Number)
    })
  })
})

function browserWith(
  surfaceSelected: (input: BrowserSurfaceSelectedInput) => Promise<BrowserSurfaceSelectedOutput>
): BrowserHostApi {
  return { surfaceSelected } as unknown as BrowserHostApi
}

function negativeAck(
  input: BrowserSurfaceSelectedInput,
  reason: 'not_registered'
): BrowserSurfaceSelectedOutput {
  return {
    schemaVersion: 1,
    status: 'noop',
    reason,
    retryable: true,
    surfaceId: input.surfaceId,
    surfaceInstanceId: null,
    selectionRevision: input.selectionRevision,
    authoritativeRevision: 0
  }
}

function instanceRequired(
  input: BrowserSurfaceSelectedInput,
  surfaceInstanceId: string
): BrowserSurfaceSelectedOutput {
  return {
    schemaVersion: 1,
    status: 'noop',
    reason: 'instance_required',
    retryable: true,
    surfaceId: input.surfaceId,
    surfaceInstanceId,
    selectionRevision: input.selectionRevision,
    authoritativeRevision: 0
  }
}

function applied(input: BrowserSurfaceSelectedInput): BrowserSurfaceSelectedOutput {
  return {
    schemaVersion: 1,
    status: 'applied',
    reason: 'selection_applied',
    retryable: false,
    surfaceId: input.surfaceId,
    surfaceInstanceId: input.surfaceInstanceId,
    selectionRevision: input.selectionRevision,
    authoritativeRevision: input.selectionRevision
  }
}
