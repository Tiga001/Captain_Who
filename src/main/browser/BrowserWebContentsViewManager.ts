// Electron main Browser WebContentsView manager.
import { randomUUID } from 'node:crypto'
import { BrowserWindow, WebContentsView, type Rectangle } from 'electron'

import type {
  BrowserBounds,
  BrowserCreateViewRequest,
  BrowserNavigateRequest,
  BrowserNavigationState,
  BrowserViewEvent,
  BrowserViewId,
  BrowserZoomState
} from '@mycopilot/protocol'

interface BrowserViewRecord {
  state: BrowserNavigationState
  view: WebContentsView
  zoomFactor: number
}

const BROWSER_EVENT_CHANNEL = 'host:browser.event'
const BROWSER_SESSION_PARTITION = 'persist:mycopilot-browser'
const MIN_BROWSER_VIEW_HEIGHT = 1
const MIN_BROWSER_VIEW_WIDTH = 1
const SAFE_HTTP_PROTOCOLS = new Set(['http:', 'https:'])

export class BrowserWebContentsViewManager {
  private readonly views = new Map<BrowserViewId, BrowserViewRecord>()

  constructor(private readonly ownerWindow: BrowserWindow) {}

  async createView(request: BrowserCreateViewRequest = {}): Promise<BrowserNavigationState> {
    const id = normalizeBrowserViewId(request.id)
    const existingRecord = this.views.get(id)
    if (existingRecord) {
      if (request.url) {
        return this.navigate({ id, url: request.url })
      }

      return existingRecord.state
    }

    const view = new WebContentsView({
      webPreferences: {
        contextIsolation: true,
        nodeIntegration: false,
        partition: BROWSER_SESSION_PARTITION,
        sandbox: true,
        webSecurity: true
      }
    })

    view.setVisible(false)
    view.setBounds({ height: 0, width: 0, x: 0, y: 0 })
    this.ownerWindow.contentView.addChildView(view)

    const record: BrowserViewRecord = {
      state: createInitialNavigationState(id),
      view,
      zoomFactor: 1
    }
    this.views.set(id, record)
    this.configureView(record)

    if (request.url) {
      return this.navigate({ id, url: request.url })
    }

    return record.state
  }

  async destroyView(id: BrowserViewId): Promise<void> {
    const record = this.views.get(id)
    if (!record) return

    record.view.setVisible(false)
    this.ownerWindow.contentView.removeChildView(record.view)
    record.view.webContents.close()
    this.views.delete(id)
    this.emit({ id, type: 'browser.destroyed' })
  }

  destroyAll(): void {
    for (const id of [...this.views.keys()]) {
      void this.destroyView(id)
    }
  }

  async setBounds(id: BrowserViewId, bounds: BrowserBounds): Promise<void> {
    const record = this.requireRecord(id)
    record.view.setBounds(toElectronBounds(bounds))
  }

  async showView(id: BrowserViewId): Promise<BrowserNavigationState> {
    const record = this.requireRecord(id)
    const bounds = record.view.getBounds()
    const shouldShow = bounds.width > 0 && bounds.height > 0

    record.view.setVisible(shouldShow)
    if (shouldShow) {
      this.ownerWindow.contentView.addChildView(record.view)
    }

    return this.refreshNavigationState(record)
  }

  async hideView(id: BrowserViewId): Promise<void> {
    const record = this.views.get(id)
    if (!record) return

    record.view.setVisible(false)
  }

  async navigate(request: BrowserNavigateRequest): Promise<BrowserNavigationState> {
    const record = this.requireRecord(request.id)
    const url = normalizeAllowedHttpUrl(request.url)

    this.updateRecordState(record, {
      errorText: null,
      isLoading: true,
      metadata: {
        ...record.state.metadata,
        iconUrl: null,
        title: getFallbackPageTitle(url),
        url
      }
    })
    this.emit({ state: record.state, type: 'browser.navigation' })

    try {
      await record.view.webContents.loadURL(url)
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      this.updateRecordState(record, {
        errorText: message,
        isLoading: false
      })
      this.emit({
        errorCode: 0,
        errorDescription: message,
        state: record.state,
        type: 'browser.failedLoad',
        validatedUrl: url
      })
    }

    return this.refreshNavigationState(record)
  }

