// Protocol layer.
export const RESOURCES_RESOLVE_FAVICON_METHOD = 'resources.resolveFavicon'

export interface ResourceFaviconRequest {
  pageUrl: string
  faviconUrl?: string | null
}

export interface ResourceFaviconResponse {
  url: string | null
}
