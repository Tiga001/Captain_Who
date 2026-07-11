import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { RefObject } from 'react'
import {
  clearBrowserViewBrowsingData,
  createBrowserView,
  destroyBrowserView,
  goBackBrowserView,
  goForwardBrowserView,
  hideBrowserView,
  listenToBrowserViewEvents,
  navigateBrowserView,
  reloadBrowserView,
  setBrowserViewBounds,
  setBrowserViewZoom,
  showBrowserView
} from './browserClient'
import type {
  BrowserBounds,
  BrowserNavigationState,
  BrowserViewEvent,
  BrowserViewId,
  BrowserZoomState
} from './browserClient'

interface UseBrowserViewOptions {
  hostRef: RefObject<HTMLDivElement | null>
  isActive: boolean
  isObscured?: boolean
  viewId: BrowserViewId
}

interface UseBrowserViewResult {
  clearBrowsingData: () => Promise<void>
  currentUrl: string | null
  errorMessage: string | null
  goBack: () => Promise<void>
  goForward: () => Promise<void>
  isLoaded: boolean
  navigationState: BrowserNavigationState
  navigateToUrl: (url: string) => Promise<void>
  reload: () => Promise<void>
  setZoom: (zoomFactor: number) => Promise<BrowserZoomState>
}

const MIN_BROWSER_VIEW_HEIGHT = 80
const MIN_BROWSER_VIEW_WIDTH = 120
const BOUNDS_SYNC_SETTLE_MS = 120

export function useBrowserView({
  hostRef,
  isActive,
  isObscured = false,
  viewId
}: UseBrowserViewOptions): UseBrowserViewResult {
  const [navigationState, setNavigationState] = useState<BrowserNavigationState>(() =>
    createEmptyNavigationState(viewId)
  )
  const [errorMessage, setErrorMessage] = useState<string | null>(null)
  const createPromiseRef = useRef<Promise<BrowserNavigationState> | null>(null)
  const isDisposedRef = useRef(false)
  const isActiveRef = useRef(isActive)
  const isObscuredRef = useRef(isObscured)
  const currentUrlRef = useRef<string | null>(null)
  const viewCreatedRef = useRef(false)

  useEffect(() => {
    isActiveRef.current = isActive
  }, [isActive])

  useEffect(() => {
    isObscuredRef.current = isObscured
  }, [isObscured])

  const applyNavigationState = useCallback((state: BrowserNavigationState) => {
    currentUrlRef.current = state.metadata.url
    setNavigationState(state)
    setErrorMessage(state.errorText ?? null)
  }, [])

  const ensureViewCreated = useCallback(async () => {
    if (viewCreatedRef.current && createPromiseRef.current) {
      return createPromiseRef.current
    }

    const createPromise = createBrowserView({ id: viewId })
      .then((state) => {
        if (isDisposedRef.current) {
          void destroyBrowserView(viewId).catch((error) => {
            console.error('Failed to destroy disposed browser view', error)
          })
          return state
        }

        viewCreatedRef.current = true
        applyNavigationState(state)
        return state
      })
      .catch((error) => {
        createPromiseRef.current = null
        throw error
      })

    createPromiseRef.current = createPromise
    return createPromise
  }, [applyNavigationState, viewId])

  const hideNativeView = useCallback(() => {
    if (!viewCreatedRef.current) return

    void hideBrowserView(viewId).catch((error) => {
      console.error('Failed to hide browser view', error)
    })
  }, [viewId])

  const syncBounds = useCallback(() => {
    const host = hostRef.current
    if (!host || !viewCreatedRef.current) return

    const bounds = getVisibleHostBounds(host)
    const shouldShow =
      isActiveRef.current &&
      !isObscuredRef.current &&
      Boolean(currentUrlRef.current) &&
      bounds !== null

    if (!shouldShow || !bounds) {
      hideNativeView()
      return
    }

    void setBrowserViewBounds(viewId, bounds)
      .then(() => showBrowserView(viewId))
      .then(applyNavigationState)
      .catch((error) => {
        console.error('Failed to position browser view', error)
      })
  }, [applyNavigationState, hideNativeView, hostRef, viewId])

  useEffect(() => {
    isDisposedRef.current = false

    return () => {
      isDisposedRef.current = true
      viewCreatedRef.current = false
      createPromiseRef.current = null
      void destroyBrowserView(viewId).catch((error) => {
        console.error('Failed to destroy browser view', error)
      })
    }
  }, [viewId])

  useEffect(() => {
    return listenToBrowserViewEvents((event) => {
      if (!isBrowserEventForView(event, viewId)) return

      if (event.type === 'browser.destroyed') {
        viewCreatedRef.current = false
        createPromiseRef.current = null
        return
      }

      if (event.type === 'browser.failedLoad') {
        setErrorMessage(event.errorDescription)
      }

      if ('state' in event && 'metadata' in event.state) {
        applyNavigationState(event.state)
      }
    })
  }, [applyNavigationState, viewId])

  useEffect(() => {
    syncBounds()
  }, [isActive, isObscured, navigationState.metadata.url, syncBounds])

  useEffect(() => {
    const host = hostRef.current
    if (!host) return undefined

    let resizeFrame = 0
    let settleTimer = 0
    const queueSyncBounds = () => {
      window.cancelAnimationFrame(resizeFrame)
      window.clearTimeout(settleTimer)
      resizeFrame = window.requestAnimationFrame(syncBounds)
      settleTimer = window.setTimeout(syncBounds, BOUNDS_SYNC_SETTLE_MS)
    }

    const resizeObserver = new ResizeObserver(queueSyncBounds)
    resizeObserver.observe(host)
    window.addEventListener('resize', queueSyncBounds)
    document.addEventListener('scroll', queueSyncBounds, true)

    queueSyncBounds()

    return () => {
      window.cancelAnimationFrame(resizeFrame)
      window.clearTimeout(settleTimer)
      resizeObserver.disconnect()
      window.removeEventListener('resize', queueSyncBounds)
      document.removeEventListener('scroll', queueSyncBounds, true)
    }
  }, [hostRef, syncBounds])

  const navigateToUrl = useCallback(
    async (url: string) => {
      await ensureViewCreated()
      setErrorMessage(null)
      const state = await navigateBrowserView({ id: viewId, url })
      applyNavigationState(state)
      syncBounds()
    },
    [applyNavigationState, ensureViewCreated, syncBounds, viewId]
  )

  const reload = useCallback(async () => {
    if (!currentUrlRef.current) return

    const state = await reloadBrowserView(viewId)
    applyNavigationState(state)
  }, [applyNavigationState, viewId])

  const goBack = useCallback(async () => {
    const state = await goBackBrowserView(viewId)
    applyNavigationState(state)
  }, [applyNavigationState, viewId])

  const goForward = useCallback(async () => {
    const state = await goForwardBrowserView(viewId)
    applyNavigationState(state)
  }, [applyNavigationState, viewId])

  const setZoom = useCallback(
    async (zoomFactor: number) => {
      await ensureViewCreated()
      return setBrowserViewZoom(viewId, zoomFactor)
    },
    [ensureViewCreated, viewId]
  )

  const clearBrowsingData = useCallback(async () => {
    await ensureViewCreated()
    await clearBrowserViewBrowsingData(viewId)
  }, [ensureViewCreated, viewId])

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
      setZoom
    }),
    [
      clearBrowsingData,
      errorMessage,
      goBack,
      goForward,
      navigationState,
      navigateToUrl,
      reload,
      setZoom
    ]
  )
}

