import type { BrowserHostApi, HostApi, MyCopilotGlobal } from '@mycopilot/host-api'
import type { BrowserSurfaceCommand } from '@mycopilot/protocol'
import { useState } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import {
  resolveBrowserSurfaceHostApi,
  useBrowserSurfaceCommand
} from '../../browser/browserSurface'

const originalMyCopilot = Object.getOwnPropertyDescriptor(window, 'mycopilot')

afterEach(() => {
  if (originalMyCopilot) {
    Object.defineProperty(window, 'mycopilot', originalMyCopilot)
  } else {
    Reflect.deleteProperty(window, 'mycopilot')
  }
})

describe('browser surface Host bridge availability', () => {
  it('does not subscribe when the Electron Host API is absent', async () => {
    Reflect.deleteProperty(window, 'mycopilot')
    const openRightSidebar = vi.fn()
    const screen = await render(<Harness openRightSidebar={openRightSidebar} />)

    await expect.element(screen.getByTestId('command')).toHaveTextContent('none')
    expect(openRightSidebar).not.toHaveBeenCalled()
  })

  it('does not subscribe when an older Host mock has no browser bridge', async () => {
    exposeHost({})
    const openRightSidebar = vi.fn()
    const screen = await render(<Harness openRightSidebar={openRightSidebar} />)

    await expect.element(screen.getByTestId('command')).toHaveTextContent('none')
    expect(openRightSidebar).not.toHaveBeenCalled()
  })

  it('subscribes to a complete bridge and cleans up the exact listener', async () => {
    let listener: ((command: BrowserSurfaceCommand) => void) | undefined
    const unsubscribe = vi.fn()
    const browser: BrowserHostApi = {
      clearBrowsingData: vi.fn(async () => undefined),
      readArtifactPreview: vi.fn(async () => ({
        ok: false as const,
        error: { code: -32_001, message: 'not found' }
      })),
      onSurfaceCommand: vi.fn((nextListener) => {
        listener = nextListener
        return unsubscribe
      }),
      surfaceReady: vi.fn(async () => ({
        schemaVersion: 1 as const,
        accepted: true as const,
        surfaceId: 'right-sidebar-browser-test'
      })),
      surfaceSelected: vi.fn(async (input) => ({
        schemaVersion: 1 as const,
        accepted: true as const,
        surfaceId: input.surfaceId
      }))
    }
    exposeHost({ browser })
    const openRightSidebar = vi.fn()
    const screen = await render(<Harness openRightSidebar={openRightSidebar} />)

    expect(browser.onSurfaceCommand).toHaveBeenCalledTimes(1)
    listener?.({
      schemaVersion: 1,
      kind: 'ensureAttached',
      requestId: '11111111-1111-4111-8111-111111111111',
      surfaceId: 'right-sidebar-browser-test'
    })
    await expect.element(screen.getByTestId('command')).toHaveTextContent('ensureAttached')
    expect(openRightSidebar).toHaveBeenCalledTimes(1)

    screen.unmount()
    expect(unsubscribe).toHaveBeenCalledTimes(1)
  })

  it('rejects a present but malformed browser bridge', () => {
    exposeHost({
      browser: {
        clearBrowsingData: vi.fn(async () => undefined),
        onSurfaceCommand: 'not-a-function',
        surfaceReady: vi.fn()
      } as unknown as BrowserHostApi
    })

    expect(() => resolveBrowserSurfaceHostApi()).toThrow(
      'MyCopilot browser surface API is malformed'
    )
  })
})

function Harness({ openRightSidebar }: { openRightSidebar: () => void }) {
  const bridge = useBrowserSurfaceCommand(openRightSidebar)
  const [readyState, setReadyState] = useState('idle')
  return (
    <div>
      <output data-testid="command">{bridge.command?.kind ?? 'none'}</output>
      <output data-testid="ready">{readyState}</output>
      <button
        onClick={() => {
          void bridge
            .surfaceReady({
              schemaVersion: 1,
              requestId: '22222222-2222-4222-8222-222222222222',
              surfaceId: 'right-sidebar-browser-test'
            })
            .then(() => setReadyState('ready'))
        }}
        type="button"
      >
        ready
      </button>
    </div>
  )
}

function exposeHost(host: Partial<HostApi>): void {
  Object.defineProperty(window, 'mycopilot', {
    configurable: true,
    value: { host } as unknown as MyCopilotGlobal
  })
}
