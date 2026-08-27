import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import {
  ArrowLeft,
  ArrowRight,
  CornerDownRight,
  Globe2,
  MonitorX,
  Minus,
  MoreVertical,
  Plus,
  RefreshCw,
  WifiOff
} from 'lucide-react'
import { BROWSER_WEBVIEW_PARTITION } from '@mycopilot/protocol'
import { createBrowserSurfaceBootstrapUrl } from '@mycopilot/protocol'
import type { WebviewTag } from 'electron'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { useDismissOnOutsidePointer } from '../../hooks/useDismissOnOutsidePointer'
import { WebviewSurface } from '../rightSidebar/surfaces/WebviewSurface'
import type { BrowserPageMetadata } from './browserTypes'
import { getFallbackPageTitle, normalizeBrowserUrl } from './browserUrl'
import { useBrowserWebview } from './useBrowserWebview'
import {
  browserSurfaceIdForPage,
  resolveBrowserSurfaceHostApi,
  synchronizeBrowserSurfaceInstance,
  synchronizeBrowserSurfaceSelection
} from './browserSurface'
import { BrowserDownloadCenter } from './BrowserDownloadCenter'
import './BrowserPanel.css'

interface BrowserPanelProps {
  automationRequestId?: string
  isActive: boolean
  onAutomationSurfaceReady?: (
    surfaceId: string,
    requestId: string,
    surfaceInstanceId: string,
    viewport?: { height: number; width: number }
  ) => void
  onPageMetadataChange?: (metadata: BrowserPageMetadata) => void
  onSurfaceInstanceChange?: (
    surfaceId: string,
    surfaceInstanceId: string,
    isCurrent: boolean
  ) => void
  onSurfaceFocus?: () => void
  pageId: string
  initialLogicalUrl?: string
  surfaceId?: string
  viewport?: { height: number; width: number }
}

const ZOOM_STEP = 0.1
const BROWSER_MENU_MAX_WIDTH = 286
const BROWSER_MENU_VIEWPORT_MARGIN = 8

interface BrowserMenuPosition {
  left: number
  top: number
  width: number
}

function clampZoom(value: number) {
  return Math.min(3, Math.max(0.3, value))
}