function createEmptyNavigationState(id: BrowserViewId): BrowserNavigationState {
  return {
    canGoBack: false,
    canGoForward: false,
    errorText: null,
    id,
    isLoading: false,
    metadata: {
      iconUrl: null,
      title: null,
      url: null
    }
  }
}

function isBrowserEventForView(event: BrowserViewEvent, viewId: BrowserViewId): boolean {
  if (event.type === 'browser.destroyed') {
    return event.id === viewId
  }

  if (event.type === 'browser.zoom') {
    return event.state.id === viewId
  }

  return event.state.id === viewId
}

function getVisibleHostBounds(host: HTMLElement): BrowserBounds | null {
  if (!isElementVisible(host)) return null

  const rect = host.getBoundingClientRect()
  return {
    height: Math.max(1, Math.round(rect.height)),
    width: Math.max(1, Math.round(rect.width)),
    x: Math.round(rect.left),
    y: Math.round(rect.top)
  }
}

function isElementVisible(element: HTMLElement): boolean {
  const rect = element.getBoundingClientRect()
  if (rect.width < MIN_BROWSER_VIEW_WIDTH || rect.height < MIN_BROWSER_VIEW_HEIGHT) {
    return false
  }

  if (
    rect.right <= 0 ||
    rect.bottom <= 0 ||
    rect.left >= window.innerWidth ||
    rect.top >= window.innerHeight
  ) {
    return false
  }

  let currentElement: HTMLElement | null = element
  while (currentElement) {
    const style = window.getComputedStyle(currentElement)
    if (style.display === 'none' || style.visibility === 'hidden') {
      return false
    }
    currentElement = currentElement.parentElement
  }

  return true
}