  async reload(id: BrowserViewId): Promise<BrowserNavigationState> {
    const record = this.requireRecord(id)
    record.view.webContents.reload()
    return this.refreshNavigationState(record)
  }

  async goBack(id: BrowserViewId): Promise<BrowserNavigationState> {
    const record = this.requireRecord(id)
    if (record.view.webContents.navigationHistory.canGoBack()) {
      record.view.webContents.navigationHistory.goBack()
    }
    return this.refreshNavigationState(record)
  }

  async goForward(id: BrowserViewId): Promise<BrowserNavigationState> {
    const record = this.requireRecord(id)
    if (record.view.webContents.navigationHistory.canGoForward()) {
      record.view.webContents.navigationHistory.goForward()
    }
    return this.refreshNavigationState(record)
  }

  async setZoom(id: BrowserViewId, zoomFactor: number): Promise<BrowserZoomState> {
    const record = this.requireRecord(id)
    const normalizedZoomFactor = clampZoomFactor(zoomFactor)
    record.zoomFactor = normalizedZoomFactor
    record.view.webContents.setZoomFactor(normalizedZoomFactor)

    const state = {
      id,
      zoomFactor: normalizedZoomFactor
    }
    this.emit({ state, type: 'browser.zoom' })

    return state
  }

  async clearBrowsingData(id: BrowserViewId): Promise<void> {
    const record = this.requireRecord(id)
    await Promise.all([
      record.view.webContents.session.clearCache(),
      record.view.webContents.session.clearStorageData()
    ])
  }

  private configureView(record: BrowserViewRecord): void {
    const { view } = record
    const { webContents } = view

    webContents.setWindowOpenHandler((details) => {
      if (isAllowedHttpUrl(details.url)) {
        void this.navigate({ id: record.state.id, url: details.url })
      } else {
        this.blockUnsupportedNavigation(record, details.url)
      }

      return { action: 'deny' }
    })

    webContents.on('will-navigate', (event, url) => {
      if (isAllowedHttpUrl(url)) return

      event.preventDefault()
      this.blockUnsupportedNavigation(record, url)
    })

    webContents.on('page-title-updated', (_event, title) => {
      const currentUrl = webContents.getURL() || record.state.metadata.url
      const nextTitle = title.trim() || getFallbackPageTitle(currentUrl)
      this.updateRecordState(record, {
        metadata: {
          ...record.state.metadata,
          url: currentUrl,
          title: nextTitle
        }
      })
      this.emitMetadata(record)
    })

    webContents.on('page-favicon-updated', (_event, favicons) => {
      this.updateRecordState(record, {
        metadata: {
          ...record.state.metadata,
          iconUrl: favicons[0] ?? null
        }
      })
      this.emitMetadata(record)
    })

    webContents.on('did-navigate', (_event, url) => {
      this.updateRecordState(record, {
        errorText: null,
        metadata: {
          ...record.state.metadata,
          title: record.state.metadata.title || getFallbackPageTitle(url),
          url
        }
      })
      this.emitMetadata(record)
    })

    webContents.on('did-navigate-in-page', (_event, url, isMainFrame) => {
      if (!isMainFrame) return

      this.updateRecordState(record, {
        metadata: {
          ...record.state.metadata,
          url
        }
      })
      this.emitMetadata(record)
    })

    webContents.on('did-start-loading', () => {
      this.updateRecordState(record, {
        isLoading: true
      })
      this.emit({ state: record.state, type: 'browser.loading' })
    })

    webContents.on('did-stop-loading', () => {
      this.updateRecordState(record, {
        isLoading: false
      })
      this.emit({ state: this.refreshNavigationState(record), type: 'browser.loading' })
    })

    webContents.on(
      'did-fail-load',
      (_event, errorCode, errorDescription, validatedUrl, isMainFrame) => {
        if (!isMainFrame || errorCode === -3) return

        this.updateRecordState(record, {
          errorText: errorDescription,
          isLoading: false,
          metadata: {
            ...record.state.metadata,
            title: getFallbackPageTitle(validatedUrl),
            url: validatedUrl || record.state.metadata.url
          }
        })
        this.emit({
          errorCode,
          errorDescription,
          state: this.refreshNavigationState(record),
          type: 'browser.failedLoad',
          validatedUrl
        })
      }
    )
  }

