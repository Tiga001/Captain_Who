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
  const lastBoundsRef = useRef<BrowserBounds | null>(null)
  const nativeVisibleRef = useRef(false)
  const boundsSyncSeqRef = useRef(0)

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

  const markNativeViewUnavailable = useCallback(() => {
    createPromiseRef.current = null
    lastBoundsRef.current = null
    nativeVisibleRef.current = false
    viewCreatedRef.current = false
  }, [])

  const handleBrowserOperationError = useCallback(
    (operation: string, error: unknown) => {
      const message = error instanceof Error ? error.message : String(error)
      console.error(`Failed to ${operation} browser view`, error)
      setErrorMessage(message)

      if (message.includes('Browser view not found')) {
        markNativeViewUnavailable()
      }
    },
    [markNativeViewUnavailable]
  )

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
        lastBoundsRef.current = null
        nativeVisibleRef.current = false
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
    if (!viewCreatedRef.current || !nativeVisibleRef.current) return

    nativeVisibleRef.current = false
    void hideBrowserView(viewId).catch((error) => {
      handleBrowserOperationError('hide', error)
    })
  }, [handleBrowserOperationError, viewId])

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
      boundsSyncSeqRef.current += 1
      hideNativeView()
      return
    }

    const syncSeq = boundsSyncSeqRef.current + 1
    boundsSyncSeqRef.current = syncSeq
    const boundsChanged =
      !lastBoundsRef.current || !areBrowserBoundsEqual(lastBoundsRef.current, bounds)
    const updateBounds = boundsChanged
      ? setBrowserViewBounds(viewId, bounds).then(() => {
          lastBoundsRef.current = bounds
        })
      : Promise.resolve()

    void updateBounds
      .then(() => {
        if (
          boundsSyncSeqRef.current !== syncSeq ||
          nativeVisibleRef.current ||
          !viewCreatedRef.current ||
          !isActiveRef.current ||
          isObscuredRef.current ||
          !currentUrlRef.current
        ) {
          return null
        }

        return showBrowserView(viewId)
      })
      .then((state) => {
        if (!state) return

        nativeVisibleRef.current = true
        applyNavigationState(state)
      })
      .catch((error) => {
        handleBrowserOperationError('position', error)
      })
  }, [applyNavigationState, handleBrowserOperationError, hideNativeView, hostRef, viewId])

  useEffect(() => {
    isDisposedRef.current = false

    return () => {
      isDisposedRef.current = true
      markNativeViewUnavailable()
      createPromiseRef.current = null
      void destroyBrowserView(viewId).catch((error) => {
        console.error('Failed to destroy browser view', error)
      })
    }
  }, [markNativeViewUnavailable, viewId])

  useEffect(() => {
    return listenToBrowserViewEvents((event) => {
      if (!isBrowserEventForView(event, viewId)) return

      if (event.type === 'browser.destroyed') {
        markNativeViewUnavailable()
        return
      }

      if (event.type === 'browser.failedLoad') {
        setErrorMessage(event.errorDescription)
      }

      if ('state' in event && 'metadata' in event.state) {
        applyNavigationState(event.state)
      }
    })
  }, [applyNavigationState, markNativeViewUnavailable, viewId])

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
      try {
        await ensureViewCreated()
        setErrorMessage(null)
        const state = await navigateBrowserView({ id: viewId, url })
        applyNavigationState(state)
        syncBounds()
      } catch (error) {
        handleBrowserOperationError('navigate', error)
      }
    },
    [applyNavigationState, ensureViewCreated, handleBrowserOperationError, syncBounds, viewId]
  )

  const reload = useCallback(async () => {
    if (!currentUrlRef.current || !viewCreatedRef.current) return

    try {
      const state = await reloadBrowserView(viewId)
      applyNavigationState(state)
    } catch (error) {
      handleBrowserOperationError('reload', error)
    }
  }, [applyNavigationState, handleBrowserOperationError, viewId])

  const goBack = useCallback(async () => {
    if (!viewCreatedRef.current) return

    try {
      const state = await goBackBrowserView(viewId)
      applyNavigationState(state)
    } catch (error) {
      handleBrowserOperationError('go back in', error)
    }
  }, [applyNavigationState, handleBrowserOperationError, viewId])

  const goForward = useCallback(async () => {
    if (!viewCreatedRef.current) return

    try {
      const state = await goForwardBrowserView(viewId)
      applyNavigationState(state)
    } catch (error) {
      handleBrowserOperationError('go forward in', error)
    }
  }, [applyNavigationState, handleBrowserOperationError, viewId])

  const setZoom = useCallback(
    async (zoomFactor: number) => {
      try {
        await ensureViewCreated()
        return await setBrowserViewZoom(viewId, zoomFactor)
      } catch (error) {
        handleBrowserOperationError('set zoom for', error)
        return { id: viewId, zoomFactor }
      }
    },
    [ensureViewCreated, handleBrowserOperationError, viewId]
  )

  const clearBrowsingData = useCallback(async () => {
    try {
      await ensureViewCreated()
      await clearBrowserViewBrowsingData(viewId)
    } catch (error) {
      handleBrowserOperationError('clear browsing data for', error)
    }
  }, [ensureViewCreated, handleBrowserOperationError, viewId])

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

function areBrowserBoundsEqual(left: BrowserBounds, right: BrowserBounds): boolean {
  return (
    left.height === right.height &&
    left.width === right.width &&
    left.x === right.x &&
    left.y === right.y
  )
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
