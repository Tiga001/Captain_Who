import { useCallback, useEffect, useRef, useState } from 'react'
import {
  ArrowLeft,
  ArrowRight,
  CornerDownRight,
  Globe2,
  Minus,
  MoreVertical,
  Plus,
  RefreshCw
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
import { browserSurfaceIdForPage } from './browserSurface'
import './BrowserPanel.css'

interface BrowserPanelProps {
  automationRequestId?: string
  isActive: boolean
  onAutomationSurfaceReady?: (surfaceId: string, requestId: string) => void
  onPageMetadataChange?: (metadata: BrowserPageMetadata) => void
  onSurfaceFocus?: () => void
  pageId: string
}

const ZOOM_STEP = 0.1

function clampZoom(value: number) {
  return Math.min(3, Math.max(0.3, value))
}

export function BrowserPanel({
  automationRequestId,
  isActive,
  onAutomationSurfaceReady,
  onPageMetadataChange,
  onSurfaceFocus,
  pageId
}: BrowserPanelProps) {
  const { t } = useFrontendConfig()
  const menuAnchorRef = useRef<HTMLDivElement>(null)
  const webviewRef = useRef<WebviewTag | null>(null)
  const submittedAutomationRequestRef = useRef<string | null>(null)
  const [addressValue, setAddressValue] = useState('')
  const [isAddressEditing, setIsAddressEditing] = useState(false)
  const [isMenuOpen, setIsMenuOpen] = useState(false)
  const [zoom, setZoomState] = useState(1)
  const viewId = browserSurfaceIdForPage(pageId)
  const {
    clearBrowsingData,
    currentUrl,
    errorMessage,
    goBack,
    goForward,
    isLoaded,
    navigationState,
    navigateToUrl,
    reload,
    setWebview,
    setZoom
  } = useBrowserWebview({ isActive })

  const closeMenu = useCallback(() => setIsMenuOpen(false), [])
  const handleSurfaceFocus = useCallback(() => {
    closeMenu()
    onSurfaceFocus?.()
  }, [closeMenu, onSurfaceFocus])
  useDismissOnOutsidePointer(menuAnchorRef, isMenuOpen, closeMenu)

  const reportAutomationSurfaceReady = useCallback(
    (webview: WebviewTag | null): void => {
      if (
        !webview ||
        !automationRequestId ||
        submittedAutomationRequestRef.current === automationRequestId
      ) {
        return
      }
      submittedAutomationRequestRef.current = automationRequestId
      onAutomationSurfaceReady?.(viewId, automationRequestId)
    },
    [automationRequestId, onAutomationSurfaceReady, viewId]
  )

  const handleWebviewReady = useCallback(
    (webview: WebviewTag | null): void => {
      webviewRef.current = webview
      setWebview(webview)
      reportAutomationSurfaceReady(webview)
    },
    [reportAutomationSurfaceReady, setWebview]
  )

  useEffect(() => {
    if (!automationRequestId) {
      submittedAutomationRequestRef.current = null
      return
    }
    reportAutomationSurfaceReady(webviewRef.current)
  }, [automationRequestId, reportAutomationSurfaceReady])

  useEffect(() => {
    if (!isActive) {
      setIsMenuOpen(false)
    }
  }, [isActive])

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

        <div className="browser-panel__menu-anchor" ref={menuAnchorRef}>
          <button
            className="browser-panel__icon-button"
            type="button"
            aria-label={t('browser.menu')}
            aria-expanded={isMenuOpen}
            onClick={() => setIsMenuOpen((current) => !current)}
          >
            <MoreVertical aria-hidden="true" />
          </button>

          {isMenuOpen && (
            <div className="browser-panel__menu">
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
            </div>
          )}
        </div>
      </header>

      <div className="browser-panel__content">
        <WebviewSurface
          accessibleTitle={t('browser.title')}
          initialUrl={createBrowserSurfaceBootstrapUrl(viewId)}
          isActive={isActive}
          isVisible={Boolean(currentUrl)}
          openLinksInSameSurface
          onFocus={handleSurfaceFocus}
          onReady={handleWebviewReady}
          partition={BROWSER_WEBVIEW_PARTITION}
          surfaceId={viewId}
        />
        {!currentUrl && (
          <div className="browser-panel__empty">
            <Globe2 aria-hidden="true" />
            <h2>{t('browser.emptyTitle')}</h2>
            <p>{t('browser.emptyDescription')}</p>
          </div>
        )}
        {errorMessage && <div className="browser-panel__error">{errorMessage}</div>}
      </div>
    </section>
  )
}
