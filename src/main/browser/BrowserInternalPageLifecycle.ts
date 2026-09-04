import type { BrowserInternalPageStoreLike } from './BrowserInternalPageStore'
import type { BrowserNetworkGuard } from './BrowserNetworkGuard'
import type { BrowserSurfaceCrashError, BrowserSurfaceLoadError } from './BrowserLoadErrorPage'
import {
  INTERNAL_ERROR_PAGE_LOAD_TIMEOUT_MS,
  INTERNAL_ERROR_PAGE_MAX_ATTEMPTS,
  INTERNAL_ERROR_PAGE_RETRY_DELAY_MS,
  InternalPageLoadAttemptTimeoutError,
  isAbortedBrowserNavigationError,
  isNavigationAlreadyPendingError,
  MAX_INTERNAL_DOCUMENTS_PER_SURFACE,
  safeActiveHistoryEntry,
  safeActiveHistoryIndex,
  safeGuestBoolean,
  safeHistoryEntries,
  safeRemoveHistoryEntry,
  safeRemoveHistoryEntryByUrl
} from './BrowserSurfaceHelpers'
import type {
  BrowserInternalDocument,
  InternalPageLoad,
  ManagedSurface
} from './BrowserSurfaceTypes'

interface BrowserInternalPageLifecycleOptions {
  isManagedSurfaceCurrent: (surface: ManagedSurface) => boolean
  networkGuard?: BrowserNetworkGuard
  publishSurfaceState: (surface: ManagedSurface) => void
  store: BrowserInternalPageStoreLike
}

/** Owns the retry and retention lifecycle for host-generated browser error documents. */
export class BrowserInternalPageLifecycle {
  private documentSequence = 0
  private readonly isManagedSurfaceCurrent: (surface: ManagedSurface) => boolean
  private readonly networkGuard?: BrowserNetworkGuard
  private readonly publishSurfaceState: (surface: ManagedSurface) => void
  private readonly store: BrowserInternalPageStoreLike

  constructor(options: BrowserInternalPageLifecycleOptions) {
    this.isManagedSurfaceCurrent = options.isManagedSurfaceCurrent
    this.networkGuard = options.networkGuard
    this.publishSurfaceState = options.publishSurfaceState
    this.store = options.store
  }

  begin(surface: ManagedSurface, document: BrowserInternalDocument): void {
    this.cancel(surface)
    const load: InternalPageLoad = { attempt: 0, document }
    surface.internalPageLoad = load
    this.scheduleAttempt(surface, load, 0)
  }

  finish(surface: ManagedSurface, load: InternalPageLoad): void {
    if (load.retryTimer) clearTimeout(load.retryTimer)
    load.retryTimer = undefined
    load.lease?.finish()
    load.lease = undefined
    if (surface.internalPageLoad === load) surface.internalPageLoad = undefined
  }

  cancel(surface: ManagedSurface): void {
    const load = surface.internalPageLoad
    if (load) this.finish(surface, load)
  }

  registerLoadError(
    surface: ManagedSurface,
    loadError: BrowserSurfaceLoadError
  ): BrowserInternalDocument {
    const materialized = this.store.register(loadError.internalPageHtml)
    return this.registerDocument(surface, {
      actionUrl: loadError.internalActionUrl,
      createdSequence: ++this.documentSequence,
      generation: surface.generation,
      internalPageUrl: materialized.url,
      loadError,
      logicalUrl: loadError.failedUrl,
      navigationEpoch: surface.navigationEpoch
    })
  }

  registerCrashError(
    surface: ManagedSurface,
    crashError: BrowserSurfaceCrashError
  ): BrowserInternalDocument {
    const materialized = this.store.register(crashError.internalPageHtml)
    return this.registerDocument(surface, {
      actionUrl: crashError.internalActionUrl,
      crashError,
      createdSequence: ++this.documentSequence,
      generation: surface.generation,
      internalPageUrl: materialized.url,
      logicalUrl: surface.logicalUrl,
      navigationEpoch: surface.navigationEpoch
    })
  }

  activate(surface: ManagedSurface, document: BrowserInternalDocument): void {
    surface.logicalUrl = document.logicalUrl
    surface.pendingNavigationUrl = document.logicalUrl
    surface.logicalFaviconUrl = null
    if (document.loadError) {
      surface.loadError = { ...document.loadError, navigationEpoch: surface.navigationEpoch }
      surface.crashError = undefined
      surface.logicalTitle = document.loadError.title
      surface.presentation = 'error-page'
      return
    }
    if (document.crashError) {
      surface.loadError = undefined
      surface.crashError = { ...document.crashError, navigationEpoch: surface.navigationEpoch }
      surface.logicalTitle = document.crashError.title
      surface.presentation = 'crash-page'
    }
  }

  rememberReplaceableHistoryEntry(surface: ManagedSurface, expectedUrl: string): void {
    const entry = safeActiveHistoryEntry(surface.guest)
    if (!entry || entry.url !== expectedUrl) return
    if (
      surface.pendingHistoryRemovals.some(
        (pending) => pending.index === entry.index && pending.url === entry.url
      )
    ) {
      return
    }
    surface.pendingHistoryRemovals.push(entry)
  }

