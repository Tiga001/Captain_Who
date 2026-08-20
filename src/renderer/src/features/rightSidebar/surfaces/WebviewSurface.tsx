import { useEffect, useLayoutEffect, useRef } from 'react'
import type { WebviewTag } from 'electron'
import './WebviewSurface.css'

interface WebviewSurfaceProps {
  accessibleTitle: string
  initialUrl?: string
  isActive: boolean
  isVisible: boolean
  openLinksInSameSurface?: boolean
  onDocumentReady?: (webview: WebviewTag) => void
  onFocus?: () => void
  onReady: (webview: WebviewTag | null) => void
  partition: string
  surfaceId: string
  viewport?: { height: number; width: number }
}

interface WebviewSurfaceCallbacks {
  onDocumentReady?: (webview: WebviewTag) => void
  onFocus?: () => void
  onReady: (webview: WebviewTag | null) => void
}

export function WebviewSurface({
  accessibleTitle,
  initialUrl = 'about:blank',
  isActive,
  isVisible,
  openLinksInSameSurface = false,
  onDocumentReady,
  onFocus,
  onReady,
  partition,
  surfaceId,
  viewport
}: WebviewSurfaceProps) {
  const hostRef = useRef<HTMLDivElement>(null)
  const webviewRef = useRef<WebviewTag | null>(null)
  const callbacksRef = useRef<WebviewSurfaceCallbacks>({ onDocumentReady, onFocus, onReady })

  useLayoutEffect(() => {
    callbacksRef.current = { onDocumentReady, onFocus, onReady }
  }, [onDocumentReady, onFocus, onReady])

  useLayoutEffect(() => {
    const host = hostRef.current
    if (!host) return undefined

    const webview = document.createElement('webview')
    let isDisposed = false
    let attached = false
    let documentReadyObserved = false
    let documentReadyReported = false
    const reportDocumentReady = () => {
      if (isDisposed || !attached || !documentReadyObserved || documentReadyReported) return
      documentReadyReported = true
      callbacksRef.current.onDocumentReady?.(webview)
    }
    const handleAttached = () => {
      if (isDisposed) return
      attached = true
      callbacksRef.current.onReady(webview)
      reportDocumentReady()
    }
    const handleDocumentReady = () => {
      if (isDisposed) return
      documentReadyObserved = true
      reportDocumentReady()
    }
    const handleFocus = () => {
      callbacksRef.current.onFocus?.()
    }

    webview.className = 'webview-surface__guest'
    webview.setAttribute('data-surface-id', surfaceId)
    webview.setAttribute('partition', partition)
    webview.setAttribute(
      'webpreferences',
      'contextIsolation=yes, nodeIntegration=no, sandbox=yes, webSecurity=yes'
    )
    if (openLinksInSameSurface) webview.setAttribute('allowpopups', '')
    webview.setAttribute('src', initialUrl)
    webview.addEventListener('did-attach', handleAttached)
    webview.addEventListener('dom-ready', handleDocumentReady)
    webview.addEventListener('did-finish-load', handleDocumentReady)
    webview.addEventListener('focus', handleFocus)
    host.replaceChildren(webview)
    webviewRef.current = webview

    return () => {
      isDisposed = true
      webview.removeEventListener('did-attach', handleAttached)
      webview.removeEventListener('dom-ready', handleDocumentReady)
      webview.removeEventListener('did-finish-load', handleDocumentReady)
      webview.removeEventListener('focus', handleFocus)
      callbacksRef.current.onReady(null)
      webviewRef.current = null
      webview.remove()
    }
  }, [initialUrl, openLinksInSameSurface, partition, surfaceId])

  useLayoutEffect(() => {
    webviewRef.current?.setAttribute('aria-label', accessibleTitle)
  }, [accessibleTitle])

  useEffect(() => {
    if (!isActive) webviewRef.current?.blur()
  }, [isActive])

  return (
    <div
      className="webview-surface"
      data-active={isActive ? 'true' : undefined}
      data-visible={isVisible ? 'true' : undefined}
      ref={hostRef}
      style={
        viewport
          ? {
              height: `${viewport.height}px`,
              inset: 'auto',
              left: 0,
              top: 0,
              width: `${viewport.width}px`
            }
          : undefined
      }
    />
  )
}
