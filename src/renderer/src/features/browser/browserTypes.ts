export interface BrowserPageMetadata {
  iconUrl: string | null
  title: string | null
  url: string | null
}

export interface BrowserNavigationState {
  canGoBack: boolean
  canGoForward: boolean
  isLoading: boolean
  metadata: BrowserPageMetadata
}