export function BrowserPanel({
  automationRequestId,
  isActive,
  onAutomationSurfaceReady,
  onPageMetadataChange,
  onSurfaceInstanceChange,
  onSurfaceFocus,
  pageId,
  initialLogicalUrl,
  surfaceId,
  viewport
}: BrowserPanelProps) {
  const { t } = useFrontendConfig()
  const menuAnchorRef = useRef<HTMLDivElement>(null)
  const webviewRef = useRef<WebviewTag | null>(null)
  const documentReadyWebviewRef = useRef<WebviewTag | null>(null)
  const isActiveRef = useRef(isActive)
  const cancelSurfaceInstanceRef = useRef<(() => void) | null>(null)
  const cancelSurfaceSelectionRef = useRef<(() => void) | null>(null)
  const surfaceInstanceWebviewRef = useRef<{
    surfaceId: string
    surfaceInstanceId: string
    webview: WebviewTag
  } | null>(null)
  const onSurfaceInstanceChangeRef = useRef(onSurfaceInstanceChange)
  const submittedAutomationRequestRef = useRef<{
    requestId: string
    surfaceInstanceId: string
    webview: WebviewTag
  } | null>(null)
  const [addressValue, setAddressValue] = useState('')
  const [isAddressEditing, setIsAddressEditing] = useState(false)
  const [isDownloadsOpen, setIsDownloadsOpen] = useState(false)
  const [isMenuOpen, setIsMenuOpen] = useState(false)
  const [menuPosition, setMenuPosition] = useState<BrowserMenuPosition | null>(null)
  const [surfaceInstanceId, setSurfaceInstanceId] = useState<string | null>(null)
  const [zoom, setZoomState] = useState(1)
  const viewId = surfaceId ?? browserSurfaceIdForPage(pageId)
  const {
    clearBrowsingData,
    currentUrl,
    goBack,
    goForward,
    hostFallbackError,
    hostFallbackCrashError,
    isLoaded,
    navigationState,
    navigateToUrl,
    reload,
    setWebview,
    setZoom
  } = useBrowserWebview({
    initialLogicalUrl,
    isActive,
    surfaceId: viewId,
    surfaceInstanceId
  })

  useLayoutEffect(() => {
    isActiveRef.current = isActive
    onSurfaceInstanceChangeRef.current = onSurfaceInstanceChange
  }, [isActive, onSurfaceInstanceChange])

  const closeMenu = useCallback(() => setIsMenuOpen(false), [])
  const handleSurfaceFocus = useCallback(() => {
    closeMenu()
    setIsDownloadsOpen(false)
    onSurfaceFocus?.()
  }, [closeMenu, onSurfaceFocus])
  const ignoreMenuPortal = useCallback(
    (target: Node) => target instanceof Element && Boolean(target.closest('.browser-panel__menu')),
    []
  )
  useDismissOnOutsidePointer(menuAnchorRef, isMenuOpen, closeMenu, ignoreMenuPortal)

  const updateMenuPosition = useCallback((): void => {
    const anchor = menuAnchorRef.current
    if (!anchor) return
    const anchorBounds = anchor.getBoundingClientRect()
    const browserBounds = anchor.closest('.browser-panel')?.getBoundingClientRect()
    const availableWidth = Math.max(0, window.innerWidth - BROWSER_MENU_VIEWPORT_MARGIN * 2)
    const width = Math.min(BROWSER_MENU_MAX_WIDTH, availableWidth)
    const right = Math.min(
      window.innerWidth - BROWSER_MENU_VIEWPORT_MARGIN,
      browserBounds?.right ?? anchorBounds.right
    )
    const next = {
      left: Math.round(Math.max(BROWSER_MENU_VIEWPORT_MARGIN, right - width)),
      top: Math.round(anchorBounds.bottom + 8),
      width: Math.round(width)
    }
    setMenuPosition((current) =>
      current?.left === next.left && current.top === next.top && current.width === next.width
        ? current
        : next
    )
  }, [])

  useLayoutEffect(() => {
    if (!isMenuOpen) {
      setMenuPosition(null)
      return
    }
    updateMenuPosition()
    const anchor = menuAnchorRef.current
    const browserPanel = anchor?.closest('.browser-panel')
    const observer = new ResizeObserver(updateMenuPosition)
    if (anchor) observer.observe(anchor)
    if (browserPanel) observer.observe(browserPanel)
    window.addEventListener('resize', updateMenuPosition)
    window.addEventListener('scroll', updateMenuPosition, true)
    return () => {
      observer.disconnect()
      window.removeEventListener('resize', updateMenuPosition)
      window.removeEventListener('scroll', updateMenuPosition, true)
    }
  }, [isMenuOpen, updateMenuPosition])

  const reportAutomationSurfaceReady = useCallback(
    (webview: WebviewTag | null): void => {
      if (
        !webview ||
        !automationRequestId ||
        documentReadyWebviewRef.current !== webview ||
        surfaceInstanceWebviewRef.current?.webview !== webview ||
        surfaceInstanceWebviewRef.current.surfaceId !== viewId ||
        (submittedAutomationRequestRef.current?.requestId === automationRequestId &&
          submittedAutomationRequestRef.current.surfaceInstanceId ===
            surfaceInstanceWebviewRef.current.surfaceInstanceId &&
          submittedAutomationRequestRef.current.webview === webview)
      ) {
        return
      }
      const surfaceInstanceId = surfaceInstanceWebviewRef.current.surfaceInstanceId
      submittedAutomationRequestRef.current = {
        requestId: automationRequestId,
        surfaceInstanceId,
        webview
      }
      const appliedViewport = viewport ? measureVisibleWebviewViewport(webview) : undefined
      onAutomationSurfaceReady?.(viewId, automationRequestId, surfaceInstanceId, appliedViewport)
    },
    [automationRequestId, onAutomationSurfaceReady, viewport, viewId]
  )

  const reportManualSurfaceSelection = useCallback(
    (webview: WebviewTag | null): void => {
      cancelSurfaceSelectionRef.current?.()
      cancelSurfaceSelectionRef.current = null
      if (!isActiveRef.current || !webview) return
      const browser = resolveBrowserSurfaceHostApi()
      if (!browser) return
      const exactWebview = webview
      cancelSurfaceSelectionRef.current = synchronizeBrowserSurfaceSelection(browser, {
        surfaceId: viewId,
        isCurrent: () =>
          isActiveRef.current && webviewRef.current === exactWebview && exactWebview.isConnected
      })
    },
    [viewId]
  )

  const handleWebviewReady = useCallback(
    (webview: WebviewTag | null): void => {
      cancelSurfaceInstanceRef.current?.()
      cancelSurfaceInstanceRef.current = null
      setSurfaceInstanceId(null)
      const previousInstance = surfaceInstanceWebviewRef.current
      if (previousInstance) {
        onSurfaceInstanceChange?.(
          previousInstance.surfaceId,
          previousInstance.surfaceInstanceId,
          false
        )
        surfaceInstanceWebviewRef.current = null
      }
      webviewRef.current = webview
      if (!webview) {
        documentReadyWebviewRef.current = null
        submittedAutomationRequestRef.current = null
      }
      setWebview(webview)
      reportManualSurfaceSelection(webview)
      if (!webview) return

      const browser = resolveBrowserSurfaceHostApi()
      if (!browser) return
      const exactWebview = webview
      cancelSurfaceInstanceRef.current = synchronizeBrowserSurfaceInstance(browser, {
        surfaceId: viewId,
        isCurrent: () => webviewRef.current === exactWebview && exactWebview.isConnected,
        onInstance: (surfaceInstanceId) => {
          if (webviewRef.current !== exactWebview || !exactWebview.isConnected) return
          const current = surfaceInstanceWebviewRef.current
          if (current && current.webview !== exactWebview) {
            onSurfaceInstanceChange?.(current.surfaceId, current.surfaceInstanceId, false)
          }
          surfaceInstanceWebviewRef.current = {
            surfaceId: viewId,
            surfaceInstanceId,
            webview: exactWebview
          }
          setSurfaceInstanceId(surfaceInstanceId)
          onSurfaceInstanceChange?.(viewId, surfaceInstanceId, true)
          reportAutomationSurfaceReady(exactWebview)
        }
      })
    },
    [
      onSurfaceInstanceChange,
      reportAutomationSurfaceReady,
      reportManualSurfaceSelection,
      setWebview,
      viewId
    ]
  )

  const handleWebviewDocumentReady = useCallback(
    (webview: WebviewTag): void => {
      // did-attach and dom-ready are separate lifecycle boundaries. React StrictMode can dispose
      // generation 1 between them; never acknowledge a request for a webview that is no longer the
      // exact DOM surface presented by this panel.
      if (webviewRef.current !== webview || !webview.isConnected) return
      documentReadyWebviewRef.current = webview
      reportAutomationSurfaceReady(webview)
    },
    [reportAutomationSurfaceReady]
  )

  useEffect(() => {
    if (!automationRequestId) {
      submittedAutomationRequestRef.current = null
      return
    }
    reportAutomationSurfaceReady(documentReadyWebviewRef.current)
  }, [automationRequestId, reportAutomationSurfaceReady])

  useEffect(() => {
    if (!isActive) {
      setIsDownloadsOpen(false)
      setIsMenuOpen(false)
    }
    reportManualSurfaceSelection(webviewRef.current)
  }, [isActive, reportManualSurfaceSelection])

  useEffect(
    () => () => {
      cancelSurfaceInstanceRef.current?.()
      cancelSurfaceInstanceRef.current = null
      const current = surfaceInstanceWebviewRef.current
      if (current) {
        onSurfaceInstanceChangeRef.current?.(current.surfaceId, current.surfaceInstanceId, false)
        surfaceInstanceWebviewRef.current = null
      }
      cancelSurfaceSelectionRef.current?.()
      cancelSurfaceSelectionRef.current = null
    },
    []
  )

  useEffect(() => {
    if (!isAddressEditing) {
      setAddressValue(currentUrl ?? '')
    }
  }, [currentUrl, isAddressEditing])

  useEffect(() => {
    const metadataTitle =
      navigationState.metadata.title?.trim() ||
      getFallbackPageTitle(navigationState.metadata.url) ||
      t('browser.newTab')

    onPageMetadataChange?.({
      iconUrl: navigationState.metadata.iconUrl,
      title: metadataTitle,
      url: navigationState.metadata.url
    })
  }, [
    navigationState.metadata.iconUrl,
    navigationState.metadata.title,
    navigationState.metadata.url,
    onPageMetadataChange,
    t
  ])

  const submitAddress = useCallback(async () => {
    const url = normalizeBrowserUrl(addressValue)
    if (!url) {
      return
    }

    setAddressValue(url)
    onPageMetadataChange?.({
      iconUrl: null,
      title: getFallbackPageTitle(url) || t('browser.newTab'),
      url
    })

    await navigateToUrl(url)
    setIsAddressEditing(false)
  }, [addressValue, navigateToUrl, onPageMetadataChange, t])

  const updateZoom = useCallback(
    async (nextZoom: number) => {
      const normalizedZoom = clampZoom(nextZoom)
      setZoomState(normalizedZoom)
      await setZoom(normalizedZoom)
    },
    [setZoom]
  )

  return (
    <section className="browser-panel" aria-label={t('browser.title')}>
      <header className="browser-panel__toolbar">
        <div className="browser-panel__navigation">
          <button
            className="browser-panel__icon-button"
            type="button"
            aria-label={t('browser.back')}
            disabled={!navigationState.canGoBack}
            title={t('browser.back')}
            onClick={() => void goBack()}
          >
            <ArrowLeft aria-hidden="true" />
          </button>
          <button
            className="browser-panel__icon-button"
            type="button"
            aria-label={t('browser.forward')}
            disabled={!navigationState.canGoForward}
            title={t('browser.forward')}
            onClick={() => void goForward()}
          >
            <ArrowRight aria-hidden="true" />
          </button>
          <button
            className="browser-panel__icon-button"
            type="button"
            aria-label={t('browser.reload')}
            disabled={!isLoaded}
            title={t('browser.reload')}
            onClick={() => void reload()}
          >
            <RefreshCw
              aria-hidden="true"
              data-loading={navigationState.isLoading ? 'true' : undefined}
            />
          </button>
        </div>

        <form
          className="browser-panel__address"
          onBlur={(event) => {
            const nextTarget = event.relatedTarget
            if (nextTarget instanceof Node && event.currentTarget.contains(nextTarget)) return

            setIsAddressEditing(false)
            setAddressValue(currentUrl ?? '')
          }}
          onSubmit={(event) => {
            event.preventDefault()
            void submitAddress()
          }}
        >
          <input
            value={addressValue}
            type="text"
            spellCheck={false}
            placeholder={t('browser.addressPlaceholder')}
            aria-label={t('browser.addressPlaceholder')}
            onChange={(event) => {
              setIsAddressEditing(true)
              setAddressValue(event.target.value)
            }}
            onFocus={() => setIsAddressEditing(true)}
          />
          <button
            className="browser-panel__open-button"
            type="submit"
            aria-label={t('browser.open')}
          >
            <CornerDownRight aria-hidden="true" />
          </button>
        </form>

        <div className="browser-panel__toolbar-actions">
          <BrowserDownloadCenter
            isOpen={isDownloadsOpen}
            onOpenChange={(open) => {
              setIsDownloadsOpen(open)
              if (open) setIsMenuOpen(false)
            }}
          />
          <div className="browser-panel__menu-anchor" ref={menuAnchorRef}>
            <button
              className="browser-panel__icon-button"
              type="button"
              aria-label={t('browser.menu')}
              aria-expanded={isMenuOpen}
              onClick={() => {
                setIsMenuOpen((current) => !current)
                setIsDownloadsOpen(false)
              }}
            >
              <MoreVertical aria-hidden="true" />
            </button>
          </div>
        </div>
      </header>

      {isMenuOpen && menuPosition
        ? createPortal(
            <div className="browser-panel__menu" style={menuPosition}>
              <button
                className="browser-panel__menu-item"
                type="button"
                onClick={() => {
                  void clearBrowsingData()
                  setIsMenuOpen(false)
                }}
              >
                <span>{t('browser.clearBrowsingData')}</span>
              </button>

              <div className="browser-panel__zoom-row">
                <span>{t('browser.zoom')}</span>
                <div className="browser-panel__zoom-control">
                  <button
                    type="button"
                    aria-label={t('browser.zoomOut')}
                    onClick={() => void updateZoom(zoom - ZOOM_STEP)}
                  >
                    <Minus aria-hidden="true" />
                  </button>
                  <strong>{Math.round(zoom * 100)}%</strong>
                  <button
                    type="button"
                    aria-label={t('browser.zoomIn')}
                    onClick={() => void updateZoom(zoom + ZOOM_STEP)}
                  >
                    <Plus aria-hidden="true" />
                  </button>
                </div>
              </div>
            </div>,
            document.body
          )
        : null}

      <div className="browser-panel__content" data-fixed-viewport={viewport ? 'true' : undefined}>
        <WebviewSurface
          accessibleTitle={t('browser.title')}
          initialUrl={createBrowserSurfaceBootstrapUrl(viewId)}
          isActive={isActive}
          isVisible={Boolean(currentUrl)}
          openLinksInSameSurface
          onDocumentReady={handleWebviewDocumentReady}
          onFocus={handleSurfaceFocus}
          onReady={handleWebviewReady}
          partition={BROWSER_WEBVIEW_PARTITION}
          surfaceId={viewId}
          viewport={viewport}
        />
        {!currentUrl && (
          <div className="browser-panel__empty">
            <Globe2 aria-hidden="true" />
            <h2>{t('browser.emptyTitle')}</h2>
            <p>{t('browser.emptyDescription')}</p>
          </div>
        )}
        {hostFallbackError && (
          <div className="browser-panel__fallback" role="status">
            <WifiOff aria-hidden="true" />
            <h2>{hostFallbackError.heading}</h2>
            <p>{hostFallbackError.summary}</p>
            <ul>
              {hostFallbackError.suggestions.map((suggestion) => (
                <li key={suggestion}>{suggestion}</li>
              ))}
            </ul>
            <code>{hostFallbackError.errorDescription}</code>
            <button type="button" onClick={() => void reload()}>
              {t('browser.reload')}
            </button>
          </div>
        )}
        {hostFallbackCrashError && (
          <div className="browser-panel__fallback" role="status">
            <MonitorX aria-hidden="true" />
            <h2>{hostFallbackCrashError.heading}</h2>
            <p>{hostFallbackCrashError.summary}</p>
            <button type="button" onClick={() => void reload()}>
              {hostFallbackCrashError.actionLabel}
            </button>
          </div>
        )}
      </div>
    </section>
  )
}

function measureVisibleWebviewViewport(webview: WebviewTag): { height: number; width: number } {
  const guestBounds = webview.getBoundingClientRect()
  const contentBounds = webview.closest('.browser-panel__content')?.getBoundingClientRect()
  const left = Math.max(guestBounds.left, contentBounds?.left ?? 0, 0)
  const top = Math.max(guestBounds.top, contentBounds?.top ?? 0, 0)
  const right = Math.min(
    guestBounds.right,
    contentBounds?.right ?? window.innerWidth,
    window.innerWidth
  )
  const bottom = Math.min(
    guestBounds.bottom,
    contentBounds?.bottom ?? window.innerHeight,
    window.innerHeight
  )
  return {
    height: Math.round(Math.max(0, bottom - top)),
    width: Math.round(Math.max(0, right - left))
  }
}