  settlePendingHistoryRemoval(surface: ManagedSurface): void {
    if (surface.pendingHistoryRemovals.length === 0) return
    const pendingEntries = surface.pendingHistoryRemovals
      .splice(0)
      .sort((left, right) => right.index - left.index)
    for (const pending of pendingEntries) {
      const activeIndex = safeActiveHistoryIndex(surface.guest)
      const entries = safeHistoryEntries(surface.guest)
      let exactIndex = entries[pending.index]?.url === pending.url ? pending.index : -1
      if (exactIndex < 0 && surface.internalDocuments.has(pending.url)) {
        exactIndex = entries.findIndex(
          (entry, index) => index !== activeIndex && entry.url === pending.url
        )
      }
      if (exactIndex < 0 || exactIndex === activeIndex) continue
      if (
        safeRemoveHistoryEntry(surface.guest, exactIndex) &&
        surface.internalDocuments.has(pending.url)
      ) {
        this.forget(surface, pending.url)
      }
    }
  }

  prune(surface: ManagedSurface): void {
    if (surface.internalDocuments.size <= MAX_INTERNAL_DOCUMENTS_PER_SURFACE) return
    const retainedUrls = new Set(safeHistoryEntries(surface.guest).map((entry) => entry.url))
    retainedUrls.add(surface.guest.getURL())
    if (surface.internalPageLoad) {
      retainedUrls.add(surface.internalPageLoad.document.internalPageUrl)
    }
    const oldest = [...surface.internalDocuments.values()].sort(
      (left, right) => left.createdSequence - right.createdSequence
    )
    for (const document of oldest) {
      if (surface.internalDocuments.size <= MAX_INTERNAL_DOCUMENTS_PER_SURFACE) break
      if (document.internalPageUrl === surface.guest.getURL()) continue
      if (
        retainedUrls.has(document.internalPageUrl) &&
        !safeRemoveHistoryEntryByUrl(surface.guest, document.internalPageUrl)
      ) {
        continue
      }
      this.forget(surface, document.internalPageUrl)
    }
  }

  forget(surface: ManagedSurface, url: string): void {
    surface.internalDocuments.delete(url)
    this.networkGuard?.forgetInternalNavigation(surface.guest, url)
    this.store.release(url)
  }

  private registerDocument(
    surface: ManagedSurface,
    document: BrowserInternalDocument
  ): BrowserInternalDocument {
    surface.internalDocuments.set(document.internalPageUrl, document)
    return document
  }

  private scheduleAttempt(surface: ManagedSurface, load: InternalPageLoad, delayMs: number): void {
    if (!this.isCurrentLoad(surface, load)) return
    if (load.retryTimer) clearTimeout(load.retryTimer)
    load.retryTimer = setTimeout(() => {
      load.retryTimer = undefined
      void this.tryLoad(surface, load).catch(() => {
        this.fail(surface, load)
      })
    }, delayMs)
  }

  private async tryLoad(surface: ManagedSurface, load: InternalPageLoad): Promise<void> {
    if (!this.isCurrentLoad(surface, load)) return
    load.attempt += 1
    if (safeGuestBoolean(surface.guest, 'isLoadingMainFrame')) {
      if (load.attempt < INTERNAL_ERROR_PAGE_MAX_ATTEMPTS) {
        this.scheduleAttempt(surface, load, INTERNAL_ERROR_PAGE_RETRY_DELAY_MS)
      } else {
        this.fail(surface, load)
      }
      return
    }

    try {
      load.lease = this.networkGuard?.beginInternalNavigation({
        generation: surface.generation,
        guest: surface.guest,
        url: load.document.internalPageUrl
      })
      let timeout: ReturnType<typeof setTimeout> | undefined
      try {
        await Promise.race([
          surface.guest.loadURL(load.document.internalPageUrl),
          new Promise<never>((_resolve, reject) => {
            timeout = setTimeout(
              () => reject(new InternalPageLoadAttemptTimeoutError()),
              INTERNAL_ERROR_PAGE_LOAD_TIMEOUT_MS
            )
          })
        ])
      } finally {
        if (timeout) clearTimeout(timeout)
      }
      if (
        this.isCurrentLoad(surface, load) &&
        surface.guest.getURL() === load.document.internalPageUrl
      ) {
        this.finish(surface, load)
        surface.navigationInProgress = false
        this.activate(surface, load.document)
        this.settlePendingHistoryRemoval(surface)
        this.prune(surface)
        this.publishSurfaceState(surface)
      }
    } catch (error) {
      load.lease?.finish()
      load.lease = undefined
      if (!this.isCurrentLoad(surface, load)) return
      if (
        load.attempt < INTERNAL_ERROR_PAGE_MAX_ATTEMPTS &&
        (error instanceof InternalPageLoadAttemptTimeoutError ||
          isNavigationAlreadyPendingError(error) ||
          isAbortedBrowserNavigationError(error))
      ) {
        this.scheduleAttempt(surface, load, INTERNAL_ERROR_PAGE_RETRY_DELAY_MS)
        return
      }
      this.fail(surface, load)
    }
  }

  private fail(surface: ManagedSurface, load: InternalPageLoad): void {
    if (!this.isCurrentLoad(surface, load)) return
    this.finish(surface, load)
    surface.navigationInProgress = false
    surface.presentation = 'host-fallback'
    this.publishSurfaceState(surface)
  }

  private isCurrentLoad(surface: ManagedSurface, load: InternalPageLoad): boolean {
    return Boolean(
      this.isManagedSurfaceCurrent(surface) &&
      !surface.guest.isDestroyed() &&
      surface.internalPageLoad === load &&
      surface.internalDocuments.get(load.document.internalPageUrl) === load.document &&
      surface.generation === load.document.generation &&
      surface.navigationEpoch === load.document.navigationEpoch
    )
  }
}
