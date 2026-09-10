import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { WebviewTag } from 'electron'
import {
  BROWSER_SURFACE_SCHEMA_VERSION,
  type BrowserSurfaceActionInput,
  type BrowserSurfacePublicCrashError,
  type BrowserSurfacePublicLoadError,
  type BrowserSurfaceState
} from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'
import type { BrowserNavigationState } from './browserTypes'
import { getFallbackPageTitle } from './browserUrl'
import { resolveBrowserSurfaceHostApi } from './browserSurface'

interface UseBrowserWebviewOptions {
  initialLogicalUrl?: string
  isActive: boolean
  surfaceId: string
  surfaceInstanceId: string | null
}

interface UseBrowserWebviewResult {
  isAgentTarget: boolean
  currentUrl: string | null
  goBack: () => Promise<void>
  goForward: () => Promise<void>
  hostFallbackError: BrowserSurfacePublicLoadError | null
  hostFallbackCrashError: BrowserSurfacePublicCrashError | null
  isLoaded: boolean
  navigationState: BrowserNavigationState
  navigateToUrl: (url: string) => Promise<void>
  reload: () => Promise<void>
  setWebview: (webview: WebviewTag | null) => void
  setZoom: (zoomFactor: number) => Promise<void>
}

export function useBrowserWebview({
  initialLogicalUrl,
  isActive,
  surfaceId,
  surfaceInstanceId
}: UseBrowserWebviewOptions): UseBrowserWebviewResult {
  const [isAgentTarget, setIsAgentTarget] = useState(false)
  const [navigationState, setNavigationState] = useState<BrowserNavigationState>(
    createEmptyNavigationState
  )
  const [hostFallbackError, setHostFallbackError] = useState<BrowserSurfacePublicLoadError | null>(
    null
  )
  const [hostFallbackCrashError, setHostFallbackCrashError] =
    useState<BrowserSurfacePublicCrashError | null>(null)
  const webviewRef = useRef<WebviewTag | null>(null)
  const faviconRequestSequenceRef = useRef(0)
  const stateRevisionRef = useRef(-1)
  const surfaceIdentityRef = useRef({ surfaceId, surfaceInstanceId })
  const initialLogicalUrlRef = useRef(initialLogicalUrl)
  const pendingNavigationRef = useRef<string | null>(null)
  const zoomFactorRef = useRef(1)

  useEffect(() => {
    initialLogicalUrlRef.current = initialLogicalUrl
  }, [initialLogicalUrl])

  const updateNavigationState = useCallback(
    (update: (current: BrowserNavigationState) => BrowserNavigationState) => {
      setNavigationState((current) => {
        return update(current)
      })
    },
    []
  )

  const resolveFavicon = useCallback(
    (pageUrlValue: string | null | undefined, faviconUrl?: string | null) => {
      const pageUrl = normalizeHttpUrl(pageUrlValue)
      if (!pageUrl) return

      const requestSequence = faviconRequestSequenceRef.current + 1
      faviconRequestSequenceRef.current = requestSequence
      void hostClient.resources
        .resolveFavicon({ faviconUrl: faviconUrl ?? null, pageUrl })
        .then(({ url }) => {
          if (!url || requestSequence !== faviconRequestSequenceRef.current) return
          updateNavigationState((current) =>
            haveSameHttpOrigin(current.metadata.url, pageUrl)
              ? { ...current, metadata: { ...current.metadata, iconUrl: url } }
              : current
          )
        })
        .catch(() => undefined)
    },
    [updateNavigationState]
  )

  const applySurfaceState = useCallback(
    (state: BrowserSurfaceState): void => {
      const identity = surfaceIdentityRef.current
      if (
        state.surfaceId !== identity.surfaceId ||
        state.surfaceInstanceId !== identity.surfaceInstanceId ||
        state.stateRevision < stateRevisionRef.current
      ) {
        return
      }
      stateRevisionRef.current = state.stateRevision
      setIsAgentTarget(state.isAgentTarget === true)
      setHostFallbackError(state.presentation === 'host-fallback' ? state.loadError : null)
      setHostFallbackCrashError(state.presentation === 'host-fallback' ? state.crashError : null)
      updateNavigationState((current) => {
        const sameOrigin = haveSameHttpOrigin(current.metadata.url, state.url)
        return {
          canGoBack: state.canGoBack,
          canGoForward: state.canGoForward,
          isLoading: state.isLoading,
          metadata: {
            iconUrl: sameOrigin ? current.metadata.iconUrl : null,
            title: state.title ?? getFallbackPageTitle(state.url),
            url: state.url
          }
        }
      })
      if (state.url && state.presentation === 'content') {
        resolveFavicon(state.url, state.faviconUrl)
      }
    },
    [resolveFavicon, updateNavigationState]
  )

  useEffect(() => {
    surfaceIdentityRef.current = { surfaceId, surfaceInstanceId }
    setIsAgentTarget(false)
    stateRevisionRef.current = -1
    faviconRequestSequenceRef.current += 1
    setHostFallbackError(null)
    setHostFallbackCrashError(null)
    if (!surfaceInstanceId) return undefined

    const browser = resolveBrowserSurfaceHostApi()
    if (!browser) return undefined
    const input = {
      schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
      surfaceId,
      surfaceInstanceId
    } as const
    const unsubscribe = browser.onSurfaceState(applySurfaceState)
    const pendingNavigation = pendingNavigationRef.current
    pendingNavigationRef.current = null
    const restoreUrl = normalizeHttpUrl(initialLogicalUrlRef.current)
    const initialState = pendingNavigation
      ? browser.surfaceAction({ ...input, action: 'navigate', url: pendingNavigation })
      : browser.surfaceState(input).then((state) => {
          return !state.url && restoreUrl
            ? browser.surfaceAction({ ...input, action: 'navigate', url: restoreUrl })
            : state
        })
    void initialState.then(applySurfaceState).catch(() => undefined)
    return unsubscribe
  }, [applySurfaceState, surfaceId, surfaceInstanceId])

  const setWebview = useCallback((webview: WebviewTag | null) => {
    webviewRef.current = webview
    if (!webview) return
    try {
      webview.setZoomFactor(zoomFactorRef.current)
    } catch {
      // A replaced/destroyed guest is recovered by the existing surface lifecycle handshake.
    }
  }, [])

  useEffect(() => {
    if (!isActive) webviewRef.current?.blur()
  }, [isActive])

  const dispatchSurfaceAction = useCallback(
    async (
      action: BrowserSurfaceActionInput['action'],
      options: { url?: string } = {}
    ): Promise<void> => {
      const identity = surfaceIdentityRef.current
      if (!identity.surfaceInstanceId) {
        if (action === 'navigate' && options.url) pendingNavigationRef.current = options.url
        return
      }
      const browser = resolveBrowserSurfaceHostApi()
      if (!browser) return
      const input: BrowserSurfaceActionInput = {
        schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
        surfaceId: identity.surfaceId,
        surfaceInstanceId: identity.surfaceInstanceId,
        action,
        ...(action === 'navigate' && options.url ? { url: options.url } : {})
      }
      try {
        applySurfaceState(await browser.surfaceAction(input))
      } catch {
        // Main publishes the authoritative safe state. Never surface raw IPC/Electron details.
      }
    },
    [applySurfaceState]
  )

  const navigateToUrl = useCallback(
    async (url: string) => {
      const normalizedUrl = normalizeHttpUrl(url)
      if (!normalizedUrl) return
      faviconRequestSequenceRef.current += 1
      setHostFallbackError(null)
      setHostFallbackCrashError(null)
      updateNavigationState((current) => ({
        ...current,
        isLoading: true,
        metadata: {
          iconUrl: haveSameHttpOrigin(current.metadata.url, normalizedUrl)
            ? current.metadata.iconUrl
            : null,
          title: getFallbackPageTitle(normalizedUrl),
          url: normalizedUrl
        }
      }))
      await dispatchSurfaceAction('navigate', { url: normalizedUrl })
    },
    [dispatchSurfaceAction, updateNavigationState]
  )

  const reload = useCallback(async () => {
    setHostFallbackError(null)
    setHostFallbackCrashError(null)
    await dispatchSurfaceAction('reload')
  }, [dispatchSurfaceAction])

  const goBack = useCallback(async () => {
    await dispatchSurfaceAction('goBack')
  }, [dispatchSurfaceAction])

  const goForward = useCallback(async () => {
    await dispatchSurfaceAction('goForward')
  }, [dispatchSurfaceAction])

  const setZoom = useCallback(async (zoomFactor: number) => {
    zoomFactorRef.current = zoomFactor
    try {
      webviewRef.current?.setZoomFactor(zoomFactor)
    } catch {
      // Zoom is a local presentation preference and never changes navigation state.
    }
  }, [])

  return useMemo(
    () => ({
      isAgentTarget,
      currentUrl: navigationState.metadata.url,
      goBack,
      goForward,
      hostFallbackError,
      hostFallbackCrashError,
      isLoaded: Boolean(navigationState.metadata.url),
      navigationState,
      navigateToUrl,
      reload,
      setWebview,
      setZoom
    }),
    [
      isAgentTarget,
      goBack,
      goForward,
      hostFallbackError,
      hostFallbackCrashError,
      navigateToUrl,
      navigationState,
      reload,
      setWebview,
      setZoom
    ]
  )
}

function createEmptyNavigationState(): BrowserNavigationState {
  return {
    canGoBack: false,
    canGoForward: false,
    isLoading: false,
    metadata: { iconUrl: null, title: null, url: null }
  }
}

function normalizeHttpUrl(value: string | null | undefined): string | null {
  if (!value) return null
  try {
    const parsed = new URL(value)
    return ['http:', 'https:', 'file:'].includes(parsed.protocol) ? parsed.toString() : null
  } catch {
    return null
  }
}

function haveSameHttpOrigin(
  left: string | null | undefined,
  right: string | null | undefined
): boolean {
  if (!left || !right) return false
  try {
    const leftUrl = new URL(left)
    const rightUrl = new URL(right)
    if (leftUrl.protocol === 'file:' || rightUrl.protocol === 'file:') {
      return leftUrl.href === rightUrl.href
    }
    return (
      ['http:', 'https:'].includes(leftUrl.protocol) &&
      ['http:', 'https:'].includes(rightUrl.protocol) &&
      leftUrl.origin === rightUrl.origin
    )
  } catch {
    return false
  }
}
