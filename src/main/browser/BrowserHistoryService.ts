import { randomUUID } from 'node:crypto'
import {
  BROWSER_DATA_SCHEMA_VERSION,
  type BrowserHistoryEntry,
  type BrowserHistoryMetadataUpdateInput
} from '@mycopilot/protocol'

import { historyHostnameForUrl } from './BrowserSurfaceHelpers'
import type { CoreServer } from '../core/coreServer'
import type { BrowserSurfaceHistoryEvent } from './BrowserSurfaceManager'

interface CurrentHistoryEntry {
  historyId: string
  url: string
}

/** Persists user-visible HTTP(S) and local file navigation metadata. It is never connected to Agent output. */
export class BrowserHistoryService {
  private readonly coreServer: CoreServer
  private readonly currentBySurface = new Map<string, CurrentHistoryEntry>()
  private readonly pendingBySurface = new Map<string, Promise<void>>()
  private readonly changedListeners = new Set<() => void>()

  constructor(coreServer: CoreServer) {
    this.coreServer = coreServer
  }

  onChanged(listener: () => void): () => void {
    this.changedListeners.add(listener)
    return () => this.changedListeners.delete(listener)
  }

  forgetSurface(input: Pick<BrowserSurfaceHistoryEvent, 'surfaceId' | 'generation'>): void {
    this.currentBySurface.delete(surfaceKey(input))
  }

  recordNavigation(input: BrowserSurfaceHistoryEvent): void {
    const normalized = normalizeHistoryUrl(input.url)
    if (!normalized) return
    const key = surfaceKey(input)
    const historyId = `browser-history:${randomUUID()}`
    const title = normalizeTitle(input.title, normalized)
    const entry: BrowserHistoryEntry = {
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      historyId,
      url: normalized.toString(),
      title,
      hostname: historyHostnameForUrl(normalized),
      faviconUrl: normalizeRemoteUrl(input.faviconUrl),
      visitedAt: input.visitedAt
    }
    this.currentBySurface.set(key, { historyId, url: entry.url })
    this.enqueue(key, async () => {
      await this.coreServer.registerBrowserHistory(entry)
      this.notifyChanged()
    })
  }

  updateMetadata(input: BrowserSurfaceHistoryEvent): void {
    const normalized = normalizeHistoryUrl(input.url)
    if (!normalized) return
    const key = surfaceKey(input)
    const current = this.currentBySurface.get(key)
    if (!current || current.url !== normalized.toString()) return
    const update: BrowserHistoryMetadataUpdateInput = {
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      historyId: current.historyId,
      title: normalizeTitle(input.title, normalized),
      faviconUrl: normalizeRemoteUrl(input.faviconUrl)
    }
    this.enqueue(key, async () => {
      const changed = await this.coreServer.updateBrowserHistoryMetadata(update)
      if (changed) this.notifyChanged()
    })
  }

  private enqueue(key: string, operation: () => Promise<void>): void {
    const previous = this.pendingBySurface.get(key) ?? Promise.resolve()
    const pending = previous
      .catch(() => undefined)
      .then(operation)
      .catch((error: unknown) => {
        console.warn('Failed to persist browser history', error)
      })
      .finally(() => {
        if (this.pendingBySurface.get(key) === pending) this.pendingBySurface.delete(key)
      })
    this.pendingBySurface.set(key, pending)
  }

  private notifyChanged(): void {
    for (const listener of this.changedListeners) {
      try {
        listener()
      } catch {
        // History observers never participate in navigation or durable persistence.
      }
    }
  }
}

function surfaceKey(input: Pick<BrowserSurfaceHistoryEvent, 'surfaceId' | 'generation'>): string {
  return `${input.surfaceId}:${input.generation}`
}

function normalizeHistoryUrl(value: string): URL | null {
  try {
    const url = new URL(value)
    if (!['http:', 'https:', 'file:'].includes(url.protocol) || url.username || url.password) {
      return null
    }
    return url
  } catch {
    return null
  }
}

function normalizeRemoteUrl(value: string | null): string | null {
  if (!value) return null
  try {
    const url = new URL(value)
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) return null
    return url.toString()
  } catch {
    return null
  }
}

function normalizeTitle(value: string | null, url: URL): string {
  const title = value?.trim()
  if (title) return title
  if (url.protocol === 'file:') {
    const encoded = url.pathname.split('/').filter(Boolean).at(-1) ?? ''
    try {
      return decodeURIComponent(encoded) || url.toString()
    } catch {
      return encoded || url.toString()
    }
  }
  return url.hostname || url.toString()
}
