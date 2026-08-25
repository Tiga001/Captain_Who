import type { BrowserHostApi, HostApi } from '@mycopilot/host-api'
import { getHostApi } from '@mycopilot/host-api'
import type {
  BrowserSurfaceCommand,
  BrowserSurfaceReadyInput,
  BrowserSurfaceReadyOutput
} from '@mycopilot/protocol'
import { useCallback, useEffect, useMemo, useState } from 'react'

export function browserSurfaceIdForPage(pageId: string): string {
  return `right-sidebar-browser-${pageId}`
}

const SURFACE_REGISTRATION_RETRY_DELAYS_MS = [20, 50, 100, 250, 500] as const
const MAX_SELECTION_RESYNCS = 2
let lastBrowserSurfaceSelectionRevision = Math.max(1, Date.now())

interface BrowserSurfaceSelectionTarget {
  isCurrent: () => boolean
  surfaceId: string
}

interface BrowserSurfaceInstanceTarget {
  isCurrent: () => boolean
  onInstance: (surfaceInstanceId: string) => void
  surfaceId: string
}

/**
 * Resolves Main's opaque incarnation for one exact Renderer webview without selecting it. This is
 * used by both background automation pages and foreground pages, so readiness and close commands
 * never rely on a surfaceId-only ABA identity.
 */
export function synchronizeBrowserSurfaceInstance(
  browser: BrowserHostApi,
  target: BrowserSurfaceInstanceTarget,
  onError: (error: unknown) => void = reportBrowserSurfaceSelectionError
): () => void {
  let cancelled = false
  let retryTimer: ReturnType<typeof setTimeout> | undefined
  let retryIndex = 0
  let resyncCount = 0
  let selectionRevision = nextBrowserSurfaceSelectionRevision()

  const isCurrent = (): boolean => !cancelled && target.isCurrent()
  const schedule = (delayMs = 0): void => {
    if (!isCurrent()) return
    if (delayMs === 0) {
      queueMicrotask(() => void dispatch())
      return
    }
    retryTimer = setTimeout(() => {
      retryTimer = undefined
      void dispatch()
    }, delayMs)
  }
  const resync = (): void => {
    if (!isCurrent()) return
    if (resyncCount >= MAX_SELECTION_RESYNCS) {
      onError(new Error('Browser surface instance could not be resynchronized'))
      return
    }
    resyncCount += 1
    retryIndex = 0
    selectionRevision = nextBrowserSurfaceSelectionRevision()
    schedule()
  }
  const dispatch = async (): Promise<void> => {
    if (!isCurrent()) return
    try {
      const output = await browser.surfaceSelected({
        schemaVersion: 1,
        surfaceId: target.surfaceId,
        surfaceInstanceId: null,
        selectionRevision
      })
      observeBrowserSurfaceSelectionRevision(output.authoritativeRevision)
      if (!isCurrent()) return
      if (output.reason === 'instance_required' && output.surfaceInstanceId) {
        target.onInstance(output.surfaceInstanceId)
        return
      }
      if (output.reason === 'not_registered' && output.retryable) {
        const delayMs = SURFACE_REGISTRATION_RETRY_DELAYS_MS[retryIndex]
        if (delayMs === undefined) {
          onError(new Error('Browser surface registration did not become available'))
          return
        }
        retryIndex += 1
        schedule(delayMs)
        return
      }
      if (output.reason === 'stale_revision' || output.reason === 'instance_mismatch') {
        resync()
      }
    } catch (error) {
      if (isCurrent()) onError(error)
    }
  }

  void dispatch()
  return () => {
    cancelled = true
    if (retryTimer) clearTimeout(retryTimer)
  }
}

/**
 * Starts one exact active-webview selection intent. The initial request is a side-effect-free
 * probe; Main returns an opaque instance binding which is echoed only while the same DOM webview
 * remains current. Missing registration is retried with a small fixed budget.
 */