  private blockUnsupportedNavigation(record: BrowserViewRecord, url: string): void {
    this.updateRecordState(record, {
      errorText: `Blocked unsupported browser URL: ${url}`,
      isLoading: false
    })
    this.emit({
      errorCode: 0,
      errorDescription: record.state.errorText ?? 'Blocked unsupported browser URL',
      state: record.state,
      type: 'browser.failedLoad',
      validatedUrl: url
    })
  }

  private emitMetadata(record: BrowserViewRecord): void {
    this.emit({
      metadata: record.state.metadata,
      state: this.refreshNavigationState(record),
      type: 'browser.metadata'
    })
  }

  private refreshNavigationState(record: BrowserViewRecord): BrowserNavigationState {
    const { webContents } = record.view
    this.updateRecordState(record, {
      canGoBack: webContents.navigationHistory.canGoBack(),
      canGoForward: webContents.navigationHistory.canGoForward(),
      isLoading: webContents.isLoading()
    })

    return record.state
  }

  private updateRecordState(
    record: BrowserViewRecord,
    patch: Partial<BrowserNavigationState>
  ): void {
    record.state = {
      ...record.state,
      ...patch,
      metadata: patch.metadata ?? record.state.metadata
    }
  }

  private requireRecord(id: BrowserViewId): BrowserViewRecord {
    const record = this.views.get(id)
    if (!record) {
      throw new Error(`Browser view not found: ${id}`)
    }

    return record
  }

  private emit(event: BrowserViewEvent): void {
    if (!this.ownerWindow.isDestroyed()) {
      this.ownerWindow.webContents.send(BROWSER_EVENT_CHANNEL, event)
    }
  }
}

function createInitialNavigationState(id: BrowserViewId): BrowserNavigationState {
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

function normalizeBrowserViewId(id: BrowserViewId | undefined): BrowserViewId {
  const normalizedId = id?.trim() || `browser-${randomUUID()}`
  if (!/^[A-Za-z0-9._:-]{1,160}$/.test(normalizedId)) {
    throw new Error('Invalid browser view id')
  }

  return normalizedId
}

function toElectronBounds(bounds: BrowserBounds): Rectangle {
  return {
    height: normalizeBoundsValue(bounds.height, MIN_BROWSER_VIEW_HEIGHT),
    width: normalizeBoundsValue(bounds.width, MIN_BROWSER_VIEW_WIDTH),
    x: normalizeBoundsValue(bounds.x, 0),
    y: normalizeBoundsValue(bounds.y, 0)
  }
}

function normalizeBoundsValue(value: number, minimum: number): number {
  if (!Number.isFinite(value)) {
    return minimum
  }

  return Math.max(minimum, Math.round(value))
}

function normalizeAllowedHttpUrl(input: string): string {
  const url = new URL(input)
  if (!SAFE_HTTP_PROTOCOLS.has(url.protocol)) {
    throw new Error(`Unsupported browser URL protocol: ${url.protocol}`)
  }

  return url.href
}

function isAllowedHttpUrl(input: string): boolean {
  try {
    normalizeAllowedHttpUrl(input)
    return true
  } catch {
    return false
  }
}

function getFallbackPageTitle(url: string | null): string | null {
  if (!url) return null

  try {
    return new URL(url).hostname.replace(/^www\./, '') || url
  } catch {
    return url
  }
}

function clampZoomFactor(value: number): number {
  if (!Number.isFinite(value)) return 1

  return Math.min(3, Math.max(0.3, value))
}
