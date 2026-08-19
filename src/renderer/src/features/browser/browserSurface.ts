import type { BrowserHostApi, HostApi } from '@mycopilot/host-api'
import { getHostApi } from '@mycopilot/host-api'
import type { BrowserSurfaceCommand, BrowserSurfaceReadyInput } from '@mycopilot/protocol'
import { useCallback, useEffect, useMemo, useState } from 'react'

export function browserSurfaceIdForPage(pageId: string): string {
  return `right-sidebar-browser-${pageId}`
}

/**
 * Keeps the Main-to-Renderer command channel scoped to AppShell. Strict parser enforcement lives
 * in Preload; this hook only translates reveal intent into shell layout state.
 */
export interface BrowserSurfaceBridge {
  command: BrowserSurfaceCommand | null
  surfaceReady: (input: BrowserSurfaceReadyInput) => Promise<void>
}

export function useBrowserSurfaceCommand(openRightSidebar: () => void): BrowserSurfaceBridge {
  const [command, setCommand] = useState<BrowserSurfaceCommand | null>(null)

  useEffect(() => {
    const browser = resolveBrowserSurfaceHostApi()
    if (!browser) return

    return browser.onSurfaceCommand((nextCommand) => {
      setCommand(nextCommand)
      if (nextCommand.kind !== 'closeSurface') openRightSidebar()
    })
  }, [openRightSidebar])

  const surfaceReady = useCallback(async (input: BrowserSurfaceReadyInput): Promise<void> => {
    const browser = resolveBrowserSurfaceHostApi()
    if (!browser) throw new Error('MyCopilot browser surface API is not available')
    await browser.surfaceReady(input)
  }, [])

  return useMemo(() => ({ command, surfaceReady }), [command, surfaceReady])
}

/**
 * Browser tests and non-Electron previews may intentionally have no Host API yet. Treat that as an
 * unavailable optional capability, while still rejecting a present-but-malformed bridge instead
 * of hiding a preload contract regression.
 */
export function resolveBrowserSurfaceHostApi(): BrowserHostApi | null {
  const maybeGlobal = globalThis as typeof globalThis & {
    window?: { mycopilot?: { host?: Partial<HostApi> } }
  }
  const exposedHost = maybeGlobal.window?.mycopilot?.host
  if (!exposedHost || exposedHost.browser === undefined) return null

  const browser = (getHostApi() as Partial<HostApi>).browser
  if (
    !browser ||
    typeof browser.onSurfaceCommand !== 'function' ||
    typeof browser.surfaceReady !== 'function' ||
    typeof browser.surfaceSelected !== 'function'
  ) {
    throw new Error('MyCopilot browser surface API is malformed')
  }
  return browser
}
