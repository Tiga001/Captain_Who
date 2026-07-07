// Renderer browser UI.
import { hostClient } from '../../host/hostClient'
import type {
  BrowserBounds,
  BrowserCreateViewRequest,
  BrowserNavigateRequest,
  BrowserNavigationState,
  BrowserPageMetadata,
  BrowserViewEvent,
  BrowserViewId,
  BrowserZoomState
} from '@mycopilot/protocol'

export type {
  BrowserBounds,
  BrowserCreateViewRequest,
  BrowserNavigateRequest,
  BrowserNavigationState,
  BrowserPageMetadata,
  BrowserViewEvent,
  BrowserViewId,
  BrowserZoomState
}

export function createBrowserView(
  request: BrowserCreateViewRequest
): Promise<BrowserNavigationState> {
  return hostClient.browser.createView(request)
}

export function destroyBrowserView(id: BrowserViewId): Promise<void> {
  return hostClient.browser.destroyView(id)
}

export function setBrowserViewBounds(id: BrowserViewId, bounds: BrowserBounds): Promise<void> {
  return hostClient.browser.setBounds(id, bounds)
}

export function showBrowserView(id: BrowserViewId): Promise<BrowserNavigationState> {
  return hostClient.browser.showView(id)
}

export function hideBrowserView(id: BrowserViewId): Promise<void> {
  return hostClient.browser.hideView(id)
}

export function navigateBrowserView(
  request: BrowserNavigateRequest
): Promise<BrowserNavigationState> {
  return hostClient.browser.navigate(request)
}

export function reloadBrowserView(id: BrowserViewId): Promise<BrowserNavigationState> {
  return hostClient.browser.reload(id)
}

export function goBackBrowserView(id: BrowserViewId): Promise<BrowserNavigationState> {
  return hostClient.browser.goBack(id)
}

export function goForwardBrowserView(id: BrowserViewId): Promise<BrowserNavigationState> {
  return hostClient.browser.goForward(id)
}

export function setBrowserViewZoom(
  id: BrowserViewId,
  zoomFactor: number
): Promise<BrowserZoomState> {
  return hostClient.browser.setZoom(id, zoomFactor)
}

export function clearBrowserViewBrowsingData(id: BrowserViewId): Promise<void> {
  return hostClient.browser.clearBrowsingData(id)
}

export function listenToBrowserViewEvents(handler: (event: BrowserViewEvent) => void): () => void {
  return hostClient.browser.onEvent(handler)
}