export function synchronizeBrowserSurfaceSelection(
  browser: BrowserHostApi,
  target: BrowserSurfaceSelectionTarget,
  onError: (error: unknown) => void = reportBrowserSurfaceSelectionError
): () => void {
  let cancelled = false
  let retryTimer: ReturnType<typeof setTimeout> | undefined
  let retryIndex = 0
  let resyncCount = 0
  let surfaceInstanceId: string | null = null
  let selectionRevision = nextBrowserSurfaceSelectionRevision()

  const isCurrent = (): boolean => !cancelled && target.isCurrent()
  const schedule = (delayMs = 0): void => {
    if (!isCurrent()) return
    if (delayMs === 0) {
      queueMicrotask(() => void dispatch())
      return
    }
    retryTimer = setTimeout(() => {
      retryTimer = undefined
      void dispatch()
    }, delayMs)
  }
  const resync = (): void => {
    if (!isCurrent()) return
    if (resyncCount >= MAX_SELECTION_RESYNCS) {
      onError(new Error('Browser surface selection could not be resynchronized'))
      return
    }
    resyncCount += 1
    retryIndex = 0
    surfaceInstanceId = null
    selectionRevision = nextBrowserSurfaceSelectionRevision()
    schedule()
  }
  const dispatch = async (): Promise<void> => {
    if (!isCurrent()) return
    try {
      const output = await browser.surfaceSelected({
        schemaVersion: 1,
        surfaceId: target.surfaceId,
        surfaceInstanceId,
        selectionRevision
      })
      observeBrowserSurfaceSelectionRevision(output.authoritativeRevision)
      if (!isCurrent()) return

      if (output.status === 'applied' || output.reason === 'selection_unchanged') return
      if (output.reason === 'instance_required') {
        surfaceInstanceId = output.surfaceInstanceId
        schedule()
        return
      }
      if (output.reason === 'not_registered' && output.retryable) {
        const delayMs = SURFACE_REGISTRATION_RETRY_DELAYS_MS[retryIndex]
        if (delayMs === undefined) {
          onError(new Error('Browser surface registration did not become available'))
          return
        }
        retryIndex += 1
        schedule(delayMs)
        return
      }
      if (output.reason === 'stale_revision' || output.reason === 'instance_mismatch') {
        // Never adopt a mismatch token directly. Re-probe with a newer intent, and only while the
        // same live webview is still the active Renderer surface.
        resync()
      }
    } catch (error) {
      if (isCurrent()) onError(error)
    }
  }

  void dispatch()
  return () => {
    cancelled = true
    if (retryTimer) clearTimeout(retryTimer)
  }
}

/** Clears Main's UI selection when no Browser page is foreground. Errors remain observable. */
export async function clearBrowserSurfaceSelection(browser: BrowserHostApi): Promise<void> {
  const selectionRevision = nextBrowserSurfaceSelectionRevision()
  const output = await browser.surfaceSelected({
    schemaVersion: 1,
    surfaceId: null,
    surfaceInstanceId: null,
    selectionRevision
  })
  observeBrowserSurfaceSelectionRevision(output.authoritativeRevision)
}

function nextBrowserSurfaceSelectionRevision(): number {
  const next = Math.max(lastBrowserSurfaceSelectionRevision + 1, Date.now())
  if (!Number.isSafeInteger(next)) throw new Error('Browser surface selection revision exhausted')
  lastBrowserSurfaceSelectionRevision = next
  return next
}

function observeBrowserSurfaceSelectionRevision(revision: number): void {
  lastBrowserSurfaceSelectionRevision = Math.max(lastBrowserSurfaceSelectionRevision, revision)
}

function reportBrowserSurfaceSelectionError(error: unknown): void {
  console.error('Failed to synchronize the active Browser surface', error)
}

/**
 * Keeps the Main-to-Renderer command channel scoped to AppShell. Strict parser enforcement lives
 * in Preload; this hook only translates reveal intent into shell layout state.
 */
export interface BrowserSurfaceBridge {
  command: BrowserSurfaceCommand | null
  surfaceReady: (input: BrowserSurfaceReadyInput) => Promise<BrowserSurfaceReadyOutput>
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

  const surfaceReady = useCallback(
    async (input: BrowserSurfaceReadyInput): Promise<BrowserSurfaceReadyOutput> => {
      const browser = resolveBrowserSurfaceHostApi()
      if (!browser) throw new Error('MyCopilot browser surface API is not available')
      return await browser.surfaceReady(input)
    },
    []
  )

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
    typeof browser.onSurfaceState !== 'function' ||
    typeof browser.surfaceAction !== 'function' ||
    typeof browser.surfaceReady !== 'function' ||
    typeof browser.surfaceSelected !== 'function' ||
    typeof browser.surfaceState !== 'function'
  ) {
    throw new Error('MyCopilot browser surface API is malformed')
  }
  return browser
}
