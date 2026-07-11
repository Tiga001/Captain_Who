export type BrowserViewId = string

export interface BrowserBounds {
  height: number
  width: number
  x: number
  y: number
}

export interface BrowserCreateViewRequest {
  id?: BrowserViewId
  url?: string
}

export interface BrowserNavigateRequest {
  id: BrowserViewId
  url: string
}

export interface BrowserPageMetadata {
  iconUrl: string | null
  title: string | null
  url: string | null
}

export interface BrowserNavigationState {
  canGoBack: boolean
  canGoForward: boolean
  errorText?: string | null
  id: BrowserViewId
  isLoading: boolean
  metadata: BrowserPageMetadata
}

export interface BrowserZoomState {
  id: BrowserViewId
  zoomFactor: number
}

export type BrowserViewEvent =
  | {
      state: BrowserNavigationState
      type: 'browser.navigation'
    }
  | {
      state: BrowserNavigationState
      type: 'browser.loading'
    }
  | {
      metadata: BrowserPageMetadata
      state: BrowserNavigationState
      type: 'browser.metadata'
    }
  | {
      errorCode: number
      errorDescription: string
      state: BrowserNavigationState
      type: 'browser.failedLoad'
      validatedUrl: string
    }
  | {
      state: BrowserZoomState
      type: 'browser.zoom'
    }
  | {
      id: BrowserViewId
      type: 'browser.destroyed'
    }
