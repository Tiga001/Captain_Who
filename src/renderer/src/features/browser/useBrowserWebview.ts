import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type {
  DidFailLoadEvent,
  DidNavigateEvent,
  DidNavigateInPageEvent,
  PageFaviconUpdatedEvent,
  PageTitleUpdatedEvent,
  RenderProcessGoneEvent,
  WebviewTag
} from 'electron'
import { parseBrowserSurfaceBootstrapUrl } from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'
import type { BrowserNavigationState } from './browserTypes'
import { getFallbackPageTitle } from './browserUrl'

interface UseBrowserWebviewOptions {
  isActive: boolean
}

interface UseBrowserWebviewResult {
  clearBrowsingData: () => Promise<void>
  currentUrl: string | null
  errorMessage: string | null
  goBack: () => Promise<void>
  goForward: () => Promise<void>
  isLoaded: boolean
  navigationState: BrowserNavigationState
  navigateToUrl: (url: string) => Promise<void>
  reload: () => Promise<void>
  setWebview: (webview: WebviewTag | null) => void
  setZoom: (zoomFactor: number) => Promise<void>
}

const ABORTED_NAVIGATION_ERROR_CODE = -3

export function useBrowserWebview({ isActive }: UseBrowserWebviewOptions): UseBrowserWebviewResult {
  const [navigationState, setNavigationState] = useState<BrowserNavigationState>(
    createEmptyNavigationState
  )
  const [errorMessage, setErrorMessage] = useState<string | null>(null)
  const [attachedWebview, setAttachedWebview] = useState<WebviewTag | null>(null)
  const webviewRef = useRef<WebviewTag | null>(null)
  const currentUrlRef = useRef<string | null>(null)
  const faviconRequestSequenceRef = useRef(0)
  const zoomFactorRef = useRef(1)

  const updateNavigationState = useCallback(
    (update: (current: BrowserNavigationState) => BrowserNavigationState) => {
      setNavigationState((current) => {
        const next = update(current)
        currentUrlRef.current = next.metadata.url
        return next
      })
    },
    []
  )

  const refreshFromWebview = useCallback(
    (webview: WebviewTag, requestedUrl?: string) => {
      try {
        const url = normalizeWebviewUrl(requestedUrl || webview.getURL())
        const title = url ? webview.getTitle().trim() || getFallbackPageTitle(url) : null

        updateNavigationState((current) => ({
          canGoBack: webview.canGoBack(),
          canGoForward: webview.canGoForward(),
          isLoading: webview.isLoading(),
          metadata: {
            iconUrl: haveSameHttpOrigin(current.metadata.url, url)
              ? current.metadata.iconUrl
              : null,
            title,
            url
          }
        }))
      } catch (error) {
        handleOperationError('refresh browser state', error, setErrorMessage)
      }
    },
    [updateNavigationState]
  )

  const setWebview = useCallback(
    (webview: WebviewTag | null) => {
      webviewRef.current = webview
      setAttachedWebview((current) => (current === webview ? current : webview))
      if (!webview) return

      try {
        webview.setZoomFactor(zoomFactorRef.current)
        refreshFromWebview(webview)
      } catch (error) {
        handleOperationError('initialize browser', error, setErrorMessage)
      }
    },
    [refreshFromWebview]
  )

  const resolveFavicon = useCallback(
    (pageUrlValue: string | null | undefined, faviconUrl?: string | null) => {
      const pageUrl = normalizeWebviewUrl(pageUrlValue)
      if (!pageUrl) return

      const requestSequence = faviconRequestSequenceRef.current + 1
      faviconRequestSequenceRef.current = requestSequence
      void hostClient.resources
        .resolveFavicon({ faviconUrl: faviconUrl ?? null, pageUrl })
        .then(({ url }) => {
          if (!url || requestSequence !== faviconRequestSequenceRef.current) return

          updateNavigationState((current) =>
            haveSameHttpOrigin(current.metadata.url, pageUrl)
              ? {
                  ...current,
                  metadata: { ...current.metadata, iconUrl: url }
                }
              : current
          )
        })
        .catch(() => undefined)
    },
    [updateNavigationState]
  )

  useEffect(() => {
    const webview = attachedWebview
    if (!webview) return undefined

    const handleStartLoading = () => {
      setErrorMessage(null)
      updateNavigationState((current) => ({ ...current, isLoading: true }))
    }
    const handleStopLoading = () => refreshFromWebview(webview)
    const handleNavigate = (event: DidNavigateEvent) => {
      if (!haveSameHttpOrigin(currentUrlRef.current, event.url)) {
        faviconRequestSequenceRef.current += 1
      }
      setErrorMessage(null)
      refreshFromWebview(webview, event.url)
      resolveFavicon(event.url)
    }
    const handleNavigateInPage = (event: DidNavigateInPageEvent) => {
      if (event.isMainFrame) refreshFromWebview(webview, event.url)
    }
    const handleTitleUpdated = (event: PageTitleUpdatedEvent) => {
      const title = event.title.trim() || getFallbackPageTitle(currentUrlRef.current)
      updateNavigationState((current) => ({
        ...current,
        metadata: { ...current.metadata, title }
      }))
    }
    const handleFaviconUpdated = (event: PageFaviconUpdatedEvent) => {
      resolveFavicon(webview.getURL(), event.favicons[0] ?? null)
    }
    const handleFailedLoad = (event: DidFailLoadEvent) => {
      if (!event.isMainFrame || event.errorCode === ABORTED_NAVIGATION_ERROR_CODE) return

      setErrorMessage(event.errorDescription)
      updateNavigationState((current) => ({ ...current, isLoading: false }))
    }
    const handleRenderProcessGone = (event: RenderProcessGoneEvent) => {
      setErrorMessage(`Browser renderer stopped: ${event.details.reason}`)
      updateNavigationState((current) => ({ ...current, isLoading: false }))
    }

    webview.addEventListener('did-start-loading', handleStartLoading)
    webview.addEventListener('did-stop-loading', handleStopLoading)
    webview.addEventListener('did-navigate', handleNavigate)
    webview.addEventListener('did-navigate-in-page', handleNavigateInPage)
    webview.addEventListener('page-title-updated', handleTitleUpdated)
    webview.addEventListener('page-favicon-updated', handleFaviconUpdated)
    webview.addEventListener('did-fail-load', handleFailedLoad)
    webview.addEventListener('render-process-gone', handleRenderProcessGone)

    return () => {
      webview.removeEventListener('did-start-loading', handleStartLoading)
      webview.removeEventListener('did-stop-loading', handleStopLoading)
      webview.removeEventListener('did-navigate', handleNavigate)
      webview.removeEventListener('did-navigate-in-page', handleNavigateInPage)
      webview.removeEventListener('page-title-updated', handleTitleUpdated)
      webview.removeEventListener('page-favicon-updated', handleFaviconUpdated)
      webview.removeEventListener('did-fail-load', handleFailedLoad)
      webview.removeEventListener('render-process-gone', handleRenderProcessGone)
    }
  }, [attachedWebview, refreshFromWebview, resolveFavicon, updateNavigationState])

  useEffect(() => {
    if (!isActive) webviewRef.current?.blur()
  }, [isActive])

  const navigateToUrl = useCallback(
    async (url: string) => {
      const webview = webviewRef.current
      if (!webview) {
        setErrorMessage('Browser surface is not ready')
        return
      }

      if (!haveSameHttpOrigin(currentUrlRef.current, url)) {
        faviconRequestSequenceRef.current += 1
      }
      setErrorMessage(null)
      updateNavigationState((current) => ({
        ...current,
        isLoading: true,
        metadata: {
          iconUrl: haveSameHttpOrigin(current.metadata.url, url) ? current.metadata.iconUrl : null,
          title: getFallbackPageTitle(url),
          url
        }
      }))

      try {
        await webview.loadURL(url)
      } catch (error) {
        if (isAbortedNavigationError(error)) return
        handleOperationError('navigate browser', error, setErrorMessage)
        updateNavigationState((current) => ({ ...current, isLoading: false }))
      }
    },
    [updateNavigationState]
  )

  const reload = useCallback(async () => {
    runWebviewOperation(webviewRef.current, 'reload browser', setErrorMessage, (webview) => {
      webview.reload()
    })
  }, [])

  const goBack = useCallback(async () => {
    runWebviewOperation(webviewRef.current, 'navigate browser back', setErrorMessage, (webview) => {
      if (webview.canGoBack()) webview.goBack()
    })
  }, [])

  const goForward = useCallback(async () => {
    runWebviewOperation(
      webviewRef.current,
      'navigate browser forward',
      setErrorMessage,
      (webview) => {
        if (webview.canGoForward()) webview.goForward()
      }
    )
  }, [])

  const setZoom = useCallback(async (zoomFactor: number) => {
    zoomFactorRef.current = zoomFactor
    runWebviewOperation(webviewRef.current, 'zoom browser', setErrorMessage, (webview) => {
      webview.setZoomFactor(zoomFactor)
    })
  }, [])

  const clearBrowsingData = useCallback(async () => {
    try {
      await hostClient.browser.clearBrowsingData()
    } catch (error) {
      handleOperationError('clear browser data', error, setErrorMessage)
    }
  }, [])

  return useMemo(
    () => ({
      clearBrowsingData,
      currentUrl: navigationState.metadata.url,
      errorMessage,
      goBack,
      goForward,
      isLoaded: Boolean(navigationState.metadata.url),
      navigationState,
      navigateToUrl,
      reload,
      setWebview,
      setZoom
    }),
    [
      clearBrowsingData,
      errorMessage,
      goBack,
      goForward,
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
    metadata: {
      iconUrl: null,
      title: null,
      url: null
    }
  }
}

function normalizeWebviewUrl(url: string | null | undefined): string | null {
  const normalized = url?.trim()
  return normalized &&
    normalized !== 'about:blank' &&
    parseBrowserSurfaceBootstrapUrl(normalized) === null
    ? normalized
    : null
}

function haveSameHttpOrigin(
  left: string | null | undefined,
  right: string | null | undefined
): boolean {
  if (!left || !right) return false

  try {
    const leftUrl = new URL(left)
    const rightUrl = new URL(right)
    if (!['http:', 'https:'].includes(leftUrl.protocol)) return false
    if (!['http:', 'https:'].includes(rightUrl.protocol)) return false
    return leftUrl.origin === rightUrl.origin
  } catch {
    return false
  }
}

function runWebviewOperation(
  webview: WebviewTag | null,
  operation: string,
  setErrorMessage: (message: string | null) => void,
  callback: (webview: WebviewTag) => void
): void {
  if (!webview) return

  try {
    callback(webview)
  } catch (error) {
    handleOperationError(operation, error, setErrorMessage)
  }
}

function handleOperationError(
  operation: string,
  error: unknown,
  setErrorMessage: (message: string | null) => void
): void {
  const message = error instanceof Error ? error.message : String(error)
  console.error(`Failed to ${operation}`, error)
  setErrorMessage(message)
}

function isAbortedNavigationError(error: unknown): boolean {
  return error instanceof Error && error.message.includes('ERR_ABORTED')
}
